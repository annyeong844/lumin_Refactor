mod delta;
mod gate;
mod retention;

pub use gate::*;
pub use retention::*;

use std::collections::BTreeMap;

use lumin_model::{
    BuildIdentity, CapabilityState, EvidenceId, FindingDisposition, FindingId, FindingRelationId,
    GateId, Limitation, LogicalSourceId, PayloadSnapshotId, PhysicalFileIdentity, RepoPath,
    RepositoryId, ResolvedSourceUse, RunId, SelectedResolutionProfile, SourceKind,
    SourceRoleClassification, SourceSpan, SymbolNamespace, append_length_prefixed, digest_hex,
};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

pub const DEAD_EXPORT_RULE_ID: &str = "dead-code/zero-exact-fan-in.v1";
pub const DEAD_CODE_CAPABILITY_ID: &str = "dead-code.v1";
pub const FINDINGS_ORDERING_ID: &str = "findings.v1";
pub const EVIDENCE_ORDERING_ID: &str = "evidence.v1";
pub const RELATIONS_ORDERING_ID: &str = "relations.v1";
pub const CAPABILITIES_ORDERING_ID: &str = "capabilities.v1";
pub const FILE_FINDINGS_ORDERING_ID: &str = "file-findings.v1";
pub const ACTIVE_GATES_ORDERING_ID: &str = "active-gates.v1";

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
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceObservationRecord {
    pub source_id: LogicalSourceId,
    pub physical_identity: PhysicalFileIdentity,
    pub payload_snapshot_id: PayloadSnapshotId,
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
    pub resolutions: Vec<ResolvedSourceUse>,
    #[serde(default)]
    pub metrics: AnalysisMetrics,
    pub findings: Vec<FindingRecord>,
    pub limitations: Vec<Limitation>,
}

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
            resolutions: &self.resolutions,
            findings: &self.findings,
            limitations: &self.limitations,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticRunEvidence<'a> {
    pub schema_version: &'a str,
    pub capabilities: &'a [CapabilityRecord],
    pub resolution_profiles: &'a [SelectedResolutionProfile],
    pub source_classifications: &'a [SourceClassificationRecord],
    pub source_contexts: &'a [SourceContextRecord],
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
    use lumin_model::{FindingDisposition, SymbolNamespace};

    use super::*;

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
