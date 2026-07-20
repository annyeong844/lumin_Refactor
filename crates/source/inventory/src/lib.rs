mod config_document;
mod package_semantics;
mod pnpm_workspace;
mod root;

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use ignore::WalkBuilder;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use lumin_model::{
    ConfigObservation, ConfigSyntax, Limitation, PhysicalAliasWriteClosure, PhysicalFileIdentity,
    RepoPath, RepoPathError, RoleOverride, ScanRole, SemanticConfigSnapshot, SourceKind,
    SourceRoleReason, SourceRoles, SourceSnapshot,
};
use serde::Deserialize;
use thiserror::Error;

pub use root::{RepositoryAdmission, repository_admission};

#[derive(Clone, Debug, Default)]
pub struct InventoryRequest {
    pub includes: Vec<String>,
    pub excludes: Vec<String>,
    pub role_overrides: Vec<RoleOverride>,
}

#[derive(Clone, Debug)]
pub struct InventorySnapshot {
    pub sources: Vec<SourceSnapshot>,
    pub limitations: Vec<Limitation>,
    pub consulted_config_paths: Vec<RepoPath>,
    pub config: SemanticConfigSnapshot,
}

#[derive(Debug, Error)]
pub enum InventoryError {
    #[error("repository root is not a directory: {0}")]
    InvalidRoot(String),
    #[error("reserved .lumin namespace is not a real directory")]
    ForeignStateNamespace,
    #[error("malformed lumin.json: {0}")]
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteTargetKind {
    ExistingFile,
    ExistingDirectory,
    NewFile,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WriteTargetObservation {
    pub path: RepoPath,
    pub kind: WriteTargetKind,
    pub physical_identity: Option<PhysicalFileIdentity>,
    pub nearest_existing_parent: Option<RepoPath>,
    pub prefix_identities: Vec<(RepoPath, PhysicalFileIdentity)>,
}

#[derive(Debug, Error)]
pub enum WriteTargetError {
    #[error("repository root cannot be leased as one directory scope")]
    UnboundedDirectory,
    #[error("planned path has no observable real parent: {0}")]
    MissingParent(String),
    #[error("planned path resolves outside the repository root: {0}")]
    OutsideRoot(String),
    #[error("planned path is not a regular file or real directory: {0}")]
    NonRegular(String),
    #[error("planned directory is reached through a symlink or junction: {0}")]
    LinkedDirectory(String),
    #[error("failed to inspect planned path {path}: {detail}")]
    Io { path: String, detail: String },
    #[error(transparent)]
    PhysicalIdentity(#[from] InventoryError),
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
    config_observations: BTreeMap<RepoPath, ConfigObservation>,
    limitations: Vec<Limitation>,
    consulted_config_paths: Vec<RepoPath>,
}

pub fn scan(root: &Path, request: &InventoryRequest) -> Result<InventorySnapshot, InventoryError> {
    validate_root(root)?;
    let (config, config_path) = read_root_config(root)?;
    let patterns = PatternSet::compile(root, config.as_ref(), request)?;
    let mut collected = collect_repository_files(root, &patterns)?;

    if let Some(path) = config_path {
        collected.consulted_config_paths.push(path);
    }
    collected.consulted_config_paths.sort();
    collected.consulted_config_paths.dedup();

    let sources = collected.sources.into_values().collect::<Vec<_>>();
    let config = package_semantics::build(
        collected.config_observations,
        &sources,
        &mut collected.limitations,
    )
    .map_err(InventoryError::MalformedConfiguration)?;

    Ok(InventorySnapshot {
        sources,
        limitations: collected.limitations,
        consulted_config_paths: collected.consulted_config_paths,
        config,
    })
}

pub fn physical_alias_write_closure(
    root: &Path,
    target: &RepoPath,
    source_paths: &[RepoPath],
) -> Result<PhysicalAliasWriteClosure, InventoryError> {
    let physical_identity = physical_file_identity(&root.join(target.to_native_relative()))?;
    let target_handle = same_file::Handle::from_path(root.join(target.to_native_relative()))
        .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?;
    let mut aliases = Vec::new();
    for source_path in source_paths {
        let handle = same_file::Handle::from_path(root.join(source_path.to_native_relative()))
            .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?;
        if handle == target_handle {
            aliases.push(source_path.clone());
        }
    }
    aliases.sort();
    aliases.dedup();
    Ok(PhysicalAliasWriteClosure {
        physical_identity,
        members: aliases,
    })
}

pub fn inspect_write_target(
    root: &Path,
    path: &RepoPath,
) -> Result<WriteTargetObservation, WriteTargetError> {
    if path.components_len() == 0 {
        return Err(WriteTargetError::UnboundedDirectory);
    }
    let canonical_root = fs::canonicalize(root).map_err(|error| WriteTargetError::Io {
        path: root.display().to_string(),
        detail: error.to_string(),
    })?;
    let native = root.join(path.to_native_relative());
    let metadata = match fs::symlink_metadata(&native) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let nearest_parent = nearest_existing_parent(root, path)?;
            let prefix_identities =
                observe_directory_prefixes(root, Some(&nearest_parent), &canonical_root)?;
            return Ok(WriteTargetObservation {
                path: path.clone(),
                kind: WriteTargetKind::NewFile,
                physical_identity: None,
                nearest_existing_parent: Some(nearest_parent),
                prefix_identities,
            });
        }
        Err(error) => {
            return Err(WriteTargetError::Io {
                path: path.display_escaped(),
                detail: error.to_string(),
            });
        }
    };

    let target_metadata = if metadata.file_type().is_symlink() {
        let followed = fs::metadata(&native).map_err(|error| WriteTargetError::Io {
            path: path.display_escaped(),
            detail: error.to_string(),
        })?;
        if followed.is_dir() {
            return Err(WriteTargetError::LinkedDirectory(path.display_escaped()));
        }
        followed
    } else {
        metadata
    };
    let prefix_identities =
        observe_directory_prefixes(root, path.parent().as_ref(), &canonical_root)?;
    ensure_contained(&canonical_root, &native, path)?;
    let kind = if target_metadata.is_file() {
        WriteTargetKind::ExistingFile
    } else if target_metadata.is_dir() {
        WriteTargetKind::ExistingDirectory
    } else {
        return Err(WriteTargetError::NonRegular(path.display_escaped()));
    };
    Ok(WriteTargetObservation {
        path: path.clone(),
        kind,
        physical_identity: Some(physical_file_identity(&native)?),
        nearest_existing_parent: None,
        prefix_identities,
    })
}

fn nearest_existing_parent(root: &Path, path: &RepoPath) -> Result<RepoPath, WriteTargetError> {
    let mut candidate = path.parent();
    while let Some(parent) = candidate {
        let native = root.join(parent.to_native_relative());
        match fs::symlink_metadata(&native) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(WriteTargetError::LinkedDirectory(parent.display_escaped()));
            }
            Ok(metadata) if metadata.is_dir() => return Ok(parent),
            Ok(_) => return Err(WriteTargetError::MissingParent(parent.display_escaped())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                candidate = parent.parent();
            }
            Err(error) => {
                return Err(WriteTargetError::Io {
                    path: parent.display_escaped(),
                    detail: error.to_string(),
                });
            }
        }
    }
    Err(WriteTargetError::MissingParent(path.display_escaped()))
}

