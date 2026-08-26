mod cache;
mod delta;
mod gate;
mod retention;
mod transition;

pub use cache::*;
pub use gate::*;
pub use retention::*;
pub use transition::*;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use lumin_model::{
    BuildIdentity, CapabilityState, EvidenceId, FindingDisposition, FindingId, FindingRelationId,
    GateId, Limitation, LogicalSourceId, PayloadSnapshotId, PhysicalFileIdentity, RepoPath,
    RepositoryId, ResolutionOutcome, ResolvedSourceUse, RunId, SelectedResolutionProfile,
    SourceKind, SourceRoleClassification, SourceSpan, SymbolNamespace, append_length_prefixed,
    digest_hex,
};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

pub const DEAD_EXPORT_RULE_ID: &str = "dead-code/zero-exact-fan-in.v1";
pub const DEAD_CODE_CAPABILITY_ID: &str = "dead-code.v1";
pub const DEPENDENCY_OWNERSHIP_CAPABILITY_ID: &str = "inventory/dependency-ownership.v1";
pub const RUN_EVIDENCE_CAPABILITY_IDS: [&str; 5] = [
    DEAD_CODE_CAPABILITY_ID,
    DEPENDENCY_OWNERSHIP_CAPABILITY_ID,
    "sfc/astro.v1",
    "sfc/svelte.v1",
    "sfc/vue.v1",
];
pub const FINDINGS_ORDERING_ID: &str = "findings.v1";
pub const EVIDENCE_ORDERING_ID: &str = "evidence.v1";
pub const RELATIONS_ORDERING_ID: &str = "relations.v1";
pub const CAPABILITIES_ORDERING_ID: &str = "capabilities.v1";
pub const FILE_FINDINGS_ORDERING_ID: &str = "file-findings.v1";
pub const ACTIVE_GATES_ORDERING_ID: &str = "active-gates.v1";

