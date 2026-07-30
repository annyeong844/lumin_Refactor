use std::collections::BTreeMap;

use lumin_evidence::{
    CollectionOrderingId, EvidencePage, EvidenceQuery, EvidenceQueryScope, FindingExplanation,
    FindingRecord, GateRecord, PageAnchor, RunEvidence,
};
use lumin_model::FindingId;
use thiserror::Error;

use crate::EngineError;

const QUERY_PAGE_SIZE: usize = 100;
const GATE_FINDINGS_PATH: &str = "gate/findings";

#[derive(Debug, Error)]
pub enum GateEvidenceQueryError {
    #[error("cursor scope, filters, ordering, or page policy do not match this query")]
    CursorScopeMismatch,
    #[error("cursor anchor no longer exists in the immutable collection")]
    CursorAnchorMissing,
    #[error("gate revision does not exist: {0}")]
    GateRevisionMissing(u64),
    #[error("gate revision has no sealed queryable evidence: {0}")]
    GateRevisionEvidenceUnavailable(u64),
    #[error("finding does not exist in the requested immutable scope: {0}")]
    FindingNotFound(String),
    #[error("nested finding collection is unavailable in this persisted record: {0}")]
    NestedCollectionUnavailable(String),
}

pub fn query_gate_findings(
    gate: &GateRecord,
    revision: u64,
    cursor: Option<EvidenceQuery>,
) -> Result<EvidencePage<FindingRecord>, EngineError> {
    let evidence = revision_evidence(gate, revision)?;
    let expected = expected_query(
        gate,
        revision,
        None,
        GATE_FINDINGS_PATH.to_owned(),
        CollectionOrderingId::findings(),
    );
    let query = validated_query(cursor, expected)?;
    page(&evidence.findings, query, |finding| {
        finding.finding_id.as_str()
    })
    .map_err(Into::into)
}

pub fn query_gate_explain(
    gate: &GateRecord,
    revision: u64,
    finding_id: &FindingId,
    evidence_cursor: Option<EvidenceQuery>,
    relations_cursor: Option<EvidenceQuery>,
) -> Result<FindingExplanation, EngineError> {
    let run_evidence = revision_evidence(gate, revision)?;
    let finding = run_evidence
        .findings
        .iter()
        .find(|finding| &finding.finding_id == finding_id)
        .ok_or_else(|| GateEvidenceQueryError::FindingNotFound(finding_id.as_str().to_owned()))?;
    if !finding.nested_collections_available {
        return Err(GateEvidenceQueryError::NestedCollectionUnavailable(format!(
            "gate/findings/{}",
            finding.finding_id.as_str()
        ))
        .into());
    }

    let evidence_path = format!("gate/findings/{}/evidence", finding.finding_id.as_str());
    let evidence_query = validated_query(
        evidence_cursor,
        expected_query(
            gate,
            revision,
            Some(finding.finding_id.clone()),
            evidence_path,
            CollectionOrderingId::evidence(),
        ),
    )?;
    let evidence = page(&finding.evidence, evidence_query, |evidence| {
        evidence.evidence_id.as_str()
    })?;

    let relations_path = format!("gate/findings/{}/relations", finding.finding_id.as_str());
    let relations_query = validated_query(
        relations_cursor,
        expected_query(
            gate,
            revision,
            Some(finding.finding_id.clone()),
            relations_path,
            CollectionOrderingId::relations(),
        ),
    )?;
    let relations = page(&finding.relations, relations_query, |relation| {
        relation.relation_id.as_str()
    })?;

    Ok(FindingExplanation {
        finding: finding.clone(),
        evidence,
        relations,
    })
}

fn revision_evidence(
    gate: &GateRecord,
    revision: u64,
) -> Result<&RunEvidence, GateEvidenceQueryError> {
    let revision_record = gate
        .revisions
        .iter()
        .find(|candidate| candidate.revision == revision)
        .ok_or(GateEvidenceQueryError::GateRevisionMissing(revision))?;
    if revision == 0 {
        gate.baseline
            .as_ref()
            .map(|baseline| &baseline.snapshot.evidence)
            .ok_or(GateEvidenceQueryError::GateRevisionEvidenceUnavailable(
                revision,
            ))
    } else {
        revision_record
            .snapshot
            .as_ref()
            .map(|snapshot| &snapshot.evidence)
            .ok_or(GateEvidenceQueryError::GateRevisionEvidenceUnavailable(
                revision,
            ))
    }
}

fn expected_query(
    gate: &GateRecord,
    revision: u64,
    finding_id: Option<FindingId>,
    collection_path: String,
    ordering: CollectionOrderingId,
) -> EvidenceQuery {
    EvidenceQuery {
        scope: EvidenceQueryScope::GateAttempt {
            gate_id: gate.gate_id.clone(),
            revision,
        },
        finding_id,
        collection_path,
        ordering,
        page_size: QUERY_PAGE_SIZE,
        filters: BTreeMap::new(),
        anchor: None,
    }
}

