use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

#[path = "support/namespace_barrier.rs"]
mod namespace_barrier;
mod support;

use namespace_barrier::NamespaceBarrier;
use support::{ProcessResult, assert_status, field, run};

const AFTER_PRE_ACQUIRE_VALIDATION: &str = "after-pre-acquire-validation";
const AFTER_COMPLETE_VALIDATION: &str = "after-complete-validation";
const BEFORE_STORE_COMMIT: &str = "before-store-commit";
const BEFORE_MIGRATION_STORE_COMMIT: &str = "before-migration-store-commit";
const BEFORE_RUN_RENAME: &str = "before-run-rename";
const BEFORE_RETENTION_MOVE: &str = "before-retention-move";
const BEFORE_CACHE_MOVE: &str = "before-cache-move";

#[test]
fn lock_replacement_never_forms_two_accepted_guard_domains()
-> Result<(), Box<dyn std::error::Error>> {
    with_context(
        "pre-acquire migration lock replacement",
        migration_rejects_lock_replacement_after_pre_acquire_validation(),
    )?;
    with_context(
        "pre-commit migration lock replacement",
        migration_commit_rejects_lock_replacement(),
    )?;
    with_context(
        "post-validation state-directory swap",
        state_swap_is_rejected_after_complete_validation(),
    )?;
    with_context(
        "pre-commit lock replacement",
        gate_commit_rejects_lock_replacement(),
    )?;
    Ok(())
}

#[test]
fn managed_parent_replacement_stops_every_guarded_transition()
-> Result<(), Box<dyn std::error::Error>> {
    with_context(
        "pre-admission copied managed-parent replacements",
        copied_managed_parents_fail_closed_before_admission(),
    )?;
    with_context(
        "post-validation runs-parent swap",
        parent_swap_is_rejected_after_complete_validation(),
    )?;
    with_context(
        "pre-rename runs-parent swap",
        run_parent_swap_stops_before_publication_rename(),
    )?;
    with_context(
        "pre-cache-move quarantine swap",
        quarantine_swap_stops_before_cache_move(),
    )?;
    with_context(
        "post-validation retention trash swap",
        trash_swap_cannot_redirect_retention_move(),
    )?;
    with_context(
        "pre-commit attempts-parent swap",
        attempt_parent_swap_stops_before_gate_commit(),
    )?;
    Ok(())
}

fn copied_managed_parents_fail_closed_before_admission() -> Result<(), Box<dyn std::error::Error>> {
    for (relative, label) in [
        (".lumin/attempts", "attempts-before-admission"),
        (".lumin/runs", "runs-before-admission"),
        (".lumin/trash", "trash-before-admission"),
        (".lumin/cache", "cache-before-admission"),
        (
            ".lumin/trash/cache-evictions",
            "quarantine-before-admission",
        ),
    ] {
        let root = fixture()?;
        let baseline_run = initialize(root.path())?;
        let canonical = root.path().join(relative);
        let mut replacement = DirectoryReplacement::prepare(root.path(), &canonical, label)?;
        assert!(
            replacement.activate()?,
            "an idle managed parent must be replaceable for the fault fixture: {relative}"
        );
        let visible_before = tree_snapshot(&canonical)?;
        let authentic_before = tree_snapshot(replacement.authentic_path())?;

        assert_integrity_failure(&run(root.path(), &["audit", "--jobs", "1"])?);
        assert_eq!(tree_snapshot(&canonical)?, visible_before);
        assert_eq!(
            tree_snapshot(replacement.authentic_path())?,
            authentic_before
        );
        replacement.restore()?;
        assert_latest_run(root.path(), &baseline_run)?;
    }

    let root = fixture()?;
    let baseline_run = initialize(root.path())?;
    let cache = root.path().join(".lumin/cache");
    let authentic = root.path().join(".cache-empty.authentic");
    fs::rename(&cache, &authentic)?;
    fs::create_dir(&cache)?;
    assert_integrity_failure(&run(root.path(), &["audit", "--jobs", "1"])?);
    assert_eq!(fs::read_dir(&cache)?.count(), 0);
    fs::remove_dir(&cache)?;
    fs::rename(authentic, cache)?;
    assert_latest_run(root.path(), &baseline_run)?;
    Ok(())
}

