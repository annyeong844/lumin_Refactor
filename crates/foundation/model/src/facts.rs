use std::sync::Arc;

use serde::{Deserialize, Serialize};

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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModuleRequestKind {
    StaticImport,
    DynamicImport,
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
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum Limitation {
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
    Workspace,
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
    WorkspaceConsumers,
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
