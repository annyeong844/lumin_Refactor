use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use lumin_evidence::{FindingRecord, RunEvidence};
use lumin_model::{
    AttemptId, CapabilityState, FindingDisposition, FindingId, Limitation, RunId, SourceSpan,
    SymbolNamespace,
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
            path: RepoPathDto {
                schema_version: "repo-path.v1",
                canonical_base64: STANDARD.encode(&finding.path.canonical),
                display: finding.path.display.clone(),
            },
            span: finding.span.clone(),
            exported_name: finding.exported_name.clone(),
            namespace: finding.namespace,
        }
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
