mod capture;
mod config_document;
mod dependency_ownership;
mod generated_config_policy;
mod package_semantics;
mod physical_path;
mod pnpm_workspace;
mod root;

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ignore::WalkBuilder;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use lumin_model::{
    ConfigAbsenceParent, ConfigObservation, ConfigSyntax, DependencyIntent, EntrySource,
    EntryUnavailableReason, Limitation, PhysicalFileIdentity, PhysicalPathRedirect, RepoPath,
    RepoPathError, RoleOverride, SOURCE_CLASSIFICATION_RULE_VERSION, ScanRole,
    SemanticConfigSnapshot, SourceClassificationRole, SourceKind, SourceRoleClassification,
    SourceRoleConfigurationSource, SourceRoleReason, SourceRoles, SourceSnapshot, digest_hex,
};
use serde::Deserialize;
use thiserror::Error;

use physical_path::{is_physical_path_redirect, observe_physical_path_redirect};

pub use generated_config_policy::{
    FieldClassification as InventoryConfigFieldClassification,
    FieldPolicy as InventoryConfigFieldPolicy, INVENTORY_CONFIG_ARTIFACT_SHA256,
    INVENTORY_CONFIG_TABLE_SHA256, INVENTORY_PACKAGE_JSON_FIELDS, INVENTORY_PNPM_WORKSPACE_FIELDS,
    INVENTORY_RESOLVER_OWNED_FIELDS,
};
pub use physical_path::{
    ConfigInputIdentity, WriteTargetError, WriteTargetKind, WriteTargetObservation,
    directory_physical_identity, inspect_write_target, observe_config_input_identity,
    observe_physical_file_identity, physical_alias_write_closure, physical_file_identity,
    rehash_existing_write_target,
};
pub use root::{RepositoryAdmission, repository_admission};

pub fn lower_native_repo_path(value: &OsStr) -> Result<RepoPath, RepoPathError> {
    RepoPath::from_native_relative(Path::new(value))
}

pub fn decode_native_repo_path_stream(bytes: &[u8]) -> Result<Vec<RepoPath>, RepoPathError> {
    RepoPath::decode_native_nul_stream(bytes)
}

pub fn is_reserved_state_path(path: &RepoPath) -> Result<bool, RepoPathError> {
    let relative = path.to_native_relative()?;
    Ok(relative.iter().next().is_some_and(reserved_state_component))
}

/// Validate caller entries BEFORE audit begins or pre-write opens/reserves a gate.
/// Reject entries whose lexical or physical path enters the reserved `.lumin` namespace,
/// or whose existing path or nearest existing parent physically escapes the canonical root.
/// Returns Err(InventoryError) on invalid entries (maps to CLI exit 2).
pub fn validate_caller_entries(root: &Path, entries: &[RepoPath]) -> Result<(), InventoryError> {
    let canonical_root = fs::canonicalize(root)
        .map_err(|error| InventoryError::RepositoryIdentity(error.to_string()))?;
    let canonical_state = canonical_reserved_state(root)?;
    for entry in entries {
        let relative = native_relative(entry)?;
        let first_component = relative.iter().next();
        if first_component.is_some_and(reserved_state_component) {
            return Err(InventoryError::ReservedEntryPath(entry.display_escaped()));
        }
        validate_entry_containment(root, &canonical_root, canonical_state.as_deref(), entry)?;
    }
    Ok(())
}