fn migration_rejects_lock_replacement_after_pre_acquire_validation()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let baseline_run = initialize(root.path())?;
    let downgraded = run(root.path(), &["store", "test-downgrade-v12"])?;
    assert_status(&downgraded, 0);

    let barrier = NamespaceBarrier::new(AFTER_PRE_ACQUIRE_VALIDATION)?;
    let mut replacement = FileReplacement::prepare(
        &root.path().join(".lumin/lifecycle.lock"),
        root.path().join("lifecycle.lock.prepared"),
        root.path().join("lifecycle.lock.authentic"),
    )?;
    let mut migration = barrier.spawn(root.path(), &["store", "migrate", "--format", "json"])?;
    let permit = barrier.accept(&mut migration)?;
    assert!(
        replacement.activate()?,
        "platform must permit lock replacement"
    );

    assert_integrity_failure(&run(
        root.path(),
        &["store", "migrate", "--format", "json"],
    )?);
    permit.release()?;
    assert_integrity_failure(&migration.finish()?);
    replacement.restore()?;

    let migrated = run(root.path(), &["store", "migrate", "--format", "json"])?;
    assert_status(&migrated, 0);
    assert_eq!(
        json(&migrated.stdout)?
            .get("schemaVersion")
            .and_then(Value::as_str),
        Some("lumin.lifecycle-store-migration.v1")
    );
    assert_latest_run(root.path(), &baseline_run)?;
    assert_run_visible(root.path(), &baseline_run)?;
    Ok(())
}

fn migration_commit_rejects_lock_replacement() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let baseline_run = initialize(root.path())?;
    let downgraded = run(root.path(), &["store", "test-downgrade-v12"])?;
    assert_status(&downgraded, 0);

    let barrier = NamespaceBarrier::new(BEFORE_MIGRATION_STORE_COMMIT)?;
    let mut replacement = FileReplacement::prepare(
        &root.path().join(".lumin/lifecycle.lock"),
        root.path().join("lifecycle.lock.migration-commit-prepared"),
        root.path()
            .join("lifecycle.lock.migration-commit-authentic"),
    )?;
    let state = root.path().join(".lumin");
    let store = state.join("lifecycle.store");
    let store_identity_before = physical_identity(&store)?;
    let durable_before = tree_without_subtree(&tree_snapshot(&state)?, "lifecycle.store");
    let mut migration = barrier.spawn(root.path(), &["store", "migrate", "--format", "json"])?;
    let permit = barrier.accept(&mut migration)?;
    assert!(
        replacement.activate()?,
        "platform must permit lock replacement"
    );

    permit.release()?;
    assert_integrity_failure(&migration.finish()?);
    replacement.restore()?;
    assert_eq!(physical_identity(&store)?, store_identity_before);
    assert_eq!(
        tree_without_subtree(&tree_snapshot(&state)?, "lifecycle.store"),
        durable_before,
        "rejected migration commit changed the durable namespace"
    );

    let migrated = run(root.path(), &["store", "migrate", "--format", "json"])?;
    assert_status(&migrated, 0);
    let root_authorization: Value =
        serde_json::from_slice(&fs::read(state.join("lifecycle-migration.json"))?)?;
    assert_eq!(
        root_authorization
            .get("authorizationSequence")
            .and_then(Value::as_u64),
        Some(1),
        "rejected migration commit appended a root authorization"
    );
    assert_latest_run(root.path(), &baseline_run)?;
    assert_run_visible(root.path(), &baseline_run)?;
    Ok(())
}

