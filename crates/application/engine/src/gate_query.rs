use std::collections::{BTreeMap, HashSet};

use lumin_evidence::{
    CollectionOrderingId, EvidencePage, EvidenceQuery, EvidenceQueryScope, FindingExplanation,
    FindingRecord, GateRecord, PageAnchor, RunEvidence, SourceClassificationRecord,
};
use lumin_model::{FindingId, RepoPath, RepositoryId, RunId};
use thiserror::Error;

use crate::EngineError;

const QUERY_PAGE_SIZE: usize = 100;
const GATE_FINDINGS_PATH: &str = "gate/findings";
const RUN_FINDINGS_PATH: &str = "run/findings";

#[derive(Debug, Error)]
pub enum EvidenceQueryError {
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
    #[error("duplicate capability_id in capability collection: {0}")]
    DuplicateCapabilityId(String),
    #[error("duplicate semantic anchor in collection: {0}")]
    DuplicateCollectionId(String),
    #[error("duplicate source classification in persisted run evidence: {0}")]
    DuplicateSourceClassification(String),
    #[error("source classification identity does not match its persisted path: {0}")]
    SourceClassificationIdentityMismatch(String),
}

pub fn query_run_findings(
    repository_id: &RepositoryId,
    run_id: &RunId,
    evidence: &RunEvidence,
    cursor: Option<EvidenceQuery>,
) -> Result<EvidencePage<FindingRecord>, EngineError> {
    let expected = EvidenceQuery {
        scope: EvidenceQueryScope::Run {
            repository_id: repository_id.clone(),
            run_id: run_id.clone(),
        },
        finding_id: None,
        collection_path: RUN_FINDINGS_PATH.to_owned(),
        ordering: CollectionOrderingId::findings(),
        page_size: QUERY_PAGE_SIZE,
        filters: BTreeMap::new(),
        anchor: None,
    };
    let query = validated_query(cursor, expected)?;
    page(&evidence.findings, query, |finding| {
        finding.finding_id.as_str()
    })
    .map_err(Into::into)
}

pub fn query_run_explain(
    repository_id: &RepositoryId,
    run_id: &RunId,
    evidence: &RunEvidence,
    finding_id: &FindingId,
    evidence_cursor: Option<EvidenceQuery>,
    relations_cursor: Option<EvidenceQuery>,
) -> Result<FindingExplanation, EngineError> {
    let finding = evidence
        .findings
        .iter()
        .find(|finding| &finding.finding_id == finding_id)
        .ok_or_else(|| EvidenceQueryError::FindingNotFound(finding_id.as_str().to_owned()))?;
    if !finding.nested_collections_available {
        return Err(EvidenceQueryError::NestedCollectionUnavailable(format!(
            "run/findings/{}",
            finding.finding_id.as_str()
        ))
        .into());
    }

    let evidence_path = format!("run/findings/{}/evidence", finding.finding_id.as_str());
    let evidence_query = validated_query(
        evidence_cursor,
        EvidenceQuery {
            scope: EvidenceQueryScope::Run {
                repository_id: repository_id.clone(),
                run_id: run_id.clone(),
            },
            finding_id: Some(finding.finding_id.clone()),
            collection_path: evidence_path,
            ordering: CollectionOrderingId::evidence(),
            page_size: QUERY_PAGE_SIZE,
            filters: BTreeMap::new(),
            anchor: None,
        },
    )?;
    let evidence_page = page(&finding.evidence, evidence_query, |ev| {
        ev.evidence_id.as_str()
    })?;

    let relations_path = format!("run/findings/{}/relations", finding.finding_id.as_str());
    let relations_query = validated_query(
        relations_cursor,
        EvidenceQuery {
            scope: EvidenceQueryScope::Run {
                repository_id: repository_id.clone(),
                run_id: run_id.clone(),
            },
            finding_id: Some(finding.finding_id.clone()),
            collection_path: relations_path,
            ordering: CollectionOrderingId::relations(),
            page_size: QUERY_PAGE_SIZE,
            filters: BTreeMap::new(),
            anchor: None,
        },
    )?;
    let relations_page = page(&finding.relations, relations_query, |rel| {
        rel.relation_id.as_str()
    })?;

    Ok(FindingExplanation {
        finding: finding.clone(),
        evidence: evidence_page,
        relations: relations_page,
    })
}