fn canonical_reserved_state(root: &Path) -> Result<Option<PathBuf>, InventoryError> {
    let state = root.join(".lumin");
    match fs::symlink_metadata(&state) {
        Ok(_) => fs::canonicalize(&state).map(Some).map_err(|error| {
            InventoryError::PhysicalIdentity(format!(
                "cannot resolve reserved .lumin namespace: {error}"
            ))
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(InventoryError::PhysicalIdentity(format!(
            "cannot inspect reserved .lumin namespace: {error}"
        ))),
    }
}

fn reserved_state_component(component: &OsStr) -> bool {
    #[cfg(windows)]
    {
        component
            .to_str()
            .is_some_and(|component| component.eq_ignore_ascii_case(".lumin"))
    }
    #[cfg(not(windows))]
    {
        component == ".lumin"
    }
}

fn validate_entry_containment(
    root: &Path,
    canonical_root: &Path,
    canonical_state: Option<&Path>,
    entry: &RepoPath,
) -> Result<(), InventoryError> {
    let mut candidate = root.join(native_relative(entry)?);
    loop {
        match fs::symlink_metadata(&candidate) {
            Ok(_) => {
                let physical = fs::canonicalize(&candidate).map_err(|error| {
                    InventoryError::PhysicalIdentity(format!(
                        "cannot resolve entry {}: {error}",
                        entry.display_escaped()
                    ))
                })?;
                if !physical.starts_with(canonical_root) {
                    return Err(InventoryError::EntryEscapesRoot(entry.display_escaped()));
                }
                if canonical_state.is_some_and(|state| physical.starts_with(state)) {
                    return Err(InventoryError::ReservedEntryPath(entry.display_escaped()));
                }
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if candidate == root || !candidate.pop() {
                    return Err(InventoryError::PhysicalIdentity(format!(
                        "cannot find an existing parent for entry {}",
                        entry.display_escaped()
                    )));
                }
            }
            Err(error) => {
                return Err(InventoryError::PhysicalIdentity(format!(
                    "cannot inspect entry {}: {error}",
                    entry.display_escaped()
                )));
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct InventoryRequest {
    pub includes: Vec<String>,
    pub excludes: Vec<String>,
    pub role_overrides: Vec<RoleOverride>,
    pub entries: Vec<RepoPath>,
    pub dependency_intents: Vec<DependencyIntent>,
}

/// Non-serde internal entry selection result used during inventory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EntrySelection {
    pub path: RepoPath,
    pub source: lumin_model::EntrySource,
    pub unavailable_reason: Option<EntryUnavailableReason>,
}

/// Observation state for a semantic policy input (root config, .gitignore files).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticPolicyState {
    Present,
    Missing,
    NonRegular,
    Unreadable,
}

/// A single semantic policy input observation: configuration file or .gitignore.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticPolicyInput {
    pub path: RepoPath,
    pub state: SemanticPolicyState,
    pub payload_sha256: Option<String>,
    pub physical_identity: Option<PhysicalFileIdentity>,
    pub absence_parent: Option<ConfigAbsenceParent>,
    pub detail: Option<String>,
}

#[derive(Clone, Debug)]
pub struct InventorySnapshot {
    pub sources: Vec<SourceSnapshot>,
    pub physical_path_redirects: Vec<PhysicalPathRedirect>,
    pub limitations: Vec<Limitation>,
    pub consulted_config_paths: Vec<RepoPath>,
    pub config: SemanticConfigSnapshot,
    pub entry_selections: Vec<EntrySelection>,
    pub policy_inputs: Vec<SemanticPolicyInput>,
}

#[derive(Debug, Error)]
pub enum InventoryError {
    #[error("repository root is not a directory: {0}")]
    InvalidRoot(String),
    #[error("reserved .lumin namespace is not a real directory")]
    ForeignStateNamespace,
    #[error("malformed configuration: {0}")]
    MalformedConfiguration(String),
    #[error("invalid scan pattern: {0}")]
    InvalidPattern(String),
    #[error("invalid repository path {path}: {source}")]
    InvalidRepoPath {
        path: String,
        #[source]
        source: RepoPathError,
    },
    #[error("failed to inspect repository root: {0}")]
    RootIo(String),
    #[error("failed to establish physical source identity: {0}")]
    PhysicalIdentity(String),
    #[error("failed to establish canonical repository identity: {0}")]
    RepositoryIdentity(String),
    #[error("caller path is in the reserved .lumin namespace: {0}")]
    ReservedEntryPath(String),
    #[error("caller path resolves outside repository root: {0}")]
    EntryEscapesRoot(String),
}

fn native_relative(path: &RepoPath) -> Result<PathBuf, InventoryError> {
    path.to_native_relative()
        .map_err(|source| InventoryError::InvalidRepoPath {
            path: path.display_escaped(),
            source,
        })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RootConfig {
    schema_version: String,
    #[serde(default)]
    entries: Vec<String>,
    #[serde(default)]
    scan: ScanConfig,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScanConfig {
    #[serde(default)]
    include: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default)]
    roles: Vec<RoleConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleConfig {
    pattern: String,
    role: String,
}

struct PatternSet {
    includes: Vec<Gitignore>,
    excludes: Vec<Gitignore>,
    config_roles: Vec<(Gitignore, ScanRole)>,
    invocation_roles: Vec<(Gitignore, ScanRole)>,
}

#[derive(Default)]
struct CollectedFiles {
    sources: BTreeMap<RepoPath, SourceSnapshot>,
    payloads: BTreeMap<PhysicalFileIdentity, Arc<[u8]>>,
    physical_path_redirects: BTreeMap<RepoPath, PhysicalPathRedirect>,
    config_observations: BTreeMap<RepoPath, ConfigObservation>,
    limitations: Vec<Limitation>,
    consulted_config_paths: Vec<RepoPath>,
}

struct FileObservationContext<'a> {
    root: &'a Path,
    canonical_root: &'a Path,
    patterns: &'a PatternSet,
    ignore: &'a ApplicableIgnore,
}

pub struct PendingInventoryScan {
    canonical_root: PathBuf,
    snapshot: InventorySnapshot,
    dependency_plan: dependency_ownership::DependencyOwnershipPlan,
}

impl PendingInventoryScan {
    pub fn dependency_input_paths(&self) -> &[RepoPath] {
        self.dependency_plan.input_paths()
    }

    pub fn finish(mut self, root: &Path) -> Result<InventorySnapshot, InventoryError> {
        let canonical_root = fs::canonicalize(root)
            .map_err(|error| InventoryError::RepositoryIdentity(error.to_string()))?;
        if canonical_root != self.canonical_root {
            return Err(InventoryError::RepositoryIdentity(
                "pending inventory scan was finished against a different repository root"
                    .to_owned(),
            ));
        }
        self.snapshot
            .policy_inputs
            .extend(dependency_ownership::capture(
                root,
                self.dependency_plan,
                &mut self.snapshot.config,
                &mut self.snapshot.limitations,
            )?);
        Ok(self.snapshot)
    }
}

pub fn scan(root: &Path, request: &InventoryRequest) -> Result<InventorySnapshot, InventoryError> {
    begin_scan(root, request)?.finish(root)
}

pub fn begin_scan(
    root: &Path,
    request: &InventoryRequest,
) -> Result<PendingInventoryScan, InventoryError> {
    validate_root(root)?;
    let canonical_root = fs::canonicalize(root)
        .map_err(|error| InventoryError::RepositoryIdentity(error.to_string()))?;
    let (config, config_path, config_policy) = read_root_config(root)?;
    let patterns = PatternSet::compile(root, config.as_ref(), request)?;

    // Build hierarchical gitignore matcher before entry classification
    let ignore = ApplicableIgnore::build(root)?;
    let observation_context = FileObservationContext {
        root,
        canonical_root: &canonical_root,
        patterns: &patterns,
        ignore: &ignore,
    };

    let mut collected = collect_repository_files(&observation_context)?;

    if let Some(path) = config_path.clone() {
        collected.consulted_config_paths.push(path);
    }
    collected.consulted_config_paths.sort();
    collected.consulted_config_paths.dedup();

    // Determine entry selections: caller entries replace config entries
    let (raw_entries, entry_source) = if !request.entries.is_empty() {
        (request.entries.clone(), EntrySource::Invocation)
    } else {
        let config_entries = config
            .as_ref()
            .map(|cfg| cfg.entries.clone())
            .unwrap_or_default();
        if config_entries.is_empty() {
            (Vec::new(), EntrySource::Configuration)
        } else {
            let parsed: Result<Vec<RepoPath>, _> = config_entries
                .iter()
                .map(|entry| RepoPath::from_portable(entry))
                .collect();
            (
                parsed
                    .map_err(|error| InventoryError::MalformedConfiguration(error.to_string()))?,
                EntrySource::Configuration,
            )
        }
    };

    // Lexical sort/dedup
    let mut deduplicated_entries = raw_entries;
    deduplicated_entries.sort();
    deduplicated_entries.dedup();

    // Classify entries and record all (available + unavailable) with reason
    let mut entry_selections = Vec::new();
    for entry_path in &deduplicated_entries {
        let classification = classify_entry(root, entry_path, &patterns, &ignore)?;
        match classification {
            EntryClassification::Available => {
                if !collected.sources.contains_key(entry_path) {
                    let relative = native_relative(entry_path)?;
                    let native_path = root.join(&relative);
                    collected.observe_file(
                        &observation_context,
                        &native_path,
                        &relative,
                        entry_path.clone(),
                    )?;
                }
                entry_selections.push(EntrySelection {
                    path: entry_path.clone(),
                    source: entry_source,
                    unavailable_reason: None,
                });
            }
            EntryClassification::Unavailable(unavailable_reason) => {
                collected
                    .limitations
                    .push(Limitation::ExplicitEntryUnavailable {
                        path: entry_path.display_escaped(),
                        source: entry_source,
                        unavailable_reason,
                    });
                entry_selections.push(EntrySelection {
                    path: entry_path.clone(),
                    source: entry_source,
                    unavailable_reason: Some(unavailable_reason),
                });
            }
        }
    }

    // Collect policy inputs: lumin.json + all applicable .gitignore files from matcher
    let mut policy_inputs = vec![config_policy];
    policy_inputs.extend(ignore.policy_inputs.iter().cloned());

    policy_inputs.extend(dependency_ownership::capture_owner_candidates(
        root,
        &request.dependency_intents,
        &mut collected.config_observations,
        &mut collected.consulted_config_paths,
        &mut collected.limitations,
    )?);
    collected.consulted_config_paths.sort();
    collected.consulted_config_paths.dedup();

    let sources = collected.sources.into_values().collect::<Vec<_>>();
    let config = package_semantics::build(
        collected.config_observations,
        &sources,
        &mut collected.limitations,
    )
    .map_err(InventoryError::MalformedConfiguration)?;
    let dependency_plan = dependency_ownership::plan(
        root,
        &request.dependency_intents,
        &config,
        &mut collected.limitations,
    )?;

    Ok(PendingInventoryScan {
        canonical_root,
        snapshot: InventorySnapshot {
            sources,
            physical_path_redirects: collected.physical_path_redirects.into_values().collect(),
            limitations: collected.limitations,
            consulted_config_paths: collected.consulted_config_paths,
            config,
            entry_selections,
            policy_inputs,
        },
        dependency_plan,
    })
}

pub fn dependency_owner_candidate_paths(
    intents: &[DependencyIntent],
) -> Result<Vec<RepoPath>, InventoryError> {
    dependency_ownership::reservation_paths(intents)
}

pub fn dependency_input_payload_sha256(
    root: &Path,
    path: &RepoPath,
) -> Result<String, InventoryError> {
    dependency_ownership::present_input_payload_sha256(root, path)
}

enum EntryClassification {
    Available,
    Unavailable(EntryUnavailableReason),
}

fn classify_entry(
    root: &Path,
    path: &RepoPath,
    patterns: &PatternSet,
    ignore: &ApplicableIgnore,
) -> Result<EntryClassification, InventoryError> {
    let relative = native_relative(path)?;
    if is_hard_excluded(&relative) || relative.iter().any(|c| is_hard_excluded(Path::new(c))) {
        return Ok(EntryClassification::Unavailable(
            EntryUnavailableReason::HardExcluded,
        ));
    }
    if patterns.excludes.iter().any(|pattern| {
        pattern
            .matched_path_or_any_parents(&relative, false)
            .is_ignore()
    }) {
        return Ok(EntryClassification::Unavailable(
            EntryUnavailableReason::Excluded,
        ));
    }
    let explicitly_included = !patterns.includes.is_empty()
        && patterns.includes.iter().any(|pattern| {
            pattern
                .matched_path_or_any_parents(&relative, false)
                .is_ignore()
        });
    if !patterns.includes.is_empty() && !explicitly_included {
        return Ok(EntryClassification::Unavailable(
            EntryUnavailableReason::OutOfDomain,
        ));
    }
    if source_kind(&relative).is_none() {
        return Ok(EntryClassification::Unavailable(
            EntryUnavailableReason::OutOfDomain,
        ));
    }
    if !explicitly_included && ignore.is_ignored(&relative, false) {
        return Ok(EntryClassification::Unavailable(
            EntryUnavailableReason::Ignored,
        ));
    }

    let native = root.join(&relative);
    let metadata = match fs::symlink_metadata(&native) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(EntryClassification::Unavailable(
                EntryUnavailableReason::Missing,
            ));
        }
        Err(error) => {
            return Err(InventoryError::RootIo(format!(
                "failed to inspect entry {}: {error}",
                path.display_escaped()
            )));
        }
    };
    if metadata.file_type().is_symlink() {
        let canonical_root =
            fs::canonicalize(root).map_err(|error| InventoryError::RootIo(error.to_string()))?;
        let target = fs::canonicalize(&native).map_err(|error| {
            InventoryError::PhysicalIdentity(format!(
                "failed to resolve entry {}: {error}",
                path.display_escaped()
            ))
        })?;
        if !target.starts_with(&canonical_root) {
            return Err(InventoryError::EntryEscapesRoot(path.display_escaped()));
        }
        let target_metadata = fs::metadata(&native).map_err(|error| {
            InventoryError::PhysicalIdentity(format!(
                "failed to inspect entry target {}: {error}",
                path.display_escaped()
            ))
        })?;
        return Ok(if target_metadata.is_file() {
            EntryClassification::Available
        } else {
            EntryClassification::Unavailable(EntryUnavailableReason::OutOfDomain)
        });
    }
    Ok(if metadata.is_file() {
        EntryClassification::Available
    } else {
        EntryClassification::Unavailable(EntryUnavailableReason::OutOfDomain)
    })
}

/// Hierarchical .gitignore matcher built deterministically from root-to-leaf.
/// Captures all applicable .gitignore files as policy inputs (with exact bytes/hash/identity).
/// Skips directories already ignored by an ancestor. Skips symlink directories.
/// Never swallows read errors.
#[derive(Clone, Debug)]
pub struct ApplicableIgnore {
    matcher: Gitignore,
    pub policy_inputs: Vec<SemanticPolicyInput>,
}

impl ApplicableIgnore {
    /// Build the hierarchical .gitignore matcher. Walks root-to-leaf, building one combined
    /// Gitignore that respects source-relative ordering/negation.
    /// Returns error on unreadable .gitignore, read_dir failure, parser/build error, or
    /// physical identity failure.
    pub fn build(root: &Path) -> Result<Self, InventoryError> {
        let mut builder = GitignoreBuilder::new(root);
        let mut policy_inputs = Vec::new();
        Self::walk_gitignores(root, root, &mut builder, &mut policy_inputs)?;
        let matcher = builder
            .build()
            .map_err(|error| InventoryError::InvalidPattern(error.to_string()))?;
        Ok(Self {
            matcher,
            policy_inputs,
        })
    }

    /// Check if a relative path is ignored by the hierarchical .gitignore.
    pub fn is_ignored(&self, relative: &Path, is_dir: bool) -> bool {
        self.matcher
            .matched_path_or_any_parents(relative, is_dir)
            .is_ignore()
    }

    fn walk_gitignores(
        root: &Path,
        dir: &Path,
        builder: &mut GitignoreBuilder,
        policy_inputs: &mut Vec<SemanticPolicyInput>,
    ) -> Result<(), InventoryError> {
        let gitignore_path = dir.join(".gitignore");
        match fs::read(&gitignore_path) {
            Ok(bytes) => {
                let relative = dir.strip_prefix(root).unwrap_or(Path::new(""));
                let repo_path = gitignore_repo_path(relative)?;
                let payload_sha256 = digest_hex(&bytes);
                let physical_identity = physical_file_identity(&gitignore_path)?;
                policy_inputs.push(SemanticPolicyInput {
                    path: repo_path,
                    state: SemanticPolicyState::Present,
                    payload_sha256: Some(payload_sha256),
                    physical_identity: Some(physical_identity),
                    absence_parent: None,
                    detail: None,
                });
                let content = std::str::from_utf8(&bytes).map_err(|error| {
                    InventoryError::InvalidPattern(format!(
                        "{} is not valid UTF-8: {error}",
                        gitignore_path.display()
                    ))
                })?;
                for line in content.lines() {
                    builder
                        .add_line(Some(gitignore_path.clone()), line)
                        .map_err(|error| InventoryError::InvalidPattern(error.to_string()))?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(InventoryError::PhysicalIdentity(format!(
                    "unreadable .gitignore at {}: {error}",
                    gitignore_path.display()
                )));
            }
        }
        // Recurse into subdirectories. Skip: hard-excluded dirs, symlink dirs,
        // and directories that the current builder would ignore.
        let entries = fs::read_dir(dir).map_err(|error| {
            InventoryError::RootIo(format!(
                "failed to read directory {}: {error}",
                dir.display()
            ))
        })?;
        // Build a snapshot of the current gitignore state to check if subdirs are ignored
        let current_matcher = builder
            .build()
            .map_err(|error| InventoryError::InvalidPattern(error.to_string()))?;
        // Re-add all lines so far since build() consumes nothing (builder is reusable)
        // Actually GitignoreBuilder::build() does not consume the builder in the ignore crate;
        // it clones internal state. So the builder remains valid for further additions.
        for entry_result in entries {
            let entry = entry_result.map_err(|error| {
                InventoryError::RootIo(format!(
                    "read_dir entry error in {}: {error}",
                    dir.display()
                ))
            })?;
            let entry_path = entry.path();
            let file_type = entry.file_type().map_err(|error| {
                InventoryError::RootIo(format!(
                    "failed to determine file type for {}: {error}",
                    entry_path.display()
                ))
            })?;
            // Skip symlink directories
            if file_type.is_symlink() {
                continue;
            }
            if !file_type.is_dir() {
                continue;
            }
            // Skip hard-excluded directories
            if is_hard_excluded(&entry_path) {
                continue;
            }
            // Skip directories already ignored by an ancestor .gitignore
            let dir_relative = entry_path.strip_prefix(root).unwrap_or(Path::new(""));
            if current_matcher
                .matched_path_or_any_parents(dir_relative, true)
                .is_ignore()
            {
                continue;
            }
            Self::walk_gitignores(root, &entry_path, builder, policy_inputs)?;
        }
        Ok(())
    }
}

/// Convert a relative directory path to the .gitignore RepoPath.
fn gitignore_repo_path(relative: &Path) -> Result<RepoPath, InventoryError> {
    if relative == Path::new("") {
        RepoPath::from_portable(".gitignore").map_err(|source| InventoryError::InvalidRepoPath {
            path: ".gitignore".to_owned(),
            source,
        })
    } else {
        let native_relative = relative.join(".gitignore");
        RepoPath::from_native_relative(&native_relative).map_err(|source| {
            InventoryError::InvalidRepoPath {
                path: native_relative.display().to_string(),
                source,
            }
        })
    }
}

fn read_root_config(
    root: &Path,
) -> Result<(Option<RootConfig>, Option<RepoPath>, SemanticPolicyInput), InventoryError> {
    let path = root.join("lumin.json");
    let repo_path = RepoPath::from_portable("lumin.json")
        .map_err(|error| InventoryError::MalformedConfiguration(error.to_string()))?;
    // Check symlink/nonregular before reading
    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(InventoryError::MalformedConfiguration(
                    "lumin.json is a symlink or non-regular file".to_owned(),
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let policy = SemanticPolicyInput {
                path: repo_path,
                state: SemanticPolicyState::Missing,
                payload_sha256: None,
                physical_identity: None,
                absence_parent: None,
                detail: None,
            };
            return Ok((None, None, policy));
        }
        Err(error) => return Err(InventoryError::MalformedConfiguration(error.to_string())),
    }
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => return Err(InventoryError::MalformedConfiguration(error.to_string())),
    };
    // Compute observation from the SAME captured bytes — do not re-read
    let payload_sha256 = digest_hex(&bytes);
    // Real file identity failure is an error, never `.ok()`
    let physical_identity = physical_file_identity(&path)?;
    let policy = SemanticPolicyInput {
        path: repo_path.clone(),
        state: SemanticPolicyState::Present,
        payload_sha256: Some(payload_sha256),
        physical_identity: Some(physical_identity),
        absence_parent: None,
        detail: None,
    };
    let config: RootConfig = serde_json::from_slice(&bytes)
        .map_err(|error| InventoryError::MalformedConfiguration(error.to_string()))?;
    if config.schema_version != "lumin-config.v1" {
        return Err(InventoryError::MalformedConfiguration(format!(
            "unsupported schemaVersion {}",
            config.schema_version
        )));
    }
    for entry in &config.entries {
        RepoPath::from_portable(entry)
            .map_err(|error| InventoryError::MalformedConfiguration(error.to_string()))?;
    }
    Ok((Some(config), Some(repo_path), policy))
}

pub fn is_supported_source_path(path: &RepoPath) -> bool {
    native_relative(path)
        .ok()
        .is_some_and(|native| source_kind(&native).is_some())
}

fn collect_repository_files(
    context: &FileObservationContext<'_>,
) -> Result<CollectedFiles, InventoryError> {
    let mut collected = CollectedFiles::default();
    let pruned_redirects = Arc::new(Mutex::new(Vec::new()));
    let captured_pruned_redirects = Arc::clone(&pruned_redirects);
    let mut builder = WalkBuilder::new(context.root);
    builder
        .hidden(false)
        .parents(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .follow_links(false)
        .filter_entry(move |entry| {
            let hard_excluded = is_hard_excluded(entry.path());
            let file_type = entry.file_type().or_else(|| {
                fs::symlink_metadata(entry.path())
                    .ok()
                    .map(|metadata| metadata.file_type())
            });
            let Some(file_type) = file_type else {
                if let Ok(mut redirects) = captured_pruned_redirects.lock() {
                    redirects.push(entry.path().to_owned());
                }
                return false;
            };
            let redirect = is_physical_path_redirect(entry.path(), &file_type);
            if hard_excluded
                || (redirect
                    && !fs::metadata(entry.path()).is_ok_and(|metadata| metadata.is_file()))
            {
                if redirect && let Ok(mut redirects) = captured_pruned_redirects.lock() {
                    redirects.push(entry.path().to_owned());
                }
                return false;
            }
            true
        });

    for result in builder.build() {
        let entry = match result {
            Ok(entry) => entry,
            Err(error) => {
                collected
                    .limitations
                    .push(Limitation::SourcePayloadUnavailable {
                        path: context.root.display().to_string(),
                        detail: error.to_string(),
                    });
                continue;
            }
        };
        let Ok(relative) = entry.path().strip_prefix(context.root) else {
            return Err(InventoryError::RootIo(format!(
                "walked path escaped root: {}",
                entry.path().display()
            )));
        };
        let path = RepoPath::from_native_relative(relative).map_err(|source| {
            InventoryError::InvalidRepoPath {
                path: relative.display().to_string(),
                source,
            }
        })?;
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        let is_redirect = is_physical_path_redirect(entry.path(), &file_type);
        if is_redirect {
            record_physical_path_redirect(context, entry.path(), path.clone(), &mut collected);
        }
        let is_file = if file_type.is_file() {
            true
        } else if is_redirect {
            match fs::metadata(entry.path()) {
                Ok(metadata) if metadata.is_file() => match fs::canonicalize(entry.path()) {
                    Ok(target) if target.starts_with(context.canonical_root) => true,
                    Ok(_) => {
                        collected
                            .limitations
                            .push(Limitation::SourcePayloadUnavailable {
                                path: path.display_escaped(),
                                detail: "source alias resolves outside the repository root"
                                    .to_owned(),
                            });
                        false
                    }
                    Err(error) => {
                        collected
                            .limitations
                            .push(Limitation::SourcePayloadUnavailable {
                                path: path.display_escaped(),
                                detail: error.to_string(),
                            });
                        false
                    }
                },
                Ok(_) => false,
                Err(error) => {
                    collected
                        .limitations
                        .push(Limitation::SourcePayloadUnavailable {
                            path: path.display_escaped(),
                            detail: error.to_string(),
                        });
                    false
                }
            }
        } else {
            false
        };
        if !is_file {
            continue;
        }
        collected.observe_file(context, entry.path(), relative, path)?;
    }
    let mut pruned_redirects = pruned_redirects
        .lock()
        .map_err(|_| InventoryError::RootIo("pruned redirect capture failed".to_owned()))?
        .clone();
    pruned_redirects.sort();
    pruned_redirects.dedup();
    for native_path in pruned_redirects {
        let relative = native_path.strip_prefix(context.root).map_err(|_| {
            InventoryError::RootIo(format!(
                "pruned redirect escaped root: {}",
                native_path.display()
            ))
        })?;
        let path = RepoPath::from_native_relative(relative).map_err(|source| {
            InventoryError::InvalidRepoPath {
                path: relative.display().to_string(),
                source,
            }
        })?;
        record_physical_path_redirect(context, &native_path, path, &mut collected);
    }
    Ok(collected)
}

fn record_physical_path_redirect(
    context: &FileObservationContext<'_>,
    native_path: &Path,
    path: RepoPath,
    collected: &mut CollectedFiles,
) {
    let (redirect, target_error) =
        observe_physical_path_redirect(context.canonical_root, native_path, path.clone());
    if let Some(detail) = target_error {
        collected
            .limitations
            .push(Limitation::SourcePayloadUnavailable {
                path: path.display_escaped(),
                detail,
            });
    }
    collected.physical_path_redirects.insert(path, redirect);
}

impl CollectedFiles {
    fn observe_file(
        &mut self,
        context: &FileObservationContext<'_>,
        native_path: &Path,
        relative: &Path,
        path: RepoPath,
    ) -> Result<(), InventoryError> {
        if let Some(syntax) = config_syntax(relative) {
            self.consulted_config_paths.push(path.clone());
            let capture = capture_config(context.root, &path, syntax)?;
            if let Some(limitation) = capture.limitation {
                self.limitations.push(limitation);
            }
            self.config_observations.insert(path, capture.observation);
            return Ok(());
        }
        if !context.patterns.admits(relative) {
            return Ok(());
        }
        if context.patterns.includes.is_empty() && context.ignore.is_ignored(relative, false) {
            return Ok(());
        }
        let Some(kind) = source_kind(relative) else {
            return Ok(());
        };

        let logical_path = path.display_escaped();
        let mut opened =
            match capture::OpenedSource::open(context.canonical_root, native_path, &logical_path) {
                Ok(opened) => opened,
                Err(error) => {
                    self.limitations.push(Limitation::SourcePayloadUnavailable {
                        path: logical_path,
                        detail: error.to_string(),
                    });
                    return Ok(());
                }
            };
        let physical_identity = opened.physical_identity().clone();
        let bytes = match self.payloads.get(&physical_identity) {
            Some(bytes) => Arc::clone(bytes),
            None => match opened.read_payload(&logical_path) {
                Ok(bytes) => {
                    self.payloads
                        .insert(physical_identity.clone(), Arc::clone(&bytes));
                    bytes
                }
                Err(error) => {
                    self.limitations.push(Limitation::SourcePayloadUnavailable {
                        path: logical_path,
                        detail: error.to_string(),
                    });
                    return Ok(());
                }
            },
        };
        if let Err(error) =
            opened.validate_path(context.canonical_root, native_path, &path.display_escaped())
        {
            self.limitations.push(Limitation::SourcePayloadUnavailable {
                path: path.display_escaped(),
                detail: error.to_string(),
            });
            return Ok(());
        }
        let roles = classify_roles(&path, relative, kind, &bytes, context.patterns)?;
        self.sources.insert(
            path.clone(),
            SourceSnapshot::new(path, kind, roles, physical_identity, bytes),
        );
        Ok(())
    }
}

pub struct ConfigCapture {
    pub observation: ConfigObservation,
    pub limitation: Option<Limitation>,
}

pub fn capture_config(
    root: &Path,
    path: &RepoPath,
    syntax: ConfigSyntax,
) -> Result<ConfigCapture, InventoryError> {
    let observation = observe_config(root, path, syntax)?;
    let limitation = config_capture_limitation(&observation, syntax);
    Ok(ConfigCapture {
        observation,
        limitation,
    })
}

fn config_capture_limitation(
    observation: &ConfigObservation,
    syntax: ConfigSyntax,
) -> Option<Limitation> {
    let path = match observation {
        ConfigObservation::Unreadable { path, .. } | ConfigObservation::NonRegular { path, .. } => {
            path.display_escaped()
        }
        ConfigObservation::Present { .. } | ConfigObservation::Missing { .. } => return None,
    };
    match (syntax, observation) {
        (ConfigSyntax::StrictJson, ConfigObservation::Unreadable { detail, .. }) => {
            Some(Limitation::PackageMetadataUnobservable {
                path,
                detail: detail.clone(),
            })
        }
        (ConfigSyntax::StrictJson, ConfigObservation::NonRegular { .. }) => {
            Some(Limitation::PackageMetadataUnobservable {
                path,
                detail: "package manifest is not a regular file".to_owned(),
            })
        }
        (ConfigSyntax::Jsonc, ConfigObservation::Unreadable { detail, .. }) => {
            Some(Limitation::TsconfigPayloadUnavailable {
                path,
                detail: detail.clone(),
            })
        }
        (ConfigSyntax::RestrictedYaml, ConfigObservation::Unreadable { detail, .. }) => {
            Some(Limitation::WorkspaceOwnershipUnsupported {
                path,
                detail: detail.clone(),
            })
        }
        (ConfigSyntax::RestrictedYaml, ConfigObservation::NonRegular { .. }) => {
            Some(Limitation::WorkspaceOwnershipUnsupported {
                path,
                detail: "pnpm workspace configuration is not a regular file".to_owned(),
            })
        }
        (ConfigSyntax::Jsonc, ConfigObservation::NonRegular { .. })
        | (_, ConfigObservation::Present { .. } | ConfigObservation::Missing { .. }) => None,
    }
}

fn observe_config(
    root: &Path,
    path: &RepoPath,
    syntax: ConfigSyntax,
) -> Result<ConfigObservation, InventoryError> {
    validate_root(root)?;
    let native = root.join(native_relative(path)?);
    let input_identity = observe_config_input_identity(root, path)?;
    if let Some(parent) = input_identity.absence_parent {
        return Ok(ConfigObservation::Missing {
            path: path.clone(),
            parent,
        });
    }
    let metadata = match fs::symlink_metadata(&native) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(InventoryError::PhysicalIdentity(format!(
                "config path changed after identity capture: {}",
                path.display_escaped()
            )));
        }
        Err(error) => {
            return Ok(ConfigObservation::Unreadable {
                path: path.clone(),
                detail: error.to_string(),
                physical_identity: input_identity.physical_identity,
            });
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Ok(ConfigObservation::NonRegular {
            path: path.clone(),
            physical_identity: input_identity.physical_identity,
        });
    }
    let mut file = match fs::File::open(&native) {
        Ok(file) => file,
        Err(error) => {
            return Ok(ConfigObservation::Unreadable {
                path: path.clone(),
                detail: error.to_string(),
                physical_identity: input_identity.physical_identity,
            });
        }
    };
    let physical_identity = capture::physical_identity_from_file(&file)?;
    let mut bytes = Vec::new();
    if let Err(error) = file.read_to_end(&mut bytes) {
        return Ok(ConfigObservation::Unreadable {
            path: path.clone(),
            detail: error.to_string(),
            physical_identity: Some(physical_identity),
        });
    }
    let current_identity = observe_config_input_identity(root, path)?;
    if current_identity.physical_identity.as_ref() != Some(&physical_identity)
        || current_identity.absence_parent.is_some()
    {
        return Err(InventoryError::PhysicalIdentity(format!(
            "config path changed physical identity during capture: {}",
            path.display_escaped()
        )));
    }
    let parsed = match syntax {
        ConfigSyntax::StrictJson | ConfigSyntax::Jsonc => {
            config_document::parse(path.clone(), &bytes, syntax)
        }
        ConfigSyntax::RestrictedYaml => pnpm_workspace::parse(path.clone(), &bytes),
    };
    let document = parsed.map_err(|error| {
        InventoryError::MalformedConfiguration(format!("{}: {error}", path.display_escaped()))
    })?;
    Ok(ConfigObservation::Present {
        document,
        physical_identity,
    })
}

fn config_syntax(path: &Path) -> Option<ConfigSyntax> {
    match path.file_name().and_then(OsStr::to_str) {
        Some("package.json") => Some(ConfigSyntax::StrictJson),
        Some("tsconfig.json" | "jsconfig.json") => Some(ConfigSyntax::Jsonc),
        Some("pnpm-workspace.yaml") => Some(ConfigSyntax::RestrictedYaml),
        _ => None,
    }
}

fn validate_root(root: &Path) -> Result<(), InventoryError> {
    let metadata = fs::metadata(root).map_err(|error| InventoryError::RootIo(error.to_string()))?;
    if !metadata.is_dir() {
        return Err(InventoryError::InvalidRoot(root.display().to_string()));
    }

    let state = root.join(".lumin");
    match fs::symlink_metadata(&state) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(InventoryError::ForeignStateNamespace)
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(InventoryError::RootIo(error.to_string())),
    }
}

