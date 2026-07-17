use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::backend;
use crate::model::{BackendKind, FaultCaseResult};
use crate::util::{copy_directory, create_durable_marker, wait_for_path, write_json_durable};

const PARENT_KINDS: [&str; 4] = ["attempts", "runs", "trash", "cache"];

const NAMESPACE_CASES: [NamespaceCaseSpec; 19] = [
    NamespaceCaseSpec::run("state-directory-copy-swap", "state-directory-copy-swap"),
    NamespaceCaseSpec::run("lifecycle-lock-replacement", "lifecycle-lock-replacement"),
    NamespaceCaseSpec::run(
        "lifecycle-lock-content-mutation",
        "lifecycle-lock-content-mutation",
    ),
    NamespaceCaseSpec::run("lifecycle-lock-extra-link", "lifecycle-lock-extra-link"),
    NamespaceCaseSpec::run("attempts-parent-replacement", "attempts-parent-replacement"),
    NamespaceCaseSpec::run("runs-parent-replacement", "runs-parent-replacement"),
    NamespaceCaseSpec::run("trash-parent-replacement", "trash-parent-replacement"),
    NamespaceCaseSpec::run("cache-parent-replacement", "cache-parent-replacement"),
    NamespaceCaseSpec::run("attempts-anchor-replacement", "attempts-anchor-replacement"),
    NamespaceCaseSpec::run("runs-anchor-replacement", "runs-anchor-replacement"),
    NamespaceCaseSpec::run("trash-anchor-replacement", "trash-anchor-replacement"),
    NamespaceCaseSpec::run("cache-anchor-replacement", "cache-anchor-replacement"),
    NamespaceCaseSpec::run(
        "runs-anchor-content-mutation",
        "runs-anchor-content-mutation",
    ),
    NamespaceCaseSpec::run("runs-anchor-extra-link", "runs-anchor-extra-link"),
    NamespaceCaseSpec::new(
        "runs-parent-replacement-after-run-rename",
        "runs-parent-replacement",
        NamespaceOperation::RunPublish,
        FaultCheckpoint::AfterMutation,
    ),
    NamespaceCaseSpec::new(
        "runs-parent-replacement-before-final-commit",
        "runs-parent-replacement",
        NamespaceOperation::RunPublish,
        FaultCheckpoint::BeforeCommit,
    ),
    NamespaceCaseSpec::new(
        "trash-parent-replacement-before-trash-move",
        "trash-parent-replacement",
        NamespaceOperation::TrashMove,
        FaultCheckpoint::BeforeMutation,
    ),
    NamespaceCaseSpec::new(
        "trash-parent-replacement-after-trash-move",
        "trash-parent-replacement",
        NamespaceOperation::TrashMove,
        FaultCheckpoint::AfterMutation,
    ),
    NamespaceCaseSpec::new(
        "trash-parent-replacement-before-final-commit",
        "trash-parent-replacement",
        NamespaceOperation::TrashMove,
        FaultCheckpoint::BeforeCommit,
    ),
];

#[derive(Clone, Copy)]
struct NamespaceCaseSpec {
    name: &'static str,
    injected_fault: &'static str,
    operation: NamespaceOperation,
    checkpoint: FaultCheckpoint,
}

impl NamespaceCaseSpec {
    const fn run(name: &'static str, injected_fault: &'static str) -> Self {
        Self::new(
            name,
            injected_fault,
            NamespaceOperation::RunPublish,
            FaultCheckpoint::BeforeMutation,
        )
    }

