use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use lumin_evidence::{
    DeclaredPathUnsupportedReason, FindingRecord, GateDecision, GateLifecycle, GateOperationKind,
    GateOperationResult, GateOperationStatus, GateRecord, GateSignal, OperationRecord,
    RepoPathProjection, RunEvidence, WriteLease, WriteLeaseKind,
};
use lumin_model::{
    AnalysisInputId, AttemptId, CapabilityState, FindingDisposition, FindingId, GateDeltaRecord,
    GateId, Limitation, OperationId, RunId, SourceSpan, SymbolNamespace,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const FINDINGS_ORDERING: &str = "findings.v1";
pub const FINDINGS_PAGE_SIZE: usize = 100;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditResponseDto {
    pub schema_version: &'static str,
    pub attempt_id: AttemptId,
    pub run_id: RunId,
    pub sequence: u64,
    pub status: CapabilityState,
    pub finding_count: usize,
    pub limitation_count: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverviewResponseDto {
    pub schema_version: &'static str,
    pub scope: ScopeDto,
    pub attempt_id: AttemptId,
    pub sequence: u64,
    pub capability_states: Vec<CapabilityStateDto>,
    pub resolution_profiles: Vec<lumin_model::SelectedResolutionProfile>,
    pub finding_count: usize,
    pub limitation_count: usize,
    pub limitations: Vec<Limitation>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityStateDto {
    pub capability_id: String,
    pub state: CapabilityState,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingCollectionDto {
    pub schema_version: &'static str,
    pub scope: ScopeDto,
    pub filters: BTreeMap<String, Vec<String>>,
    pub ordering: &'static str,
    pub scope_total: usize,
    pub total: usize,
    pub returned: usize,
    pub truncated: bool,
    pub next_cursor: Option<String>,
    pub items: Vec<FindingDto>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ScopeDto {
    Run { id: RunId },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingDto {
    pub finding_id: FindingId,
    pub rule_id: String,
    pub owner_capability: String,
    pub severity: String,
    pub confidence: String,
    pub disposition: FindingDisposition,
    pub claim: String,
    pub source_id: String,
    pub path: RepoPathDto,
    pub span: SourceSpan,
    pub exported_name: String,
    pub namespace: SymbolNamespace,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoPathDto {
    pub schema_version: &'static str,
    pub canonical_base64: String,
    pub display: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GateMutationResponseDto {
    pub schema_version: &'static str,
    pub operation_id: OperationId,
    pub request_digest: String,
    pub gate_id: GateId,
    pub revision: u64,
    pub lifecycle: GateLifecycle,
    pub decision: GateDecision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub signals: Vec<GateSignalDto>,
    pub leased_write_set: Vec<WriteLeaseDto>,
    pub deltas: Vec<GateDeltaRecord>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteLeaseDto {
    pub path: RepoPathDto,
    pub kind: WriteLeaseKind,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GateShowResponseDto {
    pub schema_version: &'static str,
    pub gate_id: GateId,
    pub lifecycle: GateLifecycle,
    pub current_revision: u64,
    pub declared_write_set: Vec<RepoPathDto>,
    pub leased_write_set: Vec<WriteLeaseDto>,
    pub transition_refs: Vec<u64>,
    pub protected_semantic_input_count: usize,
    pub baseline: Option<GateBaselineSummaryDto>,
    pub revisions: Vec<GateRevisionSummaryDto>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GateBaselineSummaryDto {
    pub analysis_contract: String,
    pub analysis_input_id: AnalysisInputId,
    pub semantic_input_count: usize,
    pub finding_count: usize,
    pub limitation_count: usize,
    pub protected_semantic_input_count: usize,
    pub transition_sequence: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GateRevisionSummaryDto {
    pub revision: u64,
    pub operation_id: OperationId,
    pub decision: GateDecision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub signals: Vec<GateSignalDto>,
    pub changed_paths: Vec<RepoPathDto>,
    pub analysis_input_id: Option<AnalysisInputId>,
    pub protected_semantic_input_count: usize,
    pub alias_group_count: usize,
    pub reconciled_transition_sequences: Vec<u64>,
    pub deltas: Vec<GateDeltaRecord>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationShowResponseDto {
    pub schema_version: &'static str,
    pub operation_id: OperationId,
    pub kind: GateOperationKind,
    pub request_digest: String,
    pub status: GateOperationStatus,
    pub gate_id: GateId,
    pub target_revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub transition_sequence: u64,
    pub interruption_count: u64,
    pub declared_write_set: Vec<RepoPathDto>,
    pub leased_write_set: Vec<WriteLeaseDto>,
    pub semantic_read_reservations: Vec<RepoPathDto>,
    pub result: Option<GateMutationResponseDto>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GateSignalDto {
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<DeclaredPathUnsupportedReason>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<RepoPathDto>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gate_ids: Vec<GateId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CursorDto {
    schema_version: String,
    run_id: RunId,
    ordering: String,
    filters: BTreeMap<String, Vec<String>>,
    last_finding_id: FindingId,
}

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("cursor is not valid Base64")]
    CursorEncoding,
    #[error("cursor payload is malformed: {0}")]
    CursorPayload(String),
    #[error("cursor scope, filters, or ordering do not match this query")]
    CursorScopeMismatch,
    #[error("cursor anchor no longer exists in the immutable run")]
    CursorAnchorMissing,
    #[error("machine response serialization failed: {0}")]
    Serialization(String),
}

pub fn audit_response(
    attempt_id: AttemptId,
    run_id: RunId,
    sequence: u64,
    evidence: &RunEvidence,
) -> AuditResponseDto {
    AuditResponseDto {
        schema_version: "lumin.audit.v1",
        attempt_id,
        run_id,
        sequence,
        status: evidence.dead_code_state(),
        finding_count: evidence.findings.len(),
        limitation_count: evidence.limitations.len(),
    }
}

pub fn overview_response(
    attempt_id: AttemptId,
    run_id: RunId,
    sequence: u64,
    evidence: &RunEvidence,
) -> OverviewResponseDto {
    OverviewResponseDto {
        schema_version: "lumin.overview.v1",
        scope: ScopeDto::Run { id: run_id },
        attempt_id,
        sequence,
        capability_states: evidence
            .capabilities
            .iter()
            .map(|capability| CapabilityStateDto {
                capability_id: capability.capability_id.clone(),
                state: capability.state,
            })
            .collect(),
        resolution_profiles: evidence.resolution_profiles.clone(),
        finding_count: evidence.findings.len(),
        limitation_count: evidence.limitations.len(),
        limitations: evidence.limitations.clone(),
    }
}

pub fn findings_response(
    run_id: RunId,
    evidence: &RunEvidence,
    cursor: Option<&str>,
) -> Result<FindingCollectionDto, ProtocolError> {
    let filters = BTreeMap::new();
    let start = match cursor {
        Some(cursor) => {
            let cursor = decode_cursor(cursor)?;
            if cursor.run_id != run_id
                || cursor.ordering != FINDINGS_ORDERING
                || cursor.filters != filters
            {
                return Err(ProtocolError::CursorScopeMismatch);
            }
            evidence
                .findings
                .iter()
                .position(|finding| finding.finding_id == cursor.last_finding_id)
                .map(|index| index + 1)
                .ok_or(ProtocolError::CursorAnchorMissing)?
        }
        None => 0,
    };
    let end = (start + FINDINGS_PAGE_SIZE).min(evidence.findings.len());
    let items = evidence.findings[start..end]
        .iter()
        .map(FindingDto::from)
        .collect::<Vec<_>>();
    let truncated = end < evidence.findings.len();
    let next_cursor = if truncated {
        items
            .last()
            .map(|finding| encode_cursor(&run_id, &filters, &finding.finding_id))
            .transpose()?
    } else {
        None
    };
    Ok(FindingCollectionDto {
        schema_version: "lumin.collection.v1",
        scope: ScopeDto::Run { id: run_id },
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

pub fn gate_mutation_response(result: &GateOperationResult) -> GateMutationResponseDto {
    GateMutationResponseDto {
        schema_version: "lumin.gate-mutation.v1",
        operation_id: result.operation_id.clone(),
        request_digest: result.request_digest.clone(),
        gate_id: result.gate_id.clone(),
        revision: result.revision,
        lifecycle: result.lifecycle,
        decision: result.decision,
        reason: result.reason.clone(),
        signals: result.signals.iter().map(GateSignalDto::from).collect(),
        leased_write_set: result
            .leased_write_set
            .iter()
            .map(WriteLeaseDto::from)
            .collect(),
        deltas: result.deltas.clone(),
    }
}

pub fn gate_show_response(gate: &GateRecord) -> GateShowResponseDto {
    GateShowResponseDto {
        schema_version: "lumin.gate.v1",
        gate_id: gate.gate_id.clone(),
        lifecycle: gate.lifecycle,
        current_revision: gate.current_revision,
        declared_write_set: gate
            .declared_write_set
            .iter()
            .map(RepoPathDto::from)
            .collect(),
        leased_write_set: gate
            .leased_write_set
            .iter()
            .map(WriteLeaseDto::from)
            .collect(),
        transition_refs: gate.transition_refs.clone(),
        protected_semantic_input_count: gate.protected_semantic_inputs.len(),
        baseline: gate
            .baseline
            .as_ref()
            .map(|baseline| GateBaselineSummaryDto {
                analysis_contract: baseline.analysis_contract.clone(),
                analysis_input_id: baseline.snapshot.analysis_input_id.clone(),
                semantic_input_count: baseline.snapshot.inputs.len(),
                finding_count: baseline.snapshot.evidence.findings.len(),
                limitation_count: baseline.snapshot.evidence.limitations.len(),
                protected_semantic_input_count: baseline.protected_semantic_inputs.len(),
                transition_sequence: baseline.transition_sequence,
            }),
        revisions: gate
            .revisions
            .iter()
            .map(|revision| GateRevisionSummaryDto {
                revision: revision.revision,
                operation_id: revision.operation_id.clone(),
                decision: revision.decision,
                reason: revision.reason.clone(),
                signals: revision.signals.iter().map(GateSignalDto::from).collect(),
                changed_paths: revision
                    .changed_paths
                    .iter()
                    .map(RepoPathDto::from)
                    .collect(),
                analysis_input_id: revision
                    .snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.analysis_input_id.clone()),
                protected_semantic_input_count: revision.protected_semantic_inputs.len(),
                alias_group_count: revision.alias_closures.len(),
                reconciled_transition_sequences: revision.reconciled_transition_sequences.clone(),
                deltas: revision.deltas.clone(),
            })
            .collect(),
    }
}

pub fn operation_show_response(operation: &OperationRecord) -> OperationShowResponseDto {
    OperationShowResponseDto {
        schema_version: "lumin.operation.v1",
        operation_id: operation.operation_id.clone(),
        kind: operation.kind,
        request_digest: operation.request_digest.clone(),
        status: operation.status,
        gate_id: operation.gate_id.clone(),
        target_revision: operation.target_revision,
        reason: operation.reason.clone(),
        transition_sequence: operation.transition_sequence,
        interruption_count: operation.interruption_count,
        declared_write_set: operation
            .declared_write_set
            .iter()
            .map(RepoPathDto::from)
            .collect(),
        leased_write_set: operation
            .leased_write_set
            .iter()
            .map(WriteLeaseDto::from)
            .collect(),
        semantic_read_reservations: operation
            .semantic_read_reservations
            .iter()
            .map(RepoPathDto::from)
            .collect(),
        result: operation.result.as_ref().map(gate_mutation_response),
    }
}

pub fn to_json(value: &impl Serialize) -> Result<String, ProtocolError> {
    serde_json::to_string(value).map_err(|error| ProtocolError::Serialization(error.to_string()))
}

impl From<&FindingRecord> for FindingDto {
    fn from(finding: &FindingRecord) -> Self {
        Self {
            finding_id: finding.finding_id.clone(),
            rule_id: finding.rule_id.clone(),
            owner_capability: finding.owner_capability.clone(),
            severity: format!("{:?}", finding.severity).to_ascii_lowercase(),
            confidence: format!("{:?}", finding.confidence).to_ascii_lowercase(),
            disposition: finding.disposition.clone(),
            claim: finding.claim.clone(),
            source_id: finding.source_id.as_str().to_owned(),
            path: RepoPathDto::from(&finding.path),
            span: finding.span.clone(),
            exported_name: finding.exported_name.clone(),
            namespace: finding.namespace,
        }
    }
}

impl From<&RepoPathProjection> for RepoPathDto {
    fn from(path: &RepoPathProjection) -> Self {
        Self {
            schema_version: "repo-path.v1",
            canonical_base64: STANDARD.encode(&path.canonical),
            display: path.display.clone(),
        }
    }
}

impl From<&WriteLease> for WriteLeaseDto {
    fn from(lease: &WriteLease) -> Self {
        Self {
            path: RepoPathDto::from(&lease.path),
            kind: lease.kind,
        }
    }
}

impl From<&GateSignal> for GateSignalDto {
    fn from(signal: &GateSignal) -> Self {
        let mut dto = Self {
            kind: signal_kind(signal),
            count: None,
            detail: None,
            reason: None,
            paths: Vec::new(),
            gate_ids: Vec::new(),
            sequence: None,
        };
        match signal {
            GateSignal::FindingWarnings { count }
            | GateSignal::PreExistingAdverseFacts { count }
            | GateSignal::RequiredEvidenceIncomplete {
                limitation_count: count,
            }
            | GateSignal::AdverseFactIntroduced { count }
            | GateSignal::AdverseFactRegressed { count }
            | GateSignal::OpacityIntroduced { count }
            | GateSignal::OpacityRegressed { count }
            | GateSignal::LifecycleEvidenceRegressed { count }
            | GateSignal::LifecycleDeltaIncomparable { count }
            | GateSignal::LifecycleBaselineUnavailable { count } => dto.count = Some(*count),
            GateSignal::AnalysisFailed { detail } => dto.detail = Some(detail.clone()),
            GateSignal::DeclaredPathUnsupported { path, reason } => {
                dto.paths.push(RepoPathDto::from(path));
                dto.reason = Some(*reason);
            }
            GateSignal::WriteConflict { paths, gate_ids }
            | GateSignal::SemanticInputConflict { paths, gate_ids } => {
                dto.paths = paths.iter().map(RepoPathDto::from).collect();
                dto.gate_ids = gate_ids.clone();
            }
            GateSignal::ProtectedInputChanged { paths } | GateSignal::UnplannedWrite { paths } => {
                dto.paths = paths.iter().map(RepoPathDto::from).collect();
            }
            GateSignal::ActiveTransitionPending { paths, gate_ids } => {
                dto.paths = paths.iter().map(RepoPathDto::from).collect();
                dto.gate_ids = gate_ids.clone();
            }
            GateSignal::TransitionChainBroken { sequence } => dto.sequence = Some(*sequence),
            GateSignal::AnalysisContractChanged | GateSignal::TransitionCatalogChanged => {}
        }
        dto
    }
}

fn signal_kind(signal: &GateSignal) -> &'static str {
    match signal {
        GateSignal::FindingWarnings { .. } => "finding-warnings",
        GateSignal::PreExistingAdverseFacts { .. } => "pre-existing-adverse-facts",
        GateSignal::RequiredEvidenceIncomplete { .. } => "required-evidence-incomplete",
        GateSignal::AnalysisFailed { .. } => "analysis-failed",
        GateSignal::DeclaredPathUnsupported { .. } => "declared-path-unsupported",
        GateSignal::WriteConflict { .. } => "write-conflict",
        GateSignal::SemanticInputConflict { .. } => "semantic-input-conflict",
        GateSignal::ProtectedInputChanged { .. } => "protected-input-changed",
        GateSignal::AnalysisContractChanged => "analysis-contract-changed",
        GateSignal::UnplannedWrite { .. } => "unplanned-write",
        GateSignal::ActiveTransitionPending { .. } => "active-transition-pending",
        GateSignal::TransitionChainBroken { .. } => "transition-chain-broken",
        GateSignal::TransitionCatalogChanged => "transition-catalog-changed",
        GateSignal::AdverseFactIntroduced { .. } => "adverse-fact-introduced",
        GateSignal::AdverseFactRegressed { .. } => "adverse-fact-regressed",
        GateSignal::OpacityIntroduced { .. } => "opacity-introduced",
        GateSignal::OpacityRegressed { .. } => "opacity-regressed",
        GateSignal::LifecycleEvidenceRegressed { .. } => "lifecycle-evidence-regressed",
        GateSignal::LifecycleDeltaIncomparable { .. } => "lifecycle-delta-incomparable",
        GateSignal::LifecycleBaselineUnavailable { .. } => "lifecycle-baseline-unavailable",
    }
}

fn encode_cursor(
    run_id: &RunId,
    filters: &BTreeMap<String, Vec<String>>,
    last_finding_id: &FindingId,
) -> Result<String, ProtocolError> {
    let cursor = CursorDto {
        schema_version: "lumin-cursor.v1".to_owned(),
        run_id: run_id.clone(),
        ordering: FINDINGS_ORDERING.to_owned(),
        filters: filters.clone(),
        last_finding_id: last_finding_id.clone(),
    };
    let bytes = serde_json::to_vec(&cursor)
        .map_err(|error| ProtocolError::Serialization(error.to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn decode_cursor(value: &str) -> Result<CursorDto, ProtocolError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| ProtocolError::CursorEncoding)?;
    let cursor: CursorDto = serde_json::from_slice(&bytes)
        .map_err(|error| ProtocolError::CursorPayload(error.to_string()))?;
    if cursor.schema_version != "lumin-cursor.v1" {
        return Err(ProtocolError::CursorScopeMismatch);
    }
    Ok(cursor)
}

#[cfg(test)]
mod tests {
    use lumin_evidence::{Confidence, RepoPathProjection, Severity};
    use lumin_model::{FindingDisposition, LogicalSourceId, RepoPath, SourceSpan, SymbolNamespace};

    use super::*;

    #[test]
    fn findings_resume_after_the_exact_cursor_anchor() -> Result<(), Box<dyn std::error::Error>> {
        let evidence = evidence_with_findings(101)?;
        let run_id = RunId::from_string("run-a".to_owned());

        let first = findings_response(run_id.clone(), &evidence, None)?;
        assert_eq!(first.scope_total, 101);
        assert_eq!(first.total, 101);
        assert_eq!(first.returned, FINDINGS_PAGE_SIZE);
        assert!(first.truncated);
        let cursor = first
            .next_cursor
            .as_deref()
            .ok_or_else(|| std::io::Error::other("truncated page did not return a cursor"))?;

        let second = findings_response(run_id, &evidence, Some(cursor))?;
        assert_eq!(second.returned, 1);
        assert!(!second.truncated);
        assert!(second.next_cursor.is_none());
        assert_ne!(first.items[99].finding_id, second.items[0].finding_id);
        Ok(())
    }

    #[test]
    fn findings_cursor_is_bound_to_its_run() -> Result<(), Box<dyn std::error::Error>> {
        let evidence = evidence_with_findings(101)?;
        let first = findings_response(RunId::from_string("run-a".to_owned()), &evidence, None)?;
        let cursor = first
            .next_cursor
            .as_deref()
            .ok_or_else(|| std::io::Error::other("truncated page did not return a cursor"))?;

        let result = findings_response(
            RunId::from_string("run-b".to_owned()),
            &evidence,
            Some(cursor),
        );
        assert!(matches!(result, Err(ProtocolError::CursorScopeMismatch)));
        Ok(())
    }

    fn evidence_with_findings(count: usize) -> Result<RunEvidence, Box<dyn std::error::Error>> {
        let mut findings = Vec::with_capacity(count);
        for index in 0..count {
            let path = RepoPath::from_portable(&format!("src/file-{index:03}.ts"))?;
            let source_id = LogicalSourceId::from_path(&path);
            let exported_name = format!("dead{index:03}");
            findings.push(FindingRecord {
                finding_id: FindingId::for_export(
                    "dead-code/zero-exact-fan-in.v1",
                    &source_id,
                    SymbolNamespace::Value,
                    &exported_name,
                ),
                rule_id: "dead-code/zero-exact-fan-in.v1".to_owned(),
                owner_capability: "dead-code.v1".to_owned(),
                severity: Severity::Warning,
                confidence: Confidence::Grounded,
                disposition: FindingDisposition::ReviewCandidate,
                claim: "zero grounded exact fan-in".to_owned(),
                source_id,
                path: RepoPathProjection::from(&path),
                span: SourceSpan { start: 0, end: 1 },
                exported_name,
                namespace: SymbolNamespace::Value,
            });
        }
        Ok(RunEvidence {
            schema_version: "lumin-evidence.v1".to_owned(),
            capabilities: Vec::new(),
            resolution_profiles: Vec::new(),
            findings,
            limitations: Vec::new(),
        })
    }
}