impl PatternSet {
    fn compile(
        root: &Path,
        config: Option<&RootConfig>,
        request: &InventoryRequest,
    ) -> Result<Self, InventoryError> {
        let configured_includes = config
            .map(|value| value.scan.include.as_slice())
            .unwrap_or_default();
        let includes = if request.includes.is_empty() {
            compile_patterns(root, configured_includes)?
        } else {
            compile_patterns(root, &request.includes)?
        };

        let mut exclude_patterns = request.excludes.clone();
        if let Some(config) = config {
            exclude_patterns.extend(config.scan.exclude.iter().cloned());
        }

        let mut config_roles = Vec::new();
        if let Some(config) = config {
            for role in &config.scan.roles {
                config_roles.push((
                    compile_pattern(root, &role.pattern)?,
                    parse_role(&role.role)?,
                ));
            }
        }
        let mut invocation_roles = Vec::new();
        for role in &request.role_overrides {
            invocation_roles.push((compile_pattern(root, &role.pattern)?, role.role));
        }

        Ok(Self {
            includes,
            excludes: compile_patterns(root, &exclude_patterns)?,
            config_roles,
            invocation_roles,
        })
    }

    fn admits(&self, relative: &Path) -> bool {
        if self.excludes.iter().any(|pattern| {
            pattern
                .matched_path_or_any_parents(relative, false)
                .is_ignore()
        }) {
            return false;
        }
        self.includes.is_empty()
            || self.includes.iter().any(|pattern| {
                pattern
                    .matched_path_or_any_parents(relative, false)
                    .is_ignore()
            })
    }
}