pub fn query_run_file_findings(
    repository_id: &RepositoryId,
    run_id: &RunId,
    evidence: &RunEvidence,
    repo_path: &lumin_model::RepoPath,
    cursor: Option<EvidenceQuery>,
) -> Result<EvidencePage<FindingRecord>, EngineError> {
    let source_id = lumin_model::LogicalSourceId::from_path(repo_path);
    let canonical_bytes = repo_path.canonical_bytes();
    let filtered: Vec<FindingRecord> = evidence
        .findings
        .iter()
        .filter(|finding| finding.path_identity() == canonical_bytes)
        .cloned()
        .collect();
    let collection_path = format!("run/files/{}", source_id.as_str());
    let mut filters = BTreeMap::new();
    filters.insert("path".to_owned(), vec![source_id.as_str().to_owned()]);
    let expected = EvidenceQuery {
        scope: EvidenceQueryScope::Run {
            repository_id: repository_id.clone(),
            run_id: run_id.clone(),
        },
        finding_id: None,
        collection_path,
        ordering: CollectionOrderingId::file_findings(),
        page_size: QUERY_PAGE_SIZE,
        filters: filters.clone(),
        anchor: None,
    };
    let query = validated_query(cursor, expected)?;
    let result = page_with_scope_total(&filtered, evidence.findings.len(), query, |finding| {
        finding.finding_id.as_str()
    })?;
    Ok(result)
}

pub fn query_run_source_classification<'a>(
    evidence: &'a RunEvidence,
    repo_path: &RepoPath,
) -> Result<Option<&'a SourceClassificationRecord>, EngineError> {
    let mut matches = evidence
        .source_classifications
        .iter()
        .filter(|record| record.path.canonical == repo_path.canonical_bytes());
    let Some(record) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err(
            EvidenceQueryError::DuplicateSourceClassification(repo_path.display_escaped()).into(),
        );
    }
    let expected_source_id = lumin_model::LogicalSourceId::from_path(repo_path);
    if record.source_id != expected_source_id {
        return Err(EvidenceQueryError::SourceClassificationIdentityMismatch(
            repo_path.display_escaped(),
        )
        .into());
    }
    Ok(Some(record))
}

pub fn query_run_relations(
    repository_id: &RepositoryId,
    run_id: &RunId,
    evidence: &RunEvidence,
    finding_id: &FindingId,
    cursor: Option<EvidenceQuery>,
) -> Result<EvidencePage<lumin_evidence::FindingRelationRecord>, EngineError> {
    let finding = evidence
        .findings
        .iter()
        .find(|finding| &finding.finding_id == finding_id)
        .ok_or_else(|| EvidenceQueryError::FindingNotFound(finding_id.as_str().to_owned()))?;
    if !finding.nested_collections_available {
        return Err(EvidenceQueryError::NestedCollectionUnavailable(format!(
            "run/findings/{}",
            finding.finding_id.as_str()
        ))
        .into());
    }
    let relations_path = format!("run/findings/{}/relations", finding.finding_id.as_str());
    let expected = EvidenceQuery {
        scope: EvidenceQueryScope::Run {
            repository_id: repository_id.clone(),
            run_id: run_id.clone(),
        },
        finding_id: Some(finding.finding_id.clone()),
        collection_path: relations_path,
        ordering: CollectionOrderingId::relations(),
        page_size: QUERY_PAGE_SIZE,
        filters: BTreeMap::new(),
        anchor: None,
    };
    let query = validated_query(cursor, expected)?;
    page(&finding.relations, query, |rel| rel.relation_id.as_str()).map_err(Into::into)
}