    const fn new(
        name: &'static str,
        injected_fault: &'static str,
        operation: NamespaceOperation,
        checkpoint: FaultCheckpoint,
    ) -> Self {
        Self {
            name,
            injected_fault,
            operation,
            checkpoint,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum NamespaceOperation {
    RunPublish,
    TrashMove,
}

impl NamespaceOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::RunPublish => "run-publish",
            Self::TrashMove => "trash-move",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FaultCheckpoint {
    BeforeMutation,
    AfterMutation,
    BeforeCommit,
}

impl FaultCheckpoint {
    const fn as_str(self) -> &'static str {
        match self {
            Self::BeforeMutation => "before-mutation",
            Self::AfterMutation => "after-mutation",
            Self::BeforeCommit => "before-commit",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct PhysicalIdentity {
    volume: u64,
    file: u64,
    kind: String,
}

#[derive(Clone, Debug)]
struct PhysicalObservation {
    identity: PhysicalIdentity,
    links: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ParentBinding {
    kind: String,
    directory_physical_identity: PhysicalIdentity,
    anchor_physical_identity: PhysicalIdentity,
    parent_nonce: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct NamespaceMarker {
    repository_root_identity: PhysicalIdentity,
    state_directory_identity: PhysicalIdentity,
    lifecycle_lock_identity: PhysicalIdentity,
    namespace_nonce: String,
    managed_parents: BTreeMap<String, ParentBinding>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct LifecycleLockHeader {
    repository_root_identity: PhysicalIdentity,
    state_directory_identity: PhysicalIdentity,
    lifecycle_lock_identity: PhysicalIdentity,
    namespace_nonce: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct AnchorHeader {
    repository_root_identity: PhysicalIdentity,
    state_directory_identity: PhysicalIdentity,
    lifecycle_lock_identity: PhysicalIdentity,
    namespace_nonce: String,
    kind: String,
    directory_physical_identity: PhysicalIdentity,
    anchor_physical_identity: PhysicalIdentity,
    parent_nonce: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct StoreHeader {
    marker_sha256: String,
    committed_mutation: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct NamespaceChildResult {
    hard_stop: bool,
    diagnostic: String,
    operation: String,
    checkpoint: String,
    physical_mutation_performed: bool,
    canonical_commit_written: bool,
}

#[derive(Debug, Serialize)]
struct NamespaceCaseObservation {
    operation: String,
    checkpoint: String,
    injection_outcome: String,
    injection_error: Option<String>,
    physical_mutation_performed: bool,
    canonical_commit_written: bool,
    child_result: Option<NamespaceChildResult>,
}

pub fn run_namespace_cases(
    backend_kind: BackendKind,
    watchdog: Duration,
    backend_root: &Path,
) -> Vec<FaultCaseResult> {
    NAMESPACE_CASES
        .into_iter()
        .map(|case| {
            let started = Instant::now();
            match run_namespace_case(backend_kind, case, watchdog, backend_root) {
                Ok(observation) => FaultCaseResult {
                    domain: "namespace".to_owned(),
                    crash_point: case.name.to_owned(),
                    status: "PASS".to_owned(),
                    error: None,
                    elapsed_micros: started.elapsed().as_micros(),
                    observation,
                },
                Err(error) => FaultCaseResult {
                    domain: "namespace".to_owned(),
                    crash_point: case.name.to_owned(),
                    status: "FAIL".to_owned(),
                    error: Some(format!("{error:#}")),
                    elapsed_micros: started.elapsed().as_micros(),
                    observation: serde_json::Value::Null,
                },
            }
        })
        .collect()
}

pub struct NamespaceChildRequest<'a> {
    pub backend_kind: BackendKind,
    pub repository_root: &'a Path,
    pub operation: &'a str,
    pub checkpoint: &'a str,
    pub ready: &'a Path,
    pub go: &'a Path,
    pub result: &'a Path,
    pub watchdog: Duration,
}

pub fn run_namespace_child(request: NamespaceChildRequest<'_>) -> Result<()> {
    let operation = parse_operation(request.operation)?;
    let checkpoint = parse_checkpoint(request.checkpoint)?;
    let child_result = execute_namespace_mutation(
        request.backend_kind,
        request.repository_root,
        operation,
        checkpoint,
        request.ready,
        request.go,
        request.watchdog,
    )?;
    write_json_durable(request.result, &child_result)
}

fn run_namespace_case(
    backend_kind: BackendKind,
    case: NamespaceCaseSpec,
    watchdog: Duration,
    backend_root: &Path,
) -> Result<serde_json::Value> {
    let repository_root = backend_root.join("namespace").join(case.name);
    if repository_root.exists() {
        fs::remove_dir_all(&repository_root)?;
    }
    fs::create_dir_all(&repository_root)?;
    initialize_namespace(backend_kind, &repository_root)?;

    let coordination = repository_root.join("coordination");
    fs::create_dir_all(&coordination)?;
    let ready = coordination.join("ready");
    let go = coordination.join("go");
    let result = coordination.join("result.json");
    let mut child = Command::new(env::current_exe()?)
        .arg("child-namespace")
        .arg("--backend")
        .arg(backend_kind.to_string())
        .arg("--repository-root")
        .arg(&repository_root)
        .arg("--operation")
        .arg(case.operation.as_str())
        .arg("--checkpoint")
        .arg(case.checkpoint.as_str())
        .arg("--ready")
        .arg(&ready)
        .arg("--go")
        .arg(&go)
        .arg("--result")
        .arg(&result)
        .arg("--watchdog-ms")
        .arg(watchdog.as_millis().to_string())
        .spawn()
        .context("spawn namespace verifier child")?;
    wait_for_path(&ready, watchdog)?;
    if let Err(error) = inject_namespace_fault(&repository_root, case.injected_fault) {
        stop_child(&mut child).context("stop namespace child after injection failure")?;
        if is_kernel_fault_prevention(&error) {
            let header = read_store_header(backend_kind, &repository_root)?;
            ensure!(
                header.committed_mutation.is_none(),
                "kernel-prevented namespace fault published a canonical store commit"
            );
            return Ok(serde_json::to_value(NamespaceCaseObservation {
                operation: case.operation.as_str().to_owned(),
                checkpoint: case.checkpoint.as_str().to_owned(),
                injection_outcome: "kernel-prevented-before-displacement".to_owned(),
                injection_error: Some(format!("{error:#}")),
                physical_mutation_performed: case.checkpoint != FaultCheckpoint::BeforeMutation,
                canonical_commit_written: false,
                child_result: None,
            })?);
        }
        return Err(error).context("inject namespace replacement fault");
    }
    create_durable_marker(&go, b"go\n")?;
    let status = wait_for_child(&mut child, watchdog)?;
    ensure!(status.success(), "namespace child failed with {status}");

    let child_result: NamespaceChildResult = serde_json::from_slice(&fs::read(&result)?)?;
    ensure!(child_result.hard_stop, "namespace replacement was accepted");
    ensure!(!child_result.canonical_commit_written);
    let header = read_store_header(backend_kind, &repository_root)?;
    ensure!(
        header.committed_mutation.is_none(),
        "namespace fault published a canonical store commit"
    );
    Ok(serde_json::to_value(NamespaceCaseObservation {
        operation: case.operation.as_str().to_owned(),
        checkpoint: case.checkpoint.as_str().to_owned(),
        injection_outcome: "injected-and-detected".to_owned(),
        injection_error: None,
        physical_mutation_performed: child_result.physical_mutation_performed,
        canonical_commit_written: child_result.canonical_commit_written,
        child_result: Some(child_result),
    })?)
}

fn initialize_namespace(backend_kind: BackendKind, repository_root: &Path) -> Result<()> {
    let state = state_directory(repository_root);
    fs::create_dir_all(&state)?;
    let lock = state.join("lifecycle.lock");
    create_bound_file(&lock)?;
    let repository_root_identity = physical_identity(repository_root)?;
    let state_directory_identity = physical_identity(&state)?;
    let lifecycle_lock_identity = physical_identity(&lock)?;
    let namespace_nonce = unique_nonce()?;
    write_bound_json(
        &lock,
        &LifecycleLockHeader {
            repository_root_identity: repository_root_identity.clone(),
            state_directory_identity: state_directory_identity.clone(),
            lifecycle_lock_identity: lifecycle_lock_identity.clone(),
            namespace_nonce: namespace_nonce.clone(),
        },
    )?;
    let mut managed_parents = BTreeMap::new();
    for kind in PARENT_KINDS {
        let parent = state.join(kind);
        fs::create_dir_all(&parent)?;
        let anchor = parent.join("namespace.anchor");
        create_bound_file(&anchor)?;
        let directory_physical_identity = physical_identity(&parent)?;
        let anchor_physical_identity = physical_identity(&anchor)?;
        let parent_nonce = unique_nonce()?;
        write_bound_json(
            &anchor,
            &AnchorHeader {
                repository_root_identity: repository_root_identity.clone(),
                state_directory_identity: state_directory_identity.clone(),
                lifecycle_lock_identity: lifecycle_lock_identity.clone(),
                namespace_nonce: namespace_nonce.clone(),
                kind: kind.to_owned(),
                directory_physical_identity: directory_physical_identity.clone(),
                anchor_physical_identity: anchor_physical_identity.clone(),
                parent_nonce: parent_nonce.clone(),
            },
        )?;
        managed_parents.insert(
            kind.to_owned(),
            ParentBinding {
                kind: kind.to_owned(),
                directory_physical_identity,
                anchor_physical_identity,
                parent_nonce,
            },
        );
    }
    initialize_mutation_fixtures(&state)?;
    let marker = NamespaceMarker {
        repository_root_identity,
        state_directory_identity,
        lifecycle_lock_identity,
        namespace_nonce,
        managed_parents,
    };
    let marker_bytes = serde_json::to_vec(&marker)?;
    write_json_durable(&state.join("repository.json"), &marker)?;
    let header = StoreHeader {
        marker_sha256: format!("{:x}", Sha256::digest(&marker_bytes)),
        committed_mutation: None,
    };
    let database = state.join("lifecycle.store");
    backend::initialize(backend_kind, &database)?;
    let header_bytes = serde_json::to_vec(&header)?;
    ensure!(backend::compare_exchange_catalog(
        backend_kind,
        &database,
        None,
        &header_bytes
    )?);
    verify_namespace(backend_kind, repository_root)?;
    Ok(())
}

fn verify_namespace(backend_kind: BackendKind, repository_root: &Path) -> Result<NamespaceMarker> {
    let state = state_directory(repository_root);
    let marker_path = state.join("repository.json");
    let marker_bytes =
        fs::read(&marker_path).with_context(|| format!("read marker {}", marker_path.display()))?;
    let marker: NamespaceMarker = serde_json::from_slice(&marker_bytes)?;
    validate_marker_entries(repository_root, &marker)?;
    let header = read_store_header(backend_kind, repository_root)
        .context("read store header during namespace verification")?;
    let canonical_marker = serde_json::to_vec(&marker)?;
    ensure!(
        header.marker_sha256 == format!("{:x}", Sha256::digest(canonical_marker)),
        "store header disagrees with namespace marker"
    );
    Ok(marker)
}

fn validate_marker_entries(repository_root: &Path, marker: &NamespaceMarker) -> Result<()> {
    validate_marker_identities(repository_root, marker)?;
    validate_marker_headers_from_paths(repository_root, marker)
}

fn validate_marker_identities(repository_root: &Path, marker: &NamespaceMarker) -> Result<()> {
    validate_global_identities(repository_root, marker)?;
    validate_managed_parent_identities(repository_root, marker)
}

fn validate_global_identities(repository_root: &Path, marker: &NamespaceMarker) -> Result<()> {
    let state = state_directory(repository_root);
    let lock = state.join("lifecycle.lock");
    let repository_root_identity = physical_identity(repository_root)?;
    ensure!(
        repository_root_identity == marker.repository_root_identity,
        "repository-root physical binding mismatch"
    );
    let state_identity = physical_identity(&state)?;
    ensure!(
        state_identity.kind == "directory",
        "state object is not a directory"
    );
    ensure!(
        state_identity == marker.state_directory_identity,
        "state-directory physical binding mismatch"
    );
    let lock_observation = physical_observation(&lock)?;
    ensure!(
        lock_observation.identity.kind == "file",
        "lifecycle lock is not a regular file"
    );
    ensure!(
        lock_observation.links == 1,
        "lifecycle lock has extra links"
    );
    ensure!(
        lock_observation.identity == marker.lifecycle_lock_identity,
        "lifecycle-lock physical binding mismatch"
    );
    Ok(())
}

fn validate_managed_parent_identities(
    repository_root: &Path,
    marker: &NamespaceMarker,
) -> Result<()> {
    let state = state_directory(repository_root);
    ensure!(
        marker.managed_parents.len() == PARENT_KINDS.len(),
        "managed-parent binding set is incomplete"
    );
    for kind in PARENT_KINDS {
        let binding = marker
            .managed_parents
            .get(kind)
            .with_context(|| format!("missing managed-parent binding {kind}"))?;
        ensure!(binding.kind == kind, "managed-parent kind mismatch");
        let parent = state.join(kind);
        let directory_identity = physical_identity(&parent)?;
        ensure!(
            directory_identity.kind == "directory",
            "managed parent {kind} is not a directory"
        );
        ensure!(
            directory_identity == binding.directory_physical_identity,
            "managed parent {kind} physical binding mismatch"
        );
        let anchor_path = parent.join("namespace.anchor");
        let anchor_observation = physical_observation(&anchor_path)?;
        ensure!(
            anchor_observation.identity.kind == "file",
            "managed parent {kind} anchor is not a regular file"
        );
        ensure!(
            anchor_observation.links == 1,
            "managed parent {kind} anchor has extra links"
        );
        ensure!(
            anchor_observation.identity == binding.anchor_physical_identity,
            "managed parent {kind} anchor physical binding mismatch"
        );
    }
    Ok(())
}

fn validate_marker_headers_from_paths(
    repository_root: &Path,
    marker: &NamespaceMarker,
) -> Result<()> {
    let state = state_directory(repository_root);
    ensure!(
        read_bound_json::<LifecycleLockHeader>(&state.join("lifecycle.lock"))?
            == expected_lock_header(marker),
        "lifecycle lock header changed"
    );
    for kind in PARENT_KINDS {
        let binding = marker
            .managed_parents
            .get(kind)
            .with_context(|| format!("missing managed-parent binding {kind}"))?;
        ensure!(
            read_bound_json::<AnchorHeader>(&state.join(kind).join("namespace.anchor"))?
                == expected_anchor_header(marker, kind, binding),
            "managed parent {kind} anchor header changed"
        );
    }
    Ok(())
}

fn expected_lock_header(marker: &NamespaceMarker) -> LifecycleLockHeader {
    LifecycleLockHeader {
        repository_root_identity: marker.repository_root_identity.clone(),
        state_directory_identity: marker.state_directory_identity.clone(),
        lifecycle_lock_identity: marker.lifecycle_lock_identity.clone(),
        namespace_nonce: marker.namespace_nonce.clone(),
    }
}

fn expected_anchor_header(
    marker: &NamespaceMarker,
    kind: &str,
    binding: &ParentBinding,
) -> AnchorHeader {
    AnchorHeader {
        repository_root_identity: marker.repository_root_identity.clone(),
        state_directory_identity: marker.state_directory_identity.clone(),
        lifecycle_lock_identity: marker.lifecycle_lock_identity.clone(),
        namespace_nonce: marker.namespace_nonce.clone(),
        kind: kind.to_owned(),
        directory_physical_identity: binding.directory_physical_identity.clone(),
        anchor_physical_identity: binding.anchor_physical_identity.clone(),
        parent_nonce: binding.parent_nonce.clone(),
    }
}

struct HeldObject {
    path: PathBuf,
    file: File,
    identity: PhysicalIdentity,
}

impl HeldObject {
    fn open(path: PathBuf, writable: bool) -> Result<Self> {
        let file = open_state_object(&path, writable)?;
        let identity = physical_identity_from_file(&file)?;
        Ok(Self {
            path,
            file,
            identity,
        })
    }

    fn verify(&self) -> Result<()> {
        ensure!(
            physical_identity_from_file(&self.file)? == self.identity,
            "held state-object identity changed: {}",
            self.path.display()
        );
        ensure!(
            physical_identity(&self.path)? == self.identity,
            "state entry no longer names held object: {}",
            self.path.display()
        );
        Ok(())
    }
}

struct HeldParent {
    directory: HeldObject,
    anchor: HeldObject,
}

struct HeldNamespace {
    marker: NamespaceMarker,
    repository_root: HeldObject,
    state_directory: HeldObject,
    lifecycle_lock: HeldObject,
    parents: BTreeMap<String, HeldParent>,
}

impl HeldNamespace {
    fn open(backend_kind: BackendKind, repository_root: &Path) -> Result<Self> {
        let state = state_directory(repository_root);
        let marker = verify_namespace(backend_kind, repository_root)
            .context("perform pre-acquire namespace admission")?;
        let held = Self {
            marker,
            repository_root: HeldObject::open(repository_root.to_path_buf(), false)
                .context("hold repository-root object")?,
            state_directory: HeldObject::open(state.clone(), false)
                .context("hold state-directory object")?,
            lifecycle_lock: HeldObject::open(state.join("lifecycle.lock"), true)
                .context("hold lifecycle-lock object")?,
            parents: BTreeMap::new(),
        };
        held.verify_globals(repository_root)
            .context("verify pre-acquire global namespace handles")?;
        Ok(held)
    }

    fn open_managed_parents(&mut self, repository_root: &Path) -> Result<()> {
        ensure!(self.parents.is_empty(), "managed parents were already held");
        let state = state_directory(repository_root);
        for kind in PARENT_KINDS {
            let parent_path = state.join(kind);
            self.parents.insert(
                kind.to_owned(),
                HeldParent {
                    directory: HeldObject::open(parent_path.clone(), false)?,
                    anchor: HeldObject::open(parent_path.join("namespace.anchor"), false)?,
                },
            );
        }
        Ok(())
    }

    fn verify_globals(&self, repository_root: &Path) -> Result<()> {
        self.repository_root.verify()?;
        self.state_directory.verify()?;
        self.lifecycle_lock.verify()?;
        validate_global_identities(repository_root, &self.marker)?;
        Ok(())
    }

    fn verify(&self, backend_kind: BackendKind, repository_root: &Path) -> Result<()> {
        self.verify_globals(repository_root)?;
        ensure!(
            self.parents.len() == PARENT_KINDS.len(),
            "complete managed-parent handle set is not held"
        );
        for (kind, parent) in &self.parents {
            parent
                .directory
                .verify()
                .with_context(|| format!("verify held managed parent {kind}"))?;
            parent
                .anchor
                .verify()
                .with_context(|| format!("verify held managed-parent anchor {kind}"))?;
        }
        validate_marker_identities(repository_root, &self.marker)?;
        let marker_path = state_directory(repository_root).join("repository.json");
        let current_marker: NamespaceMarker = serde_json::from_slice(
            &fs::read(&marker_path)
                .with_context(|| format!("read namespace marker {}", marker_path.display()))?,
        )?;
        ensure!(
            current_marker == self.marker,
            "namespace marker changed while handles were held"
        );
        ensure!(
            read_held_json::<LifecycleLockHeader>(
                &self.lifecycle_lock.file,
                "held lifecycle lock",
            )? == expected_lock_header(&self.marker),
            "held lifecycle lock header changed"
        );
        for kind in PARENT_KINDS {
            let binding = self
                .marker
                .managed_parents
                .get(kind)
                .with_context(|| format!("missing managed-parent binding {kind}"))?;
            let parent = self
                .parents
                .get(kind)
                .with_context(|| format!("missing held managed parent {kind}"))?;
            ensure!(
                read_held_json::<AnchorHeader>(
                    &parent.anchor.file,
                    &format!("held managed-parent anchor {kind}"),
                )? == expected_anchor_header(&self.marker, kind, binding),
                "held managed parent {kind} anchor header changed"
            );
        }
        let header = read_store_header(backend_kind, repository_root)
            .context("read store header while namespace handles were held")?;
        let canonical_marker = serde_json::to_vec(&self.marker)?;
        ensure!(
            header.marker_sha256 == format!("{:x}", Sha256::digest(canonical_marker)),
            "store header is bound to a different namespace marker"
        );
        Ok(())
    }
}

fn execute_namespace_mutation(
    backend_kind: BackendKind,
    repository_root: &Path,
    operation: NamespaceOperation,
    checkpoint: FaultCheckpoint,
    ready: &Path,
    go: &Path,
    watchdog: Duration,
) -> Result<NamespaceChildResult> {
    let mut held = HeldNamespace::open(backend_kind, repository_root)
        .context("open held namespace for mutation")?;
    held.lifecycle_lock
        .file
        .lock()
        .context("acquire held lifecycle-lock guard")?;
    held.verify_globals(repository_root)
        .context("revalidate global namespace after lock acquisition")?;
    held.open_managed_parents(repository_root)
        .context("open managed-parent handles after lock acquisition")?;
    held.verify(backend_kind, repository_root)?;
    let pause = FaultPauseContext {
        held: &held,
        backend_kind,
        repository_root,
        operation,
        checkpoint,
        ready,
        go,
        watchdog,
    };
    let mut physical_mutation_performed = false;

    if checkpoint == FaultCheckpoint::BeforeMutation
        && let Some(result) = pause_for_fault_and_revalidate(&pause, physical_mutation_performed)?
    {
        return Ok(result);
    }

    perform_namespace_mutation(repository_root, operation)?;
    physical_mutation_performed = true;
    if checkpoint == FaultCheckpoint::AfterMutation
        && let Some(result) = pause_for_fault_and_revalidate(&pause, physical_mutation_performed)?
    {
        return Ok(result);
    }

    held.verify(backend_kind, repository_root)?;
    if checkpoint == FaultCheckpoint::BeforeCommit
        && let Some(result) = pause_for_fault_and_revalidate(&pause, physical_mutation_performed)?
    {
        return Ok(result);
    }

    held.verify(backend_kind, repository_root)?;
    commit_namespace_mutation(backend_kind, repository_root, operation)?;
    held.verify(backend_kind, repository_root)?;
    Ok(NamespaceChildResult {
        hard_stop: false,
        diagnostic: "mutation committed without an injected integrity fault".to_owned(),
        operation: operation.as_str().to_owned(),
        checkpoint: checkpoint.as_str().to_owned(),
        physical_mutation_performed,
        canonical_commit_written: true,
    })
}

struct FaultPauseContext<'a> {
    held: &'a HeldNamespace,
    backend_kind: BackendKind,
    repository_root: &'a Path,
    operation: NamespaceOperation,
    checkpoint: FaultCheckpoint,
    ready: &'a Path,
    go: &'a Path,
    watchdog: Duration,
}

fn pause_for_fault_and_revalidate(
    context: &FaultPauseContext<'_>,
    physical_mutation_performed: bool,
) -> Result<Option<NamespaceChildResult>> {
    create_durable_marker(context.ready, context.checkpoint.as_str().as_bytes())?;
    wait_for_path(context.go, context.watchdog)?;
    match context
        .held
        .verify(context.backend_kind, context.repository_root)
    {
        Ok(()) => Ok(None),
        Err(error) => Ok(Some(NamespaceChildResult {
            hard_stop: true,
            diagnostic: format!("{error:#}"),
            operation: context.operation.as_str().to_owned(),
            checkpoint: context.checkpoint.as_str().to_owned(),
            physical_mutation_performed,
            canonical_commit_written: false,
        })),
    }
}

fn perform_namespace_mutation(repository_root: &Path, operation: NamespaceOperation) -> Result<()> {
    let state = state_directory(repository_root);
    let (source, destination) = match operation {
        NamespaceOperation::RunPublish => (
            state.join("attempts").join("staged-run"),
            state.join("runs").join("published-run"),
        ),
        NamespaceOperation::TrashMove => (
            state.join("runs").join("retained-run"),
            state.join("trash").join("plan-1").join("retained-run"),
        ),
    };
    fs::rename(&source, &destination).with_context(|| {
        format!(
            "perform {} physical move {} -> {}",
            operation.as_str(),
            source.display(),
            destination.display()
        )
    })
}

fn commit_namespace_mutation(
    backend_kind: BackendKind,
    repository_root: &Path,
    operation: NamespaceOperation,
) -> Result<()> {
    let database = state_directory(repository_root).join("lifecycle.store");
    let current = backend::read_catalog(backend_kind, &database)?
        .context("store namespace header is missing before mutation commit")?;
    let mut header: StoreHeader = serde_json::from_slice(&current)?;
    ensure!(
        header.committed_mutation.is_none(),
        "namespace mutation was already committed"
    );
    header.committed_mutation = Some(operation.as_str().to_owned());
    let replacement = serde_json::to_vec(&header)?;
    ensure!(
        backend::compare_exchange_catalog(backend_kind, &database, Some(&current), &replacement)?,
        "namespace mutation store commit lost its expected header"
    );
    Ok(())
}

fn read_store_header(backend_kind: BackendKind, repository_root: &Path) -> Result<StoreHeader> {
    let database = state_directory(repository_root).join("lifecycle.store");
    let header_bytes = backend::read_catalog(backend_kind, &database)?
        .context("store namespace header is missing")?;
    Ok(serde_json::from_slice(&header_bytes)?)
}

fn initialize_mutation_fixtures(state: &Path) -> Result<()> {
    let staged_run = state.join("attempts").join("staged-run");
    fs::create_dir_all(&staged_run)?;
    fs::write(staged_run.join("payload.bin"), b"staged-run-payload")?;
    let retained_run = state.join("runs").join("retained-run");
    fs::create_dir_all(&retained_run)?;
    fs::write(retained_run.join("payload.bin"), b"retained-run-payload")?;
    fs::create_dir_all(state.join("trash").join("plan-1"))?;
    Ok(())
}

fn parse_operation(value: &str) -> Result<NamespaceOperation> {
    match value {
        "run-publish" => Ok(NamespaceOperation::RunPublish),
        "trash-move" => Ok(NamespaceOperation::TrashMove),
        _ => bail!("unknown namespace operation {value:?}"),
    }
}

fn parse_checkpoint(value: &str) -> Result<FaultCheckpoint> {
    match value {
        "before-mutation" => Ok(FaultCheckpoint::BeforeMutation),
        "after-mutation" => Ok(FaultCheckpoint::AfterMutation),
        "before-commit" => Ok(FaultCheckpoint::BeforeCommit),
        _ => bail!("unknown namespace fault checkpoint {value:?}"),
    }
}

fn create_bound_file(path: &Path) -> Result<()> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("create bound state object {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("flush bound state object {}", path.display()))
}

fn write_bound_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    let mut file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .with_context(|| format!("open bound state object {}", path.display()))?;
    file.write_all(&bytes)
        .with_context(|| format!("write bound state object {}", path.display()))?;
    file.write_all(b"\n")?;
    file.sync_all()
        .with_context(|| format!("flush bound state object {}", path.display()))
}

fn read_bound_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let bytes =
        fs::read(path).with_context(|| format!("read bound state object {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("parse bound state object {}", path.display()))
}

fn read_held_json<T: for<'de> Deserialize<'de>>(file: &File, label: &str) -> Result<T> {
    let byte_len = usize::try_from(file.metadata()?.len())
        .with_context(|| format!("{label} is too large to read"))?;
    let mut bytes = vec![0_u8; byte_len];
    let mut offset = 0_usize;
    while offset < bytes.len() {
        let read = positioned_read(file, &mut bytes[offset..], offset as u64)
            .with_context(|| format!("read {label} at byte {offset}"))?;
        ensure!(read != 0, "{label} ended before its recorded length");
        offset += read;
    }
    serde_json::from_slice(&bytes).with_context(|| format!("parse {label}"))
}

#[cfg(windows)]
fn positioned_read(file: &File, buffer: &mut [u8], offset: u64) -> std::io::Result<usize> {
    use std::os::windows::fs::FileExt;

    file.seek_read(buffer, offset)
}

#[cfg(unix)]
fn positioned_read(file: &File, buffer: &mut [u8], offset: u64) -> std::io::Result<usize> {
    use std::os::unix::fs::FileExt;

    file.read_at(buffer, offset)
}

fn inject_namespace_fault(repository_root: &Path, fault: &str) -> Result<()> {
    let state = state_directory(repository_root);
    match fault {
        "state-directory-copy-swap" => replace_state_directory_with_copy(&state),
        "lifecycle-lock-replacement" => replace_lifecycle_lock(
            &state,
            serde_json::to_vec(&expected_lock_header(&read_namespace_marker(&state)?))?,
        ),
        "lifecycle-lock-content-mutation" => {
            replace_lifecycle_lock(&state, b"mutated-lifecycle-lock\n".to_vec())
        }
        "lifecycle-lock-extra-link" => {
            fs::hard_link(
                state.join("lifecycle.lock"),
                state.join("lifecycle.lock.extra-link"),
            )?;
            Ok(())
        }
        "runs-anchor-content-mutation" => {
            fs::write(
                state.join("runs").join("namespace.anchor"),
                b"mutated-anchor\n",
            )?;
            Ok(())
        }
        "runs-anchor-extra-link" => {
            fs::hard_link(
                state.join("runs").join("namespace.anchor"),
                state.join("runs").join("namespace.anchor.extra-link"),
            )?;
            Ok(())
        }
        value if value.ends_with("-parent-replacement") => {
            let kind = value.trim_end_matches("-parent-replacement");
            ensure!(PARENT_KINDS.contains(&kind));
            replace_directory_with_copy(&state.join(kind))
        }
        value if value.ends_with("-anchor-replacement") => {
            let kind = value.trim_end_matches("-anchor-replacement");
            ensure!(PARENT_KINDS.contains(&kind));
            replace_file_with_copy(&state.join(kind).join("namespace.anchor"))
        }
        _ => bail!("unknown namespace fault {fault}"),
    }
}

fn replace_directory_with_copy(path: &Path) -> Result<()> {
    let backup = sibling_backup(path);
    let candidate = sibling_candidate(path);
    copy_directory(path, &candidate)?;
    rename_fault_object(path, &backup)?;
    fs::rename(&candidate, path).with_context(|| {
        format!(
            "install copied replacement {} -> {}",
            candidate.display(),
            path.display()
        )
    })
}

fn replace_file_with_copy(path: &Path) -> Result<()> {
    let backup = sibling_backup(path);
    let candidate = sibling_candidate(path);
    fs::copy(path, &candidate)?;
    rename_fault_object(path, &backup)?;
    fs::rename(&candidate, path).with_context(|| {
        format!(
            "install copied replacement {} -> {}",
            candidate.display(),
            path.display()
        )
    })
}

fn replace_state_directory_with_copy(state: &Path) -> Result<()> {
    let backup = sibling_backup(state);
    let candidate = sibling_candidate(state);
    let marker = read_namespace_marker(state)?;
    copy_state_directory_without_lock(state, &candidate)?;
    let replacement_lock = candidate.join("lifecycle.lock");
    create_bound_file(&replacement_lock)?;
    write_bound_json(&replacement_lock, &expected_lock_header(&marker))?;
    rename_fault_object(state, &backup)?;
    fs::rename(&candidate, state).with_context(|| {
        format!(
            "install copied state-directory replacement {} -> {}",
            candidate.display(),
            state.display()
        )
    })
}

fn replace_lifecycle_lock(state: &Path, mut contents: Vec<u8>) -> Result<()> {
    let path = state.join("lifecycle.lock");
    let backup = sibling_backup(&path);
    let candidate = sibling_candidate(&path);
    if !contents.ends_with(b"\n") {
        contents.push(b'\n');
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&candidate)?;
    file.write_all(&contents)?;
    file.sync_all()?;
    rename_fault_object(&path, &backup)?;
    fs::rename(&candidate, &path).with_context(|| {
        format!(
            "install lifecycle-lock replacement {} -> {}",
            candidate.display(),
            path.display()
        )
    })
}

fn read_namespace_marker(state: &Path) -> Result<NamespaceMarker> {
    let path = state.join("repository.json");
    let bytes =
        fs::read(&path).with_context(|| format!("read namespace marker {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("parse namespace marker {}", path.display()))
}

fn copy_state_directory_without_lock(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if entry.file_name() == "lifecycle.lock" {
            continue;
        }
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn sibling_backup(path: &Path) -> PathBuf {
    path.with_file_name(format!(
        "{}.replaced",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("object")
    ))
}

fn sibling_candidate(path: &Path) -> PathBuf {
    path.with_file_name(format!(
        "{}.candidate",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("object")
    ))
}

#[cfg(windows)]
fn rename_fault_object(source: &Path, destination: &Path) -> Result<()> {
    use std::mem::{offset_of, size_of};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_RENAME_INFO,
        FILE_RENAME_INFO_0, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileRenameInfoEx,
        SetFileInformationByHandle,
    };

    ensure!(
        source.parent() == destination.parent(),
        "fault rename must remain in one parent directory"
    );
    let destination_name = fs::canonicalize(
        destination
            .parent()
            .context("fault rename destination has no parent")?,
    )?
    .join(
        destination
            .file_name()
            .context("fault rename destination has no file name")?,
    )
    .as_os_str()
    .encode_wide()
    .chain(std::iter::once(0))
    .collect::<Vec<_>>();
    let mut options = OpenOptions::new();
    options
        .access_mode(DELETE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    let object = options
        .open(source)
        .with_context(|| format!("open fault object for rename {}", source.display()))?;
    let file_name_bytes = destination_name
        .len()
        .checked_sub(1)
        .context("fault rename destination is empty")?
        .checked_mul(size_of::<u16>())
        .context("fault rename destination is too long")?;
    let buffer_bytes = size_of::<FILE_RENAME_INFO>()
        .checked_add(
            destination_name
                .len()
                .checked_mul(size_of::<u16>())
                .context("fault rename destination is too long")?,
        )
        .context("fault rename buffer is too large")?;
    ensure!(
        offset_of!(FILE_RENAME_INFO, FileName) + file_name_bytes <= buffer_bytes,
        "fault rename buffer does not contain its file name"
    );
    let mut storage = vec![0_u64; buffer_bytes.div_ceil(size_of::<u64>())];
    let information = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    let information_bytes =
        u32::try_from(buffer_bytes).context("fault rename buffer exceeds u32")?;
    let file_name_length =
        u32::try_from(file_name_bytes).context("fault rename name exceeds u32")?;
    // SAFETY: storage is u64-aligned and sized for FILE_RENAME_INFO plus the UTF-16 name;
    // both handles remain valid for the call and the copied name has exactly FileNameLength bytes.
    let renamed = unsafe {
        (*information).Anonymous = FILE_RENAME_INFO_0 { Flags: 0x1 | 0x2 };
        (*information).RootDirectory = std::ptr::null_mut();
        (*information).FileNameLength = file_name_length;
        std::ptr::copy_nonoverlapping(
            destination_name.as_ptr(),
            (*information).FileName.as_mut_ptr(),
            destination_name.len(),
        );
        SetFileInformationByHandle(
            object.as_raw_handle().cast(),
            FileRenameInfoEx,
            information.cast(),
            information_bytes,
        )
    };
    if renamed == 0 {
        return Err(std::io::Error::last_os_error()).with_context(|| {
            format!(
                "fault rename {} -> {}",
                source.display(),
                destination.display()
            )
        });
    }
    Ok(())
}

#[cfg(unix)]
fn rename_fault_object(source: &Path, destination: &Path) -> Result<()> {
    fs::rename(source, destination).with_context(|| {
        format!(
            "fault rename {} -> {}",
            source.display(),
            destination.display()
        )
    })
}

fn state_directory(repository_root: &Path) -> PathBuf {
    repository_root.join(".lumin")
}

fn unique_nonce() -> Result<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).context("generate namespace nonce")?;
    Ok(bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>())
}

fn wait_for_child(child: &mut Child, watchdog: Duration) -> Result<ExitStatus> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if started.elapsed() >= watchdog {
            child.kill()?;
            child.wait()?;
            bail!("namespace child watchdog expired");
        }
        thread::sleep(Duration::from_millis(1));
    }
}

fn stop_child(child: &mut Child) -> Result<ExitStatus> {
    if let Some(status) = child.try_wait()? {
        return Ok(status);
    }
    child.kill()?;
    Ok(child.wait()?)
}

#[cfg(windows)]
fn is_kernel_fault_prevention(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::raw_os_error)
            .is_some_and(|code| matches!(code, 5 | 32 | 33))
    })
}

#[cfg(unix)]
fn is_kernel_fault_prevention(_error: &anyhow::Error) -> bool {
    false
}

#[cfg(windows)]
fn open_state_object(path: &Path, writable: bool) -> Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let mut options = OpenOptions::new();
    options
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    if writable {
        options.write(true);
    }
    options.open(path).with_context(|| {
        format!(
            "open state object without following links: {}",
            path.display()
        )
    })
}

#[cfg(windows)]
fn open_identity_object(path: &Path) -> Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let mut options = OpenOptions::new();
    options
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    options.open(path).with_context(|| {
        format!(
            "open state object identity without following links: {}",
            path.display()
        )
    })
}

#[cfg(windows)]
fn observe_physical_object(file: &File) -> Result<PhysicalObservation> {
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        GetFileInformationByHandle,
    };

    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: File owns a valid handle and information points to writable initialized storage.
    let result =
        unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), &mut information) };
    if result == 0 {
        return Err(std::io::Error::last_os_error()).context("read physical identity from handle");
    }
    ensure!(
        information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0,
        "reparse-point state object is forbidden"
    );
    let kind = if information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        "directory"
    } else {
        "file"
    };
    Ok(PhysicalObservation {
        identity: PhysicalIdentity {
            volume: information.dwVolumeSerialNumber as u64,
            file: ((information.nFileIndexHigh as u64) << 32) | information.nFileIndexLow as u64,
            kind: kind.to_owned(),
        },
        links: information.nNumberOfLinks as u64,
    })
}