fn compile_patterns(root: &Path, patterns: &[String]) -> Result<Vec<Gitignore>, InventoryError> {
    patterns
        .iter()
        .map(|pattern| compile_pattern(root, pattern))
        .collect()
}

fn compile_pattern(root: &Path, pattern: &str) -> Result<Gitignore, InventoryError> {
    if pattern.is_empty() || pattern.starts_with('!') || pattern.contains("..") {
        return Err(InventoryError::InvalidPattern(pattern.to_owned()));
    }
    let mut builder = GitignoreBuilder::new(root);
    builder
        .add_line(None, pattern)
        .map_err(|error| InventoryError::InvalidPattern(error.to_string()))?;
    builder
        .build()
        .map_err(|error| InventoryError::InvalidPattern(error.to_string()))
}

fn parse_role(value: &str) -> Result<ScanRole, InventoryError> {
    match value {
        "test" => Ok(ScanRole::Test),
        "production" => Ok(ScanRole::Production),
        "generated" => Ok(ScanRole::Generated),
        "vendor" => Ok(ScanRole::Vendor),
        "authored" => Ok(ScanRole::Authored),
        _ => Err(InventoryError::MalformedConfiguration(format!(
            "unknown source role {value}"
        ))),
    }
}

fn classify_roles(
    path: &RepoPath,
    relative: &Path,
    kind: SourceKind,
    bytes: &[u8],
    patterns: &PatternSet,
) -> Result<SourceRoles, InventoryError> {
    let test_like = default_test_role(relative);
    let generated = generated_marker(bytes).then_some(SourceRoleReason::LeadingGeneratedComment);
    let declaration = kind.is_declaration();
    let mut classifications = Vec::new();

    if let Some(reason) = test_like {
        push_classification(
            &mut classifications,
            SourceClassificationRole::Test,
            reason,
            SourceRoleConfigurationSource::CompiledDefault,
        );
    }
    if let Some(reason) = generated {
        push_classification(
            &mut classifications,
            SourceClassificationRole::Generated,
            reason,
            SourceRoleConfigurationSource::CompiledDefault,
        );
    }
    if declaration {
        push_classification(
            &mut classifications,
            SourceClassificationRole::Declaration,
            SourceRoleReason::DeclarationExtension,
            SourceRoleConfigurationSource::CompiledDefault,
        );
    }

    apply_roles(
        &mut classifications,
        path,
        relative,
        &patterns.config_roles,
        SourceRoleConfigurationSource::Configuration,
    )?;
    apply_roles(
        &mut classifications,
        path,
        relative,
        &patterns.invocation_roles,
        SourceRoleConfigurationSource::Invocation,
    )?;
    Ok(SourceRoles::from_classifications(classifications))
}

