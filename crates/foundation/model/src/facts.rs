use std::ffi::OsStr;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use thiserror::Error;

use crate::{
    EmbeddedSourceUnitId, LogicalSourceId, PayloadSnapshotId, PhysicalFileIdentity, RepoPath,
    digest_hex,
};

pub const SOURCE_CLASSIFICATION_RULE_VERSION: &str = "source-classification.v1";

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    JavaScript,
    Jsx,
    Mjs,
    CommonJs,
    TypeScript,
    Tsx,
    Mts,
    Cts,
    DeclarationTs,
    DeclarationMts,
    DeclarationCts,
    Vue,
    Svelte,
    Astro,
}

impl SourceKind {
    pub fn from_repo_path(path: &RepoPath) -> Option<Self> {
        path.to_native_relative()
            .ok()
            .as_deref()
            .and_then(Self::from_native_path)
    }

    pub fn from_native_path(path: &Path) -> Option<Self> {
        let name = path.file_name()?;
        if os_ends_with_ascii(name, ".d.mts") {
            return Some(Self::DeclarationMts);
        }
        if os_ends_with_ascii(name, ".d.cts") {
            return Some(Self::DeclarationCts);
        }
        if os_ends_with_ascii(name, ".d.ts") {
            return Some(Self::DeclarationTs);
        }
        match path.extension().and_then(OsStr::to_str) {
            Some("js") => Some(Self::JavaScript),
            Some("jsx") => Some(Self::Jsx),
            Some("mjs") => Some(Self::Mjs),
            Some("cjs") => Some(Self::CommonJs),
            Some("ts") => Some(Self::TypeScript),
            Some("tsx") => Some(Self::Tsx),
            Some("mts") => Some(Self::Mts),
            Some("cts") => Some(Self::Cts),
            Some("vue") => Some(Self::Vue),
            Some("svelte") => Some(Self::Svelte),
            Some("astro") => Some(Self::Astro),
            _ => None,
        }
    }

    pub fn is_declaration(self) -> bool {
        matches!(
            self,
            Self::DeclarationTs | Self::DeclarationMts | Self::DeclarationCts
        )
    }