fn state_swap_is_rejected_after_complete_validation() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let baseline_run = initialize(root.path())?;
    let mut replacement = DirectoryReplacement::prepare(
        root.path(),
        &root.path().join(".lumin"),
        "state-after-validation",
    )?;
    let barrier = NamespaceBarrier::new(AFTER_COMPLETE_VALIDATION)?;
    let mut audit = barrier.spawn(root.path(), &["audit", "--jobs", "1"])?;
    let permit = barrier.accept(&mut audit)?;
    let replaced = replacement.activate()?;
    if replaced {
        assert_integrity_failure(&run(root.path(), &["audit", "--jobs", "1"])?);
        permit.release()?;
        assert_integrity_failure(&audit.finish()?);
        replacement.restore()?;
        assert_latest_run(root.path(), &baseline_run)?;
    } else {
        permit.release()?;
        let completed = audit.finish()?;
        assert_status(&completed, 0);
        let completed_run = field(&completed.stdout, "runId")?;
        replacement.restore()?;
        assert_latest_run(root.path(), &completed_run)?;
    }
    assert_run_visible(root.path(), &baseline_run)?;
    Ok(())
}

fn gate_commit_rejects_lock_replacement() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let baseline_run = initialize(root.path())?;
    let barrier = NamespaceBarrier::new(BEFORE_STORE_COMMIT)?;
    let mut replacement = FileReplacement::prepare(
        &root.path().join(".lumin/lifecycle.lock"),
        root.path().join("lifecycle.lock.commit-prepared"),
        root.path().join("lifecycle.lock.commit-authentic"),
    )?;
    let mut pre_write = barrier.spawn(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "lock-swap-before-commit",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    let permit = barrier.accept(&mut pre_write)?;
    assert!(
        replacement.activate()?,
        "platform must permit lock replacement"
    );

    assert_integrity_failure(&run(root.path(), &["audit", "--jobs", "1"])?);
    permit.release()?;
    assert_integrity_failure(&pre_write.finish()?);
    replacement.restore()?;

    assert_operation_absent(root.path(), "lock-swap-before-commit")?;
    assert_latest_run(root.path(), &baseline_run)?;
    assert_run_visible(root.path(), &baseline_run)?;
    Ok(())
}

fn parent_swap_is_rejected_after_complete_validation() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let baseline_run = initialize(root.path())?;
    let runs = root.path().join(".lumin/runs");
    let mut replacement =
        ManagedParentReplacement::prepare(root.path(), &runs, "runs-after-validation")?;
    let barrier = NamespaceBarrier::new(AFTER_COMPLETE_VALIDATION)?;
    let mut audit = barrier.spawn(root.path(), &["audit", "--jobs", "1"])?;
    let permit = barrier.accept(&mut audit)?;
    replacement.activate()?;
    let (visible_before, authentic_before) = replacement.snapshots()?;

    permit.release()?;
    assert_integrity_failure(&audit.finish()?);
    replacement.assert_unchanged(&visible_before, authentic_before.as_deref())?;
    replacement.restore()?;

    assert_latest_run(root.path(), &baseline_run)?;
    assert_run_visible(root.path(), &baseline_run)?;
    Ok(())
}

fn run_parent_swap_stops_before_publication_rename() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let baseline_run = initialize(root.path())?;
    fs::write(
        root.path().join("src/lib.ts"),
        "export const visible = 2;\n",
    )?;
    let runs = root.path().join(".lumin/runs");
    let mut replacement =
        ManagedParentReplacement::prepare(root.path(), &runs, "runs-before-rename")?;
    let barrier = NamespaceBarrier::new(BEFORE_RUN_RENAME)?;
    let mut audit = barrier.spawn(root.path(), &["audit", "--jobs", "1"])?;
    let permit = barrier.accept(&mut audit)?;
    replacement.activate()?;

    let (visible_before, authentic_before) = replacement.snapshots()?;
    let staging_snapshot = authentic_before.as_deref().unwrap_or(&visible_before);
    assert!(
        staging_snapshot.iter().any(|entry| {
            entry.relative.contains(".run_") && entry.relative.contains(".staging")
        }),
        "audit reached the rename barrier without a durable staging directory"
    );
    permit.release()?;
    assert_integrity_failure(&audit.finish()?);
    replacement.assert_unchanged(&visible_before, authentic_before.as_deref())?;
    replacement.restore()?;

    assert_latest_run(root.path(), &baseline_run)?;
    assert_run_visible(root.path(), &baseline_run)?;
    Ok(())
}

