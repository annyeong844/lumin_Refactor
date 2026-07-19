use serde::{Deserialize, Serialize};

use crate::{EmbeddedSourceUnitId, LogicalSourceId, RepoPath, digest_hex};

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

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRoles {
    pub test_like: Option<SourceRoleReason>,
    pub generated: Option<SourceRoleReason>,
    pub vendored: Option<SourceRoleReason>,
    pub declaration: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleOverride {
    pub pattern: String,
    pub role: ScanRole,
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
    pub payload_sha256: String,
    pub bytes: Vec<u8>,
}

impl SourceSnapshot {
    pub fn new(path: RepoPath, kind: SourceKind, roles: SourceRoles, bytes: Vec<u8>) -> Self {
        let id = LogicalSourceId::from_path(&path);
        let payload_sha256 = digest_hex(&bytes);
        Self {
            id,
            path,
            kind,
            roles,
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModuleRequestKind {
    StaticImport,
    DynamicImport,
    Require,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
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
    },
    Unsupported {
        specifier: String,
        reason: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum Limitation {
    JsModuleUseUnknown {
        source_id: LogicalSourceId,
        detail: String,
    },
    SourcePayloadUnavailable {
        path: String,
        detail: String,
    },
    InternalSpecifierUnresolved {
        importer: LogicalSourceId,
        specifier: String,
        candidates: Vec<String>,
    },
    PackageImportsUnsupported {
        path: String,
        detail: String,
    },
    ImporterFormatUnsupported {
        path: String,
        detail: String,
    },
    PublicSurfaceUnsupported {
        path: String,
        detail: String,
    },
    TsconfigSemanticsUnsupported {
        path: String,
        detail: String,
    },
    PackageDependencySemanticsUnsupported {
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
    SfcExternalScriptUnresolved {
        source_id: LogicalSourceId,
        specifier: String,
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