    pub fn is_js_family(self) -> bool {
        !matches!(self, Self::Vue | Self::Svelte | Self::Astro)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SfcDialect {
    Vue,
    Svelte,
    Astro,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SourceRoles {
    classifications: Vec<SourceRoleClassification>,
}

impl SourceRoles {
    pub fn from_classifications(classifications: Vec<SourceRoleClassification>) -> Self {
        Self { classifications }
    }

    pub fn classifications(&self) -> &[SourceRoleClassification] {
        &self.classifications
    }

    pub fn test_like_reason(&self) -> Option<SourceRoleReason> {
        self.classifications
            .iter()
            .fold(None, |reason, classification| match classification.role {
                SourceClassificationRole::Test => Some(classification.reason),
                SourceClassificationRole::Production => None,
                _ => reason,
            })
    }

    pub fn generated_reason(&self) -> Option<SourceRoleReason> {
        self.classifications
            .iter()
            .fold(None, |reason, classification| match classification.role {
                SourceClassificationRole::Generated => Some(classification.reason),
                SourceClassificationRole::Authored => None,
                _ => reason,
            })
    }

    pub fn vendored_reason(&self) -> Option<SourceRoleReason> {
        self.classifications
            .iter()
            .fold(None, |reason, classification| match classification.role {
                SourceClassificationRole::Vendor => Some(classification.reason),
                SourceClassificationRole::Authored => None,
                _ => reason,
            })
    }

    pub fn is_test_like(&self) -> bool {
        self.test_like_reason().is_some()
    }

    pub fn is_generated(&self) -> bool {
        self.generated_reason().is_some()
    }

    pub fn is_vendored(&self) -> bool {
        self.vendored_reason().is_some()
    }

    pub fn is_declaration(&self) -> bool {
        self.classifications
            .iter()
            .any(|classification| classification.role == SourceClassificationRole::Declaration)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleOverride {
    pub pattern: String,
    pub role: ScanRole,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("invalid scan pattern `{pattern}`: {detail}")]
pub struct ScanPatternError {
    pattern: String,
    detail: &'static str,
}

pub fn validate_scan_pattern(pattern: &str) -> Result<(), ScanPatternError> {
    if pattern.is_empty() {
        return Err(scan_pattern_error(pattern, "pattern is empty"));
    }
    if pattern.starts_with('!') {
        return Err(scan_pattern_error(
            pattern,
            "negated invocation patterns are unsupported",
        ));
    }
    if pattern.contains("..") {
        return Err(scan_pattern_error(
            pattern,
            "parent traversal is unsupported",
        ));
    }

    let mut glob = if pattern.ends_with("\\ ") {
        pattern
    } else {
        pattern.trim_end()
    };
    if glob.is_empty() || glob.starts_with('#') {
        return Ok(());
    }
    if glob.starts_with("\\!") || glob.starts_with("\\#") || glob.starts_with('/') {
        glob = &glob[1..];
    }
    if glob.ends_with('/') {
        glob = &glob[..glob.len() - 1];
        if glob.ends_with('\\') {
            glob = &glob[..glob.len() - 1];
        }
    }
    validate_scan_glob_shape(pattern, glob)
}

fn validate_scan_glob_shape(pattern: &str, glob: &str) -> Result<(), ScanPatternError> {
    let characters = glob.chars().collect::<Vec<_>>();
    let mut index = 0;
    let mut alternate_depth = 0_u64;
    let mut classes_enabled = true;
    while index < characters.len() {
        match characters[index] {
            '\\' => {
                index += 1;
                if index == characters.len() {
                    return Err(scan_pattern_error(pattern, "dangling escape"));
                }
            }
            '[' if classes_enabled => {
                let Some(end) = scan_class_end(&characters, index) else {
                    classes_enabled = false;
                    index += 1;
                    continue;
                };
                validate_scan_class(pattern, &characters, index, end)?;
                index = end;
            }
            '{' => alternate_depth += 1,
            '}' => {
                alternate_depth = alternate_depth.checked_sub(1).ok_or_else(|| {
                    scan_pattern_error(pattern, "alternate group closes before it opens")
                })?;
            }
            _ => {}
        }
        index += 1;
    }
    if alternate_depth != 0 {
        return Err(scan_pattern_error(pattern, "alternate group is unclosed"));
    }
    Ok(())
}

fn scan_class_end(characters: &[char], opening: usize) -> Option<usize> {
    let mut index = opening + 1;
    if matches!(characters.get(index), Some('!') | Some('^')) {
        index += 1;
    }
    let mut first = true;
    while let Some(character) = characters.get(index) {
        if *character == ']' && !first {
            return Some(index);
        }
        first = false;
        index += 1;
    }
    None
}

fn validate_scan_class(
    pattern: &str,
    characters: &[char],
    opening: usize,
    closing: usize,
) -> Result<(), ScanPatternError> {
    let mut index = opening + 1;
    if matches!(characters.get(index), Some('!') | Some('^')) {
        index += 1;
    }
    let mut ranges = Vec::<(char, char)>::new();
    let mut first = true;
    let mut in_range = false;
    while index < closing {
        let character = characters[index];
        if character == '-' {
            if first {
                ranges.push(('-', '-'));
            } else if in_range {
                let Some(range) = ranges.last_mut() else {
                    return Err(scan_pattern_error(
                        pattern,
                        "character range omitted its start",
                    ));
                };
                if '-' < range.0 {
                    return Err(scan_pattern_error(pattern, "descending character range"));
                }
                range.1 = '-';
                in_range = false;
            } else {
                in_range = true;
            }
        } else {
            if in_range {
                let Some(range) = ranges.last_mut() else {
                    return Err(scan_pattern_error(
                        pattern,
                        "character range omitted its start",
                    ));
                };
                if character < range.0 {
                    return Err(scan_pattern_error(pattern, "descending character range"));
                }
                range.1 = character;
            } else {
                ranges.push((character, character));
            }
            in_range = false;
        }
        first = false;
        index += 1;
    }
    Ok(())
}

fn scan_pattern_error(pattern: &str, detail: &'static str) -> ScanPatternError {
    ScanPatternError {
        pattern: pattern.to_owned(),
        detail,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScanRole {
    Test,
    Production,
    Generated,
    Vendor,
    Authored,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceClassificationRole {
    Test,
    Production,
    Generated,
    Vendor,
    Authored,
    Declaration,
}

impl From<ScanRole> for SourceClassificationRole {
    fn from(role: ScanRole) -> Self {
        match role {
            ScanRole::Test => Self::Test,
            ScanRole::Production => Self::Production,
            ScanRole::Generated => Self::Generated,
            ScanRole::Vendor => Self::Vendor,
            ScanRole::Authored => Self::Authored,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceRoleConfigurationSource {
    CompiledDefault,
    Configuration,
    Invocation,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRoleClassification {
    pub role: SourceClassificationRole,
    pub rule_version: String,
    pub reason: SourceRoleReason,
    pub configuration_source: SourceRoleConfigurationSource,
}

impl SourceRoleClassification {
    pub fn is_owner_produced(&self) -> bool {
        if self.rule_version != SOURCE_CLASSIFICATION_RULE_VERSION {
            return false;
        }
        match (self.role, self.reason, self.configuration_source) {
            (
                SourceClassificationRole::Test,
                SourceRoleReason::TestPathRule | SourceRoleReason::TestBasenameRule,
                SourceRoleConfigurationSource::CompiledDefault,
            )
            | (
                SourceClassificationRole::Generated,
                SourceRoleReason::LeadingGeneratedComment,
                SourceRoleConfigurationSource::CompiledDefault,
            )
            | (
                SourceClassificationRole::Declaration,
                SourceRoleReason::DeclarationExtension,
                SourceRoleConfigurationSource::CompiledDefault,
            ) => true,
            (role, reason, source) => {
                matches!(
                    (role, reason),
                    (
                        SourceClassificationRole::Test,
                        SourceRoleReason::ExplicitTestRole
                    ) | (
                        SourceClassificationRole::Production,
                        SourceRoleReason::ExplicitProductionRole
                    ) | (
                        SourceClassificationRole::Generated,
                        SourceRoleReason::ExplicitGeneratedRole
                    ) | (
                        SourceClassificationRole::Vendor,
                        SourceRoleReason::ExplicitVendorRole
                    ) | (
                        SourceClassificationRole::Authored,
                        SourceRoleReason::ExplicitAuthoredRole
                    )
                ) && matches!(
                    source,
                    SourceRoleConfigurationSource::Configuration
                        | SourceRoleConfigurationSource::Invocation
                )
            }
        }
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
    let suffix = suffix.encode_utf16().collect::<Vec<_>>();
    value.encode_wide().collect::<Vec<_>>().ends_with(&suffix)
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EntrySource {
    Invocation,
    Configuration,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EntryUnavailableReason {
    Missing,
    Ignored,
    Excluded,
    OutOfDomain,
    HardExcluded,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceRoleReason {
    TestPathRule,
    TestBasenameRule,
    LeadingGeneratedComment,
    ExplicitTestRole,
    ExplicitProductionRole,
    ExplicitGeneratedRole,
    ExplicitVendorRole,
    ExplicitAuthoredRole,
    DeclarationExtension,
}

#[derive(Clone, Debug)]
pub struct SourceSnapshot {
    pub id: LogicalSourceId,
    pub path: RepoPath,
    pub kind: SourceKind,
    pub roles: SourceRoles,
    pub physical_identity: PhysicalFileIdentity,
    pub payload_snapshot_id: PayloadSnapshotId,
    pub payload_sha256: String,
    pub bytes: Arc<[u8]>,
}

impl SourceSnapshot {
    pub fn new(
        path: RepoPath,
        kind: SourceKind,
        roles: SourceRoles,
        physical_identity: PhysicalFileIdentity,
        bytes: impl Into<Arc<[u8]>>,
    ) -> Self {
        let id = LogicalSourceId::from_path(&path);
        let bytes = bytes.into();
        let payload_sha256 = digest_hex(&bytes);
        let payload_snapshot_id =
            PayloadSnapshotId::for_capture(&physical_identity, &payload_sha256);
        Self {
            id,
            path,
            kind,
            roles,
            physical_identity,
            payload_snapshot_id,
            payload_sha256,
            bytes,
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "kebab-case")]
pub enum SourceUnitId {
    Logical(LogicalSourceId),
    Embedded(EmbeddedSourceUnitId),
}

#[derive(Clone, Debug)]
pub struct EmbeddedSourceUnit {
    pub id: EmbeddedSourceUnitId,
    pub parent_source_id: LogicalSourceId,
    pub parent_span: SourceSpan,
    pub kind: SourceKind,
    pub payload_sha256: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalEmbeddedSourceRef {
    pub parent_source_id: LogicalSourceId,
    pub target_source_id: LogicalSourceId,
    pub target_kind: SourceKind,
    pub specifier: String,
    pub parent_span: SourceSpan,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SfcTemplateUseKind {
    Static,
    Dynamic,
    Namespace,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SfcTemplateUse {
    pub tag_name: String,
    pub binding_name: String,
    pub kind: SfcTemplateUseKind,
    pub span: SourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SfcResourceUse {
    pub specifier: String,
    pub span: SourceSpan,
}

#[derive(Clone, Debug)]
pub struct SfcDecomposition {
    pub source_id: LogicalSourceId,
    pub dialect: SfcDialect,
    pub state: CapabilityState,
    pub module_export_known: bool,
    pub inline_scripts: Vec<EmbeddedSourceUnit>,
    pub external_scripts: Vec<ExternalEmbeddedSourceRef>,
    pub template_uses: Vec<SfcTemplateUse>,
    pub resource_uses: Vec<SfcResourceUse>,
    pub limitations: Vec<Limitation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SfcScriptAttachment {
    pub parent_source_id: LogicalSourceId,
    pub target_source_id: LogicalSourceId,
    pub parent_span: SourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SfcComponentUse {
    pub parent_source_id: LogicalSourceId,
    pub tag_name: String,
    pub binding_name: String,
    pub source_use: SourceUseFact,
    pub template_span: SourceSpan,
}

#[derive(Clone, Debug)]
pub struct SfcAnalysis {
    pub source_id: LogicalSourceId,
    pub dialect: SfcDialect,
    pub state: CapabilityState,
    pub file_facts: Vec<FileFacts>,
    pub script_attachments: Vec<SfcScriptAttachment>,
    pub component_uses: Vec<SfcComponentUse>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SymbolNamespace {
    Value,
    Type,
}

impl SymbolNamespace {
    pub(crate) fn tag(self) -> u8 {
        match self {
            Self::Value => 1,
            Self::Type => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImportKind {
    Named,
    Default,
    Namespace,
    SideEffect,
    ReExportNamed,
    ReExportAll,
    DynamicBroad,
    CommonJsComputed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModuleRequestKind {
    StaticImport,
    DynamicImport,
    ImportMetaGlob,
    Require,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceSpan {
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportFact {
    pub source_id: LogicalSourceId,
    pub exported_name: String,
    pub local_name: Option<String>,
    pub namespace: SymbolNamespace,
    pub span: SourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceUseFact {
    pub importer: LogicalSourceId,
    pub specifier: String,
    pub imported_name: Option<String>,
    pub local_name: Option<String>,
    pub namespace: SymbolNamespace,
    pub kind: ImportKind,
    pub request_kind: ModuleRequestKind,
    pub span: SourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventoryBoundSourceUse {
    pub source_use: SourceUseFact,
    pub target: LogicalSourceId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileFacts {
    pub source_id: LogicalSourceId,
    pub source_unit: SourceUnitId,
    pub exports: Vec<ExportFact>,
    pub uses: Vec<SourceUseFact>,
    pub limitations: Vec<Limitation>,
}

impl FileFacts {
    pub fn physical(source_id: LogicalSourceId) -> Self {
        Self {
            source_unit: SourceUnitId::Logical(source_id.clone()),
            source_id,
            exports: Vec::new(),
            uses: Vec::new(),
            limitations: Vec::new(),
        }
    }

    pub fn embedded(parent_source_id: LogicalSourceId, unit_id: EmbeddedSourceUnitId) -> Self {
        Self {
            source_id: parent_source_id,
            source_unit: SourceUnitId::Embedded(unit_id),
            exports: Vec::new(),
            uses: Vec::new(),
            limitations: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedSourceUse {
    pub source_use: SourceUseFact,
    pub outcome: ResolutionOutcome,
}

pub fn external_package_name(specifier: &str) -> String {
    if specifier.starts_with('#') {
        return specifier.to_owned();
    }
    if let Some(scoped) = specifier.strip_prefix('@') {
        let mut parts = scoped.split('/');
        let scope = parts.next().unwrap_or_default();
        let package = parts.next().unwrap_or_default();
        if package.is_empty() {
            format!("@{scope}")
        } else {
            format!("@{scope}/{package}")
        }
    } else {
        specifier.split('/').next().unwrap_or(specifier).to_owned()
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum UnresolvedTargetScope {
    ExplicitTargets,
    KnownNoTarget { package: String },
    OpaqueWorkspace,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DynamicImportTargetScope {
    ExplicitTargets,
    SourceInventory,
    Workspace,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ImportMetaGlobTargetScope {
    ExplicitTargets,
    Package,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ResolutionOutcome {
    Internal {
        target: LogicalSourceId,
    },
    External {
        package: String,
    },
    NonSourceAsset {
        specifier: String,
    },
    Unresolved {
        specifier: String,
        candidates: Vec<String>,
        #[serde(
            default,
            rename = "targetScope",
            alias = "target_scope",
            skip_serializing_if = "Option::is_none"
        )]
        target_scope: Option<UnresolvedTargetScope>,
    },
    Unsupported {
        specifier: String,
        reason: String,
    },
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyIntentIdentity {
    pub consumer: LogicalSourceId,
    pub dependency: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PackageScopeId(String);

impl PackageScopeId {
    pub fn from_root(root: &RepoPath) -> Self {
        Self(format!("package_{}", digest_hex(root.canonical_bytes())))
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageScope {
    id: PackageScopeId,
    root: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    canonical_root: Vec<u8>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PackageScopeWire {
    id: PackageScopeId,
    root: String,
    #[serde(default)]
    canonical_root: Vec<u8>,
}

impl<'de> Deserialize<'de> for PackageScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = PackageScopeWire::deserialize(deserializer)?;
        if wire.canonical_root.is_empty() {
            return Ok(Self {
                id: wire.id,
                root: wire.root,
                canonical_root: wire.canonical_root,
            });
        }
        let root = RepoPath::from_canonical_bytes(&wire.canonical_root)
            .map_err(|error| D::Error::custom(error.to_string()))?;
        let expected = Self::from_root(&root);
        if wire.id != expected.id || wire.root != expected.root {
            return Err(D::Error::custom(
                "package scope identity disagrees with its canonical root",
            ));
        }
        Ok(expected)
    }
}

impl PackageScope {
    pub fn from_root(root: &RepoPath) -> Self {
        Self {
            id: PackageScopeId::from_root(root),
            root: root.display_escaped(),
            canonical_root: root.canonical_bytes().to_vec(),
        }
    }

    pub fn id(&self) -> &PackageScopeId {
        &self.id
    }

    pub fn canonical_root(&self) -> &[u8] {
        &self.canonical_root
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum Limitation {
    JsRecoverableParseLocal {
        source_id: LogicalSourceId,
        detail: String,
    },
    JsModuleUseUnknown {
        source_id: LogicalSourceId,
        detail: String,
    },
    DynamicImportNonLiteral {
        source_id: LogicalSourceId,
        source_unit: SourceUnitId,
        span: SourceSpan,
        #[serde(
            default,
            rename = "staticPrefix",
            alias = "static_prefix",
            skip_serializing_if = "Option::is_none"
        )]
        static_prefix: Option<String>,
        candidates: Vec<LogicalSourceId>,
        #[serde(rename = "targetScope", alias = "target_scope")]
        target_scope: DynamicImportTargetScope,
    },
    ImportMetaGlobUnsupported {
        source_id: LogicalSourceId,
        source_unit: Box<SourceUnitId>,
        span: SourceSpan,
        patterns: Box<[String]>,
        candidates: Vec<LogicalSourceId>,
        #[serde(rename = "targetScope", alias = "target_scope")]
        target_scope: ImportMetaGlobTargetScope,
        detail: String,
    },
    CommonJsComputedMember {
        source_id: LogicalSourceId,
        specifier: String,
        span: SourceSpan,
        target: LogicalSourceId,
    },
    SourcePayloadUnavailable {
        path: String,
        detail: String,
    },
    InternalSpecifierUnresolved {
        importer: LogicalSourceId,
        specifier: String,
        candidates: Vec<String>,
        #[serde(
            default,
            rename = "targetScope",
            alias = "target_scope",
            skip_serializing_if = "Option::is_none"
        )]
        target_scope: Option<UnresolvedTargetScope>,
    },
    PackageImportsUnsupported {
        path: String,
        detail: String,
    },
    AliasShapeUnsupported {
        source_id: LogicalSourceId,
        detail: String,
    },
    AbsoluteInternalSpecifierUnsupported {
        source_id: LogicalSourceId,
        detail: String,
    },
    ImporterFormatUnsupported {
        path: String,
        detail: String,
    },
    PublicSurfaceUnsupported {
        path: String,
        detail: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        importer: Option<LogicalSourceId>,
    },
    TsconfigSemanticsUnsupported {
        path: String,
        detail: String,
    },
    PackageIdentityUnsupported {
        path: String,
        detail: String,
    },
    PackageMetadataUnobservable {
        path: String,
        detail: String,
    },
    PackagePrivacyUnsupported {
        path: String,
        detail: String,
    },
    DependencyOwnerAmbiguous {
        path: String,
        #[serde(
            default,
            rename = "packageScope",
            alias = "package_scope",
            skip_serializing_if = "Option::is_none"
        )]
        package_scope: Option<Box<PackageScope>>,
        #[serde(
            default,
            rename = "requiredIntent",
            alias = "required_intent",
            skip_serializing_if = "Option::is_none"
        )]
        required_intent: Option<Box<DependencyIntentIdentity>>,
        detail: String,
    },
    WorkspaceOwnershipUnsupported {
        path: String,
        detail: String,
    },
    PnpmDependencySemanticsUnsupported {
        path: String,
        detail: String,
    },
    TsconfigPayloadUnavailable {
        path: String,
        detail: String,
    },
    SfcDialectUnavailable {
        source_id: LogicalSourceId,
        dialect: String,
    },
    SfcDecompositionUnknown {
        source_id: LogicalSourceId,
        detail: String,
    },
    VueExternalScriptModeConflict {
        source_id: LogicalSourceId,
        target_source_id: LogicalSourceId,
        declared: String,
        actual: String,
    },
    VueTemplateOpaque {
        source_id: LogicalSourceId,
        detail: String,
    },
    ExplicitEntryUnavailable {
        path: String,
        source: EntrySource,
        unavailable_reason: EntryUnavailableReason,
    },
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum LimitationFactOwner {
    Inventory,
    Js,
    Resolve,
    Sfc,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum LimitationScopePolicy {
    File,
    Workspace,
    ResolvedModule,
    ExplicitTargetsOrWorkspace,
    ExplicitTargetsOrSourceInventoryOrWorkspace,
    ExplicitTargetsOrKnownNoTargetOrWorkspace,
    SourceOwnerPackageOrWorkspace,
    OwningPackage,
    OwningPackageOrWorkspace,
    ConfiguredPackagesOrWorkspace,
    ManifestOwnerPackageOrWorkspace,
    WorkspaceFromConfig,
    ParentAndTargetOwnersOrWorkspace,
    ImportedTargetsOrPackage,
    EntryOwnerPackageOrWorkspace,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum LimitationAbsenceEffect {
    LocalDefinitions,
    WorkspaceConsumers,
    ModuleValueExports,
    CandidateConsumers,
    PackageConsumers,
    PackageTargetsAndConsumers,
    ConfigurationDomain,
    PublicSurface,
    PackageIdentityAndDependencyOwner,
    PackagePrivacy,
    DependencyOwnerAndInferredWrites,
    WorkspaceOwnership,
    PnpmDependencyAndInferredWrites,
    ScriptAndTemplateBindings,
    ComponentIdentity,
    UnreachableModules,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum LimitationGateRelevance {
    RequiredEvidence,
    RequiredOwner,
    NormalizedOpacity,
    NormalizedUnresolvedOrRequiredEvidence,
    NormalizedOpacityOrRequiredEvidence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LimitationRegistryEntry {
    pub reason: &'static str,
    pub fact_owner: LimitationFactOwner,
    pub scope: LimitationScopePolicy,
    pub absence_effect: LimitationAbsenceEffect,
    pub gate_relevance: LimitationGateRelevance,
}

macro_rules! define_limitation_registry {
    (
        $(
            $variant:ident => {
                owner: $owner:ident,
                scope: $scope:ident,
                absence: $absence:ident,
                gate: $gate:ident,
            }
        ),+ $(,)?
    ) => {
        pub const LIMITATION_REGISTRY: &[LimitationRegistryEntry] = &[
            $(
                LimitationRegistryEntry {
                    reason: stringify!($variant),
                    fact_owner: LimitationFactOwner::$owner,
                    scope: LimitationScopePolicy::$scope,
                    absence_effect: LimitationAbsenceEffect::$absence,
                    gate_relevance: LimitationGateRelevance::$gate,
                },
            )+
        ];

        impl Limitation {
            pub const fn registry_entry(&self) -> LimitationRegistryEntry {
                match self {
                    $(
                        Self::$variant { .. } => LimitationRegistryEntry {
                            reason: stringify!($variant),
                            fact_owner: LimitationFactOwner::$owner,
                            scope: LimitationScopePolicy::$scope,
                            absence_effect: LimitationAbsenceEffect::$absence,
                            gate_relevance: LimitationGateRelevance::$gate,
                        },
                    )+
                }
            }
        }
    };
}

define_limitation_registry! {
    JsRecoverableParseLocal => {
        owner: Js,
        scope: File,
        absence: LocalDefinitions,
        gate: RequiredEvidence,
    },
    JsModuleUseUnknown => {
        owner: Js,
        scope: Workspace,
        absence: WorkspaceConsumers,
        gate: RequiredEvidence,
    },
    DynamicImportNonLiteral => {
        owner: Js,
        scope: ExplicitTargetsOrSourceInventoryOrWorkspace,
        absence: CandidateConsumers,
        gate: NormalizedOpacityOrRequiredEvidence,
    },
    ImportMetaGlobUnsupported => {
        owner: Js,
        scope: ImportedTargetsOrPackage,
        absence: CandidateConsumers,
        gate: NormalizedOpacityOrRequiredEvidence,
    },
    CommonJsComputedMember => {
        owner: Js,
        scope: ResolvedModule,
        absence: ModuleValueExports,
        gate: NormalizedOpacity,
    },
    SourcePayloadUnavailable => {
        owner: Inventory,
        scope: Workspace,
        absence: WorkspaceConsumers,
        gate: RequiredEvidence,
    },
    InternalSpecifierUnresolved => {
        owner: Resolve,
        scope: ExplicitTargetsOrKnownNoTargetOrWorkspace,
        absence: CandidateConsumers,
        gate: NormalizedUnresolvedOrRequiredEvidence,
    },
    PackageImportsUnsupported => {
        owner: Resolve,
        scope: OwningPackage,
        absence: PackageConsumers,
        gate: RequiredEvidence,
    },
    AliasShapeUnsupported => {
        owner: Resolve,
        scope: SourceOwnerPackageOrWorkspace,
        absence: PackageConsumers,
        gate: RequiredEvidence,
    },
    AbsoluteInternalSpecifierUnsupported => {
        owner: Resolve,
        scope: Workspace,
        absence: WorkspaceConsumers,
        gate: RequiredEvidence,
    },
    ImporterFormatUnsupported => {
        owner: Resolve,
        scope: OwningPackageOrWorkspace,
        absence: PackageTargetsAndConsumers,
        gate: RequiredEvidence,
    },
    PublicSurfaceUnsupported => {
        owner: Resolve,
        scope: OwningPackage,
        absence: PublicSurface,
        gate: RequiredEvidence,
    },
    TsconfigSemanticsUnsupported => {
        owner: Resolve,
        scope: ConfiguredPackagesOrWorkspace,
        absence: ConfigurationDomain,
        gate: RequiredEvidence,
    },
    PackageIdentityUnsupported => {
        owner: Inventory,
        scope: Workspace,
        absence: PackageIdentityAndDependencyOwner,
        gate: RequiredEvidence,
    },
    PackageMetadataUnobservable => {
        owner: Inventory,
        scope: ManifestOwnerPackageOrWorkspace,
        absence: PublicSurface,
        gate: RequiredEvidence,
    },
    PackagePrivacyUnsupported => {
        owner: Inventory,
        scope: OwningPackage,
        absence: PackagePrivacy,
        gate: RequiredEvidence,
    },
    DependencyOwnerAmbiguous => {
        owner: Inventory,
        scope: OwningPackageOrWorkspace,
        absence: DependencyOwnerAndInferredWrites,
        gate: RequiredEvidence,
    },
    WorkspaceOwnershipUnsupported => {
        owner: Inventory,
        scope: Workspace,
        absence: WorkspaceOwnership,
        gate: RequiredEvidence,
    },
    PnpmDependencySemanticsUnsupported => {
        owner: Inventory,
        scope: WorkspaceFromConfig,
        absence: PnpmDependencyAndInferredWrites,
        gate: RequiredEvidence,
    },
    TsconfigPayloadUnavailable => {
        owner: Inventory,
        scope: ConfiguredPackagesOrWorkspace,
        absence: ConfigurationDomain,
        gate: RequiredEvidence,
    },
    SfcDialectUnavailable => {
        owner: Sfc,
        scope: Workspace,
        absence: WorkspaceConsumers,
        gate: RequiredOwner,
    },
    SfcDecompositionUnknown => {
        owner: Sfc,
        scope: Workspace,
        absence: WorkspaceConsumers,
        gate: RequiredEvidence,
    },
    VueExternalScriptModeConflict => {
        owner: Sfc,
        scope: ParentAndTargetOwnersOrWorkspace,
        absence: ScriptAndTemplateBindings,
        gate: RequiredEvidence,
    },
    VueTemplateOpaque => {
        owner: Sfc,
        scope: ImportedTargetsOrPackage,
        absence: ComponentIdentity,
        gate: NormalizedOpacityOrRequiredEvidence,
    },
    ExplicitEntryUnavailable => {
        owner: Inventory,
        scope: EntryOwnerPackageOrWorkspace,
        absence: UnreachableModules,
        gate: RequiredEvidence,
    },
}

impl Limitation {
    pub fn canonical_cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.registry_entry()
            .reason
            .cmp(other.registry_entry().reason)
            .then_with(|| self.cmp(other))
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityState {
    Complete,
    Incomplete,
    Unavailable,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum FindingDisposition {
    ReviewCandidate,
    ReviewOnly { reason: ReviewOnlyReason },
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewOnlyReason {
    GeneratedSource,
    VendoredSource,
    GeneratedAndVendoredSource,
}