fn quarantine_swap_stops_before_cache_move() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let baseline_run = initialize(root.path())?;
    let written = run(
        root.path(),
        &["cache", "test-write", "move.bin", "must-stay-active"],
    )?;
    assert_status(&written, 0);
    let quarantine = root.path().join(".lumin/trash/cache-evictions");
    let mut replacement =
        ManagedParentReplacement::prepare(root.path(), &quarantine, "quarantine-before-move")?;
    let barrier = NamespaceBarrier::new(BEFORE_CACHE_MOVE)?;
    let mut cleanup = barrier.spawn(
        root.path(),
        &[
            "cache",
            "clean",
            "--operation-id",
            "parent-swap-cache-clean",
        ],
    )?;
    let permit = barrier.accept(&mut cleanup)?;
    replacement.activate()?;
    let (visible_before, authentic_before) = replacement.snapshots()?;

    permit.release()?;
    assert_integrity_failure(&cleanup.finish()?);
    assert_eq!(
        fs::read(root.path().join(".lumin/cache/move.bin"))?,
        b"must-stay-active"
    );
    replacement.assert_unchanged(&visible_before, authentic_before.as_deref())?;
    replacement.restore()?;

    assert_cleanup_not_committed(root.path(), "parent-swap-cache-clean")?;
    assert_latest_run(root.path(), &baseline_run)?;
    assert_run_visible(root.path(), &baseline_run)?;
    Ok(())
}

fn trash_swap_cannot_redirect_retention_move() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let first_run = initialize(root.path())?;
    fs::write(
        root.path().join("src/lib.ts"),
        "export const visible = 2;\n",
    )?;
    let second_run = field(
        &run_success(root.path(), &["audit", "--jobs", "1"])?.stdout,
        "runId",
    )?;
    let plan = run_success(
        root.path(),
        &[
            "runs",
            "prune",
            "plan",
            "--before",
            "9000000000000",
            "--operation-id",
            "parent-swap-plan",
        ],
    )?;
    let plan_id = required_string(&json(&plan.stdout)?, "/result/planId")?.to_owned();
    let trash = root.path().join(".lumin/trash");
    let mut replacement =
        ManagedParentReplacement::prepare(root.path(), &trash, "trash-before-retention-move")?;
    let barrier = NamespaceBarrier::new(BEFORE_RETENTION_MOVE)?;
    let mut confirm = barrier.spawn(
        root.path(),
        &[
            "runs",
            "prune",
            "confirm",
            &plan_id,
            "--operation-id",
            "parent-swap-confirm",
        ],
    )?;
    let permit = barrier.accept(&mut confirm)?;
    let attempts_before = tree_snapshot(&root.path().join(".lumin/attempts"))?;
    replacement.activate()?;
    let foreign_before = replacement.foreign_binding_snapshot()?;
    let move_destination = replacement.move_destination_root().to_path_buf();

    permit.release()?;
    assert_integrity_failure(&confirm.finish()?);
    let attempts_after = tree_snapshot(&root.path().join(".lumin/attempts"))?;
    let removed_attempts = attempts_before
        .iter()
        .filter(|entry| {
            entry.kind == "directory"
                && entry.relative != "."
                && !entry.relative.contains('/')
                && !attempts_after
                    .iter()
                    .any(|current| current.relative == entry.relative)
        })
        .collect::<Vec<_>>();
    assert_eq!(removed_attempts.len(), 1);
    let moved_attempt = removed_attempts[0];
    assert_eq!(
        attempts_after,
        tree_without_subtree(&attempts_before, &moved_attempt.relative)
    );
    assert!(
        tree_snapshot(&move_destination)?.iter().any(|entry| {
            entry.kind == "directory" && entry.physical_identity == moved_attempt.physical_identity
        }),
        "the moved attempt must remain bound to the authentic held trash parent"
    );
    replacement.assert_foreign_binding_unchanged(&foreign_before)?;
    replacement.restore()?;

    let shown = run_success(root.path(), &["runs", "prune", "plan", "show", &plan_id])?;
    assert_ne!(
        json(&shown.stdout)?.get("state").and_then(Value::as_str),
        Some("pruned")
    );
    let operation = run_success(root.path(), &["operation", "show", "parent-swap-confirm"])?;
    assert_ne!(
        json(&operation.stdout)?
            .pointer("/operation/status")
            .and_then(Value::as_str),
        Some("committed")
    );
    let recovered = run_success(
        root.path(),
        &[
            "runs",
            "prune",
            "confirm",
            &plan_id,
            "--operation-id",
            "parent-swap-confirm",
        ],
    )?;
    assert_eq!(
        json(&recovered.stdout)?
            .get("schemaVersion")
            .and_then(Value::as_str),
        Some("lumin.retention-mutation.v1")
    );
    let recovered_plan = run_success(root.path(), &["runs", "prune", "plan", "show", &plan_id])?;
    assert_eq!(
        json(&recovered_plan.stdout)?
            .get("state")
            .and_then(Value::as_str),
        Some("pruned")
    );
    let recovered_operation =
        run_success(root.path(), &["operation", "show", "parent-swap-confirm"])?;
    assert_eq!(
        json(&recovered_operation.stdout)?
            .pointer("/operation/status")
            .and_then(Value::as_str),
        Some("committed")
    );
    assert!(!root.path().join(".lumin/runs").join(&first_run).exists());
    assert_latest_run(root.path(), &second_run)?;
    assert_run_visible(root.path(), &second_run)?;
    Ok(())
}