pub fn query_gate_findings(
    repository_id: &RepositoryId,
    gate: &GateRecord,
    revision: u64,
    cursor: Option<EvidenceQuery>,
) -> Result<EvidencePage<FindingRecord>, EngineError> {
    let evidence = revision_evidence(gate, revision)?;
    let expected = expected_query(
        repository_id,
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
    repository_id: &RepositoryId,
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
        .ok_or_else(|| EvidenceQueryError::FindingNotFound(finding_id.as_str().to_owned()))?;
    if !finding.nested_collections_available {
        return Err(EvidenceQueryError::NestedCollectionUnavailable(format!(
            "gate/findings/{}",
            finding.finding_id.as_str()
        ))
        .into());
    }

    let evidence_path = format!("gate/findings/{}/evidence", finding.finding_id.as_str());
    let evidence_query = validated_query(
        evidence_cursor,
        expected_query(
            repository_id,
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
            repository_id,
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

fn revision_evidence(gate: &GateRecord, revision: u64) -> Result<&RunEvidence, EvidenceQueryError> {
    let revision_record = gate
        .revisions
        .iter()
        .find(|candidate| candidate.revision == revision)
        .ok_or(EvidenceQueryError::GateRevisionMissing(revision))?;
    if revision == 0 {
        gate.baseline
            .as_ref()
            .map(|baseline| &baseline.snapshot.evidence)
            .ok_or(EvidenceQueryError::GateRevisionEvidenceUnavailable(
                revision,
            ))
    } else {
        revision_record
            .snapshot
            .as_ref()
            .map(|snapshot| &snapshot.evidence)
            .ok_or(EvidenceQueryError::GateRevisionEvidenceUnavailable(
                revision,
            ))
    }
}

fn expected_query(
    repository_id: &RepositoryId,
    gate: &GateRecord,
    revision: u64,
    finding_id: Option<FindingId>,
    collection_path: String,
    ordering: CollectionOrderingId,
) -> EvidenceQuery {
    EvidenceQuery {
        scope: EvidenceQueryScope::GateAttempt {
            repository_id: repository_id.clone(),
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

pub(crate) fn validated_query(
    cursor: Option<EvidenceQuery>,
    expected: EvidenceQuery,
) -> Result<EvidenceQuery, EvidenceQueryError> {
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
        return Err(EvidenceQueryError::CursorScopeMismatch);
    }
    Ok(cursor)
}

pub(crate) fn page<T: Clone>(
    items: &[T],
    query: EvidenceQuery,
    semantic_id: impl Fn(&T) -> &str,
) -> Result<EvidencePage<T>, EvidenceQueryError> {
    detect_duplicate_anchors(items, &semantic_id)?;
    let start = match &query.anchor {
        Some(anchor) => {
            let resume_offset = items
                .iter()
                .position(|item| semantic_id(item) == anchor.as_str())
                .map(|index| index + 1)
                .ok_or(EvidenceQueryError::CursorAnchorMissing)?;
            if query.page_size == 0
                || !resume_offset.is_multiple_of(query.page_size)
                || resume_offset >= items.len()
            {
                return Err(EvidenceQueryError::CursorScopeMismatch);
            }
            resume_offset
        }
        None => 0,
    };
    let end = start.saturating_add(query.page_size).min(items.len());
    let page_items = items[start..end].to_vec();
    let next_query = if end < items.len() {
        let last = page_items
            .last()
            .ok_or(EvidenceQueryError::CursorAnchorMissing)?;
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

fn page_with_scope_total<T: Clone>(
    items: &[T],
    scope_total: usize,
    query: EvidenceQuery,
    semantic_id: impl Fn(&T) -> &str,
) -> Result<EvidencePage<T>, EvidenceQueryError> {
    detect_duplicate_anchors(items, &semantic_id)?;
    let start = match &query.anchor {
        Some(anchor) => {
            let resume_offset = items
                .iter()
                .position(|item| semantic_id(item) == anchor.as_str())
                .map(|index| index + 1)
                .ok_or(EvidenceQueryError::CursorAnchorMissing)?;
            if query.page_size == 0
                || !resume_offset.is_multiple_of(query.page_size)
                || resume_offset >= items.len()
            {
                return Err(EvidenceQueryError::CursorScopeMismatch);
            }
            resume_offset
        }
        None => 0,
    };
    let end = start.saturating_add(query.page_size).min(items.len());
    let page_items = items[start..end].to_vec();
    let next_query = if end < items.len() {
        let last = page_items
            .last()
            .ok_or(EvidenceQueryError::CursorAnchorMissing)?;
        let mut next = query.clone();
        next.anchor = Some(PageAnchor::from_string(semantic_id(last).to_owned()));
        Some(next)
    } else {
        None
    };
    Ok(EvidencePage {
        query,
        scope_total,
        total: items.len(),
        items: page_items,
        next_query,
    })
}

fn detect_duplicate_anchors<T>(
    items: &[T],
    semantic_id: &impl Fn(&T) -> &str,
) -> Result<(), EvidenceQueryError> {
    let mut seen = HashSet::new();
    for item in items {
        let id = semantic_id(item);
        if !seen.insert(id) {
            return Err(EvidenceQueryError::DuplicateCollectionId(id.to_owned()));
        }
    }
    Ok(())
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
        RepoPath, RepositoryId, RunId, SourceSpan, SymbolNamespace,
    };

    use super::*;

    fn test_repository_id() -> RepositoryId {
        RepositoryId::from_string("repository-test".to_owned())
    }

    #[test]
    fn existing_nonboundary_anchor_is_rejected() {
        let items = vec!["finding-a", "finding-b", "finding-c"];
        let query = EvidenceQuery {
            scope: EvidenceQueryScope::Run {
                repository_id: test_repository_id(),
                run_id: RunId::from_string("run-test".to_owned()),
            },
            finding_id: None,
            collection_path: RUN_FINDINGS_PATH.to_owned(),
            ordering: CollectionOrderingId::findings(),
            page_size: 2,
            filters: BTreeMap::new(),
            anchor: Some(PageAnchor::from_string("finding-a".to_owned())),
        };

        assert!(matches!(
            page(&items, query, |item| item),
            Err(EvidenceQueryError::CursorScopeMismatch)
        ));
    }

    #[test]
    fn findings_cursor_is_bound_to_exact_gate_revision() -> Result<(), Box<dyn std::error::Error>> {
        let repo_id = test_repository_id();
        let gate = gate_with_nested_finding()?;
        let first = query_gate_findings(&repo_id, &gate, 1, None)?;
        assert_eq!(first.items.len(), QUERY_PAGE_SIZE);
        let cursor = first
            .next_query
            .ok_or_else(|| std::io::Error::other("missing gate findings cursor"))?;
        let second = query_gate_findings(&repo_id, &gate, 1, Some(cursor.clone()))?;
        assert_eq!(second.items.len(), 1);

        let result = query_gate_findings(&repo_id, &gate, 0, Some(cursor));
        assert!(matches!(
            result,
            Err(EngineError::EvidenceQuery(
                EvidenceQueryError::CursorScopeMismatch
            ))
        ));
        Ok(())
    }

    #[test]
    fn explain_pages_nested_collections_and_rejects_legacy_absence()
    -> Result<(), Box<dyn std::error::Error>> {
        let repo_id = test_repository_id();
        let mut gate = gate_with_nested_finding()?;
        let finding_id = gate.revisions[1]
            .snapshot
            .as_ref()
            .ok_or_else(|| std::io::Error::other("missing snapshot"))?
            .evidence
            .findings[0]
            .finding_id
            .clone();
        let first = query_gate_explain(&repo_id, &gate, 1, &finding_id, None, None)?;
        assert_eq!(first.evidence.items.len(), QUERY_PAGE_SIZE);
        assert_eq!(first.relations.items.len(), QUERY_PAGE_SIZE);
        let second = query_gate_explain(
            &repo_id,
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
        let legacy = query_gate_explain(&repo_id, &gate, 1, &finding_id, None, None);
        assert!(matches!(
            legacy,
            Err(EngineError::EvidenceQuery(
                EvidenceQueryError::NestedCollectionUnavailable(_)
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
            source_classifications: Vec::new(),
            findings,
            limitations: Vec::new(),
        };
        let snapshot = AnalysisSnapshot {
            analysis_input_id: AnalysisInputId::from_string("analysis-input-1".to_owned()),
            inputs: Vec::new(),
            scan_invocation: Default::default(),
            entry_selections: Vec::new(),
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
                scan_invocation: Default::default(),
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

    #[test]
    fn run_findings_pages_immutable_evidence() -> Result<(), Box<dyn std::error::Error>> {
        let repo_id = test_repository_id();
        let run_id = RunId::from_string("run-a".to_owned());
        let evidence = run_evidence_with_nested()?;
        let first = query_run_findings(&repo_id, &run_id, &evidence, None)?;
        assert_eq!(first.items.len(), QUERY_PAGE_SIZE);
        assert_eq!(first.scope_total, 101);
        let cursor = first
            .next_query
            .ok_or_else(|| std::io::Error::other("missing run findings cursor"))?;
        let second = query_run_findings(&repo_id, &run_id, &evidence, Some(cursor.clone()))?;
        assert_eq!(second.items.len(), 1);
        assert!(second.next_query.is_none());

        // Cross-run cursor rejected
        let other_run = RunId::from_string("run-b".to_owned());
        let result = query_run_findings(&repo_id, &other_run, &evidence, Some(cursor.clone()));
        assert!(matches!(
            result,
            Err(EngineError::EvidenceQuery(
                EvidenceQueryError::CursorScopeMismatch
            ))
        ));

        // Cross-repository cursor rejected (same run ID, different repository)
        let other_repo = RepositoryId::from_string("repository-other".to_owned());
        let result = query_run_findings(&other_repo, &run_id, &evidence, Some(cursor));
        assert!(matches!(
            result,
            Err(EngineError::EvidenceQuery(
                EvidenceQueryError::CursorScopeMismatch
            ))
        ));
        Ok(())
    }

    #[test]
    fn run_explain_pages_nested_collections() -> Result<(), Box<dyn std::error::Error>> {
        let repo_id = test_repository_id();
        let run_id = RunId::from_string("run-a".to_owned());
        let evidence = run_evidence_with_nested()?;
        let finding_id = evidence.findings[0].finding_id.clone();
        let first = query_run_explain(&repo_id, &run_id, &evidence, &finding_id, None, None)?;
        assert_eq!(first.evidence.items.len(), QUERY_PAGE_SIZE);
        assert_eq!(first.relations.items.len(), QUERY_PAGE_SIZE);
        let second = query_run_explain(
            &repo_id,
            &run_id,
            &evidence,
            &finding_id,
            first.evidence.next_query,
            first.relations.next_query,
        )?;
        assert_eq!(second.evidence.items.len(), 1);
        assert_eq!(second.relations.items.len(), 1);
        Ok(())
    }

    #[test]
    fn run_explain_rejects_unavailable_nested() -> Result<(), Box<dyn std::error::Error>> {
        let repo_id = test_repository_id();
        let run_id = RunId::from_string("run-a".to_owned());
        let mut evidence = run_evidence_with_nested()?;
        evidence.findings[0].nested_collections_available = false;
        let finding_id = evidence.findings[0].finding_id.clone();
        let result = query_run_explain(&repo_id, &run_id, &evidence, &finding_id, None, None);
        assert!(matches!(
            result,
            Err(EngineError::EvidenceQuery(
                EvidenceQueryError::NestedCollectionUnavailable(_)
            ))
        ));
        Ok(())
    }

    fn run_evidence_with_nested() -> Result<RunEvidence, Box<dyn std::error::Error>> {
        let mut findings = (0..101)
            .map(|index| finding(index, if index == 0 { 101 } else { 1 }))
            .collect::<Result<Vec<_>, _>>()?;
        sort_findings(&mut findings);
        Ok(RunEvidence {
            schema_version: "lumin-evidence.v1".to_owned(),
            capabilities: Vec::new(),
            resolution_profiles: Vec::new(),
            source_classifications: Vec::new(),
            findings,
            limitations: Vec::new(),
        })
    }

    #[test]
    fn duplicate_semantic_ids_fail_closed_before_paging() -> Result<(), Box<dyn std::error::Error>>
    {
        let repo_id = test_repository_id();
        let run_id = RunId::from_string("run-duplicate".to_owned());
        let mut evidence = run_evidence_with_nested()?;
        evidence.findings.push(evidence.findings[0].clone());
        let result = query_run_findings(&repo_id, &run_id, &evidence, None);
        let error = match result {
            Err(error) => error,
            Ok(_) => return Err("duplicate finding ID was accepted".into()),
        };
        assert!(matches!(
            &error,
            EngineError::EvidenceQuery(EvidenceQueryError::DuplicateCollectionId(_))
        ));
        assert_eq!(error.lifecycle_exit_code(), 1);

        let mut evidence = run_evidence_with_nested()?;
        let duplicate = evidence.findings[0].relations[0].clone();
        evidence.findings[0].relations.push(duplicate);
        assert!(matches!(
            query_run_relations(
                &repo_id,
                &run_id,
                &evidence,
                &evidence.findings[0].finding_id,
                None,
            ),
            Err(EngineError::EvidenceQuery(
                EvidenceQueryError::DuplicateCollectionId(_)
            ))
        ));
        Ok(())
    }

    #[test]
    fn file_findings_bind_canonical_path_scope_and_total() -> Result<(), Box<dyn std::error::Error>>
    {
        let repo_id = test_repository_id();
        let run_id = RunId::from_string("run-files".to_owned());
        let mut evidence = run_evidence_with_nested()?;
        let path = RepoPath::from_portable("src/shared.ts")?;
        for finding in &mut evidence.findings {
            finding.path = RepoPathProjection::from(&path);
        }
        sort_findings(&mut evidence.findings);

        let first = query_run_file_findings(&repo_id, &run_id, &evidence, &path, None)?;
        assert_eq!(first.scope_total, 101);
        assert_eq!(first.total, 101);
        assert_eq!(first.items.len(), QUERY_PAGE_SIZE);
        assert_eq!(first.query.ordering, CollectionOrderingId::file_findings());
        let source_id = LogicalSourceId::from_path(&path);
        assert_eq!(
            first.query.collection_path,
            format!("run/files/{}", source_id.as_str())
        );
        assert_eq!(
            first.query.filters.get("path"),
            Some(&vec![source_id.as_str().to_owned()])
        );
        assert!(
            first
                .items
                .iter()
                .all(|finding| finding.path_identity() == path.canonical_bytes())
        );

        let second = query_run_file_findings(
            &repo_id,
            &run_id,
            &evidence,
            &path,
            first.next_query.clone(),
        )?;
        assert_eq!(second.items.len(), 1);
        assert!(second.next_query.is_none());
        let other_path = RepoPath::from_portable("src/other.ts")?;
        assert!(matches!(
            query_run_file_findings(&repo_id, &run_id, &evidence, &other_path, first.next_query,),
            Err(EngineError::EvidenceQuery(
                EvidenceQueryError::CursorScopeMismatch
            ))
        ));
        Ok(())
    }

    #[test]
    fn related_reuses_the_finding_relation_scope_and_canonical_order()
    -> Result<(), Box<dyn std::error::Error>> {
        let repo_id = test_repository_id();
        let run_id = RunId::from_string("run-related".to_owned());
        let evidence = run_evidence_with_nested()?;
        let finding = &evidence.findings[0];
        let first = query_run_relations(&repo_id, &run_id, &evidence, &finding.finding_id, None)?;
        assert_eq!(first.scope_total, 101);
        assert_eq!(first.total, 101);
        assert_eq!(first.items.len(), QUERY_PAGE_SIZE);
        assert_eq!(first.query.finding_id.as_ref(), Some(&finding.finding_id));
        assert_eq!(
            first.query.collection_path,
            format!("run/findings/{}/relations", finding.finding_id.as_str())
        );
        assert_eq!(first.query.ordering, CollectionOrderingId::relations());
        assert_eq!(
            first
                .items
                .iter()
                .map(|relation| relation.target_finding_id.as_str())
                .collect::<Vec<_>>(),
            (0..100)
                .map(|index| format!("target-{index:03}"))
                .collect::<Vec<_>>()
        );
        let second = query_run_relations(
            &repo_id,
            &run_id,
            &evidence,
            &finding.finding_id,
            first.next_query,
        )?;
        assert_eq!(second.items.len(), 1);
        assert_eq!(second.items[0].target_finding_id.as_str(), "target-100");
        Ok(())
    }
}
