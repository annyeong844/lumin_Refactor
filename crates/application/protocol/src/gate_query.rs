use std::collections::BTreeMap;

use lumin_evidence::{
    CAPABILITIES_ORDERING_ID, CapabilityRecord, CollectionOrderingId, EVIDENCE_ORDERING_ID,
    EvidencePage, EvidenceQuery, EvidenceQueryScope, EvidenceRecord, FINDINGS_ORDERING_ID,
    FindingExplanation, FindingRecord, FindingRelationRecord, PageAnchor, RELATIONS_ORDERING_ID,
};
use lumin_model::{
    BuildIdentity, EvidenceId, FindingId, FindingRelationId, GateId, RepositoryId, RunId,
    SourceSpan,
};
use serde::{Deserialize, Serialize};

use crate::cursor::{decode_cursor_payload, encode_cursor_payload};
use crate::{
    CapabilityStateDto, FindingCollectionDto, FindingDto, ProtocolError, RepoPathDto, ScopeDto,
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
            if cursor.schema_version != "lumin-run-cursor.v1" {
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
            if cursor.schema_version != "lumin-binary-cursor.v1" {
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
            if cursor.schema_version != "lumin-gate-cursor.v1" {
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
        Err(ProtocolError::CursorScopeMismatch)
    }
}

fn encode_next_cursor(query: Option<&EvidenceQuery>) -> Result<Option<String>, ProtocolError> {
    query.map(encode_query_cursor).transpose()
}

fn encode_query_cursor(query: &EvidenceQuery) -> Result<String, ProtocolError> {
    let anchor = query
        .anchor
        .as_ref()
        .ok_or(ProtocolError::CursorAnchorMissing)?;
    match &query.scope {
        EvidenceQueryScope::Binary { build_identity } => encode_cursor_payload(&BinaryCursorDto {
            schema_version: "lumin-binary-cursor.v1".to_owned(),
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
            schema_version: "lumin-run-cursor.v1".to_owned(),
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
            schema_version: "lumin-gate-cursor.v1".to_owned(),
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
    use lumin_model::{FindingDisposition, LogicalSourceId, RepositoryId, SymbolNamespace};

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
        let serialized = r#"{
            "findingId":"finding-legacy",
            "ruleId":"dead-code/zero-exact-fan-in.v1",
            "ownerCapability":"dead-code.v1",
            "severity":"warning",
            "confidence":"grounded",
            "disposition":{"kind":"review-candidate"},
            "claim":"legacy finding",
            "sourceId":"source-legacy",
            "path":{"canonical":[115,114,99],"components":[],"display":"src"},
            "span":{"start":0,"end":1},
            "exportedName":"dead",
            "namespace":"value"
        }"#;
        let finding: FindingRecord = serde_json::from_str(serialized)?;
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