fn attempt_parent_swap_stops_before_gate_commit() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let baseline_run = initialize(root.path())?;
    let attempts = root.path().join(".lumin/attempts");
    let mut replacement =
        ManagedParentReplacement::prepare(root.path(), &attempts, "attempts-before-commit")?;
    let barrier = NamespaceBarrier::new(BEFORE_STORE_COMMIT)?;
    let mut pre_write = barrier.spawn(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "parent-swap-before-commit",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    let permit = barrier.accept(&mut pre_write)?;
    replacement.activate()?;
    let (visible_before, authentic_before) = replacement.snapshots()?;

    permit.release()?;
    assert_integrity_failure(&pre_write.finish()?);
    replacement.assert_unchanged(&visible_before, authentic_before.as_deref())?;
    replacement.restore()?;

    assert_operation_absent(root.path(), "parent-swap-before-commit")?;
    assert_latest_run(root.path(), &baseline_run)?;
    assert_run_visible(root.path(), &baseline_run)?;
    Ok(())
}

fn fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(
        root.path().join("src/lib.ts"),
        "export const visible = 1;\n",
    )?;
    Ok(root)
}

fn initialize(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let output = run_success(root, &["audit", "--jobs", "1"])?;
    field(&output.stdout, "runId")
}

fn run_success(
    root: &Path,
    arguments: &[&str],
) -> Result<ProcessResult, Box<dyn std::error::Error>> {
    let output = run(root, arguments)?;
    assert_status(&output, 0);
    Ok(output)
}

fn assert_run_visible(root: &Path, run_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let overview = run(root, &["overview", "--run", run_id])?;
    assert_status(&overview, 0);
    assert_eq!(
        json(&overview.stdout)?
            .pointer("/scope/id")
            .and_then(Value::as_str),
        Some(run_id)
    );
    Ok(())
}

fn assert_latest_run(root: &Path, run_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let overview = run(root, &["overview"])?;
    assert_status(&overview, 0);
    assert_eq!(
        json(&overview.stdout)?
            .pointer("/scope/id")
            .and_then(Value::as_str),
        Some(run_id)
    );
    Ok(())
}

fn assert_operation_absent(
    root: &Path,
    operation_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let operation = run(root, &["operation", "show", operation_id])?;
    assert_status(&operation, 2);
    assert!(operation.stdout.is_empty());
    assert!(operation.stderr.contains("operation does not exist"));
    Ok(())
}

fn assert_cleanup_not_committed(
    root: &Path,
    operation_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let operation = run(root, &["operation", "show", operation_id])?;
    assert_status(&operation, 0);
    assert_ne!(
        json(&operation.stdout)?
            .get("status")
            .and_then(Value::as_str),
        Some("committed")
    );
    Ok(())
}

fn assert_integrity_failure(result: &ProcessResult) {
    assert_status(result, 1);
    assert!(result.stdout.is_empty());
    assert!(
        result.stderr.contains("state namespace integrity failure"),
        "unexpected integrity diagnostic: {}",
        result.stderr
    );
}