fn apply_roles(
    classifications: &mut Vec<SourceRoleClassification>,
    path: &RepoPath,
    relative: &Path,
    rules: &[(Gitignore, ScanRole)],
    configuration_source: SourceRoleConfigurationSource,
) -> Result<(), InventoryError> {
    let matched = [
        ScanRole::Test,
        ScanRole::Production,
        ScanRole::Generated,
        ScanRole::Vendor,
        ScanRole::Authored,
    ]
    .into_iter()
    .filter(|candidate| {
        rules.iter().any(|(pattern, role)| {
            role == candidate
                && pattern
                    .matched_path_or_any_parents(relative, false)
                    .is_ignore()
        })
    })
    .collect::<Vec<_>>();

    for (left, right) in [
        (ScanRole::Test, ScanRole::Production),
        (ScanRole::Generated, ScanRole::Authored),
        (ScanRole::Vendor, ScanRole::Authored),
    ] {
        if matched.contains(&left) && matched.contains(&right) {
            let tier = match configuration_source {
                SourceRoleConfigurationSource::Configuration => "configuration",
                SourceRoleConfigurationSource::Invocation => "invocation",
                SourceRoleConfigurationSource::CompiledDefault => "compiled-default",
            };
            return Err(InventoryError::MalformedConfiguration(format!(
                "contradictory {tier} source role declarations for {}: {} conflicts with {}",
                path.display_escaped(),
                role_name(left),
                role_name(right)
            )));
        }
    }

    for role in matched {
        let reason = match role {
            ScanRole::Test => SourceRoleReason::ExplicitTestRole,
            ScanRole::Production => SourceRoleReason::ExplicitProductionRole,
            ScanRole::Generated => SourceRoleReason::ExplicitGeneratedRole,
            ScanRole::Vendor => SourceRoleReason::ExplicitVendorRole,
            ScanRole::Authored => SourceRoleReason::ExplicitAuthoredRole,
        };
        push_classification(classifications, role.into(), reason, configuration_source);
    }
    Ok(())
}