fn validated_query(
    cursor: Option<EvidenceQuery>,
    expected: EvidenceQuery,
) -> Result<EvidenceQuery, GateEvidenceQueryError> {
    let Some(cursor) = cursor else {
        return Ok(expected);
    };
    if cursor.scope != expected.scope
        || cursor.finding_id != expected.finding_id
        || cursor.collection_path != expected.collection_path
        || cursor.ordering != expected.ordering
        || cursor.page_size != expected.page_size
        || cursor.filters != expected.filters
        || cursor.anchor.is_none()
    {
        return Err(GateEvidenceQueryError::CursorScopeMismatch);
    }
    Ok(cursor)
}

fn page<T: Clone>(
    items: &[T],
    query: EvidenceQuery,
    semantic_id: impl Fn(&T) -> &str,
) -> Result<EvidencePage<T>, GateEvidenceQueryError> {
    let start = match &query.anchor {
        Some(anchor) => items
            .iter()
            .position(|item| semantic_id(item) == anchor.as_str())
            .map(|index| index + 1)
            .ok_or(GateEvidenceQueryError::CursorAnchorMissing)?,
        None => 0,
    };
    let end = start.saturating_add(query.page_size).min(items.len());
    let page_items = items[start..end].to_vec();
    let next_query = if end < items.len() {
        let last = page_items
            .last()
            .ok_or(GateEvidenceQueryError::CursorAnchorMissing)?;
        let mut next = query.clone();
        next.anchor = Some(PageAnchor::from_string(semantic_id(last).to_owned()));
        Some(next)
    } else {
        None
    };
    Ok(EvidencePage {
        query,
        scope_total: items.len(),
        total: items.len(),
        items: page_items,
        next_query,
    })
}

#[cfg(test)]
mod tests {
    use lumin_evidence::{
        AnalysisSnapshot, Confidence, EvidenceRecord, FindingRelationRecord, GateAnalysisOptions,
        GateBaseline, GateDecision, GateLifecycle, GateRevision, RepoPathProjection, Severity,
        finding_relation_id, sort_findings,
    };
    use lumin_model::{
        AnalysisInputId, EvidenceId, FindingDisposition, GateId, LogicalSourceId, OperationId,
        RepoPath, SourceSpan, SymbolNamespace,
    };

    use super::*;

    #[test]
    fn findings_cursor_is_bound_to_exact_gate_revision() -> Result<(), Box<dyn std::error::Error>> {
        let gate = gate_with_nested_finding()?;
        let first = query_gate_findings(&gate, 1, None)?;
        assert_eq!(first.items.len(), QUERY_PAGE_SIZE);
        let cursor = first
            .next_query
            .ok_or_else(|| std::io::Error::other("missing gate findings cursor"))?;
        let second = query_gate_findings(&gate, 1, Some(cursor.clone()))?;
        assert_eq!(second.items.len(), 1);

        let result = query_gate_findings(&gate, 0, Some(cursor));
        assert!(matches!(
            result,
            Err(EngineError::EvidenceQuery(
                GateEvidenceQueryError::CursorScopeMismatch
            ))
        ));
        Ok(())
    }

    #[test]
    fn explain_pages_nested_collections_and_rejects_legacy_absence()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut gate = gate_with_nested_finding()?;
        let finding_id = gate.revisions[1]
            .snapshot
            .as_ref()
            .ok_or_else(|| std::io::Error::other("missing snapshot"))?
            .evidence
            .findings[0]
            .finding_id
            .clone();
        let first = query_gate_explain(&gate, 1, &finding_id, None, None)?;
        assert_eq!(first.evidence.items.len(), QUERY_PAGE_SIZE);
        assert_eq!(first.relations.items.len(), QUERY_PAGE_SIZE);
        let second = query_gate_explain(
            &gate,
            1,
            &finding_id,
            first.evidence.next_query,
            first.relations.next_query,
        )?;
        assert_eq!(second.evidence.items.len(), 1);
        assert_eq!(second.relations.items.len(), 1);

        gate.revisions[1]
            .snapshot
            .as_mut()
            .ok_or_else(|| std::io::Error::other("missing snapshot"))?
            .evidence
            .findings[0]
            .nested_collections_available = false;
        let legacy = query_gate_explain(&gate, 1, &finding_id, None, None);
        assert!(matches!(
            legacy,
            Err(EngineError::EvidenceQuery(
                GateEvidenceQueryError::NestedCollectionUnavailable(_)
            ))
        ));
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
                relation_id: finding_relation_id(&finding_id, "related", &target, &evidence_id),
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
            nested_collections_available: true,
            evidence,
            relations,
        })
    }
}
