mod config_document;
mod package_semantics;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use ignore::WalkBuilder;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use lumin_model::{
    ConfigObservation, ConfigSyntax, Limitation, RepoPath, RepoPathError, RoleOverride, ScanRole,
    SemanticConfigSnapshot, SourceKind, SourceRoleReason, SourceRoles, SourceSnapshot,
};
use serde::Deserialize;
use thiserror::Error;

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
    pnpm_roots: BTreeSet<RepoPath>,
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
        &collected.pnpm_roots,
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
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }

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
        if relative.file_name() == Some(OsStr::new("pnpm-workspace.yaml")) {
            self.consulted_config_paths.push(path.clone());
            self.pnpm_roots
                .insert(path.parent().unwrap_or_else(RepoPath::empty));
            self.limitations
                .push(Limitation::WorkspaceOwnershipUnsupported {
                path: path.display_escaped(),
                detail: "restricted pnpm workspace parsing is not implemented yet; package workspaces are not used as fallback"
                    .to_owned(),
            });
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
    let document = config_document::parse(path.clone(), &bytes, syntax).map_err(|error| {
        InventoryError::MalformedConfiguration(format!("{}: {error}", path.display_escaped()))
    })?;
    Ok(ConfigObservation::Present(document))
}

fn config_syntax(path: &Path) -> Option<ConfigSyntax> {
    match path.file_name().and_then(OsStr::to_str) {
        Some("package.json") => Some(ConfigSyntax::StrictJson),
        Some("tsconfig.json" | "jsconfig.json") => Some(ConfigSyntax::Jsonc),
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
    fn pnpm_presence_disables_package_workspace_fallback() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("packages/a"))?;
        fs::write(
            root.path().join("package.json"),
            r#"{"name":"root","workspaces":["packages/*"]}"#,
        )?;
        fs::write(
            root.path().join("pnpm-workspace.yaml"),
            "packages:\n  - packages/*\n",
        )?;
        fs::write(
            root.path().join("packages/a/package.json"),
            r#"{"name":"package-a"}"#,
        )?;

        let inventory = scan(root.path(), &InventoryRequest::default())?;
        let package_a = inventory
            .config
            .packages
            .iter()
            .find(|package| package.root.display_escaped() == "packages/a")
            .ok_or("package-a missing")?;

        assert!(package_a.workspace_root.is_none());
        assert!(inventory.limitations.iter().any(|limitation| matches!(
            limitation,
            Limitation::WorkspaceOwnershipUnsupported { path, .. }
                if path == "pnpm-workspace.yaml"
        )));
        Ok(())
    }
}