fn observe_directory_prefixes(
    root: &Path,
    parent: Option<&RepoPath>,
    canonical_root: &Path,
) -> Result<Vec<(RepoPath, PhysicalFileIdentity)>, WriteTargetError> {
    let Some(parent) = parent else {
        return Ok(Vec::new());
    };
    let mut prefixes = Vec::new();
    let mut cursor = Some(parent.clone());
    while let Some(path) = cursor {
        let is_root = path.components_len() == 0;
        prefixes.push(path.clone());
        if is_root {
            break;
        }
        cursor = path.parent();
    }
    prefixes.reverse();

    let mut observed = Vec::with_capacity(prefixes.len());
    for prefix in prefixes {
        let native = root.join(prefix.to_native_relative());
        let metadata = fs::symlink_metadata(&native).map_err(|error| WriteTargetError::Io {
            path: prefix.display_escaped(),
            detail: error.to_string(),
        })?;
        if metadata.file_type().is_symlink() {
            return Err(WriteTargetError::LinkedDirectory(prefix.display_escaped()));
        }
        if !metadata.is_dir() {
            return Err(WriteTargetError::MissingParent(prefix.display_escaped()));
        }
        ensure_contained(canonical_root, &native, &prefix)?;
        observed.push((prefix, physical_file_identity(&native)?));
    }
    Ok(observed)
}