#[cfg(unix)]
fn open_state_object(path: &Path, writable: bool) -> Result<File> {
    use std::os::unix::fs::MetadataExt;

    let before = fs::symlink_metadata(path)?;
    ensure!(!before.file_type().is_symlink());
    let mut options = OpenOptions::new();
    options.read(true);
    if writable {
        options.write(true);
    }
    let file = options.open(path)?;
    let after = file.metadata()?;
    ensure!(before.dev() == after.dev() && before.ino() == after.ino());
    Ok(file)
}

#[cfg(unix)]
fn open_identity_object(path: &Path) -> Result<File> {
    open_state_object(path, false)
}

#[cfg(unix)]
fn observe_physical_object(file: &File) -> Result<PhysicalObservation> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file.metadata()?;
    let kind = if metadata.is_dir() {
        "directory"
    } else {
        "file"
    };
    Ok(PhysicalObservation {
        identity: PhysicalIdentity {
            volume: metadata.dev(),
            file: metadata.ino(),
            kind: kind.to_owned(),
        },
        links: metadata.nlink(),
    })
}

fn physical_identity_from_file(file: &File) -> Result<PhysicalIdentity> {
    Ok(observe_physical_object(file)?.identity)
}

fn physical_observation(path: &Path) -> Result<PhysicalObservation> {
    let file = open_identity_object(path)?;
    observe_physical_object(&file)
}

