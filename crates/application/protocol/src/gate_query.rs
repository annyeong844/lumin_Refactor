use std::collections::BTreeMap;

use lumin_evidence::{
    CAPABILITIES_ORDERING_ID, CapabilityRecord, CollectionOrderingId, EVIDENCE_ORDERING_ID,
    EvidencePage, EvidenceQuery, EvidenceQueryScope, EvidenceRecord, FINDINGS_ORDERING_ID,
    FindingExplanation, FindingRecord, FindingRelationRecord, PageAnchor, RELATIONS_ORDERING_ID,
    SourceClassificationRecord, SourceContextRecord, SourceObservationRecord,
};
use lumin_model::{
    BuildIdentity, EvidenceId, FindingId, FindingRelationId, GateId, RepositoryId,
    ResolvedSourceUse, RunId, SelectedResolutionProfile, SourceSpan,
};
use serde::{Deserialize, Serialize};

use crate::cursor::{decode_cursor_payload, encode_cursor_payload};
use crate::{
    CapabilityStateDto, FindingCollectionDto, FindingDto, ProtocolError, RepoPathDto, ScopeDto,
    SourceClassificationDto, SourceContextDto, SourceObservationDto,
};

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
    repository_id: RepositoryId,
    gate_id: GateId,
    revision: u64,
    finding_id: Option<FindingId>,
    collection_path: String,
    ordering: String,
    page_size: usize,
    filters: BTreeMap<String, Vec<String>>,
    last_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunCursorDto {
    schema_version: String,
    repository_id: RepositoryId,
    run_id: RunId,
    finding_id: Option<FindingId>,
    collection_path: String,
    ordering: String,
    page_size: usize,
    filters: BTreeMap<String, Vec<String>>,
    last_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BinaryCursorDto {
    schema_version: String,
    build_id: BuildIdentity,
    collection_path: String,
    ordering: String,
    page_size: usize,
    filters: BTreeMap<String, Vec<String>>,
    last_id: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityCollectionDto {
    pub schema_version: &'static str,
    pub scope: ScopeDto,
    pub filters: BTreeMap<String, Vec<String>>,
    pub ordering: &'static str,
    pub scope_total: usize,
    pub total: usize,
    pub returned: usize,
    pub truncated: bool,
    pub next_cursor: Option<String>,
    pub items: Vec<CapabilityStateDto>,
}

pub fn decode_run_query_cursor(
    value: Option<&str>,
) -> Result<Option<EvidenceQuery>, ProtocolError> {
    value
        .map(|value| {
            let cursor: RunCursorDto = decode_cursor_payload(value)?;
            if cursor.schema_version != "lumin-run-cursor.v2" {
                return Err(ProtocolError::CursorScopeMismatch);
            }
            Ok(EvidenceQuery {
                scope: EvidenceQueryScope::Run {
                    repository_id: cursor.repository_id,
                    run_id: cursor.run_id,
                },
                finding_id: cursor.finding_id,
                collection_path: cursor.collection_path,
                ordering: CollectionOrderingId::from_string(cursor.ordering),
                page_size: cursor.page_size,
                filters: cursor.filters,
                anchor: Some(PageAnchor::from_string(cursor.last_id)),
            })
        })
        .transpose()
}

pub fn decode_binary_query_cursor(
    value: Option<&str>,
) -> Result<Option<EvidenceQuery>, ProtocolError> {
    value
        .map(|value| {
            let cursor: BinaryCursorDto = decode_cursor_payload(value)?;
            if cursor.schema_version != "lumin-binary-cursor.v2" {
                return Err(ProtocolError::CursorScopeMismatch);
            }
            Ok(EvidenceQuery {
                scope: EvidenceQueryScope::Binary {
                    build_identity: cursor.build_id,
                },
                finding_id: None,
                collection_path: cursor.collection_path,
                ordering: CollectionOrderingId::from_string(cursor.ordering),
                page_size: cursor.page_size,
                filters: cursor.filters,
                anchor: Some(PageAnchor::from_string(cursor.last_id)),
            })
        })
        .transpose()
}

pub fn decode_gate_query_cursor(
    value: Option<&str>,
) -> Result<Option<EvidenceQuery>, ProtocolError> {
    value
        .map(|value| {
            let cursor: GateCursorDto = decode_cursor_payload(value)?;
            if cursor.schema_version != "lumin-gate-cursor.v2" {
                return Err(ProtocolError::CursorScopeMismatch);
            }
            Ok(EvidenceQuery {
                scope: EvidenceQueryScope::GateAttempt {
                    repository_id: cursor.repository_id,
                    gate_id: cursor.gate_id,
                    revision: cursor.revision,
                },
                finding_id: cursor.finding_id,
                collection_path: cursor.collection_path,
                ordering: CollectionOrderingId::from_string(cursor.ordering),
                page_size: cursor.page_size,
                filters: cursor.filters,
                anchor: Some(PageAnchor::from_string(cursor.last_id)),
            })
        })
        .transpose()
}

pub fn gate_findings_response(
    page: &EvidencePage<FindingRecord>,
) -> Result<FindingCollectionDto, ProtocolError> {
    let next_cursor = encode_next_cursor(page.next_query.as_ref())?;
    Ok(FindingCollectionDto {
        schema_version: "lumin.collection.v1",
        scope: scope(&page.query),
        filters: page.query.filters.clone(),
        ordering: ordering(&page.query, FINDINGS_ORDERING_ID)?,
        scope_total: page.scope_total,
        total: page.total,
        returned: page.items.len(),
        truncated: next_cursor.is_some(),
        next_cursor,
        source_classification: None,
        source_context: None,
        source_observation: None,
        resolution_profile: None,
        resolutions: Vec::new(),
        items: page.items.iter().map(FindingDto::from).collect(),
    })
}

pub fn gate_explain_response(
    explanation: &FindingExplanation,
) -> Result<GateExplainResponseDto, ProtocolError> {
    let scope = scope(&explanation.evidence.query);
    Ok(GateExplainResponseDto {
        schema_version: "lumin.gate-explain.v1",
        scope: scope.clone(),
        finding: FindingDto::from(&explanation.finding),
        evidence: evidence_response(&explanation.evidence, scope.clone())?,
        relations: relations_response(&explanation.relations, scope)?,
    })
}

pub fn run_findings_response(
    page: &EvidencePage<FindingRecord>,
) -> Result<FindingCollectionDto, ProtocolError> {
    let next_cursor = encode_next_cursor(page.next_query.as_ref())?;
    Ok(FindingCollectionDto {
        schema_version: "lumin.collection.v1",
        scope: scope(&page.query),
        filters: page.query.filters.clone(),
        ordering: ordering(&page.query, FINDINGS_ORDERING_ID)?,
        scope_total: page.scope_total,
        total: page.total,
        returned: page.items.len(),
        truncated: next_cursor.is_some(),
        next_cursor,
        source_classification: None,
        source_context: None,
        source_observation: None,
        resolution_profile: None,
        resolutions: Vec::new(),
        items: page.items.iter().map(FindingDto::from).collect(),
    })
}

pub fn capabilities_response(
    page: &EvidencePage<CapabilityRecord>,
) -> Result<CapabilityCollectionDto, ProtocolError> {
    let next_cursor = encode_next_cursor(page.next_query.as_ref())?;
    Ok(CapabilityCollectionDto {
        schema_version: "lumin.collection.v1",
        scope: scope(&page.query),
        filters: page.query.filters.clone(),
        ordering: ordering(&page.query, CAPABILITIES_ORDERING_ID)?,
        scope_total: page.scope_total,
        total: page.total,
        returned: page.items.len(),
        truncated: next_cursor.is_some(),
        next_cursor,
        items: page.items.iter().map(CapabilityStateDto::from).collect(),
    })
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunExplainResponseDto {
    pub schema_version: &'static str,
    pub scope: ScopeDto,
    pub finding: FindingDto,
    pub evidence: EvidenceCollectionDto,
    pub relations: RelationCollectionDto,
}

pub fn run_explain_response(
    explanation: &FindingExplanation,
) -> Result<RunExplainResponseDto, ProtocolError> {
    let scope = scope(&explanation.evidence.query);
    Ok(RunExplainResponseDto {
        schema_version: "lumin.run-explain.v1",
        scope: scope.clone(),
        finding: FindingDto::from(&explanation.finding),
        evidence: evidence_response(&explanation.evidence, scope.clone())?,
        relations: relations_response(&explanation.relations, scope)?,
    })
}

pub fn run_relations_response(
    page: &EvidencePage<FindingRelationRecord>,
) -> Result<RelationCollectionDto, ProtocolError> {
    let scope = scope(&page.query);
    relations_response(page, scope)
}

pub fn run_file_findings_response(
    page: &EvidencePage<FindingRecord>,
    source_classification: Option<&SourceClassificationRecord>,
    source_context: Option<&SourceContextRecord>,
    source_observation: Option<&SourceObservationRecord>,
    resolution_profile: Option<&SelectedResolutionProfile>,
    resolutions: &[ResolvedSourceUse],
) -> Result<FindingCollectionDto, ProtocolError> {
    let next_cursor = encode_next_cursor(page.next_query.as_ref())?;
    Ok(FindingCollectionDto {
        schema_version: "lumin.collection.v1",
        scope: scope(&page.query),
        filters: page.query.filters.clone(),
        ordering: ordering(&page.query, lumin_evidence::FILE_FINDINGS_ORDERING_ID)?,
        scope_total: page.scope_total,
        total: page.total,
        returned: page.items.len(),
        truncated: next_cursor.is_some(),
        next_cursor,
        source_classification: source_classification.map(SourceClassificationDto::from),
        source_context: source_context.map(SourceContextDto::from),
        source_observation: source_observation.map(SourceObservationDto::from),
        resolution_profile: resolution_profile.cloned(),
        resolutions: resolutions.to_vec(),
        items: page.items.iter().map(FindingDto::from).collect(),
    })
}

// --- Active Gates Catalog ---

pub const ACTIVE_GATES_PAGE_SIZE: usize = 100;
const ACTIVE_GATES_CURSOR_SCHEMA: &str = "lumin-active-gates-cursor.v1";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ActiveGatesCursorDto {
    schema_version: String,
    repository_id: RepositoryId,
    revision: u64,
    ordering: String,
    page_size: usize,
    opening_sequence: u64,
    gate_id: GateId,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveGatesCollectionDto {
    pub schema_version: &'static str,
    pub repository_id: RepositoryId,
    pub revision: u64,
    pub filters: BTreeMap<String, Vec<String>>,
    pub ordering: &'static str,
    pub scope_total: usize,
    pub total: usize,
    pub returned: usize,
    pub truncated: bool,
    pub next_cursor: Option<String>,
    pub items: Vec<ActiveGateItemDto>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveGateItemDto {
    pub gate_id: GateId,
    pub current_revision: u64,
    pub opening_transition_sequence: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedActiveGatesCursor {
    pub repository_id: RepositoryId,
    pub revision: u64,
    pub page_size: usize,
    pub opening_sequence: u64,
    pub gate_id: GateId,
}

pub fn decode_active_gates_cursor(value: &str) -> Result<DecodedActiveGatesCursor, ProtocolError> {
    let cursor: ActiveGatesCursorDto = decode_cursor_payload(value)?;
    if cursor.schema_version != ACTIVE_GATES_CURSOR_SCHEMA {
        return Err(ProtocolError::CursorScopeMismatch);
    }
    if cursor.ordering != lumin_evidence::ACTIVE_GATES_ORDERING_ID {
        return Err(ProtocolError::CursorScopeMismatch);
    }
    if cursor.page_size != ACTIVE_GATES_PAGE_SIZE {
        return Err(ProtocolError::CursorScopeMismatch);
    }
    Ok(DecodedActiveGatesCursor {
        repository_id: cursor.repository_id,
        revision: cursor.revision,
        page_size: cursor.page_size,
        opening_sequence: cursor.opening_sequence,
        gate_id: cursor.gate_id,
    })
}

pub fn active_gates_response(
    repository_id: RepositoryId,
    revision: u64,
    scope_total: usize,
    total: usize,
    items: Vec<ActiveGateItemDto>,
    truncated: bool,
) -> Result<ActiveGatesCollectionDto, ProtocolError> {
    let next_cursor = if truncated {
        let last = items
            .last()
            .ok_or_else(|| ProtocolError::ResponseCursorAnchorMissing("active gates".to_owned()))?;
        Some(encode_cursor_payload(&ActiveGatesCursorDto {
            schema_version: ACTIVE_GATES_CURSOR_SCHEMA.to_owned(),
            repository_id: repository_id.clone(),
            revision,
            ordering: lumin_evidence::ACTIVE_GATES_ORDERING_ID.to_owned(),
            page_size: ACTIVE_GATES_PAGE_SIZE,
            opening_sequence: last.opening_transition_sequence,
            gate_id: last.gate_id.clone(),
        })?)
    } else {
        None
    };
    Ok(ActiveGatesCollectionDto {
        schema_version: "lumin.active-gates.v1",
        repository_id,
        revision,
        filters: BTreeMap::new(),
        ordering: lumin_evidence::ACTIVE_GATES_ORDERING_ID,
        scope_total,
        total,
        returned: items.len(),
        truncated,
        next_cursor,
        items,
    })
}

fn evidence_response(
    page: &EvidencePage<EvidenceRecord>,
    scope: ScopeDto,
) -> Result<EvidenceCollectionDto, ProtocolError> {
    let next_cursor = encode_next_cursor(page.next_query.as_ref())?;
    Ok(EvidenceCollectionDto {
        schema_version: "lumin.collection.v1",
        scope,
        filters: page.query.filters.clone(),
        ordering: ordering(&page.query, EVIDENCE_ORDERING_ID)?,
        scope_total: page.scope_total,
        total: page.total,
        returned: page.items.len(),
        truncated: next_cursor.is_some(),
        next_cursor,
        items: page.items.iter().map(EvidenceDto::from).collect(),
    })
}

fn relations_response(
    page: &EvidencePage<FindingRelationRecord>,
    scope: ScopeDto,
) -> Result<RelationCollectionDto, ProtocolError> {
    let next_cursor = encode_next_cursor(page.next_query.as_ref())?;
    Ok(RelationCollectionDto {
        schema_version: "lumin.collection.v1",
        scope,
        filters: page.query.filters.clone(),
        ordering: ordering(&page.query, RELATIONS_ORDERING_ID)?,
        scope_total: page.scope_total,
        total: page.total,
        returned: page.items.len(),
        truncated: next_cursor.is_some(),
        next_cursor,
        items: page.items.iter().map(FindingRelationDto::from).collect(),
    })
}

fn scope(query: &EvidenceQuery) -> ScopeDto {
    match &query.scope {
        EvidenceQueryScope::Binary { build_identity } => ScopeDto::Binary {
            build_id: build_identity.clone(),
        },
        EvidenceQueryScope::Run { run_id, .. } => ScopeDto::Run { id: run_id.clone() },
        EvidenceQueryScope::GateAttempt {
            gate_id, revision, ..
        } => ScopeDto::GateAttempt {
            gate_id: gate_id.clone(),
            revision: *revision,
        },
    }
}

fn ordering(query: &EvidenceQuery, expected: &'static str) -> Result<&'static str, ProtocolError> {
    if query.ordering.as_str() == expected {
        Ok(expected)
    } else {
        Err(ProtocolError::ResponseOrderingMismatch {
            expected,
            observed: query.ordering.as_str().to_owned(),
        })
    }
}

fn encode_next_cursor(query: Option<&EvidenceQuery>) -> Result<Option<String>, ProtocolError> {
    query.map(encode_query_cursor).transpose()
}

fn encode_query_cursor(query: &EvidenceQuery) -> Result<String, ProtocolError> {
    let anchor = query
        .anchor
        .as_ref()
        .ok_or_else(|| ProtocolError::ResponseCursorAnchorMissing(query.collection_path.clone()))?;
    match &query.scope {
        EvidenceQueryScope::Binary { build_identity } => encode_cursor_payload(&BinaryCursorDto {
            schema_version: "lumin-binary-cursor.v2".to_owned(),
            build_id: build_identity.clone(),
            collection_path: query.collection_path.clone(),
            ordering: query.ordering.as_str().to_owned(),
            page_size: query.page_size,
            filters: query.filters.clone(),
            last_id: anchor.as_str().to_owned(),
        }),
        EvidenceQueryScope::Run {
            repository_id,
            run_id,
        } => encode_cursor_payload(&RunCursorDto {
            schema_version: "lumin-run-cursor.v2".to_owned(),
            repository_id: repository_id.clone(),
            run_id: run_id.clone(),
            finding_id: query.finding_id.clone(),
            collection_path: query.collection_path.clone(),
            ordering: query.ordering.as_str().to_owned(),
            page_size: query.page_size,
            filters: query.filters.clone(),
            last_id: anchor.as_str().to_owned(),
        }),
        EvidenceQueryScope::GateAttempt {
            repository_id,
            gate_id,
            revision,
        } => encode_cursor_payload(&GateCursorDto {
            schema_version: "lumin-gate-cursor.v2".to_owned(),
            repository_id: repository_id.clone(),
            gate_id: gate_id.clone(),
            revision: *revision,
            finding_id: query.finding_id.clone(),
            collection_path: query.collection_path.clone(),
            ordering: query.ordering.as_str().to_owned(),
            page_size: query.page_size,
            filters: query.filters.clone(),
            last_id: anchor.as_str().to_owned(),
        }),
    }
}

#[cfg(test)]
fn encode_gate_query_cursor(query: &EvidenceQuery) -> Result<String, ProtocolError> {
    encode_query_cursor(query)
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

impl From<&CapabilityRecord> for CapabilityStateDto {
    fn from(record: &CapabilityRecord) -> Self {
        Self {
            capability_id: record.capability_id.clone(),
            state: record.state,
        }
    }
}

#[cfg(test)]
mod tests {
    use lumin_evidence::{Confidence, RepoPathProjection, Severity};
    use lumin_model::{
        FindingDisposition, LogicalSourceId, RepoPath, RepositoryId, SymbolNamespace,
    };

    use super::*;

    fn test_repository_id() -> RepositoryId {
        RepositoryId::from_string("repository-test".to_owned())
    }

    #[test]
    fn cursor_codec_preserves_engine_query_contract() -> Result<(), Box<dyn std::error::Error>> {
        let query = EvidenceQuery {
            scope: EvidenceQueryScope::GateAttempt {
                repository_id: test_repository_id(),
                gate_id: GateId::from_string("gate-a".to_owned()),
                revision: 7,
            },
            finding_id: Some(FindingId::from_string("finding-a".to_owned())),
            collection_path: "gate/findings/finding-a/evidence".to_owned(),
            ordering: CollectionOrderingId::evidence(),
            page_size: 100,
            filters: BTreeMap::new(),
            anchor: Some(PageAnchor::from_string("evidence-a".to_owned())),
        };
        let encoded = encode_gate_query_cursor(&query)?;
        assert_eq!(decode_gate_query_cursor(Some(&encoded))?, Some(query));
        Ok(())
    }

    #[test]
    fn legacy_missing_nested_fields_remain_explicitly_unavailable()
    -> Result<(), Box<dyn std::error::Error>> {
        let serialized = serde_json::json!({
            "findingId": "finding-legacy",
            "ruleId": "dead-code/zero-exact-fan-in.v1",
            "ownerCapability": "dead-code.v1",
            "severity": "warning",
            "confidence": "grounded",
            "disposition": { "kind": "review-candidate" },
            "claim": "legacy finding",
            "sourceId": "source-legacy",
            "path": RepoPathProjection::from(&RepoPath::from_portable("src")?),
            "span": { "start": 0, "end": 1 },
            "exportedName": "dead",
            "namespace": "value"
        });
        let finding: FindingRecord = serde_json::from_value(serialized)?;
        assert!(!finding.nested_collections_available);
        assert!(finding.evidence.is_empty());
        assert!(finding.relations.is_empty());
        Ok(())
    }

    #[test]
    fn page_projection_encodes_engine_derived_next_query() -> Result<(), Box<dyn std::error::Error>>
    {
        let query = EvidenceQuery {
            scope: EvidenceQueryScope::GateAttempt {
                repository_id: test_repository_id(),
                gate_id: GateId::from_string("gate-a".to_owned()),
                revision: 1,
            },
            finding_id: None,
            collection_path: "gate/findings".to_owned(),
            ordering: CollectionOrderingId::findings(),
            page_size: 100,
            filters: BTreeMap::new(),
            anchor: None,
        };
        let mut next_query = query.clone();
        next_query.anchor = Some(PageAnchor::from_string("finding-a".to_owned()));
        let page = EvidencePage {
            query,
            scope_total: 101,
            total: 101,
            items: vec![finding()],
            next_query: Some(next_query.clone()),
        };
        let response = gate_findings_response(&page)?;
        let decoded = decode_gate_query_cursor(response.next_cursor.as_deref())?;
        assert_eq!(decoded, Some(next_query));
        Ok(())
    }

    #[test]
    fn query_cursor_rejects_response_without_an_anchor() {
        let query = EvidenceQuery {
            scope: EvidenceQueryScope::GateAttempt {
                repository_id: test_repository_id(),
                gate_id: GateId::from_string("gate-a".to_owned()),
                revision: 1,
            },
            finding_id: None,
            collection_path: "gate/findings".to_owned(),
            ordering: CollectionOrderingId::findings(),
            page_size: 100,
            filters: BTreeMap::new(),
            anchor: None,
        };
        let page = EvidencePage {
            query: query.clone(),
            scope_total: 2,
            total: 2,
            items: vec![finding()],
            next_query: Some(query),
        };

        assert!(matches!(
            gate_findings_response(&page),
            Err(ProtocolError::ResponseCursorAnchorMissing(collection))
                if collection == "gate/findings"
        ));
    }

    #[test]
    fn response_projection_rejects_mismatched_ordering() {
        let page = EvidencePage {
            query: EvidenceQuery {
                scope: EvidenceQueryScope::GateAttempt {
                    repository_id: test_repository_id(),
                    gate_id: GateId::from_string("gate-a".to_owned()),
                    revision: 1,
                },
                finding_id: None,
                collection_path: "gate/findings".to_owned(),
                ordering: CollectionOrderingId::capabilities(),
                page_size: 100,
                filters: BTreeMap::new(),
                anchor: None,
            },
            scope_total: 1,
            total: 1,
            items: vec![finding()],
            next_query: None,
        };

        assert!(matches!(
            gate_findings_response(&page),
            Err(ProtocolError::ResponseOrderingMismatch { expected, observed })
                if expected == FINDINGS_ORDERING_ID && observed == CAPABILITIES_ORDERING_ID
        ));
    }

    fn finding() -> FindingRecord {
        FindingRecord {
            finding_id: FindingId::from_string("finding-a".to_owned()),
            rule_id: "dead-code/zero-exact-fan-in.v1".to_owned(),
            owner_capability: "dead-code.v1".to_owned(),
            severity: Severity::Warning,
            confidence: Confidence::Grounded,
            disposition: FindingDisposition::ReviewCandidate,
            claim: "zero grounded exact fan-in".to_owned(),
            source_id: LogicalSourceId::from_string("source-a".to_owned()),
            path: RepoPathProjection {
                canonical: b"src/a.ts".to_vec(),
                components: Vec::new(),
                display: "src/a.ts".to_owned(),
            },
            span: SourceSpan { start: 0, end: 1 },
            exported_name: "dead".to_owned(),
            namespace: SymbolNamespace::Value,
            nested_collections_available: true,
            evidence: Vec::new(),
            relations: Vec::new(),
        }
    }
}

#[cfg(test)]
mod active_gate_catalog_tests {
    use super::*;

    #[test]
    fn active_gate_cursor_round_trips_and_binds_ordering() -> Result<(), Box<dyn std::error::Error>>
    {
        let repository_id = RepositoryId::from_string("repository-active".to_owned());
        let gate_id = GateId::from_string("gate-active".to_owned());
        let response = active_gates_response(
            repository_id.clone(),
            7,
            101,
            101,
            vec![ActiveGateItemDto {
                gate_id: gate_id.clone(),
                current_revision: 3,
                opening_transition_sequence: 41,
            }],
            true,
        )?;
        let decoded = decode_active_gates_cursor(
            response
                .next_cursor
                .as_deref()
                .ok_or("missing active gate cursor")?,
        )?;
        assert_eq!(
            decoded,
            DecodedActiveGatesCursor {
                repository_id,
                revision: 7,
                page_size: ACTIVE_GATES_PAGE_SIZE,
                opening_sequence: 41,
                gate_id,
            }
        );

        let wrong_ordering = encode_cursor_payload(&ActiveGatesCursorDto {
            schema_version: ACTIVE_GATES_CURSOR_SCHEMA.to_owned(),
            repository_id: RepositoryId::from_string("repository-active".to_owned()),
            revision: 7,
            ordering: "runs.v1".to_owned(),
            page_size: ACTIVE_GATES_PAGE_SIZE,
            opening_sequence: 41,
            gate_id: GateId::from_string("gate-active".to_owned()),
        })?;
        assert!(matches!(
            decode_active_gates_cursor(&wrong_ordering),
            Err(ProtocolError::CursorScopeMismatch)
        ));
        Ok(())
    }

    #[test]
    fn active_gate_response_has_the_mandatory_collection_envelope()
    -> Result<(), Box<dyn std::error::Error>> {
        let response = active_gates_response(
            RepositoryId::from_string("repository-active".to_owned()),
            2,
            1,
            1,
            vec![ActiveGateItemDto {
                gate_id: GateId::from_string("gate-active".to_owned()),
                current_revision: 0,
                opening_transition_sequence: 9,
            }],
            false,
        )?;
        let value = serde_json::to_value(response)?;
        assert_eq!(value["schemaVersion"], "lumin.active-gates.v1");
        assert_eq!(value["repositoryId"], "repository-active");
        assert_eq!(value["revision"], 2);
        assert_eq!(value["filters"], serde_json::json!({}));
        assert_eq!(value["ordering"], lumin_evidence::ACTIVE_GATES_ORDERING_ID);
        assert_eq!(value["scopeTotal"], 1);
        assert_eq!(value["total"], 1);
        assert_eq!(value["returned"], 1);
        assert_eq!(value["truncated"], false);
        assert!(value["nextCursor"].is_null());
        assert_eq!(value["items"][0]["gateId"], "gate-active");
        assert_eq!(value["items"][0]["currentRevision"], 0);
        assert_eq!(value["items"][0]["openingTransitionSequence"], 9);
        Ok(())
    }

    #[test]
    fn active_gate_response_rejects_truncation_without_an_anchor() {
        assert!(matches!(
            active_gates_response(
                RepositoryId::from_string("repository-active".to_owned()),
                2,
                1,
                1,
                Vec::new(),
                true,
            ),
            Err(ProtocolError::ResponseCursorAnchorMissing(collection))
                if collection == "active gates"
        ));
    }
}
