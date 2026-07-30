use std::collections::BTreeMap;

use lumin_evidence::{
    EvidenceRecord, FindingRecord, FindingRelationRecord, GateRecord, RunEvidence,
};
use lumin_model::{EvidenceId, FindingId, FindingRelationId, GateId, SourceSpan};
use serde::{Deserialize, Serialize};

use crate::cursor::{decode_cursor_payload, encode_cursor_payload};
use crate::{
    FINDINGS_ORDERING, FINDINGS_PAGE_SIZE, FindingCollectionDto, FindingDto, ProtocolError,
    RepoPathDto, ScopeDto,
};

pub const EVIDENCE_ORDERING: &str = "evidence.v1";
pub const RELATIONS_ORDERING: &str = "relations.v1";
pub const NESTED_PAGE_SIZE: usize = 100;

const GATE_FINDINGS_PATH: &str = "gate/findings";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceCollectionDto {
    pub schema_version: &'static str,
    pub scope: ScopeDto,
    pub filters: BTreeMap<String, Vec<String>>,
    pub ordering: &'static str,
    pub scope_total: usize,
    pub total: usize,
    pub returned: usize,
    pub truncated: bool,
    pub next_cursor: Option<String>,
    pub items: Vec<EvidenceDto>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationCollectionDto {
    pub schema_version: &'static str,
    pub scope: ScopeDto,
    pub filters: BTreeMap<String, Vec<String>>,
    pub ordering: &'static str,
    pub scope_total: usize,
    pub total: usize,
    pub returned: usize,
    pub truncated: bool,
    pub next_cursor: Option<String>,
    pub items: Vec<FindingRelationDto>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GateExplainResponseDto {
    pub schema_version: &'static str,
    pub scope: ScopeDto,
    pub finding: FindingDto,
    pub evidence: EvidenceCollectionDto,
    pub relations: RelationCollectionDto,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceDto {
    pub evidence_id: EvidenceId,
    pub kind: String,
    pub source_id: String,
    pub path: RepoPathDto,
    pub span: SourceSpan,
    pub payload_sha256: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingRelationDto {
    pub relation_id: FindingRelationId,
    pub kind: String,
    pub target_finding_id: FindingId,
    pub grounding_evidence_id: EvidenceId,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GateCursorDto {
    schema_version: String,
    gate_id: GateId,
    revision: u64,
    finding_id: Option<FindingId>,
    collection_path: String,
    ordering: String,
    page_size: usize,
    filters: BTreeMap<String, Vec<String>>,
    last_id: String,
}

pub fn gate_findings_response(
    gate: &GateRecord,
    revision: u64,
    cursor: Option<&str>,
) -> Result<FindingCollectionDto, ProtocolError> {
    let evidence = revision_evidence(gate, revision)?;
    let filters = BTreeMap::new();
    let start = match cursor {
        Some(cursor) => {
            let cursor = validated_cursor(
                cursor,
                gate,
                revision,
                None,
                GATE_FINDINGS_PATH,
                FINDINGS_ORDERING,
                FINDINGS_PAGE_SIZE,
                &filters,
            )?;
            evidence
                .findings
                .iter()
                .position(|finding| finding.finding_id.as_str() == cursor.last_id)
                .map(|index| index + 1)
                .ok_or(ProtocolError::CursorAnchorMissing)?
        }
        None => 0,
    };
    let end = start
        .saturating_add(FINDINGS_PAGE_SIZE)
        .min(evidence.findings.len());
    let items = evidence.findings[start..end]
        .iter()
        .map(FindingDto::from)
        .collect::<Vec<_>>();
    let truncated = end < evidence.findings.len();
    let next_cursor = if truncated {
        let last_id = items
            .last()
            .ok_or(ProtocolError::CursorAnchorMissing)?
            .finding_id
            .as_str();
        Some(encode_gate_cursor(
            gate,
            revision,
            None,
            GATE_FINDINGS_PATH,
            FINDINGS_ORDERING,
            FINDINGS_PAGE_SIZE,
            &filters,
            last_id,
        )?)
    } else {
        None
    };
    Ok(FindingCollectionDto {
        schema_version: "lumin.collection.v1",
        scope: gate_scope(gate, revision),
        filters,
        ordering: FINDINGS_ORDERING,
        scope_total: evidence.findings.len(),
        total: evidence.findings.len(),
        returned: items.len(),
        truncated,
        next_cursor,
        items,
    })
}

pub fn gate_explain_response(
    gate: &GateRecord,
    revision: u64,
    finding_id: &FindingId,
    evidence_cursor: Option<&str>,
    relations_cursor: Option<&str>,
) -> Result<GateExplainResponseDto, ProtocolError> {
    let run_evidence = revision_evidence(gate, revision)?;
    let finding = run_evidence
        .findings
        .iter()
        .find(|finding| &finding.finding_id == finding_id)
        .ok_or_else(|| ProtocolError::FindingNotFound(finding_id.as_str().to_owned()))?;
    let scope = gate_scope(gate, revision);
    Ok(GateExplainResponseDto {
        schema_version: "lumin.gate-explain.v1",
        scope: scope.clone(),
        finding: FindingDto::from(finding),
        evidence: evidence_page(gate, revision, finding, evidence_cursor, scope.clone())?,
        relations: relations_page(gate, revision, finding, relations_cursor, scope)?,
    })
}

fn evidence_page(
    gate: &GateRecord,
    revision: u64,
    finding: &FindingRecord,
    cursor: Option<&str>,
    scope: ScopeDto,
) -> Result<EvidenceCollectionDto, ProtocolError> {
    let filters = BTreeMap::new();
    let collection_path = format!("gate/findings/{}/evidence", finding.finding_id.as_str());
    let start = match cursor {
        Some(cursor) => {
            let cursor = validated_cursor(
                cursor,
                gate,
                revision,
                Some(&finding.finding_id),
                &collection_path,
                EVIDENCE_ORDERING,
                NESTED_PAGE_SIZE,
                &filters,
            )?;
            finding
                .evidence
                .iter()
                .position(|evidence| evidence.evidence_id.as_str() == cursor.last_id)
                .map(|index| index + 1)
                .ok_or(ProtocolError::CursorAnchorMissing)?
        }
        None => 0,
    };
    let end = start
        .saturating_add(NESTED_PAGE_SIZE)
        .min(finding.evidence.len());
    let items = finding.evidence[start..end]
        .iter()
        .map(EvidenceDto::from)
        .collect::<Vec<_>>();
    let truncated = end < finding.evidence.len();
    let next_cursor = if truncated {
        let last_id = items
            .last()
            .ok_or(ProtocolError::CursorAnchorMissing)?
            .evidence_id
            .as_str();
        Some(encode_gate_cursor(
            gate,
            revision,
            Some(&finding.finding_id),
            &collection_path,
            EVIDENCE_ORDERING,
            NESTED_PAGE_SIZE,
            &filters,
            last_id,
        )?)
    } else {
        None
    };
    Ok(EvidenceCollectionDto {
        schema_version: "lumin.collection.v1",
        scope,
        filters,
        ordering: EVIDENCE_ORDERING,
        scope_total: finding.evidence.len(),
        total: finding.evidence.len(),
        returned: items.len(),
        truncated,
        next_cursor,
        items,
    })
}

fn relations_page(
    gate: &GateRecord,
    revision: u64,
    finding: &FindingRecord,
    cursor: Option<&str>,
    scope: ScopeDto,
) -> Result<RelationCollectionDto, ProtocolError> {
    let filters = BTreeMap::new();
    let collection_path = format!("gate/findings/{}/relations", finding.finding_id.as_str());
    let start = match cursor {
        Some(cursor) => {
            let cursor = validated_cursor(
                cursor,
                gate,
                revision,
                Some(&finding.finding_id),
                &collection_path,
                RELATIONS_ORDERING,
                NESTED_PAGE_SIZE,
                &filters,
            )?;
            finding
                .relations
                .iter()
                .position(|relation| relation.relation_id.as_str() == cursor.last_id)
                .map(|index| index + 1)
                .ok_or(ProtocolError::CursorAnchorMissing)?
        }
        None => 0,
    };
    let end = start
        .saturating_add(NESTED_PAGE_SIZE)
        .min(finding.relations.len());
    let items = finding.relations[start..end]
        .iter()
        .map(FindingRelationDto::from)
        .collect::<Vec<_>>();
    let truncated = end < finding.relations.len();
    let next_cursor = if truncated {
        let last_id = items
            .last()
            .ok_or(ProtocolError::CursorAnchorMissing)?
            .relation_id
            .as_str();
        Some(encode_gate_cursor(
            gate,
            revision,
            Some(&finding.finding_id),
            &collection_path,
            RELATIONS_ORDERING,
            NESTED_PAGE_SIZE,
            &filters,
            last_id,
        )?)
    } else {
        None
    };
    Ok(RelationCollectionDto {
        schema_version: "lumin.collection.v1",
        scope,
        filters,
        ordering: RELATIONS_ORDERING,
        scope_total: finding.relations.len(),
        total: finding.relations.len(),
        returned: items.len(),
        truncated,
        next_cursor,
        items,
    })
}

fn revision_evidence(gate: &GateRecord, revision: u64) -> Result<&RunEvidence, ProtocolError> {
    let revision_record = gate
        .revisions
        .iter()
        .find(|candidate| candidate.revision == revision)
        .ok_or(ProtocolError::GateRevisionMissing(revision))?;
    if revision == 0 {
        gate.baseline
            .as_ref()
            .map(|baseline| &baseline.snapshot.evidence)
            .ok_or(ProtocolError::GateRevisionEvidenceUnavailable(revision))
    } else {
        revision_record
            .snapshot
            .as_ref()
            .map(|snapshot| &snapshot.evidence)
            .ok_or(ProtocolError::GateRevisionEvidenceUnavailable(revision))
    }
}

fn gate_scope(gate: &GateRecord, revision: u64) -> ScopeDto {
    ScopeDto::GateAttempt {
        gate_id: gate.gate_id.clone(),
        revision,
    }
}

#[allow(clippy::too_many_arguments)]
fn encode_gate_cursor(
    gate: &GateRecord,
    revision: u64,
    finding_id: Option<&FindingId>,
    collection_path: &str,
    ordering: &str,
    page_size: usize,
    filters: &BTreeMap<String, Vec<String>>,
    last_id: &str,
) -> Result<String, ProtocolError> {
    encode_cursor_payload(&GateCursorDto {
        schema_version: "lumin-gate-cursor.v1".to_owned(),
        gate_id: gate.gate_id.clone(),
        revision,
        finding_id: finding_id.cloned(),
        collection_path: collection_path.to_owned(),
        ordering: ordering.to_owned(),
        page_size,
        filters: filters.clone(),
        last_id: last_id.to_owned(),
    })
}

#[allow(clippy::too_many_arguments)]
fn validated_cursor(
    value: &str,
    gate: &GateRecord,
    revision: u64,
    finding_id: Option<&FindingId>,
    collection_path: &str,
    ordering: &str,
    page_size: usize,
    filters: &BTreeMap<String, Vec<String>>,
) -> Result<GateCursorDto, ProtocolError> {
    let cursor: GateCursorDto = decode_cursor_payload(value)?;
    if cursor.schema_version != "lumin-gate-cursor.v1"
        || cursor.gate_id != gate.gate_id
        || cursor.revision != revision
        || cursor.finding_id.as_ref() != finding_id
        || cursor.collection_path != collection_path
        || cursor.ordering != ordering
        || cursor.page_size != page_size
        || &cursor.filters != filters
    {
        return Err(ProtocolError::CursorScopeMismatch);
    }
    Ok(cursor)
}

impl From<&EvidenceRecord> for EvidenceDto {
    fn from(evidence: &EvidenceRecord) -> Self {
        Self {
            evidence_id: evidence.evidence_id.clone(),
            kind: evidence.kind.clone(),
            source_id: evidence.source_id.as_str().to_owned(),
            path: RepoPathDto::from(&evidence.path),
            span: evidence.span.clone(),
            payload_sha256: evidence.payload_sha256.clone(),
        }
    }
}

impl From<&FindingRelationRecord> for FindingRelationDto {
    fn from(relation: &FindingRelationRecord) -> Self {
        Self {
            relation_id: relation.relation_id.clone(),
            kind: relation.kind.clone(),
            target_finding_id: relation.target_finding_id.clone(),
            grounding_evidence_id: relation.grounding_evidence_id.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use lumin_evidence::{
        AnalysisSnapshot, Confidence, GateAnalysisOptions, GateBaseline, GateDecision,
        GateLifecycle, GateRevision, RepoPathProjection, Severity, sort_findings,
    };
    use lumin_model::{
        AnalysisInputId, FindingDisposition, LogicalSourceId, OperationId, RepoPath,
        SymbolNamespace,
    };

    use super::*;

    #[test]
    fn gate_findings_cursor_is_bound_to_gate_and_revision() -> Result<(), Box<dyn std::error::Error>>
    {
        let gate = gate_with_nested_finding()?;
        let first = gate_findings_response(&gate, 1, None)?;
        assert_eq!(first.returned, FINDINGS_PAGE_SIZE);
        assert!(first.truncated);
        let cursor = first
            .next_cursor
            .as_deref()
            .ok_or_else(|| std::io::Error::other("missing gate findings cursor"))?;
        let second = gate_findings_response(&gate, 1, Some(cursor))?;
        assert_eq!(second.returned, 1);
        assert!(!second.truncated);

        assert!(matches!(
            gate_findings_response(&gate, 0, Some(cursor)),
            Err(ProtocolError::CursorScopeMismatch)
        ));
        let mut other_gate = gate.clone();
        other_gate.gate_id = GateId::from_string("gate-other".to_owned());
        assert!(matches!(
            gate_findings_response(&other_gate, 1, Some(cursor)),
            Err(ProtocolError::CursorScopeMismatch)
        ));
        Ok(())
    }

    #[test]
    fn gate_explain_pages_all_nested_evidence_and_relations()
    -> Result<(), Box<dyn std::error::Error>> {
        let gate = gate_with_nested_finding()?;
        let first_finding = gate_findings_response(&gate, 1, None)?
            .items
            .into_iter()
            .next()
            .ok_or_else(|| std::io::Error::other("missing finding"))?;
        let first = gate_explain_response(&gate, 1, &first_finding.finding_id, None, None)?;
        assert_eq!(first.evidence.returned, NESTED_PAGE_SIZE);
        assert_eq!(first.relations.returned, NESTED_PAGE_SIZE);
        assert!(first.evidence.truncated);
        assert!(first.relations.truncated);

        let second = gate_explain_response(
            &gate,
            1,
            &first_finding.finding_id,
            first.evidence.next_cursor.as_deref(),
            first.relations.next_cursor.as_deref(),
        )?;
        assert_eq!(second.evidence.returned, 1);
        assert_eq!(second.relations.returned, 1);
        assert!(!second.evidence.truncated);
        assert!(!second.relations.truncated);
        assert_ne!(
            first.evidence.items[99].evidence_id,
            second.evidence.items[0].evidence_id
        );
        assert_ne!(
            first.relations.items[99].relation_id,
            second.relations.items[0].relation_id
        );
        Ok(())
    }

    fn gate_with_nested_finding() -> Result<GateRecord, Box<dyn std::error::Error>> {
        let mut findings = (0..101)
            .map(|index| finding(index, if index == 0 { 101 } else { 1 }))
            .collect::<Result<Vec<_>, _>>()?;
        sort_findings(&mut findings);
        let evidence = RunEvidence {
            schema_version: "lumin-evidence.v1".to_owned(),
            capabilities: Vec::new(),
            resolution_profiles: Vec::new(),
            findings,
            limitations: Vec::new(),
        };
        let snapshot = AnalysisSnapshot {
            analysis_input_id: AnalysisInputId::from_string("analysis-input-1".to_owned()),
            inputs: Vec::new(),
            evidence: evidence.clone(),
        };
        Ok(GateRecord {
            schema_version: "lumin-gate.v1".to_owned(),
            gate_id: GateId::from_string("gate-a".to_owned()),
            lifecycle: GateLifecycle::Closed,
            current_revision: 1,
            declared_write_set: Vec::new(),
            leased_write_set: Vec::new(),
            alias_closures: Vec::new(),
            transition_refs: Vec::new(),
            analysis_options: GateAnalysisOptions {
                jobs: 1,
                resolution_profile: None,
            },
            baseline: Some(GateBaseline {
                analysis_contract: "contract".to_owned(),
                snapshot: snapshot.clone(),
                protected_semantic_inputs: Vec::new(),
                transition_sequence: 0,
            }),
            protected_semantic_inputs: Vec::new(),
            revisions: vec![
                GateRevision {
                    revision: 0,
                    operation_id: OperationId::from_string("open".to_owned()),
                    committed_unix_millis: None,
                    decision: GateDecision::Allow,
                    reason: None,
                    signals: Vec::new(),
                    changed_paths: Vec::new(),
                    snapshot: None,
                    protected_semantic_inputs: Vec::new(),
                    alias_closures: Vec::new(),
                    reconciled_transition_sequences: Vec::new(),
                    deltas: Vec::new(),
                },
                GateRevision {
                    revision: 1,
                    operation_id: OperationId::from_string("close".to_owned()),
                    committed_unix_millis: None,
                    decision: GateDecision::Allow,
                    reason: None,
                    signals: Vec::new(),
                    changed_paths: Vec::new(),
                    snapshot: Some(snapshot),
                    protected_semantic_inputs: Vec::new(),
                    alias_closures: Vec::new(),
                    reconciled_transition_sequences: Vec::new(),
                    deltas: Vec::new(),
                },
            ],
        })
    }

    fn finding(
        index: usize,
        nested_count: usize,
    ) -> Result<FindingRecord, Box<dyn std::error::Error>> {
        let path = RepoPath::from_portable(&format!("src/file-{index:03}.ts"))?;
        let source_id = LogicalSourceId::from_path(&path);
        let finding_id = FindingId::for_export(
            "dead-code/zero-exact-fan-in.v1",
            &source_id,
            SymbolNamespace::Value,
            &format!("dead{index:03}"),
        );
        let mut evidence = Vec::new();
        let mut relations = Vec::new();
        for nested in 0..nested_count {
            let span = SourceSpan {
                start: nested as u32,
                end: nested as u32 + 1,
            };
            let evidence_id = EvidenceId::for_source_span(
                "definition",
                &source_id,
                span.start,
                span.end,
                &format!("payload-{nested:03}"),
            );
            evidence.push(EvidenceRecord {
                evidence_id: evidence_id.clone(),
                kind: "definition".to_owned(),
                source_id: source_id.clone(),
                path: RepoPathProjection::from(&path),
                span,
                payload_sha256: format!("payload-{nested:03}"),
            });
            let target = FindingId::from_string(format!("target-{nested:03}"));
            relations.push(FindingRelationRecord {
                relation_id: FindingRelationId::for_finding(
                    &finding_id,
                    "related",
                    &target,
                    &evidence_id,
                ),
                kind: "related".to_owned(),
                target_finding_id: target,
                grounding_evidence_id: evidence_id,
            });
        }
        Ok(FindingRecord {
            finding_id,
            rule_id: "dead-code/zero-exact-fan-in.v1".to_owned(),
            owner_capability: "dead-code.v1".to_owned(),
            severity: Severity::Warning,
            confidence: Confidence::Grounded,
            disposition: FindingDisposition::ReviewCandidate,
            claim: "zero grounded exact fan-in".to_owned(),
            source_id,
            path: RepoPathProjection::from(&path),
            span: SourceSpan { start: 0, end: 1 },
            exported_name: format!("dead{index:03}"),
            namespace: SymbolNamespace::Value,
            evidence,
            relations,
        })
    }
}