fn role_name(role: ScanRole) -> &'static str {
    match role {
        ScanRole::Test => "test",
        ScanRole::Production => "production",
        ScanRole::Generated => "generated",
        ScanRole::Vendor => "vendor",
        ScanRole::Authored => "authored",
    }
}

fn push_classification(
    classifications: &mut Vec<SourceRoleClassification>,
    role: SourceClassificationRole,
    reason: SourceRoleReason,
    configuration_source: SourceRoleConfigurationSource,
) {
    classifications.push(SourceRoleClassification {
        role,
        rule_version: SOURCE_CLASSIFICATION_RULE_VERSION.to_owned(),
        reason,
        configuration_source,
    });
}

fn default_test_role(path: &Path) -> Option<SourceRoleReason> {
    if path.components().any(|component| {
        let value = component.as_os_str();
        value == "test" || value == "tests" || value == "__tests__" || value == "__mocks__"
    }) {
        return Some(SourceRoleReason::TestPathRule);
    }
    let file_name = path.file_name()?;
    let stem = Path::new(file_name).file_stem()?;
    if os_ends_with_ascii(stem, ".test") || os_ends_with_ascii(stem, ".spec") {
        Some(SourceRoleReason::TestBasenameRule)
    } else {
        None
    }
}

fn generated_marker(bytes: &[u8]) -> bool {
    let prefix = &bytes[..bytes.len().min(2048)];
    let prefix = prefix.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(prefix);
    let prefix = prefix
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .map_or(&[][..], |index| &prefix[index..]);
    if let Some(line) = prefix.strip_prefix(b"//") {
        let line = line.split(|byte| *byte == b'\n').next().unwrap_or(line);
        return line
            .windows(b"@generated".len())
            .any(|part| part == b"@generated");
    }
    if let Some(comment) = prefix.strip_prefix(b"/*")
        && let Some(end) = comment.windows(2).position(|part| part == b"*/")
    {
        return comment[..end]
            .windows(b"@generated".len())
            .any(|part| part == b"@generated");
    }
    false
}