fn physical_identity(path: &Path) -> Result<PhysicalIdentity> {
    Ok(physical_observation(path)?.identity)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fault_rename_displaces_open_file() -> Result<()> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("source.lock");
        let destination = root.path().join("source.lock.replaced");
        fs::write(&source, b"bound")?;
        let held = open_state_object(&source, true)?;

        rename_fault_object(&source, &destination)?;

        ensure!(!source.exists());
        ensure!(destination.exists());
        ensure!(physical_identity_from_file(&held)? == physical_identity(&destination)?);
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn fault_rename_reports_kernel_prevention_for_open_child() -> Result<()> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("managed-parent");
        let destination = root.path().join("managed-parent.replaced");
        fs::create_dir(&source)?;
        let child = source.join("namespace.anchor");
        fs::write(&child, b"bound")?;
        let held_directory = open_state_object(&source, false)?;
        let held_child = open_state_object(&child, false)?;

        let error = match rename_fault_object(&source, &destination) {
            Ok(()) => bail!("Windows displaced a directory with an open child unexpectedly"),
            Err(error) => error,
        };

        ensure!(is_kernel_fault_prevention(&error));
        ensure!(source.exists());
        ensure!(!destination.exists());
        ensure!(physical_identity_from_file(&held_directory)? == physical_identity(&source)?);
        ensure!(physical_identity_from_file(&held_child)? == physical_identity(&child)?);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn fault_rename_displaces_directory_with_open_child() -> Result<()> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("managed-parent");
        let destination = root.path().join("managed-parent.replaced");
        fs::create_dir(&source)?;
        let child = source.join("namespace.anchor");
        fs::write(&child, b"bound")?;
        let held_directory = open_state_object(&source, false)?;
        let held_child = open_state_object(&child, false)?;

        rename_fault_object(&source, &destination)?;

        ensure!(!source.exists());
        ensure!(destination.exists());
        ensure!(physical_identity_from_file(&held_directory)? == physical_identity(&destination)?);
        ensure!(
            physical_identity_from_file(&held_child)?
                == physical_identity(&destination.join("namespace.anchor"))?
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn directory_identity_survives_legitimate_link_count_change() -> Result<()> {
        let root = tempfile::tempdir()?;
        let directory = root.path().join("managed-parent");
        fs::create_dir(&directory)?;
        let before = physical_observation(&directory)?;

        fs::create_dir(directory.join("payload-directory"))?;
        let after = physical_observation(&directory)?;

        ensure!(before.identity == after.identity);
        ensure!(before.links != after.links);
        Ok(())
    }
}
