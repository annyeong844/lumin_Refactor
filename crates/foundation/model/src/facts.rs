use serde::{Deserialize, Serialize};

use crate::{LogicalSourceId, RepoPath, digest_hex};

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
    pub namespace: SymbolNamespace,
    pub kind: ImportKind,
    pub span: SourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileFacts {
    pub source_id: LogicalSourceId,
    pub exports: Vec<ExportFact>,
    pub uses: Vec<SourceUseFact>,
    pub limitations: Vec<Limitation>,
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
    SfcDialectUnavailable {
        source_id: LogicalSourceId,
        dialect: String,
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
