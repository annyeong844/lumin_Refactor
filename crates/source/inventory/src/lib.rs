use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use ignore::WalkBuilder;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use lumin_model::{
    Limitation, RepoPath, RepoPathError, RoleOverride, ScanRole, SourceKind, SourceRoleReason,
    SourceRoles, SourceSnapshot,
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

pub fn scan(root: &Path, request: &InventoryRequest) -> Result<InventorySnapshot, InventoryError> {
    validate_root(root)?;
    let (config, config_path) = read_root_config(root)?;
    let patterns = PatternSet::compile(root, config.as_ref(), request)?;

    let mut sources = BTreeMap::<RepoPath, SourceSnapshot>::new();
    let mut limitations = Vec::new();
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
                limitations.push(Limitation::SourcePayloadUnavailable {
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
        if !patterns.admits(relative) {
            continue;
        }
        if let Some(limitation) = unsupported_semantic_config(relative) {
            limitations.push(limitation);
            continue;
        }
        let Some(kind) = source_kind(relative) else {
            continue;
        };

        let path = RepoPath::from_native_relative(relative).map_err(|source| {
            InventoryError::InvalidRepoPath {
                path: relative.display().to_string(),
                source,
            }
        })?;
        let bytes = match fs::read(entry.path()) {
            Ok(bytes) => bytes,
            Err(error) => {
                limitations.push(Limitation::SourcePayloadUnavailable {
                    path: path.display_escaped(),
                    detail: error.to_string(),
                });
                continue;
            }
        };
        let roles = classify_roles(relative, kind, &bytes, &patterns);
        sources.insert(path.clone(), SourceSnapshot::new(path, kind, roles, bytes));
    }

    let mut consulted_config_paths = Vec::new();
    if let Some(path) = config_path {
        consulted_config_paths.push(path);
    }

    Ok(InventorySnapshot {
        sources: sources.into_values().collect(),
        limitations,
        consulted_config_paths,
    })
}

fn unsupported_semantic_config(path: &Path) -> Option<Limitation> {
    let name = path.file_name()?;
    if name == "package.json" {
        return Some(Limitation::PublicSurfaceUnsupported {
            path: path.display().to_string(),
            detail: "package semantics are not implemented in the first audit increment".to_owned(),
        });
    }
    if name == "tsconfig.json" || name == "jsconfig.json" {
        return Some(Limitation::TsconfigSemanticsUnsupported {
            path: path.display().to_string(),
            detail: "tsconfig semantics are not implemented in the first audit increment"
                .to_owned(),
        });
    }
    if name == "pnpm-workspace.yaml" {
        return Some(Limitation::PackageDependencySemanticsUnsupported {
            path: path.display().to_string(),
            detail: "workspace semantics are not implemented in the first audit increment"
                .to_owned(),
        });
    }
    None
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
}
