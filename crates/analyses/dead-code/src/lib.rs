use std::collections::BTreeMap;

use lumin_evidence::{
    Confidence, DEAD_CODE_CAPABILITY_ID, DEAD_EXPORT_RULE_ID, FindingRecord, RepoPathProjection,
    Severity, sort_findings,
};
use lumin_graph::SymbolGraph;
use lumin_model::{
    FindingDisposition, FindingId, Limitation, LogicalSourceId, RepoPath, ReviewOnlyReason,
    SemanticConfigSnapshot, SourceSnapshot,
};

pub const DEAD_ANALYSIS_VERSION: &str = "zero-exact-fan-in.v1";

pub fn analyze(
    sources: &[SourceSnapshot],
    graph: &SymbolGraph,
    config: &SemanticConfigSnapshot,
    limitations: &[Limitation],
) -> Vec<FindingRecord> {
    let paths = sources
        .iter()
        .map(|source| (source.id.clone(), source.path.clone()))
        .collect::<BTreeMap<LogicalSourceId, RepoPath>>();
    let (workspace_blocked, blocked_paths) = blocked_absence_scope(sources, config, limitations);
    let mut findings = Vec::new();
    for export in graph.exports.values() {
        if export.roles.declaration
            || export.production_exact_fan_in > 0
            || export.production_broad_fan_in > 0
            || export.public_surface_count > 0
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

fn blocked_absence_scope(
    sources: &[SourceSnapshot],
    config: &SemanticConfigSnapshot,
    limitations: &[Limitation],
) -> (bool, Vec<String>) {
    let mut workspace_blocked = false;
    let mut blocked_paths = Vec::new();
    for limitation in limitations {
        match limitation {
            Limitation::InternalSpecifierUnresolved { candidates, .. } => {
                blocked_paths.extend(candidates.iter().cloned());
            }
            Limitation::JsModuleUseUnknown { .. }
            | Limitation::SourcePayloadUnavailable { .. }
            | Limitation::PackageIdentityUnsupported { .. }
            | Limitation::SfcDialectUnavailable { .. }
            | Limitation::SfcDecompositionUnknown { .. } => workspace_blocked = true,
            Limitation::SfcExternalScriptUnresolved { source_id, .. }
            | Limitation::VueTemplateOpaque { source_id, .. } => {
                if !block_source_owner(source_id, sources, config, &mut blocked_paths) {
                    workspace_blocked = true;
                }
            }
            Limitation::VueExternalScriptModeConflict {
                source_id,
                target_source_id,
                ..
            } => {
                let parent_known =
                    block_source_owner(source_id, sources, config, &mut blocked_paths);
                let target_known =
                    block_source_owner(target_source_id, sources, config, &mut blocked_paths);
                if !parent_known || !target_known {
                    workspace_blocked = true;
                }
            }
            Limitation::PublicSurfaceUnsupported { path, .. }
            | Limitation::PackageImportsUnsupported { path, .. }
            | Limitation::ImporterFormatUnsupported { path, .. }
            | Limitation::PackageDependencySemanticsUnsupported { path, .. }
            | Limitation::PackagePrivacyUnsupported { path, .. }
            | Limitation::DependencyOwnerAmbiguous { path, .. } => {
                if !block_owned_package(path, sources, config, &mut blocked_paths) {
                    workspace_blocked = true;
                }
            }
            Limitation::PackageMetadataUnobservable { path, .. } => {
                if !block_manifest_parent(path, sources, config, &mut blocked_paths) {
                    workspace_blocked = true;
                }
            }
            Limitation::TsconfigSemanticsUnsupported { path, .. }
            | Limitation::TsconfigPayloadUnavailable { path, .. } => {
                if !block_config_package(path, sources, config, &mut blocked_paths) {
                    workspace_blocked = true;
                }
            }
            Limitation::WorkspaceOwnershipUnsupported { path, .. }
            | Limitation::PnpmDependencySemanticsUnsupported { path, .. } => {
                if !block_workspace(path, sources, config, &mut blocked_paths) {
                    workspace_blocked = true;
                }
            }
        }
    }
    blocked_paths.sort();
    blocked_paths.dedup();
    (workspace_blocked, blocked_paths)
}

fn block_source_owner(
    source_id: &LogicalSourceId,
    sources: &[SourceSnapshot],
    config: &SemanticConfigSnapshot,
    blocked_paths: &mut Vec<String>,
) -> bool {
    let Some(package_root) = config.source_packages.get(source_id) else {
        return false;
    };
    block_sources_under(package_root, sources, blocked_paths);
    true
}

fn block_owned_package(
    manifest_path: &str,
    sources: &[SourceSnapshot],
    config: &SemanticConfigSnapshot,
    blocked_paths: &mut Vec<String>,
) -> bool {
    let Some(package) = config
        .packages
        .iter()
        .find(|package| package.manifest_path.display_escaped() == manifest_path)
    else {
        return false;
    };
    for source in sources {
        if config.source_packages.get(&source.id) == Some(&package.root) {
            blocked_paths.push(source.path.display_escaped());
        }
    }
    true
}

fn block_manifest_parent(
    manifest_path: &str,
    sources: &[SourceSnapshot],
    config: &SemanticConfigSnapshot,
    blocked_paths: &mut Vec<String>,
) -> bool {
    let Some(root) = config
        .observations
        .keys()
        .find(|path| path.display_escaped() == manifest_path)
        .and_then(RepoPath::parent)
    else {
        return false;
    };
    block_sources_under(&root, sources, blocked_paths);
    true
}

fn block_config_package(
    config_path: &str,
    sources: &[SourceSnapshot],
    config: &SemanticConfigSnapshot,
    blocked_paths: &mut Vec<String>,
) -> bool {
    let Some(path) = config
        .observations
        .keys()
        .find(|path| path.display_escaped() == config_path)
    else {
        return false;
    };
    let Some(package) = config
        .packages
        .iter()
        .filter(|package| path.is_within(&package.root))
        .max_by_key(|package| package.root.components_len())
    else {
        return false;
    };
    block_sources_under(&package.root, sources, blocked_paths);
    true
}

fn block_workspace(
    limitation_path: &str,
    sources: &[SourceSnapshot],
    config: &SemanticConfigSnapshot,
    blocked_paths: &mut Vec<String>,
) -> bool {
    let package_root = config
        .packages
        .iter()
        .find(|package| package.manifest_path.display_escaped() == limitation_path)
        .map(|package| package.root.clone());
    let pnpm_root = config.workspaces.iter().find_map(|workspace| {
        let path = workspace.root.join_portable("pnpm-workspace.yaml").ok()?;
        (path.display_escaped() == limitation_path).then(|| workspace.root.clone())
    });
    let Some(root) = package_root.or(pnpm_root) else {
        return false;
    };
    block_sources_under(&root, sources, blocked_paths);
    true
}

fn block_sources_under(
    root: &RepoPath,
    sources: &[SourceSnapshot],
    blocked_paths: &mut Vec<String>,
) {
    blocked_paths.extend(
        sources
            .iter()
            .filter(|source| source.path.is_within(root))
            .map(|source| source.path.display_escaped()),
    );
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