fn json(value: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(value)
}

fn required_string<'a>(
    value: &'a Value,
    pointer: &str,
) -> Result<&'a str, Box<dyn std::error::Error>> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other(format!("missing string field {pointer}")).into())
}

fn with_context<T>(
    context: &str,
    result: Result<T, Box<dyn std::error::Error>>,
) -> Result<T, Box<dyn std::error::Error>> {
    result.map_err(|error| std::io::Error::other(format!("{context}: {error}")).into())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TreeEntry {
    relative: String,
    kind: &'static str,
    physical_identity: String,
    bytes: Vec<u8>,
}

type ReplacementSnapshots = (Vec<TreeEntry>, Option<Vec<TreeEntry>>);

#[derive(Debug, Eq, PartialEq)]
enum ForeignBindingSnapshot {
    Parent(Vec<TreeEntry>),
    Anchor {
        physical_identity: String,
        bytes: Vec<u8>,
    },
}

fn tree_snapshot(root: &Path) -> Result<Vec<TreeEntry>, Box<dyn std::error::Error>> {
    let mut entries = vec![TreeEntry {
        relative: ".".to_owned(),
        kind: "directory",
        physical_identity: physical_identity(root)?,
        bytes: Vec::new(),
    }];
    collect_tree(root, root, &mut entries)?;
    entries.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok(entries)
}

fn tree_without_subtree(entries: &[TreeEntry], subtree: &str) -> Vec<TreeEntry> {
    let child_prefix = format!("{subtree}/");
    entries
        .iter()
        .filter(|entry| entry.relative != subtree && !entry.relative.starts_with(&child_prefix))
        .cloned()
        .collect()
}

fn collect_tree(
    root: &Path,
    directory: &Path,
    entries: &mut Vec<TreeEntry>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut children = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(fs::DirEntry::file_name);
    for child in children {
        let path = child.path();
        let relative = path
            .strip_prefix(root)?
            .to_str()
            .ok_or_else(|| std::io::Error::other("test state path is not UTF-8"))?
            .replace('\\', "/");
        let file_type = child.file_type()?;
        if file_type.is_dir() {
            entries.push(TreeEntry {
                relative,
                kind: "directory",
                physical_identity: physical_identity(&path)?,
                bytes: Vec::new(),
            });
            collect_tree(root, &path, entries)?;
        } else if file_type.is_file() {
            entries.push(TreeEntry {
                relative,
                kind: "file",
                physical_identity: physical_identity(&path)?,
                bytes: fs::read(path)?,
            });
        } else {
            return Err(std::io::Error::other("test state tree contains a redirect").into());
        }
    }
    Ok(())
}

fn physical_identity(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    lumin_engine::state_entry_physical_identity_for_test(path)
        .map(|identity| format!("{identity:?}"))
        .map_err(Into::into)
}

struct FileReplacement {
    canonical: PathBuf,
    prepared: PathBuf,
    authentic: PathBuf,
    active: bool,
}

impl FileReplacement {
    fn prepare(
        canonical: &Path,
        prepared: PathBuf,
        authentic: PathBuf,
    ) -> Result<Self, std::io::Error> {
        fs::copy(canonical, &prepared)?;
        Ok(Self {
            canonical: canonical.to_path_buf(),
            prepared,
            authentic,
            active: false,
        })
    }

    fn activate(&mut self) -> Result<bool, std::io::Error> {
        match fs::rename(&self.canonical, &self.authentic) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                return Ok(false);
            }
            Err(error) => return Err(error),
        }
        if let Err(error) = fs::rename(&self.prepared, &self.canonical) {
            fs::rename(&self.authentic, &self.canonical)?;
            return Err(error);
        }
        self.active = true;
        Ok(true)
    }

    fn restore(mut self) -> Result<(), std::io::Error> {
        if self.active {
            fs::rename(&self.canonical, &self.prepared)?;
            fs::rename(&self.authentic, &self.canonical)?;
            self.active = false;
        }
        fs::remove_file(self.prepared)
    }
}