fn is_hard_excluded(path: &Path) -> bool {
    let Some(name) = path.file_name() else {
        return false;
    };
    name == ".git" || name == ".lumin" || name == "node_modules"
}

fn source_kind(path: &Path) -> Option<SourceKind> {
    let name = path.file_name()?;
    if os_ends_with_ascii(name, ".d.mts") {
        return Some(SourceKind::DeclarationMts);
    }
    if os_ends_with_ascii(name, ".d.cts") {
        return Some(SourceKind::DeclarationCts);
    }
    if os_ends_with_ascii(name, ".d.ts") {
        return Some(SourceKind::DeclarationTs);
    }
    match path.extension().and_then(OsStr::to_str) {
        Some("js") => Some(SourceKind::JavaScript),
        Some("jsx") => Some(SourceKind::Jsx),
        Some("mjs") => Some(SourceKind::Mjs),
        Some("cjs") => Some(SourceKind::CommonJs),
        Some("ts") => Some(SourceKind::TypeScript),
        Some("tsx") => Some(SourceKind::Tsx),
        Some("mts") => Some(SourceKind::Mts),
        Some("cts") => Some(SourceKind::Cts),
        Some("vue") => Some(SourceKind::Vue),
        Some("svelte") => Some(SourceKind::Svelte),
        Some("astro") => Some(SourceKind::Astro),
        _ => None,
    }
}

#[cfg(unix)]
fn os_ends_with_ascii(value: &OsStr, suffix: &str) -> bool {
    use std::os::unix::ffi::OsStrExt;
    value.as_bytes().ends_with(suffix.as_bytes())
}

#[cfg(windows)]
fn os_ends_with_ascii(value: &OsStr, suffix: &str) -> bool {
    use std::os::windows::ffi::OsStrExt;
    let suffix: Vec<u16> = suffix.encode_utf16().collect();
    value.encode_wide().collect::<Vec<_>>().ends_with(&suffix)
}

#[cfg(test)]
mod tests;
