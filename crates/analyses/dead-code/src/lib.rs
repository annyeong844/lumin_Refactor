use std::collections::BTreeMap;

use lumin_evidence::{
    Confidence, DEAD_CODE_CAPABILITY_ID, DEAD_EXPORT_RULE_ID, FindingRecord, RepoPathProjection,
    Severity, sort_findings,
};
use lumin_graph::SymbolGraph;
use lumin_model::{
    FindingDisposition, FindingId, Limitation, LogicalSourceId, RepoPath, ReviewOnlyReason,
    SourceSnapshot,
};

pub const DEAD_ANALYSIS_VERSION: &str = "zero-exact-fan-in.v1";

pub fn analyze(
    sources: &[SourceSnapshot],
    graph: &SymbolGraph,
    limitations: &[Limitation],
) -> Vec<FindingRecord> {
    let paths = sources
        .iter()
        .map(|source| (source.id.clone(), source.path.clone()))
        .collect::<BTreeMap<LogicalSourceId, RepoPath>>();
    let (workspace_blocked, blocked_paths) = blocked_absence_scope(limitations);
    let mut findings = Vec::new();
    for export in graph.exports.values() {
        if export.roles.declaration
            || export.production_exact_fan_in > 0
            || export.production_broad_fan_in > 0
        {
            continue;
        }
        let Some(path) = paths.get(&export.fact.source_id) else {
            continue;
        };
        if workspace_blocked
            || blocked_paths
                .iter()
                .any(|blocked| blocked == &path.display_escaped())
        {
            continue;
        }
        let finding_id = FindingId::for_export(
            DEAD_EXPORT_RULE_ID,
            &export.fact.source_id,
            export.fact.namespace,
            &export.fact.exported_name,
        );
        let disposition = disposition(
            export.roles.generated.is_some(),
            export.roles.vendored.is_some(),
        );
        let claim = if export.test_exact_fan_in > 0 || export.test_broad_fan_in > 0 {
            format!(
                "export `{}` has zero production fan-in and is consumed only by test-like sources",
                export.fact.exported_name
            )
        } else {
            format!(
                "export `{}` has zero grounded exact fan-in",
                export.fact.exported_name
            )
        };
        findings.push(FindingRecord {
            finding_id,
            rule_id: DEAD_EXPORT_RULE_ID.to_owned(),
            owner_capability: DEAD_CODE_CAPABILITY_ID.to_owned(),
            severity: Severity::Warning,
            confidence: Confidence::Grounded,
            disposition,
            claim,
            source_id: export.fact.source_id.clone(),
            path: RepoPathProjection::from(path),
            span: export.fact.span.clone(),
            exported_name: export.fact.exported_name.clone(),
            namespace: export.fact.namespace,
        });
    }
    sort_findings(&mut findings);
    findings
}

fn blocked_absence_scope(limitations: &[Limitation]) -> (bool, Vec<String>) {
    let mut workspace_blocked = false;
    let mut blocked_paths = Vec::new();
    for limitation in limitations {
        match limitation {
            Limitation::InternalSpecifierUnresolved { candidates, .. } => {
                blocked_paths.extend(candidates.iter().cloned());
            }
            Limitation::JsModuleUseUnknown { .. }
            | Limitation::SourcePayloadUnavailable { .. }
            | Limitation::PublicSurfaceUnsupported { .. }
            | Limitation::TsconfigSemanticsUnsupported { .. }
            | Limitation::PackageDependencySemanticsUnsupported { .. }
            | Limitation::SfcDialectUnavailable { .. } => workspace_blocked = true,
        }
    }
    blocked_paths.sort();
    blocked_paths.dedup();
    (workspace_blocked, blocked_paths)
}

fn disposition(generated: bool, vendored: bool) -> FindingDisposition {
    match (generated, vendored) {
        (false, false) => FindingDisposition::ReviewCandidate,
        (true, false) => FindingDisposition::ReviewOnly {
            reason: ReviewOnlyReason::GeneratedSource,
        },
        (false, true) => FindingDisposition::ReviewOnly {
            reason: ReviewOnlyReason::VendoredSource,
        },
        (true, true) => FindingDisposition::ReviewOnly {
            reason: ReviewOnlyReason::GeneratedAndVendoredSource,
        },
    }
}
