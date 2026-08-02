use std::collections::BTreeMap;

use lumin_evidence::{
    Confidence, DEAD_CODE_CAPABILITY_ID, DEAD_EXPORT_RULE_ID, EvidenceRecord, FindingRecord,
    FindingRelationRecord, RepoPathProjection, Severity, finding_relation_id, sort_findings,
};
use lumin_graph::SymbolGraph;
use lumin_model::{
    EvidenceId, FindingDisposition, FindingId, Limitation, LogicalSourceId, RepoPath,
    ReviewOnlyReason, SemanticConfigSnapshot, SourceSnapshot,
};

pub fn analyze(
    sources: &[SourceSnapshot],
    graph: &SymbolGraph,
    config: &SemanticConfigSnapshot,
    limitations: &[Limitation],
) -> Vec<FindingRecord> {
    let sources_by_id = sources
        .iter()
        .map(|source| {
            (
                source.id.clone(),
                (source.path.clone(), source.payload_sha256.clone()),
            )
        })
        .collect::<BTreeMap<LogicalSourceId, (RepoPath, String)>>();
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
        let Some((path, payload_sha256)) = sources_by_id.get(&export.fact.source_id) else {
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
        let evidence_id = EvidenceId::for_source_span(
            "definition",
            &export.fact.source_id,
            export.fact.span.start,
            export.fact.span.end,
            payload_sha256,
        );
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
            nested_collections_available: true,
            evidence: vec![EvidenceRecord {
                evidence_id,
                kind: "definition".to_owned(),
                source_id: export.fact.source_id.clone(),
                path: RepoPathProjection::from(path),
                span: export.fact.span.clone(),
                payload_sha256: payload_sha256.clone(),
            }],
            relations: Vec::new(),
        });
    }

    // Build finding_id set for the full canonical findings (all dispositions including ReviewOnly).
    let canonical_finding_ids: std::collections::BTreeSet<FindingId> =
        findings.iter().map(|f| f.finding_id.clone()).collect();

    // Add test-only-reexport evidence and relations from graph test re-exports.
    for test_re_export in &graph.test_re_exports {
        let Some((importer_path, importer_payload_sha256)) =
            sources_by_id.get(&test_re_export.importer_source_id)
        else {
            continue;
        };

        // Find the finding for the target export.
        let target_finding_id = FindingId::for_export(
            DEAD_EXPORT_RULE_ID,
            &test_re_export.target.source_id,
            test_re_export.target.namespace,
            &test_re_export.target.exported_name,
        );

        // Find the finding for the re-export alias (the importer's own export).
        let alias_finding_id = FindingId::for_export(
            DEAD_EXPORT_RULE_ID,
            &test_re_export.importer_export.source_id,
            test_re_export.importer_export.namespace,
            &test_re_export.importer_export.exported_name,
        );

        // Create the test-only-reexport evidence grounded by importer source.
        let reexport_evidence_id = EvidenceId::for_source_span(
            "test-only-reexport",
            &test_re_export.importer_source_id,
            test_re_export.use_span.start,
            test_re_export.use_span.end,
            importer_payload_sha256,
        );

        let reexport_evidence = EvidenceRecord {
            evidence_id: reexport_evidence_id.clone(),
            kind: "test-only-reexport".to_owned(),
            source_id: test_re_export.importer_source_id.clone(),
            path: RepoPathProjection::from(importer_path),
            span: test_re_export.use_span.clone(),
            payload_sha256: importer_payload_sha256.clone(),
        };

        // Attach evidence row to the target finding.
        if let Some(target_finding) = findings
            .iter_mut()
            .find(|f| f.finding_id == target_finding_id)
        {
            target_finding.evidence.push(reexport_evidence);

            // Add relation only if the alias re-export export is itself in the
            // full canonical findings set.
            if canonical_finding_ids.contains(&alias_finding_id) {
                let relation_id = finding_relation_id(
                    &target_finding_id,
                    "test-only-reexport",
                    &alias_finding_id,
                    &reexport_evidence_id,
                );
                target_finding.relations.push(FindingRelationRecord {
                    relation_id,
                    kind: "test-only-reexport".to_owned(),
                    target_finding_id: alias_finding_id,
                    grounding_evidence_id: reexport_evidence_id,
                });
            }
        }
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
            Limitation::VueTemplateOpaque { source_id, .. }
            | Limitation::AliasShapeUnsupported { source_id, .. } => {
                if !block_source_owner(source_id, sources, config, &mut blocked_paths) {
                    workspace_blocked = true;
                }
            }
            Limitation::AbsoluteInternalSpecifierUnsupported { .. } => workspace_blocked = true,
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
            Limitation::ExplicitEntryUnavailable { path, .. } => {
                if !block_entry_package_prefix(path, sources, config, &mut blocked_paths) {
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

/// Block the nearest package-root prefix for an entry path, else workspace.
fn block_entry_package_prefix(
    entry_path: &str,
    sources: &[SourceSnapshot],
    config: &SemanticConfigSnapshot,
    blocked_paths: &mut Vec<String>,
) -> bool {
    // Find the nearest package root that is a prefix of the entry path
    let mut best_package_root: Option<&RepoPath> = None;
    for package in &config.packages {
        let root_display = package.root.display_escaped();
        let is_prefix = entry_path == root_display
            || entry_path.starts_with(&format!("{root_display}/"))
            || root_display.is_empty();
        if is_prefix {
            if let Some(current) = best_package_root {
                if package.root.components_len() > current.components_len() {
                    best_package_root = Some(&package.root);
                }
            } else {
                best_package_root = Some(&package.root);
            }
        }
    }
    if let Some(root) = best_package_root {
        block_sources_under(root, sources, blocked_paths);
        return true;
    }
    // No package root found — block workspace
    false
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

#[cfg(test)]
mod tests {
    use super::*;
    use lumin_graph::{ExportIdentity, GraphExport, GraphTestReExport, SymbolGraph};
    use lumin_model::{
        ExportFact, LogicalSourceId, RepoPath, SourceKind, SourceRoleReason, SourceRoles,
        SourceSnapshot, SourceSpan, SymbolNamespace,
    };

    fn make_source(
        path: &str,
        test_like: bool,
    ) -> Result<SourceSnapshot, Box<dyn std::error::Error>> {
        let repo_path = RepoPath::from_portable(path)?;
        let roles = SourceRoles {
            test_like: if test_like {
                Some(SourceRoleReason::TestPathRule)
            } else {
                None
            },
            ..Default::default()
        };
        Ok(SourceSnapshot::new(
            repo_path,
            SourceKind::TypeScript,
            roles,
            lumin_model::PhysicalFileIdentity::Unix {
                device: 1,
                inode: 1,
            },
            b"content".to_vec(),
        ))
    }

    fn empty_config() -> SemanticConfigSnapshot {
        SemanticConfigSnapshot::default()
    }

    #[test]
    fn test_only_reexport_evidence_attached_to_target_finding()
    -> Result<(), Box<dyn std::error::Error>> {
        // prod "src/lib.ts" exports "helper" (zero production fan-in -> candidate finding).
        // test "test/barrel.ts" re-exports "helper" from "src/lib.ts".
        let prod_source = make_source("src/lib.ts", false)?;
        let test_source = make_source("test/barrel.ts", true)?;

        let export_span = SourceSpan { start: 0, end: 20 };
        let use_span = SourceSpan { start: 0, end: 30 };

        let prod_export = ExportFact {
            source_id: prod_source.id.clone(),
            exported_name: "helper".to_owned(),
            local_name: Some("helper".to_owned()),
            namespace: SymbolNamespace::Value,
            span: export_span.clone(),
        };

        let test_re_export_fact = ExportFact {
            source_id: test_source.id.clone(),
            exported_name: "helper".to_owned(),
            local_name: Some("helper".to_owned()),
            namespace: SymbolNamespace::Value,
            span: use_span.clone(),
        };

        let target_identity = ExportIdentity {
            source_id: prod_source.id.clone(),
            namespace: SymbolNamespace::Value,
            exported_name: "helper".to_owned(),
        };

        let mut graph = SymbolGraph::default();
        graph.exports.insert(
            target_identity.clone(),
            GraphExport {
                fact: prod_export.clone(),
                roles: SourceRoles::default(),
                production_exact_fan_in: 0,
                test_exact_fan_in: 1,
                production_broad_fan_in: 0,
                test_broad_fan_in: 0,
                public_surface_count: 0,
            },
        );
        // The test source also has an export (the re-export alias) with zero fan-in.
        let alias_identity = ExportIdentity {
            source_id: test_source.id.clone(),
            namespace: SymbolNamespace::Value,
            exported_name: "helper".to_owned(),
        };
        graph.exports.insert(
            alias_identity.clone(),
            GraphExport {
                fact: test_re_export_fact.clone(),
                roles: SourceRoles {
                    test_like: Some(SourceRoleReason::TestPathRule),
                    ..Default::default()
                },
                production_exact_fan_in: 0,
                test_exact_fan_in: 0,
                production_broad_fan_in: 0,
                test_broad_fan_in: 0,
                public_surface_count: 0,
            },
        );
        graph.test_re_exports.push(GraphTestReExport {
            importer_source_id: test_source.id.clone(),
            importer_export: test_re_export_fact.clone(),
            use_span: use_span.clone(),
            target: target_identity.clone(),
        });

        let findings = analyze(
            &[prod_source.clone(), test_source.clone()],
            &graph,
            &empty_config(),
            &[],
        );

        // Find the target finding.
        let target_finding_id = FindingId::for_export(
            DEAD_EXPORT_RULE_ID,
            &prod_source.id,
            SymbolNamespace::Value,
            "helper",
        );
        let target_finding = findings
            .iter()
            .find(|f| f.finding_id == target_finding_id)
            .ok_or("target finding must exist")?;

        // Should have definition evidence + test-only-reexport evidence.
        assert_eq!(target_finding.evidence.len(), 2);
        let reexport_ev = target_finding
            .evidence
            .iter()
            .find(|e| e.kind == "test-only-reexport")
            .ok_or("test-only-reexport evidence must exist")?;
        assert_eq!(reexport_ev.source_id, test_source.id);
        assert_eq!(reexport_ev.span, use_span);
        assert_eq!(reexport_ev.payload_sha256, test_source.payload_sha256);

        // The alias re-export export is itself a canonical finding, so a relation
        // should exist.
        assert_eq!(target_finding.relations.len(), 1);
        let relation = &target_finding.relations[0];
        assert_eq!(relation.kind, "test-only-reexport");
        let alias_finding_id = FindingId::for_export(
            DEAD_EXPORT_RULE_ID,
            &test_source.id,
            SymbolNamespace::Value,
            "helper",
        );
        assert_eq!(relation.target_finding_id, alias_finding_id);
        Ok(())
    }

    #[test]
    fn test_only_reexport_no_relation_when_alias_not_in_findings()
    -> Result<(), Box<dyn std::error::Error>> {
        // If the alias re-export export is NOT in the canonical finding set
        // (e.g. it has production fan-in), no relation should be added.
        let prod_source = make_source("src/lib.ts", false)?;
        let test_source = make_source("test/barrel.ts", true)?;

        let export_span = SourceSpan { start: 0, end: 20 };
        let use_span = SourceSpan { start: 0, end: 30 };

        let prod_export = ExportFact {
            source_id: prod_source.id.clone(),
            exported_name: "helper".to_owned(),
            local_name: Some("helper".to_owned()),
            namespace: SymbolNamespace::Value,
            span: export_span.clone(),
        };

        let test_re_export_fact = ExportFact {
            source_id: test_source.id.clone(),
            exported_name: "helper".to_owned(),
            local_name: Some("helper".to_owned()),
            namespace: SymbolNamespace::Value,
            span: use_span.clone(),
        };

        let target_identity = ExportIdentity {
            source_id: prod_source.id.clone(),
            namespace: SymbolNamespace::Value,
            exported_name: "helper".to_owned(),
        };

        let mut graph = SymbolGraph::default();
        graph.exports.insert(
            target_identity.clone(),
            GraphExport {
                fact: prod_export.clone(),
                roles: SourceRoles::default(),
                production_exact_fan_in: 0,
                test_exact_fan_in: 1,
                production_broad_fan_in: 0,
                test_broad_fan_in: 0,
                public_surface_count: 0,
            },
        );
        // The alias has production fan-in -> not a candidate.
        let alias_identity = ExportIdentity {
            source_id: test_source.id.clone(),
            namespace: SymbolNamespace::Value,
            exported_name: "helper".to_owned(),
        };
        graph.exports.insert(
            alias_identity.clone(),
            GraphExport {
                fact: test_re_export_fact.clone(),
                roles: SourceRoles {
                    test_like: Some(SourceRoleReason::TestPathRule),
                    ..Default::default()
                },
                production_exact_fan_in: 1,
                test_exact_fan_in: 0,
                production_broad_fan_in: 0,
                test_broad_fan_in: 0,
                public_surface_count: 0,
            },
        );
        graph.test_re_exports.push(GraphTestReExport {
            importer_source_id: test_source.id.clone(),
            importer_export: test_re_export_fact.clone(),
            use_span: use_span.clone(),
            target: target_identity.clone(),
        });

        let findings = analyze(
            &[prod_source.clone(), test_source.clone()],
            &graph,
            &empty_config(),
            &[],
        );

        let target_finding_id = FindingId::for_export(
            DEAD_EXPORT_RULE_ID,
            &prod_source.id,
            SymbolNamespace::Value,
            "helper",
        );
        let target_finding = findings
            .iter()
            .find(|f| f.finding_id == target_finding_id)
            .ok_or("target finding must exist")?;

        // Evidence should still be present.
        assert!(
            target_finding
                .evidence
                .iter()
                .any(|e| e.kind == "test-only-reexport")
        );

        // But no relation since alias is not in the canonical finding set (has production fan-in).
        assert!(target_finding.relations.is_empty());
        Ok(())
    }

    #[test]
    fn test_only_reexport_not_attached_when_target_has_production_fan_in()
    -> Result<(), Box<dyn std::error::Error>> {
        // If the target export has production fan-in, it's not in findings at all.
        let prod_source = make_source("src/lib.ts", false)?;
        let test_source = make_source("test/barrel.ts", true)?;

        let export_span = SourceSpan { start: 0, end: 20 };
        let use_span = SourceSpan { start: 0, end: 30 };

        let prod_export = ExportFact {
            source_id: prod_source.id.clone(),
            exported_name: "helper".to_owned(),
            local_name: Some("helper".to_owned()),
            namespace: SymbolNamespace::Value,
            span: export_span.clone(),
        };

        let test_re_export_fact = ExportFact {
            source_id: test_source.id.clone(),
            exported_name: "helper".to_owned(),
            local_name: Some("helper".to_owned()),
            namespace: SymbolNamespace::Value,
            span: use_span.clone(),
        };

        let target_identity = ExportIdentity {
            source_id: prod_source.id.clone(),
            namespace: SymbolNamespace::Value,
            exported_name: "helper".to_owned(),
        };

        let mut graph = SymbolGraph::default();
        // Target has production fan-in, so it won't be a finding.
        graph.exports.insert(
            target_identity.clone(),
            GraphExport {
                fact: prod_export,
                roles: SourceRoles::default(),
                production_exact_fan_in: 1,
                test_exact_fan_in: 1,
                production_broad_fan_in: 0,
                test_broad_fan_in: 0,
                public_surface_count: 0,
            },
        );
        graph.test_re_exports.push(GraphTestReExport {
            importer_source_id: test_source.id.clone(),
            importer_export: test_re_export_fact,
            use_span: use_span.clone(),
            target: target_identity,
        });

        let findings = analyze(&[prod_source, test_source], &graph, &empty_config(), &[]);

        // No finding for the target, so no test-only-reexport evidence attached.
        let target_finding_id = FindingId::for_export(
            DEAD_EXPORT_RULE_ID,
            &LogicalSourceId::from_path(&RepoPath::from_portable("src/lib.ts")?),
            SymbolNamespace::Value,
            "helper",
        );
        assert!(
            findings
                .iter()
                .find(|f| f.finding_id == target_finding_id)
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn test_only_reexport_relation_includes_review_only_alias()
    -> Result<(), Box<dyn std::error::Error>> {
        // A ReviewOnly finding (generated/vendored) must still be relation-eligible.
        // The canonical finding set includes ALL dispositions.
        let prod_source = make_source("src/lib.ts", false)?;
        let test_source = make_source("test/barrel.ts", true)?;

        let export_span = SourceSpan { start: 0, end: 20 };
        let use_span = SourceSpan { start: 0, end: 30 };

        let prod_export = ExportFact {
            source_id: prod_source.id.clone(),
            exported_name: "helper".to_owned(),
            local_name: Some("helper".to_owned()),
            namespace: SymbolNamespace::Value,
            span: export_span.clone(),
        };

        let test_re_export_fact = ExportFact {
            source_id: test_source.id.clone(),
            exported_name: "helper".to_owned(),
            local_name: Some("helper".to_owned()),
            namespace: SymbolNamespace::Value,
            span: use_span.clone(),
        };

        let target_identity = ExportIdentity {
            source_id: prod_source.id.clone(),
            namespace: SymbolNamespace::Value,
            exported_name: "helper".to_owned(),
        };

        let mut graph = SymbolGraph::default();
        graph.exports.insert(
            target_identity.clone(),
            GraphExport {
                fact: prod_export.clone(),
                roles: SourceRoles::default(),
                production_exact_fan_in: 0,
                test_exact_fan_in: 1,
                production_broad_fan_in: 0,
                test_broad_fan_in: 0,
                public_surface_count: 0,
            },
        );
        // The alias is a generated source (ReviewOnly disposition) with zero fan-in.
        let alias_identity = ExportIdentity {
            source_id: test_source.id.clone(),
            namespace: SymbolNamespace::Value,
            exported_name: "helper".to_owned(),
        };
        graph.exports.insert(
            alias_identity.clone(),
            GraphExport {
                fact: test_re_export_fact.clone(),
                roles: SourceRoles {
                    test_like: Some(SourceRoleReason::TestPathRule),
                    generated: Some(SourceRoleReason::TestPathRule),
                    ..Default::default()
                },
                production_exact_fan_in: 0,
                test_exact_fan_in: 0,
                production_broad_fan_in: 0,
                test_broad_fan_in: 0,
                public_surface_count: 0,
            },
        );
        graph.test_re_exports.push(GraphTestReExport {
            importer_source_id: test_source.id.clone(),
            importer_export: test_re_export_fact.clone(),
            use_span: use_span.clone(),
            target: target_identity.clone(),
        });

        let findings = analyze(
            &[prod_source.clone(), test_source.clone()],
            &graph,
            &empty_config(),
            &[],
        );

        // The alias finding exists with ReviewOnly disposition (generated source).
        let alias_finding_id = FindingId::for_export(
            DEAD_EXPORT_RULE_ID,
            &test_source.id,
            SymbolNamespace::Value,
            "helper",
        );
        let alias_finding = findings
            .iter()
            .find(|f| f.finding_id == alias_finding_id)
            .ok_or("alias ReviewOnly finding must exist")?;
        assert!(matches!(
            alias_finding.disposition,
            FindingDisposition::ReviewOnly { .. }
        ));

        // The target finding must have a relation pointing to the ReviewOnly alias.
        let target_finding_id = FindingId::for_export(
            DEAD_EXPORT_RULE_ID,
            &prod_source.id,
            SymbolNamespace::Value,
            "helper",
        );
        let target_finding = findings
            .iter()
            .find(|f| f.finding_id == target_finding_id)
            .ok_or("target finding must exist")?;
        assert_eq!(target_finding.relations.len(), 1);
        assert_eq!(
            target_finding.relations[0].target_finding_id,
            alias_finding_id
        );
        Ok(())
    }
}