pub fn is_supported_source_path(path: &RepoPath) -> bool {
    source_kind(&path.to_native_relative()).is_some()
}

pub fn physical_file_identity(path: &Path) -> Result<PhysicalFileIdentity, InventoryError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::metadata(path)
            .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?;
        Ok(PhysicalFileIdentity::Unix {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(windows)]
    {
        let handle = winapi_util::Handle::from_path_any(path)
            .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?;
        let information = winapi_util::file::information(&handle)
            .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?;
        let volume_serial = u32::try_from(information.volume_serial_number()).map_err(|_| {
            InventoryError::PhysicalIdentity("volume serial number exceeds u32".to_owned())
        })?;
        Ok(PhysicalFileIdentity::Windows {
            volume_serial,
            file_index: information.file_index(),
        })
    }
}

pub fn observe_config_physical_identity(
    root: &Path,
    path: &RepoPath,
) -> Result<Option<PhysicalFileIdentity>, InventoryError> {
    validate_root(root)?;
    let native = root.join(path.to_native_relative());
    match fs::symlink_metadata(&native) {
        Ok(_) => physical_file_identity(&native).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(InventoryError::PhysicalIdentity(error.to_string())),
    }
}

pub fn directory_physical_identity(
    root: &Path,
    path: &RepoPath,
) -> Result<PhysicalFileIdentity, WriteTargetError> {
    if path.components_len() == 0 {
        let metadata = fs::symlink_metadata(root).map_err(|error| WriteTargetError::Io {
            path: path.display_escaped(),
            detail: error.to_string(),
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(WriteTargetError::LinkedDirectory(path.display_escaped()));
        }
        return physical_file_identity(root).map_err(WriteTargetError::from);
    }
    let observation = inspect_write_target(root, path)?;
    if observation.kind != WriteTargetKind::ExistingDirectory {
        return Err(WriteTargetError::NonRegular(path.display_escaped()));
    }
    observation
        .physical_identity
        .ok_or_else(|| WriteTargetError::NonRegular(path.display_escaped()))
}

fn ensure_contained(
    canonical_root: &Path,
    native: &Path,
    logical: &RepoPath,
) -> Result<(), WriteTargetError> {
    let canonical = fs::canonicalize(native).map_err(|error| WriteTargetError::Io {
        path: logical.display_escaped(),
        detail: error.to_string(),
    })?;
    if !canonical.starts_with(canonical_root) {
        return Err(WriteTargetError::OutsideRoot(logical.display_escaped()));
    }
    Ok(())
}

fn collect_repository_files(
    root: &Path,
    patterns: &PatternSet,
) -> Result<CollectedFiles, InventoryError> {
    let mut collected = CollectedFiles::default();
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .parents(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false)
        .follow_links(false)
        .filter_entry(|entry| !is_hard_excluded(entry.path()));

    for result in builder.build() {
        let entry = match result {
            Ok(entry) => entry,
            Err(error) => {
                collected
                    .limitations
                    .push(Limitation::SourcePayloadUnavailable {
                        path: root.display().to_string(),
                        detail: error.to_string(),
                    });
                continue;
            }
        };
        let Ok(relative) = entry.path().strip_prefix(root) else {
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
        let is_file = if file_type.is_file() {
            true
        } else if file_type.is_symlink() {
            match fs::metadata(entry.path()) {
                Ok(metadata) if metadata.is_file() => {
                    let canonical_root = fs::canonicalize(root)
                        .map_err(|error| InventoryError::RootIo(error.to_string()))?;
                    match fs::canonicalize(entry.path()) {
                        Ok(target) if target.starts_with(&canonical_root) => true,
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
                    }
                }
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
        collected.observe_file(root, entry.path(), relative, path, patterns)?;
    }
    Ok(collected)
}

impl CollectedFiles {
    fn observe_file(
        &mut self,
        root: &Path,
        native_path: &Path,
        relative: &Path,
        path: RepoPath,
        patterns: &PatternSet,
    ) -> Result<(), InventoryError> {
        if let Some(syntax) = config_syntax(relative) {
            self.consulted_config_paths.push(path.clone());
            match observe_config(root, &path, syntax)? {
                observation @ ConfigObservation::Present(_) => {
                    self.config_observations.insert(path, observation);
                }
                observation @ ConfigObservation::Unreadable { .. } => {
                    let limitation = match syntax {
                        ConfigSyntax::StrictJson => Limitation::PackageMetadataUnobservable {
                            path: path.display_escaped(),
                            detail: "package manifest could not be read".to_owned(),
                        },
                        ConfigSyntax::Jsonc => Limitation::TsconfigPayloadUnavailable {
                            path: path.display_escaped(),
                            detail: "controlling config could not be read".to_owned(),
                        },
                        ConfigSyntax::RestrictedYaml => Limitation::WorkspaceOwnershipUnsupported {
                            path: path.display_escaped(),
                            detail: "pnpm workspace configuration could not be read".to_owned(),
                        },
                    };
                    self.limitations.push(limitation);
                    self.config_observations.insert(path, observation);
                }
                observation => {
                    self.config_observations.insert(path, observation);
                }
            }
            return Ok(());
        }
        if !patterns.admits(relative) {
            return Ok(());
        }
        let Some(kind) = source_kind(relative) else {
            return Ok(());
        };

        let bytes = match fs::read(native_path) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.limitations.push(Limitation::SourcePayloadUnavailable {
                    path: path.display_escaped(),
                    detail: error.to_string(),
                });
                return Ok(());
            }
        };
        let roles = classify_roles(relative, kind, &bytes, patterns);
        self.sources
            .insert(path.clone(), SourceSnapshot::new(path, kind, roles, bytes));
        Ok(())
    }
}

pub fn observe_config(
    root: &Path,
    path: &RepoPath,
    syntax: ConfigSyntax,
) -> Result<ConfigObservation, InventoryError> {
    validate_root(root)?;
    let native = root.join(path.to_native_relative());
    let metadata = match fs::symlink_metadata(&native) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ConfigObservation::Missing { path: path.clone() });
        }
        Err(error) => {
            return Ok(ConfigObservation::Unreadable {
                path: path.clone(),
                detail: error.to_string(),
            });
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Ok(ConfigObservation::NonRegular { path: path.clone() });
    }
    let bytes = match fs::read(&native) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Ok(ConfigObservation::Unreadable {
                path: path.clone(),
                detail: error.to_string(),
            });
        }
    };
    let parsed = match syntax {
        ConfigSyntax::StrictJson | ConfigSyntax::Jsonc => {
            config_document::parse(path.clone(), &bytes, syntax)
        }
        ConfigSyntax::RestrictedYaml => pnpm_workspace::parse(path.clone(), &bytes),
    };
    let document = parsed.map_err(|error| {
        InventoryError::MalformedConfiguration(format!("{}: {error}", path.display_escaped()))
    })?;
    Ok(ConfigObservation::Present(document))
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