struct DirectoryReplacement {
    canonical: PathBuf,
    prepared: PathBuf,
    authentic: PathBuf,
    active: bool,
}

impl DirectoryReplacement {
    fn prepare(root: &Path, canonical: &Path, label: &str) -> Result<Self, std::io::Error> {
        let prepared = root.join(format!(".{label}.prepared"));
        let authentic = root.join(format!(".{label}.authentic"));
        copy_directory(canonical, &prepared)?;
        Ok(Self {
            canonical: canonical.to_path_buf(),
            prepared,
            authentic,
            active: false,
        })
    }

    fn activate(&mut self) -> Result<bool, std::io::Error> {
        match fs::rename(&self.canonical, &self.authentic) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                return Ok(false);
            }
            Err(error) => return Err(error),
        }
        if let Err(error) = fs::rename(&self.prepared, &self.canonical) {
            fs::rename(&self.authentic, &self.canonical)?;
            return Err(error);
        }
        self.active = true;
        Ok(true)
    }

    fn authentic_path(&self) -> &Path {
        &self.authentic
    }

    fn restore(mut self) -> Result<(), std::io::Error> {
        if self.active {
            fs::rename(&self.canonical, &self.prepared)?;
            fs::rename(&self.authentic, &self.canonical)?;
            self.active = false;
        }
        fs::remove_dir_all(self.prepared)
    }
}

struct ManagedParentReplacement {
    directory: DirectoryReplacement,
    anchor: FileReplacement,
    directory_active: bool,
}

impl ManagedParentReplacement {
    fn prepare(root: &Path, canonical: &Path, label: &str) -> Result<Self, std::io::Error> {
        Ok(Self {
            directory: DirectoryReplacement::prepare(root, canonical, label)?,
            anchor: FileReplacement::prepare(
                &canonical.join("namespace.anchor"),
                root.join(format!(".{label}-anchor.prepared")),
                root.join(format!(".{label}-anchor.authentic")),
            )?,
            directory_active: false,
        })
    }

    fn activate(&mut self) -> Result<(), std::io::Error> {
        self.directory_active = self.directory.activate()?;
        if !self.directory_active && !self.anchor.activate()? {
            return Err(std::io::Error::other(
                "platform permitted neither managed-parent nor anchor replacement",
            ));
        }
        Ok(())
    }

    fn snapshots(&self) -> Result<ReplacementSnapshots, Box<dyn std::error::Error>> {
        let visible = tree_snapshot(&self.directory.canonical)?;
        let authentic = self
            .directory_active
            .then(|| tree_snapshot(self.directory.authentic_path()))
            .transpose()?;
        Ok((visible, authentic))
    }

    fn move_destination_root(&self) -> &Path {
        if self.directory_active {
            self.directory.authentic_path()
        } else {
            &self.directory.canonical
        }
    }

    fn foreign_binding_snapshot(
        &self,
    ) -> Result<ForeignBindingSnapshot, Box<dyn std::error::Error>> {
        if self.directory_active {
            Ok(ForeignBindingSnapshot::Parent(tree_snapshot(
                &self.directory.canonical,
            )?))
        } else {
            Ok(ForeignBindingSnapshot::Anchor {
                physical_identity: physical_identity(&self.anchor.canonical)?,
                bytes: fs::read(&self.anchor.canonical)?,
            })
        }
    }

    fn assert_foreign_binding_unchanged(
        &self,
        expected: &ForeignBindingSnapshot,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let observed = self.foreign_binding_snapshot()?;
        assert_eq!(&observed, expected);
        Ok(())
    }

    fn assert_unchanged(
        &self,
        visible: &[TreeEntry],
        authentic: Option<&[TreeEntry]>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(tree_snapshot(&self.directory.canonical)?, visible);
        if let Some(authentic) = authentic {
            assert_eq!(tree_snapshot(self.directory.authentic_path())?, authentic);
        }
        Ok(())
    }

    fn restore(self) -> Result<(), std::io::Error> {
        self.anchor.restore()?;
        self.directory.restore()
    }
}

fn copy_directory(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::create_dir(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory(&source_path, &target_path)?;
        } else {
            fs::copy(source_path, target_path)?;
        }
    }
    Ok(())
}