// The architecture check must inspect Limitation variants outside macro token streams.
#[allow(clippy::match_like_matches_macro)]
pub fn dead_code_capability_state(limitations: &[Limitation]) -> CapabilityState {
    if limitations.iter().any(|limitation| match limitation {
        Limitation::DependencyOwnerAmbiguous { .. }
        | Limitation::PnpmDependencySemanticsUnsupported { .. } => false,
        _ => true,
    }) {
        CapabilityState::Incomplete
    } else {
        CapabilityState::Complete
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EvidenceQueryScope {
    Binary {
        build_identity: BuildIdentity,
    },
    Run {
        repository_id: RepositoryId,
        run_id: RunId,
    },
    GateAttempt {
        repository_id: RepositoryId,
        gate_id: GateId,
        revision: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectionOrderingId(String);

impl CollectionOrderingId {
    pub fn from_string(value: String) -> Self {
        Self(value)
    }

    pub fn findings() -> Self {
        Self(FINDINGS_ORDERING_ID.to_owned())
    }

    pub fn evidence() -> Self {
        Self(EVIDENCE_ORDERING_ID.to_owned())
    }

    pub fn relations() -> Self {
        Self(RELATIONS_ORDERING_ID.to_owned())
    }

    pub fn capabilities() -> Self {
        Self(CAPABILITIES_ORDERING_ID.to_owned())
    }

    pub fn file_findings() -> Self {
        Self(FILE_FINDINGS_ORDERING_ID.to_owned())
    }

    pub fn active_gates() -> Self {
        Self(ACTIVE_GATES_ORDERING_ID.to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageAnchor(String);

impl PageAnchor {
    pub fn from_string(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceQuery {
    pub scope: EvidenceQueryScope,
    pub finding_id: Option<FindingId>,
    pub collection_path: String,
    pub ordering: CollectionOrderingId,
    pub page_size: usize,
    pub filters: BTreeMap<String, Vec<String>>,
    pub anchor: Option<PageAnchor>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidencePage<T> {
    pub query: EvidenceQuery,
    pub scope_total: usize,
    pub total: usize,
    pub items: Vec<T>,
    pub next_query: Option<EvidenceQuery>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FindingExplanation {
    pub finding: FindingRecord,
    pub evidence: EvidencePage<EvidenceRecord>,
    pub relations: EvidencePage<FindingRelationRecord>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    Warning,
}

impl Severity {
    pub fn rank(self) -> u8 {
        match self {
            Self::Warning => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Confidence {
    Grounded,
}

impl Confidence {
    pub fn rank(self) -> u8 {
        match self {
            Self::Grounded => 1,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceRecord {
    pub evidence_id: EvidenceId,
    pub kind: String,
    pub source_id: LogicalSourceId,
    pub path: RepoPathProjection,
    pub span: SourceSpan,
    pub payload_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingRelationRecord {
    pub relation_id: FindingRelationId,
    pub kind: String,
    pub target_finding_id: FindingId,
    pub grounding_evidence_id: EvidenceId,
}

pub fn finding_relation_id(
    source_finding_id: &FindingId,
    kind: &str,
    target_finding_id: &FindingId,
    grounding_evidence_id: &EvidenceId,
) -> FindingRelationId {
    let mut bytes = Vec::new();
    append_length_prefixed(&mut bytes, b"lumin-finding-relation-id.v1");
    append_length_prefixed(&mut bytes, source_finding_id.as_str().as_bytes());
    append_length_prefixed(&mut bytes, kind.as_bytes());
    append_length_prefixed(&mut bytes, target_finding_id.as_str().as_bytes());
    append_length_prefixed(&mut bytes, grounding_evidence_id.as_str().as_bytes());
    FindingRelationId::from_string(format!("relation_{}", digest_hex(&bytes)))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingRecord {
    pub finding_id: FindingId,
    pub rule_id: String,
    pub owner_capability: String,
    pub severity: Severity,
    pub confidence: Confidence,
    pub disposition: FindingDisposition,
    pub claim: String,
    pub source_id: LogicalSourceId,
    pub path: RepoPathProjection,
    pub span: SourceSpan,
    pub exported_name: String,
    pub namespace: SymbolNamespace,
    #[serde(default)]
    pub nested_collections_available: bool,
    #[serde(default)]
    pub evidence: Vec<EvidenceRecord>,
    #[serde(default)]
    pub relations: Vec<FindingRelationRecord>,
}

impl FindingRecord {
    pub fn path_identity(&self) -> &[u8] {
        &self.path.canonical
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoPathProjection {
    pub canonical: Vec<u8>,
    pub components: Vec<Vec<u8>>,
    pub display: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoPathProjectionWire {
    canonical: Vec<u8>,
    #[serde(default, deserialize_with = "deserialize_present_path_components")]
    components: Option<Vec<Vec<u8>>>,
    display: String,
}

fn deserialize_present_path_components<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<Vec<u8>>>, D::Error>
where
    D: Deserializer<'de>,
{
    Vec::<Vec<u8>>::deserialize(deserializer).map(Some)
}

impl<'de> Deserialize<'de> for RepoPathProjection {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = RepoPathProjectionWire::deserialize(deserializer)?;
        let path = RepoPath::from_canonical_bytes(&wire.canonical)
            .map_err(|error| D::Error::custom(error.to_string()))?;
        let projection = Self::from(&path);
        if wire
            .components
            .as_ref()
            .is_some_and(|components| components != &projection.components)
        {
            return Err(D::Error::custom(
                "repo path projection components disagree with canonical path",
            ));
        }
        if wire.display != projection.display {
            return Err(D::Error::custom(
                "repo path projection display disagrees with canonical path",
            ));
        }
        Ok(projection)
    }
}

impl From<&RepoPath> for RepoPathProjection {
    fn from(path: &RepoPath) -> Self {
        Self {
            canonical: path.canonical_bytes().to_vec(),
            components: path.component_keys(),
            display: path.display_escaped(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceClassificationRecord {
    pub source_id: LogicalSourceId,
    pub path: RepoPathProjection,
    pub classifications: Vec<SourceRoleClassification>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceContextRecord {
    pub source_id: LogicalSourceId,
    pub path: RepoPathProjection,
    pub kind: SourceKind,
    pub package_root: Option<RepoPathProjection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub configuration_paths: Vec<RepoPathProjection>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceObservationRecord {
    pub source_id: LogicalSourceId,
    pub physical_identity: PhysicalFileIdentity,
    pub payload_snapshot_id: PayloadSnapshotId,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyOwnerRecord {
    pub consumer: LogicalSourceId,
    pub consumer_path: RepoPathProjection,
    pub dependency: String,
    pub package_root: RepoPathProjection,
    pub manifest_path: RepoPathProjection,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub manifest_payload_sha256: String,
    pub lockfile_path: Option<RepoPathProjection>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisMetrics {
    pub logical_source_count: usize,
    pub physical_source_count: usize,
    pub payload_snapshot_count: usize,
    pub js_parse_product_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityRecord {
    pub capability_id: String,
    pub state: CapabilityState,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunEvidence {
    pub schema_version: String,
    pub capabilities: Vec<CapabilityRecord>,
    pub resolution_profiles: Vec<SelectedResolutionProfile>,
    #[serde(default)]
    pub source_classifications: Vec<SourceClassificationRecord>,
    #[serde(default)]
    pub source_contexts: Vec<SourceContextRecord>,
    #[serde(default)]
    pub source_observations: Vec<SourceObservationRecord>,
    #[serde(default)]
    pub dependency_owners: Vec<DependencyOwnerRecord>,
    #[serde(default)]
    pub resolutions: Vec<ResolvedSourceUse>,
    #[serde(default)]
    pub metrics: AnalysisMetrics,
    pub findings: Vec<FindingRecord>,
    pub limitations: Vec<Limitation>,
}

pub const RUN_EVIDENCE_SCHEMA_VERSION: &str = "lumin-evidence.v1";

impl RunEvidence {
    pub fn dead_code_state(&self) -> CapabilityState {
        self.capabilities
            .iter()
            .find(|record| record.capability_id == DEAD_CODE_CAPABILITY_ID)
            .map_or(CapabilityState::Unavailable, |record| record.state)
    }

    pub fn semantic_projection(&self) -> SemanticRunEvidence<'_> {
        SemanticRunEvidence {
            schema_version: &self.schema_version,
            capabilities: &self.capabilities,
            resolution_profiles: &self.resolution_profiles,
            source_classifications: &self.source_classifications,
            source_contexts: &self.source_contexts,
            dependency_owners: &self.dependency_owners,
            resolutions: &self.resolutions,
            findings: &self.findings,
            limitations: &self.limitations,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunEvidenceIdentityError {
    Duplicate {
        collection: &'static str,
        identity: String,
    },
    SourceIdentityMismatch {
        collection: &'static str,
        identity: String,
    },
    DerivedIdentityMismatch {
        collection: &'static str,
        identity: String,
    },
    MissingReference {
        collection: &'static str,
        identity: String,
    },
    InventoryMismatch {
        collection: &'static str,
    },
    MetricMismatch {
        metric: &'static str,
        expected: usize,
        observed: usize,
    },
}

impl fmt::Display for RunEvidenceIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Duplicate {
                collection,
                identity,
            } => write!(formatter, "duplicate identity in {collection}: {identity}"),
            Self::SourceIdentityMismatch {
                collection,
                identity,
            } => write!(
                formatter,
                "source identity disagrees with its path in {collection}: {identity}"
            ),
            Self::DerivedIdentityMismatch {
                collection,
                identity,
            } => write!(
                formatter,
                "derived identity disagrees with its semantic owner in {collection}: {identity}"
            ),
            Self::MissingReference {
                collection,
                identity,
            } => write!(
                formatter,
                "referenced identity is absent from {collection}: {identity}"
            ),
            Self::InventoryMismatch { collection } => {
                write!(formatter, "persisted inventory disagrees with {collection}")
            }
            Self::MetricMismatch {
                metric,
                expected,
                observed,
            } => write!(
                formatter,
                "persisted analysis metric {metric} is {observed}, expected {expected}"
            ),
        }
    }
}

impl std::error::Error for RunEvidenceIdentityError {}

pub fn validate_run_evidence_identities(
    evidence: &RunEvidence,
) -> Result<(), RunEvidenceIdentityError> {
    require_unique(
        "capabilities",
        evidence
            .capabilities
            .iter()
            .map(|record| record.capability_id.as_str()),
    )?;
    require_source_records(
        "source classifications",
        evidence
            .source_classifications
            .iter()
            .map(|record| (record.source_id.as_str(), &record.source_id, &record.path)),
    )?;
    require_source_records(
        "source contexts",
        evidence
            .source_contexts
            .iter()
            .map(|record| (record.source_id.as_str(), &record.source_id, &record.path)),
    )?;
    require_unique(
        "source observations",
        evidence
            .source_observations
            .iter()
            .map(|record| record.source_id.as_str()),
    )?;
    let source_ids = evidence
        .source_observations
        .iter()
        .map(|record| record.source_id.as_str())
        .collect::<BTreeSet<_>>();
    require_unique(
        "resolution profiles",
        evidence
            .resolution_profiles
            .iter()
            .map(|record| record.source_id.as_str()),
    )?;
    for resolution in &evidence.resolutions {
        if !source_ids.contains(resolution.source_use.importer.as_str()) {
            return Err(RunEvidenceIdentityError::MissingReference {
                collection: "resolution importers",
                identity: resolution.source_use.importer.as_str().to_owned(),
            });
        }
        if let ResolutionOutcome::Internal { target } = &resolution.outcome
            && !source_ids.contains(target.as_str())
        {
            return Err(RunEvidenceIdentityError::MissingReference {
                collection: "internal resolution targets",
                identity: target.as_str().to_owned(),
            });
        }
    }
    validate_analysis_metrics(evidence)?;
    require_unique(
        "findings",
        evidence
            .findings
            .iter()
            .map(|record| record.finding_id.as_str()),
    )?;
    for finding in &evidence.findings {
        if !finding.nested_collections_available
            && (!finding.evidence.is_empty() || !finding.relations.is_empty())
        {
            return Err(RunEvidenceIdentityError::InventoryMismatch {
                collection: "finding nested-collection availability",
            });
        }
        require_source_binding(
            "findings",
            finding.finding_id.as_str(),
            &finding.source_id,
            &finding.path,
        )?;
        let expected = FindingId::for_export(
            &finding.rule_id,
            &finding.source_id,
            finding.namespace,
            &finding.exported_name,
        );
        if finding.finding_id != expected {
            return Err(RunEvidenceIdentityError::DerivedIdentityMismatch {
                collection: "findings",
                identity: finding.finding_id.as_str().to_owned(),
            });
        }
    }
    let finding_ids = evidence
        .findings
        .iter()
        .map(|record| record.finding_id.as_str())
        .collect::<BTreeSet<_>>();
    for finding in &evidence.findings {
        require_unique(
            "finding evidence",
            finding
                .evidence
                .iter()
                .map(|record| record.evidence_id.as_str()),
        )?;
        for record in &finding.evidence {
            require_source_binding(
                "finding evidence",
                record.evidence_id.as_str(),
                &record.source_id,
                &record.path,
            )?;
            let expected = EvidenceId::for_source_span(
                &record.kind,
                &record.source_id,
                record.span.start,
                record.span.end,
                &record.payload_sha256,
            );
            if record.evidence_id != expected {
                return Err(RunEvidenceIdentityError::DerivedIdentityMismatch {
                    collection: "finding evidence",
                    identity: record.evidence_id.as_str().to_owned(),
                });
            }
        }
        let finding_evidence_ids = finding
            .evidence
            .iter()
            .map(|record| record.evidence_id.as_str())
            .collect::<BTreeSet<_>>();
        require_unique(
            "finding relations",
            finding
                .relations
                .iter()
                .map(|record| record.relation_id.as_str()),
        )?;
        for relation in &finding.relations {
            let expected = finding_relation_id(
                &finding.finding_id,
                &relation.kind,
                &relation.target_finding_id,
                &relation.grounding_evidence_id,
            );
            if relation.relation_id != expected {
                return Err(RunEvidenceIdentityError::DerivedIdentityMismatch {
                    collection: "finding relations",
                    identity: relation.relation_id.as_str().to_owned(),
                });
            }
            if !finding_ids.contains(relation.target_finding_id.as_str()) {
                return Err(RunEvidenceIdentityError::MissingReference {
                    collection: "finding relation targets",
                    identity: relation.target_finding_id.as_str().to_owned(),
                });
            }
            if !finding_evidence_ids.contains(relation.grounding_evidence_id.as_str()) {
                return Err(RunEvidenceIdentityError::MissingReference {
                    collection: "finding relation grounding evidence",
                    identity: relation.grounding_evidence_id.as_str().to_owned(),
                });
            }
        }
    }
    let observed_capability_ids = evidence
        .capabilities
        .iter()
        .map(|record| record.capability_id.as_str())
        .collect::<BTreeSet<_>>();
    let expected_capability_ids = RUN_EVIDENCE_CAPABILITY_IDS
        .into_iter()
        .collect::<BTreeSet<_>>();
    if observed_capability_ids != expected_capability_ids {
        return Err(RunEvidenceIdentityError::InventoryMismatch {
            collection: "the compiled capability registry",
        });
    }
    Ok(())
}

fn validate_analysis_metrics(evidence: &RunEvidence) -> Result<(), RunEvidenceIdentityError> {
    let expected = [
        (
            "logicalSourceCount",
            evidence.source_observations.len(),
            evidence.metrics.logical_source_count,
        ),
        (
            "physicalSourceCount",
            evidence
                .source_observations
                .iter()
                .map(|record| &record.physical_identity)
                .collect::<BTreeSet<_>>()
                .len(),
            evidence.metrics.physical_source_count,
        ),
        (
            "payloadSnapshotCount",
            evidence
                .source_observations
                .iter()
                .map(|record| &record.payload_snapshot_id)
                .collect::<BTreeSet<_>>()
                .len(),
            evidence.metrics.payload_snapshot_count,
        ),
    ];
    for (metric, expected, observed) in expected {
        if observed != expected {
            return Err(RunEvidenceIdentityError::MetricMismatch {
                metric,
                expected,
                observed,
            });
        }
    }
    Ok(())
}

fn require_source_binding(
    collection: &'static str,
    identity: &str,
    source_id: &LogicalSourceId,
    path: &RepoPathProjection,
) -> Result<(), RunEvidenceIdentityError> {
    let projected = RepoPath::from_canonical_bytes(&path.canonical)
        .map(|path| LogicalSourceId::from_path(&path));
    if !projected.is_ok_and(|projected| projected == *source_id) {
        return Err(RunEvidenceIdentityError::SourceIdentityMismatch {
            collection,
            identity: identity.to_owned(),
        });
    }
    Ok(())
}

fn require_unique<'a>(
    collection: &'static str,
    identities: impl IntoIterator<Item = &'a str>,
) -> Result<(), RunEvidenceIdentityError> {
    let mut observed = BTreeSet::new();
    for identity in identities {
        if !observed.insert(identity) {
            return Err(RunEvidenceIdentityError::Duplicate {
                collection,
                identity: identity.to_owned(),
            });
        }
    }
    Ok(())
}

fn require_source_records<'a>(
    collection: &'static str,
    records: impl IntoIterator<Item = (&'a str, &'a LogicalSourceId, &'a RepoPathProjection)>,
) -> Result<(), RunEvidenceIdentityError> {
    let mut source_ids = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for (identity, source_id, path) in records {
        require_source_binding(collection, identity, source_id, path)?;
        if !source_ids.insert(identity) || !paths.insert(path.canonical.as_slice()) {
            return Err(RunEvidenceIdentityError::Duplicate {
                collection,
                identity: identity.to_owned(),
            });
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticRunEvidence<'a> {
    pub schema_version: &'a str,
    pub capabilities: &'a [CapabilityRecord],
    pub resolution_profiles: &'a [SelectedResolutionProfile],
    pub source_classifications: &'a [SourceClassificationRecord],
    pub source_contexts: &'a [SourceContextRecord],
    pub dependency_owners: &'a [DependencyOwnerRecord],
    pub resolutions: &'a [ResolvedSourceUse],
    pub findings: &'a [FindingRecord],
    pub limitations: &'a [Limitation],
}

pub fn sort_findings(findings: &mut [FindingRecord]) {
    for finding in findings.iter_mut() {
        finding.evidence.sort_by(|left, right| {
            left.kind
                .cmp(&right.kind)
                .then_with(|| left.source_id.cmp(&right.source_id))
                .then_with(|| left.span.start.cmp(&right.span.start))
                .then_with(|| left.span.end.cmp(&right.span.end))
                .then_with(|| left.evidence_id.cmp(&right.evidence_id))
        });
        finding.evidence.dedup();
        finding.relations.sort_by(|left, right| {
            left.kind
                .cmp(&right.kind)
                .then_with(|| left.target_finding_id.cmp(&right.target_finding_id))
                .then_with(|| left.relation_id.cmp(&right.relation_id))
        });
        finding.relations.dedup();
    }
    findings.sort_by(|left, right| {
        right
            .severity
            .rank()
            .cmp(&left.severity.rank())
            .then_with(|| right.confidence.rank().cmp(&left.confidence.rank()))
            .then_with(|| left.rule_id.cmp(&right.rule_id))
            .then_with(|| left.path_identity().cmp(right.path_identity()))
            .then_with(|| left.span.start.cmp(&right.span.start))
            .then_with(|| left.span.end.cmp(&right.span.end))
            .then_with(|| left.finding_id.cmp(&right.finding_id))
    });
}

#[cfg(test)]
mod tests {
    use lumin_model::{
        FindingDisposition, ImportKind, ModuleRequestKind, ResolutionOutcome, ResolvedSourceUse,
        SourceUseFact, SymbolNamespace,
    };

    use super::*;

    fn finding_fixture(path: &RepoPath, exported_name: &str) -> FindingRecord {
        let source_id = LogicalSourceId::from_path(path);
        let span = SourceSpan { start: 0, end: 1 };
        let payload_sha256 = format!("{exported_name}-payload");
        FindingRecord {
            finding_id: FindingId::for_export(
                DEAD_EXPORT_RULE_ID,
                &source_id,
                SymbolNamespace::Value,
                exported_name,
            ),
            rule_id: DEAD_EXPORT_RULE_ID.to_owned(),
            owner_capability: DEAD_CODE_CAPABILITY_ID.to_owned(),
            severity: Severity::Warning,
            confidence: Confidence::Grounded,
            disposition: FindingDisposition::ReviewCandidate,
            claim: format!("{exported_name} fixture"),
            source_id: source_id.clone(),
            path: RepoPathProjection::from(path),
            span: span.clone(),
            exported_name: exported_name.to_owned(),
            namespace: SymbolNamespace::Value,
            nested_collections_available: true,
            evidence: vec![EvidenceRecord {
                evidence_id: EvidenceId::for_source_span(
                    "definition",
                    &source_id,
                    span.start,
                    span.end,
                    &payload_sha256,
                ),
                kind: "definition".to_owned(),
                source_id,
                path: RepoPathProjection::from(path),
                span,
                payload_sha256,
            }],
            relations: Vec::new(),
        }
    }

    fn run_evidence_fixture(findings: Vec<FindingRecord>) -> RunEvidence {
        RunEvidence {
            schema_version: RUN_EVIDENCE_SCHEMA_VERSION.to_owned(),
            capabilities: RUN_EVIDENCE_CAPABILITY_IDS
                .into_iter()
                .map(|capability_id| CapabilityRecord {
                    capability_id: capability_id.to_owned(),
                    state: CapabilityState::Complete,
                })
                .collect(),
            resolution_profiles: Vec::new(),
            source_classifications: Vec::new(),
            source_contexts: Vec::new(),
            source_observations: Vec::new(),
            dependency_owners: Vec::new(),
            resolutions: Vec::new(),
            metrics: AnalysisMetrics::default(),
            findings,
            limitations: Vec::new(),
        }
    }

    #[test]
    fn dependency_ownership_only_limitations_do_not_degrade_dead_code() {
        let dependency_only = vec![
            Limitation::DependencyOwnerAmbiguous {
                path: "src/main.ts".to_owned(),
                package_scope: None,
                required_intent: None,
                detail: "ambiguous owner".to_owned(),
            },
            Limitation::PnpmDependencySemanticsUnsupported {
                path: "pnpm-workspace.yaml".to_owned(),
                detail: "unsupported pnpm semantics".to_owned(),
            },
        ];
        assert_eq!(
            dead_code_capability_state(&dependency_only),
            CapabilityState::Complete
        );
        assert_eq!(
            dead_code_capability_state(&[Limitation::PackageMetadataUnobservable {
                path: "package.json".to_owned(),
                detail: "unreadable".to_owned(),
            }]),
            CapabilityState::Incomplete
        );
    }

    #[test]
    fn persisted_run_evidence_requires_unique_owned_identities()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut evidence = RunEvidence {
            schema_version: RUN_EVIDENCE_SCHEMA_VERSION.to_owned(),
            capabilities: vec![
                CapabilityRecord {
                    capability_id: DEAD_CODE_CAPABILITY_ID.to_owned(),
                    state: CapabilityState::Complete,
                },
                CapabilityRecord {
                    capability_id: DEAD_CODE_CAPABILITY_ID.to_owned(),
                    state: CapabilityState::Incomplete,
                },
            ],
            resolution_profiles: Vec::new(),
            source_classifications: Vec::new(),
            source_contexts: Vec::new(),
            source_observations: Vec::new(),
            dependency_owners: Vec::new(),
            resolutions: Vec::new(),
            metrics: AnalysisMetrics::default(),
            findings: Vec::new(),
            limitations: Vec::new(),
        };
        assert!(matches!(
            validate_run_evidence_identities(&evidence),
            Err(RunEvidenceIdentityError::Duplicate {
                collection: "capabilities",
                ..
            })
        ));

        evidence.capabilities.truncate(1);
        let path = RepoPath::from_portable("src/a.ts")?;
        let other = RepoPath::from_portable("src/b.ts")?;
        evidence.source_contexts.push(SourceContextRecord {
            source_id: LogicalSourceId::from_path(&other),
            path: RepoPathProjection::from(&path),
            kind: SourceKind::TypeScript,
            package_root: None,
            configuration_paths: Vec::new(),
        });
        assert!(matches!(
            validate_run_evidence_identities(&evidence),
            Err(RunEvidenceIdentityError::SourceIdentityMismatch {
                collection: "source contexts",
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn persisted_findings_require_the_owner_derived_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = RepoPath::from_portable("src/finding.ts")?;
        let mut finding = finding_fixture(&path, "forged");
        finding.finding_id = FindingId::from_string("finding_forged".to_owned());
        let evidence = run_evidence_fixture(vec![finding]);

        assert!(matches!(
            validate_run_evidence_identities(&evidence),
            Err(RunEvidenceIdentityError::DerivedIdentityMismatch {
                collection: "findings",
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn persisted_findings_require_their_source_path_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let owner_path = RepoPath::from_portable("src/owner.ts")?;
        let forged_path = RepoPath::from_portable("src/forged.ts")?;
        let mut finding = finding_fixture(&owner_path, "misplaced");
        finding.path = RepoPathProjection::from(&forged_path);
        let evidence = run_evidence_fixture(vec![finding]);

        assert!(matches!(
            validate_run_evidence_identities(&evidence),
            Err(RunEvidenceIdentityError::SourceIdentityMismatch {
                collection: "findings",
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn persisted_finding_evidence_requires_its_source_path_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let owner_path = RepoPath::from_portable("src/owner.ts")?;
        let forged_path = RepoPath::from_portable("src/forged.ts")?;
        let mut finding = finding_fixture(&owner_path, "misplaced-evidence");
        finding
            .evidence
            .first_mut()
            .ok_or("finding fixture omitted evidence")?
            .path = RepoPathProjection::from(&forged_path);
        let evidence = run_evidence_fixture(vec![finding]);

        assert!(matches!(
            validate_run_evidence_identities(&evidence),
            Err(RunEvidenceIdentityError::SourceIdentityMismatch {
                collection: "finding evidence",
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn persisted_nested_collections_require_an_available_projection()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = RepoPath::from_portable("src/nested.ts")?;
        let mut finding = finding_fixture(&path, "nested");
        finding.nested_collections_available = false;
        let evidence = run_evidence_fixture(vec![finding]);

        assert!(matches!(
            validate_run_evidence_identities(&evidence),
            Err(RunEvidenceIdentityError::InventoryMismatch {
                collection: "finding nested-collection availability"
            })
        ));
        Ok(())
    }

    #[test]
    fn persisted_internal_resolutions_reference_the_source_inventory()
    -> Result<(), Box<dyn std::error::Error>> {
        let importer_path = RepoPath::from_portable("src/importer.ts")?;
        let target_path = RepoPath::from_portable("src/target.ts")?;
        let missing_path = RepoPath::from_portable("src/missing.ts")?;
        let importer = LogicalSourceId::from_path(&importer_path);
        let target = LogicalSourceId::from_path(&target_path);
        let missing = LogicalSourceId::from_path(&missing_path);
        let mut evidence = run_evidence_fixture(Vec::new());
        evidence.source_observations = vec![
            SourceObservationRecord {
                source_id: importer.clone(),
                physical_identity: PhysicalFileIdentity::Unix {
                    device: 1,
                    inode: 1,
                },
                payload_snapshot_id: PayloadSnapshotId::from_string("importer-payload".to_owned()),
            },
            SourceObservationRecord {
                source_id: target.clone(),
                physical_identity: PhysicalFileIdentity::Unix {
                    device: 1,
                    inode: 2,
                },
                payload_snapshot_id: PayloadSnapshotId::from_string("target-payload".to_owned()),
            },
        ];
        evidence.metrics = AnalysisMetrics {
            logical_source_count: 2,
            physical_source_count: 2,
            payload_snapshot_count: 2,
            js_parse_product_count: 0,
        };
        evidence.resolutions = vec![ResolvedSourceUse {
            source_use: SourceUseFact {
                importer: importer.clone(),
                specifier: "./target".to_owned(),
                imported_name: Some("target".to_owned()),
                local_name: Some("target".to_owned()),
                namespace: SymbolNamespace::Value,
                kind: ImportKind::Named,
                request_kind: ModuleRequestKind::StaticImport,
                span: SourceSpan { start: 0, end: 1 },
            },
            outcome: ResolutionOutcome::Internal {
                target: target.clone(),
            },
        }];
        validate_run_evidence_identities(&evidence)?;

        let mut missing_importer = evidence.clone();
        missing_importer.resolutions[0].source_use.importer = missing.clone();
        assert!(matches!(
            validate_run_evidence_identities(&missing_importer),
            Err(RunEvidenceIdentityError::MissingReference {
                collection: "resolution importers",
                ..
            })
        ));

        let mut missing_target = evidence;
        missing_target.resolutions[0].outcome = ResolutionOutcome::Internal { target: missing };
        assert!(matches!(
            validate_run_evidence_identities(&missing_target),
            Err(RunEvidenceIdentityError::MissingReference {
                collection: "internal resolution targets",
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn persisted_capabilities_match_the_complete_compiled_inventory() {
        let canonical = run_evidence_fixture(Vec::new());
        assert!(validate_run_evidence_identities(&canonical).is_ok());

        let mut missing = canonical.clone();
        missing.capabilities.pop();
        assert!(matches!(
            validate_run_evidence_identities(&missing),
            Err(RunEvidenceIdentityError::InventoryMismatch {
                collection: "the compiled capability registry"
            })
        ));

        let mut opaque = canonical;
        opaque.capabilities.push(CapabilityRecord {
            capability_id: "opaque.v1".to_owned(),
            state: CapabilityState::Complete,
        });
        assert!(matches!(
            validate_run_evidence_identities(&opaque),
            Err(RunEvidenceIdentityError::InventoryMismatch {
                collection: "the compiled capability registry"
            })
        ));
    }

    #[test]
    fn persisted_metrics_are_derived_from_source_observations()
    -> Result<(), Box<dyn std::error::Error>> {
        let paths = [
            RepoPath::from_portable("src/one.ts")?,
            RepoPath::from_portable("src/two.ts")?,
            RepoPath::from_portable("src/three.ts")?,
        ];
        let physical_one = PhysicalFileIdentity::Unix {
            device: 1,
            inode: 1,
        };
        let physical_two = PhysicalFileIdentity::Unix {
            device: 1,
            inode: 2,
        };
        let payload_one = PayloadSnapshotId::from_string("payload-one".to_owned());
        let payload_two = PayloadSnapshotId::from_string("payload-two".to_owned());
        let mut evidence = run_evidence_fixture(Vec::new());
        evidence.source_observations = vec![
            SourceObservationRecord {
                source_id: LogicalSourceId::from_path(&paths[0]),
                physical_identity: physical_one.clone(),
                payload_snapshot_id: payload_one.clone(),
            },
            SourceObservationRecord {
                source_id: LogicalSourceId::from_path(&paths[1]),
                physical_identity: physical_one,
                payload_snapshot_id: payload_one,
            },
            SourceObservationRecord {
                source_id: LogicalSourceId::from_path(&paths[2]),
                physical_identity: physical_two,
                payload_snapshot_id: payload_two,
            },
        ];
        evidence.metrics = AnalysisMetrics {
            logical_source_count: 3,
            physical_source_count: 2,
            payload_snapshot_count: 2,
            js_parse_product_count: 7,
        };
        validate_run_evidence_identities(&evidence)?;

        for metric in [
            "logicalSourceCount",
            "physicalSourceCount",
            "payloadSnapshotCount",
        ] {
            let mut forged = evidence.clone();
            match metric {
                "logicalSourceCount" => forged.metrics.logical_source_count += 1,
                "physicalSourceCount" => forged.metrics.physical_source_count += 1,
                "payloadSnapshotCount" => forged.metrics.payload_snapshot_count += 1,
                _ => unreachable!(),
            }
            assert!(matches!(
                validate_run_evidence_identities(&forged),
                Err(RunEvidenceIdentityError::MetricMismatch {
                    metric: observed,
                    ..
                }) if observed == metric
            ));
        }
        Ok(())
    }

    #[test]
    fn persisted_relations_require_existing_target_and_grounding_evidence()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = RepoPath::from_portable("src/relation-endpoints.ts")?;
        let mut source = finding_fixture(&path, "source");
        let target = finding_fixture(&path, "target");
        let grounding_evidence_id = source.evidence[0].evidence_id.clone();
        source.relations.push(FindingRelationRecord {
            relation_id: finding_relation_id(
                &source.finding_id,
                "related",
                &target.finding_id,
                &grounding_evidence_id,
            ),
            kind: "related".to_owned(),
            target_finding_id: target.finding_id.clone(),
            grounding_evidence_id,
        });
        let canonical = run_evidence_fixture(vec![source, target]);
        validate_run_evidence_identities(&canonical)?;

        let mut missing_target = canonical.clone();
        missing_target.findings.pop();
        assert!(matches!(
            validate_run_evidence_identities(&missing_target),
            Err(RunEvidenceIdentityError::MissingReference {
                collection: "finding relation targets",
                ..
            })
        ));

        let mut missing_grounding = canonical;
        missing_grounding.findings[0].evidence.clear();
        assert!(matches!(
            validate_run_evidence_identities(&missing_grounding),
            Err(RunEvidenceIdentityError::MissingReference {
                collection: "finding relation grounding evidence",
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn persisted_relations_require_the_owner_derived_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = RepoPath::from_portable("src/relation.ts")?;
        let source_id = LogicalSourceId::from_path(&path);
        let finding_id = FindingId::for_export(
            DEAD_EXPORT_RULE_ID,
            &source_id,
            SymbolNamespace::Value,
            "source",
        );
        let target_finding_id = FindingId::for_export(
            DEAD_EXPORT_RULE_ID,
            &source_id,
            SymbolNamespace::Value,
            "target",
        );
        let evidence_id =
            EvidenceId::for_source_span("definition", &source_id, 0, 1, "payload-sha256");
        let evidence = RunEvidence {
            schema_version: RUN_EVIDENCE_SCHEMA_VERSION.to_owned(),
            capabilities: vec![CapabilityRecord {
                capability_id: DEAD_CODE_CAPABILITY_ID.to_owned(),
                state: CapabilityState::Complete,
            }],
            resolution_profiles: Vec::new(),
            source_classifications: Vec::new(),
            source_contexts: Vec::new(),
            source_observations: Vec::new(),
            dependency_owners: Vec::new(),
            resolutions: Vec::new(),
            metrics: AnalysisMetrics::default(),
            findings: vec![FindingRecord {
                finding_id,
                rule_id: DEAD_EXPORT_RULE_ID.to_owned(),
                owner_capability: DEAD_CODE_CAPABILITY_ID.to_owned(),
                severity: Severity::Warning,
                confidence: Confidence::Grounded,
                disposition: FindingDisposition::ReviewCandidate,
                claim: "relation identity fixture".to_owned(),
                source_id,
                path: RepoPathProjection::from(&path),
                span: SourceSpan { start: 0, end: 1 },
                exported_name: "source".to_owned(),
                namespace: SymbolNamespace::Value,
                nested_collections_available: true,
                evidence: Vec::new(),
                relations: vec![FindingRelationRecord {
                    relation_id: FindingRelationId::from_string("relation_forged".to_owned()),
                    kind: "related".to_owned(),
                    target_finding_id,
                    grounding_evidence_id: evidence_id,
                }],
            }],
            limitations: Vec::new(),
        };

        assert!(matches!(
            validate_run_evidence_identities(&evidence),
            Err(RunEvidenceIdentityError::DerivedIdentityMismatch {
                collection: "finding relations",
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn persisted_finding_evidence_requires_the_owner_derived_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = RepoPath::from_portable("src/evidence.ts")?;
        let source_id = LogicalSourceId::from_path(&path);
        let finding_id = FindingId::for_export(
            DEAD_EXPORT_RULE_ID,
            &source_id,
            SymbolNamespace::Value,
            "evidence",
        );
        let evidence = RunEvidence {
            schema_version: RUN_EVIDENCE_SCHEMA_VERSION.to_owned(),
            capabilities: vec![CapabilityRecord {
                capability_id: DEAD_CODE_CAPABILITY_ID.to_owned(),
                state: CapabilityState::Complete,
            }],
            resolution_profiles: Vec::new(),
            source_classifications: Vec::new(),
            source_contexts: Vec::new(),
            source_observations: Vec::new(),
            dependency_owners: Vec::new(),
            resolutions: Vec::new(),
            metrics: AnalysisMetrics::default(),
            findings: vec![FindingRecord {
                finding_id,
                rule_id: DEAD_EXPORT_RULE_ID.to_owned(),
                owner_capability: DEAD_CODE_CAPABILITY_ID.to_owned(),
                severity: Severity::Warning,
                confidence: Confidence::Grounded,
                disposition: FindingDisposition::ReviewCandidate,
                claim: "evidence identity fixture".to_owned(),
                source_id: source_id.clone(),
                path: RepoPathProjection::from(&path),
                span: SourceSpan { start: 0, end: 1 },
                exported_name: "evidence".to_owned(),
                namespace: SymbolNamespace::Value,
                nested_collections_available: true,
                evidence: vec![EvidenceRecord {
                    evidence_id: EvidenceId::from_string("evidence_forged".to_owned()),
                    kind: "definition".to_owned(),
                    source_id,
                    path: RepoPathProjection::from(&path),
                    span: SourceSpan { start: 0, end: 1 },
                    payload_sha256: "payload-sha256".to_owned(),
                }],
                relations: Vec::new(),
            }],
            limitations: Vec::new(),
        };

        assert!(matches!(
            validate_run_evidence_identities(&evidence),
            Err(RunEvidenceIdentityError::DerivedIdentityMismatch {
                collection: "finding evidence",
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn canonicalization_deduplicates_identical_nested_rows()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = RepoPath::from_portable("src/lib.ts")?;
        let source_id = LogicalSourceId::from_path(&path);
        let finding_id = FindingId::for_export(
            DEAD_EXPORT_RULE_ID,
            &source_id,
            SymbolNamespace::Value,
            "dead",
        );
        let span = SourceSpan { start: 0, end: 4 };
        let evidence_id = EvidenceId::for_source_span(
            "definition",
            &source_id,
            span.start,
            span.end,
            "payload-sha256",
        );
        let evidence = EvidenceRecord {
            evidence_id: evidence_id.clone(),
            kind: "definition".to_owned(),
            source_id: source_id.clone(),
            path: RepoPathProjection::from(&path),
            span: span.clone(),
            payload_sha256: "payload-sha256".to_owned(),
        };
        let target_finding_id = FindingId::from_string("finding-target".to_owned());
        let relation = FindingRelationRecord {
            relation_id: finding_relation_id(
                &finding_id,
                "related",
                &target_finding_id,
                &evidence_id,
            ),
            kind: "related".to_owned(),
            target_finding_id,
            grounding_evidence_id: evidence_id,
        };
        let mut findings = vec![FindingRecord {
            finding_id,
            rule_id: DEAD_EXPORT_RULE_ID.to_owned(),
            owner_capability: DEAD_CODE_CAPABILITY_ID.to_owned(),
            severity: Severity::Warning,
            confidence: Confidence::Grounded,
            disposition: FindingDisposition::ReviewCandidate,
            claim: "zero grounded exact fan-in".to_owned(),
            source_id,
            path: RepoPathProjection::from(&path),
            span,
            exported_name: "dead".to_owned(),
            namespace: SymbolNamespace::Value,
            nested_collections_available: true,
            evidence: vec![evidence.clone(), evidence],
            relations: vec![relation.clone(), relation],
        }];

        sort_findings(&mut findings);

        assert!(findings[0].nested_collections_available);
        assert_eq!(findings[0].evidence.len(), 1);
        assert_eq!(findings[0].relations.len(), 1);
        Ok(())
    }
}

#[cfg(test)]
fn ordering_finding(
    finding_id: &str,
    rule_id: &str,
    path: &RepoPath,
    span: SourceSpan,
) -> FindingRecord {
    FindingRecord {
        finding_id: FindingId::from_string(finding_id.to_owned()),
        rule_id: rule_id.to_owned(),
        owner_capability: DEAD_CODE_CAPABILITY_ID.to_owned(),
        severity: Severity::Warning,
        confidence: Confidence::Grounded,
        disposition: FindingDisposition::ReviewCandidate,
        claim: "ordering fixture".to_owned(),
        source_id: LogicalSourceId::from_path(path),
        path: RepoPathProjection::from(path),
        span,
        exported_name: finding_id.to_owned(),
        namespace: SymbolNamespace::Value,
        nested_collections_available: true,
        evidence: Vec::new(),
        relations: Vec::new(),
    }
}

#[test]
fn canonical_ordering_is_stable_across_insertion_orders_and_ties()
-> Result<(), Box<dyn std::error::Error>> {
    let path_a = RepoPath::from_portable("src/a.ts")?;
    let path_b = RepoPath::from_portable("src/b.ts")?;
    let source_a = LogicalSourceId::from_string("source-a".to_owned());
    let source_b = LogicalSourceId::from_string("source-b".to_owned());
    let evidence = |id: &str, kind: &str, source_id: &LogicalSourceId, start: u32| EvidenceRecord {
        evidence_id: EvidenceId::from_string(id.to_owned()),
        kind: kind.to_owned(),
        source_id: source_id.clone(),
        path: RepoPathProjection::from(&path_a),
        span: SourceSpan {
            start,
            end: start + 1,
        },
        payload_sha256: id.to_owned(),
    };
    let relation = |id: &str, kind: &str, target: &str| FindingRelationRecord {
        relation_id: FindingRelationId::from_string(id.to_owned()),
        kind: kind.to_owned(),
        target_finding_id: FindingId::from_string(target.to_owned()),
        grounding_evidence_id: EvidenceId::from_string(format!("evidence-{id}")),
    };

    let mut tied = ordering_finding(
        "finding-a",
        "rule-a",
        &path_a,
        SourceSpan { start: 0, end: 1 },
    );
    tied.evidence = vec![
        evidence("evidence-z", "z-kind", &source_a, 0),
        evidence("evidence-source-b", "a-kind", &source_b, 0),
        evidence("evidence-b", "a-kind", &source_a, 0),
        evidence("evidence-span", "a-kind", &source_a, 1),
        evidence("evidence-a", "a-kind", &source_a, 0),
    ];
    tied.relations = vec![
        relation("relation-z", "z-kind", "target-a"),
        relation("relation-target-b", "a-kind", "target-b"),
        relation("relation-b", "a-kind", "target-a"),
        relation("relation-a", "a-kind", "target-a"),
    ];
    let base = vec![
        ordering_finding(
            "finding-rule-b",
            "rule-b",
            &path_a,
            SourceSpan { start: 0, end: 1 },
        ),
        ordering_finding(
            "finding-path-b",
            "rule-a",
            &path_b,
            SourceSpan { start: 0, end: 1 },
        ),
        ordering_finding(
            "finding-span",
            "rule-a",
            &path_a,
            SourceSpan { start: 1, end: 2 },
        ),
        ordering_finding(
            "finding-b",
            "rule-a",
            &path_a,
            SourceSpan { start: 0, end: 1 },
        ),
        tied,
    ];
    let expected_findings = [
        "finding-a",
        "finding-b",
        "finding-span",
        "finding-path-b",
        "finding-rule-b",
    ];
    let expected_evidence = [
        "evidence-a",
        "evidence-b",
        "evidence-span",
        "evidence-source-b",
        "evidence-z",
    ];
    let expected_relations = [
        "relation-a",
        "relation-b",
        "relation-target-b",
        "relation-z",
    ];

    for shift in 0..base.len() {
        let mut findings = base.clone();
        findings.rotate_left(shift);
        if shift % 2 == 1 {
            findings.reverse();
        }
        sort_findings(&mut findings);
        assert_eq!(
            findings
                .iter()
                .map(|finding| finding.finding_id.as_str())
                .collect::<Vec<_>>(),
            expected_findings
        );
        assert_eq!(
            findings[0]
                .evidence
                .iter()
                .map(|item| item.evidence_id.as_str())
                .collect::<Vec<_>>(),
            expected_evidence
        );
        assert_eq!(
            findings[0]
                .relations
                .iter()
                .map(|item| item.relation_id.as_str())
                .collect::<Vec<_>>(),
            expected_relations
        );
    }
    Ok(())
}