fn read_root_config(root: &Path) -> Result<(Option<RootConfig>, Option<RepoPath>), InventoryError> {
    let path = root.join("lumin.json");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((None, None)),
        Err(error) => return Err(InventoryError::MalformedConfiguration(error.to_string())),
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
    let repo_path = RepoPath::from_portable("lumin.json")
        .map_err(|error| InventoryError::MalformedConfiguration(error.to_string()))?;
    Ok((Some(config), Some(repo_path)))
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
    relative: &Path,
    kind: SourceKind,
    bytes: &[u8],
    patterns: &PatternSet,
) -> SourceRoles {
    let mut roles = SourceRoles {
        test_like: default_test_role(relative),
        generated: generated_marker(bytes).then_some(SourceRoleReason::LeadingGeneratedComment),
        vendored: None,
        declaration: kind.is_declaration(),
    };

    apply_roles(&mut roles, relative, &patterns.config_roles);
    apply_roles(&mut roles, relative, &patterns.invocation_roles);
    roles
}

fn apply_roles(roles: &mut SourceRoles, relative: &Path, rules: &[(Gitignore, ScanRole)]) {
    for (pattern, role) in rules {
        if !pattern
            .matched_path_or_any_parents(relative, false)
            .is_ignore()
        {
            continue;
        }
        match role {
            ScanRole::Test => roles.test_like = Some(SourceRoleReason::ExplicitTestRole),
            ScanRole::Production => roles.test_like = None,
            ScanRole::Generated => roles.generated = Some(SourceRoleReason::ExplicitGeneratedRole),
            ScanRole::Vendor => roles.vendored = Some(SourceRoleReason::ExplicitVendorRole),
            ScanRole::Authored => {
                roles.generated = None;
                roles.vendored = None;
            }
        }
    }
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
mod tests {
    use super::*;

    #[test]
    fn generated_marker_must_be_in_leading_comment() {
        assert!(generated_marker(b"// @generated\nexport const value = 1;"));
        assert!(generated_marker(
            b" /* tool @generated output */\nexport const value = 1;"
        ));
        assert!(!generated_marker(b"const text = '@generated';"));
    }

    #[test]
    fn config_identity_observation_preserves_hard_link_alias_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::create_dir(root.path().join("config"))?;
        fs::write(root.path().join("config/base.json"), "{}\n")?;
        fs::hard_link(
            root.path().join("config/base.json"),
            root.path().join("config/alias.json"),
        )?;
        let base = RepoPath::from_portable("config/base.json")?;
        let alias = RepoPath::from_portable("config/alias.json")?;

        assert_eq!(
            observe_config_physical_identity(root.path(), &base)?,
            observe_config_physical_identity(root.path(), &alias)?
        );
        Ok(())
    }

    #[test]
    fn scans_generated_and_explicit_vendor_roles() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("src"))?;
        fs::write(
            root.path().join("lumin.json"),
            r#"{"schemaVersion":"lumin-config.v1","scan":{"roles":[{"pattern":"src/vendor.ts","role":"vendor"}]}}"#,
        )?;
        fs::write(
            root.path().join("src/generated.ts"),
            "// @generated\nexport const generated = 1;",
        )?;
        fs::write(
            root.path().join("src/vendor.ts"),
            "export const vendored = 1;",
        )?;

        let inventory = scan(root.path(), &InventoryRequest::default())?;
        assert_eq!(inventory.sources.len(), 2);
        assert_eq!(
            inventory
                .sources
                .iter()
                .filter(|source| source.roles.generated.is_some())
                .count(),
            1
        );
        assert_eq!(
            inventory
                .sources
                .iter()
                .filter(|source| source.roles.vendored.is_some())
                .count(),
            1
        );
        Ok(())
    }

    #[test]
    fn workspace_object_form_selects_only_matching_package_roots()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("packages/a"))?;
        fs::create_dir_all(root.path().join("tools/b"))?;
        fs::write(
            root.path().join("package.json"),
            r#"{"name":"root","workspaces":{"packages":["packages/*"]}}"#,
        )?;
        fs::write(
            root.path().join("packages/a/package.json"),
            r#"{"name":"package-a"}"#,
        )?;
        fs::write(
            root.path().join("tools/b/package.json"),
            r#"{"name":"tool-b"}"#,
        )?;

        let inventory = scan(root.path(), &InventoryRequest::default())?;
        let package_a = inventory
            .config
            .packages
            .iter()
            .find(|package| package.root.display_escaped() == "packages/a")
            .ok_or("package-a missing")?;
        let tool_b = inventory
            .config
            .packages
            .iter()
            .find(|package| package.root.display_escaped() == "tools/b")
            .ok_or("tool-b missing")?;

        assert_eq!(
            package_a
                .workspace_root
                .as_ref()
                .map(RepoPath::display_escaped),
            Some(String::new())
        );
        assert!(tool_b.workspace_root.is_none());
        Ok(())
    }

    #[test]
    fn pnpm_membership_replaces_package_workspaces_and_applies_exclusions()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("packages/a"))?;
        fs::create_dir_all(root.path().join("tools/included"))?;
        fs::create_dir_all(root.path().join("tools/excluded"))?;
        fs::write(
            root.path().join("package.json"),
            r#"{"name":"root","workspaces":["packages/*"]}"#,
        )?;
        fs::write(
            root.path().join("pnpm-workspace.yaml"),
            "packages:\n  - '!tools/excluded'\n  - tools/**\n",
        )?;
        fs::write(
            root.path().join("packages/a/package.json"),
            r#"{"name":"package-a"}"#,
        )?;
        fs::write(
            root.path().join("tools/included/package.json"),
            r#"{"name":"included"}"#,
        )?;
        fs::write(
            root.path().join("tools/excluded/package.json"),
            r#"{"name":"excluded"}"#,
        )?;

        let inventory = scan(root.path(), &InventoryRequest::default())?;
        let package_a = inventory
            .config
            .packages
            .iter()
            .find(|package| package.root.display_escaped() == "packages/a")
            .ok_or("package-a missing")?;
        let included = inventory
            .config
            .packages
            .iter()
            .find(|package| package.root.display_escaped() == "tools/included")
            .ok_or("included package missing")?;
        let excluded = inventory
            .config
            .packages
            .iter()
            .find(|package| package.root.display_escaped() == "tools/excluded")
            .ok_or("excluded package missing")?;

        assert!(package_a.workspace_root.is_none());
        assert_eq!(
            included
                .workspace_root
                .as_ref()
                .map(RepoPath::display_escaped),
            Some(String::new())
        );
        assert!(excluded.workspace_root.is_none());
        assert!(inventory.limitations.is_empty());
        Ok(())
    }

    #[test]
    fn pnpm_missing_packages_keeps_only_the_root_member() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("packages/a"))?;
        fs::write(root.path().join("package.json"), r#"{"name":"root"}"#)?;
        fs::write(root.path().join("pnpm-workspace.yaml"), "{}\n")?;
        fs::write(
            root.path().join("packages/a/package.json"),
            r#"{"name":"package-a"}"#,
        )?;

        let inventory = scan(root.path(), &InventoryRequest::default())?;
        let workspace = inventory
            .config
            .workspaces
            .iter()
            .find(|workspace| workspace.source == lumin_model::WorkspaceSource::PnpmWorkspace)
            .ok_or("pnpm workspace missing")?;
        assert_eq!(workspace.members, vec![RepoPath::empty()]);
        Ok(())
    }

    #[test]
    fn pnpm_package_configs_forms_are_visible_typed_limitations()
    -> Result<(), Box<dyn std::error::Error>> {
        for yaml in [
            "packageConfigs:\n  project-1:\n    saveExact: true\n",
            "packageConfigs:\n  - match: [project-1, project-2]\n    saveExact: true\n",
        ] {
            let root = tempfile::tempdir()?;
            fs::write(root.path().join("package.json"), r#"{"name":"root"}"#)?;
            fs::write(root.path().join("pnpm-workspace.yaml"), yaml)?;

            let inventory = scan(root.path(), &InventoryRequest::default())?;
            assert!(inventory.limitations.iter().any(|limitation| matches!(
                limitation,
                Limitation::PnpmDependencySemanticsUnsupported { path, .. }
                    if path == "pnpm-workspace.yaml"
            )));
        }
        Ok(())
    }

    #[test]
    fn malformed_pnpm_is_a_hard_stop_without_package_workspace_fallback()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("packages/a"))?;
        fs::write(
            root.path().join("package.json"),
            r#"{"name":"root","workspaces":["packages/*"]}"#,
        )?;
        fs::write(
            root.path().join("pnpm-workspace.yaml"),
            "packages: []\npackages: [packages/*]\n",
        )?;
        fs::write(
            root.path().join("packages/a/package.json"),
            r#"{"name":"package-a"}"#,
        )?;

        let result = scan(root.path(), &InventoryRequest::default());
        assert!(matches!(
            result,
            Err(InventoryError::MalformedConfiguration(_))
        ));
        Ok(())
    }
}
