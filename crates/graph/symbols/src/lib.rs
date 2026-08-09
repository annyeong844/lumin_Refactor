use std::collections::BTreeMap;

use lumin_model::{
    ExportFact, FileFacts, ImportKind, LogicalSourceId, PackageSurfaceDeclaration,
    ResolutionOutcome, ResolvedSourceUse, SourceRoles, SourceSnapshot, SourceSpan, SymbolNamespace,
};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ExportIdentity {
    pub source_id: LogicalSourceId,
    pub namespace: SymbolNamespace,
    pub exported_name: String,
}

#[derive(Clone, Debug)]
pub struct GraphExport {
    pub fact: ExportFact,
    pub roles: SourceRoles,
    pub production_exact_fan_in: u64,
    pub test_exact_fan_in: u64,
    pub production_broad_fan_in: u64,
    pub test_broad_fan_in: u64,
    pub public_surface_count: u64,
}

/// A test-like re-export: a `ReExportNamed` use from a test-like importer
/// whose importer file has a matching `ExportFact` (same source_id, span, namespace).
/// This associates the re-export alias in the importer with the target export.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphTestReExport {
    /// The importer's LogicalSourceId (the test-like file doing the re-export).
    pub importer_source_id: LogicalSourceId,
    /// The ExportFact from the importer file that matches the re-export use.
    pub importer_export: ExportFact,
    /// The span of the re-export use in the importer.
    pub use_span: SourceSpan,
    /// The identity of the target export being re-exported.
    pub target: ExportIdentity,
}

#[derive(Clone, Debug, Default)]
pub struct SymbolGraph {
    pub exports: BTreeMap<ExportIdentity, GraphExport>,
    pub test_re_exports: Vec<GraphTestReExport>,
}

pub fn build(
    sources: &[SourceSnapshot],
    file_facts: &[FileFacts],
    resolved_uses: &[ResolvedSourceUse],
    package_surfaces: &[PackageSurfaceDeclaration],
) -> SymbolGraph {
    let roles = sources
        .iter()
        .map(|source| (source.id.clone(), source.roles.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut graph = SymbolGraph::default();

    // Index exports by file for importer-export matching.
    let exports_by_source: BTreeMap<LogicalSourceId, Vec<&ExportFact>> = {
        let mut map: BTreeMap<LogicalSourceId, Vec<&ExportFact>> = BTreeMap::new();
        for file in file_facts {
            for export in &file.exports {
                map.entry(file.source_id.clone()).or_default().push(export);
            }
        }
        map
    };

    for file in file_facts {
        let source_roles = roles.get(&file.source_id).cloned().unwrap_or_default();
        for export in &file.exports {
            let identity = ExportIdentity {
                source_id: export.source_id.clone(),
                namespace: export.namespace,
                exported_name: export.exported_name.clone(),
            };
            graph
                .exports
                .entry(identity)
                .or_insert_with(|| GraphExport {
                    fact: export.clone(),
                    roles: source_roles.clone(),
                    production_exact_fan_in: 0,
                    test_exact_fan_in: 0,
                    production_broad_fan_in: 0,
                    test_broad_fan_in: 0,
                    public_surface_count: 0,
                });
        }
    }

    for resolved in resolved_uses {
        let ResolutionOutcome::Internal { target } = &resolved.outcome else {
            continue;
        };
        let importer_is_test = roles
            .get(&resolved.source_use.importer)
            .is_some_and(SourceRoles::is_test_like);
        match resolved.source_use.kind {
            ImportKind::Named | ImportKind::Default | ImportKind::ReExportNamed => {
                let Some(imported_name) = &resolved.source_use.imported_name else {
                    continue;
                };
                let identity = ExportIdentity {
                    source_id: target.clone(),
                    namespace: resolved.source_use.namespace,
                    exported_name: imported_name.clone(),
                };
                if let Some(export) = graph.exports.get_mut(&identity) {
                    if importer_is_test {
                        export.test_exact_fan_in += 1;
                    } else {
                        export.production_exact_fan_in += 1;
                    }
                }

                // Collect test-like ReExportNamed with matching importer ExportFact.
                if importer_is_test
                    && resolved.source_use.kind == ImportKind::ReExportNamed
                    && graph.exports.contains_key(&identity)
                    && let Some(importer_exports) =
                        exports_by_source.get(&resolved.source_use.importer)
                {
                    for importer_export in importer_exports {
                        if importer_export.source_id == resolved.source_use.importer
                            && importer_export.namespace == resolved.source_use.namespace
                            && importer_export.span == resolved.source_use.span
                        {
                            graph.test_re_exports.push(GraphTestReExport {
                                importer_source_id: resolved.source_use.importer.clone(),
                                importer_export: (*importer_export).clone(),
                                use_span: resolved.source_use.span.clone(),
                                target: identity.clone(),
                            });
                        }
                    }
                }
            }
            ImportKind::Namespace | ImportKind::DynamicBroad | ImportKind::ReExportAll => {
                for (identity, export) in &mut graph.exports {
                    if identity.source_id == *target
                        && identity.namespace == resolved.source_use.namespace
                    {
                        if importer_is_test {
                            export.test_broad_fan_in += 1;
                        } else {
                            export.production_broad_fan_in += 1;
                        }
                    }
                }
            }
            ImportKind::SideEffect => {}
        }
    }

    // Deterministic ordering of test re-exports.
    graph.test_re_exports.sort_by(|a, b| {
        a.importer_source_id
            .cmp(&b.importer_source_id)
            .then_with(|| a.use_span.start.cmp(&b.use_span.start))
            .then_with(|| a.use_span.end.cmp(&b.use_span.end))
            .then_with(|| a.target.cmp(&b.target))
    });
    graph.test_re_exports.dedup();

    for surface in package_surfaces {
        for (identity, export) in &mut graph.exports {
            if identity.source_id == surface.target && identity.namespace == surface.namespace {
                export.public_surface_count += 1;
            }
        }
    }

    graph
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumin_model::{
        ExportFact, FileFacts, ImportKind, ModuleRequestKind, ResolutionOutcome, ResolvedSourceUse,
        SOURCE_CLASSIFICATION_RULE_VERSION, SourceClassificationRole, SourceKind,
        SourceRoleClassification, SourceRoleConfigurationSource, SourceRoleReason, SourceRoles,
        SourceSnapshot, SourceSpan, SourceUnitId, SourceUseFact, SymbolNamespace,
    };

    fn test_like_roles() -> SourceRoles {
        SourceRoles::from_classifications(vec![SourceRoleClassification {
            role: SourceClassificationRole::Test,
            rule_version: SOURCE_CLASSIFICATION_RULE_VERSION.to_owned(),
            reason: SourceRoleReason::TestPathRule,
            configuration_source: SourceRoleConfigurationSource::CompiledDefault,
        }])
    }

    fn make_source(
        path: &str,
        test_like: bool,
    ) -> Result<SourceSnapshot, Box<dyn std::error::Error>> {
        let repo_path = lumin_model::RepoPath::from_portable(path)?;
        let roles = if test_like {
            test_like_roles()
        } else {
            SourceRoles::default()
        };
        Ok(SourceSnapshot::new(
            repo_path,
            SourceKind::TypeScript,
            roles,
            lumin_model::PhysicalFileIdentity::Unix {
                device: 1,
                inode: 1,
            },
            Vec::new(),
        ))
    }

    #[test]
    fn test_re_export_named_from_test_source_populates_graph_test_re_export()
    -> Result<(), Box<dyn std::error::Error>> {
        // Setup: production source "src/lib.ts" exports "helper".
        // Test source "test/re-export.ts" re-exports "helper" from "src/lib.ts".
        let prod_source = make_source("src/lib.ts", false)?;
        let test_source = make_source("test/re-export.ts", true)?;

        let export_span = SourceSpan { start: 0, end: 20 };
        let use_span = SourceSpan { start: 0, end: 30 };

        let prod_export = ExportFact {
            source_id: prod_source.id.clone(),
            exported_name: "helper".to_owned(),
            local_name: Some("helper".to_owned()),
            namespace: SymbolNamespace::Value,
            span: export_span.clone(),
        };

        // The test file re-exports "helper" — the importer file also has an ExportFact
        // with the same span as the use (this is how OXC represents re-exports: the
        // export statement span matches the use span).
        let test_re_export_fact = ExportFact {
            source_id: test_source.id.clone(),
            exported_name: "helper".to_owned(),
            local_name: Some("helper".to_owned()),
            namespace: SymbolNamespace::Value,
            span: use_span.clone(),
        };

        let prod_file_facts = FileFacts {
            source_id: prod_source.id.clone(),
            source_unit: SourceUnitId::Logical(prod_source.id.clone()),
            exports: vec![prod_export.clone()],
            uses: Vec::new(),
            limitations: Vec::new(),
        };
        let test_file_facts = FileFacts {
            source_id: test_source.id.clone(),
            source_unit: SourceUnitId::Logical(test_source.id.clone()),
            exports: vec![test_re_export_fact.clone()],
            uses: Vec::new(),
            limitations: Vec::new(),
        };

        let resolved_use = ResolvedSourceUse {
            source_use: SourceUseFact {
                importer: test_source.id.clone(),
                specifier: "../src/lib".to_owned(),
                imported_name: Some("helper".to_owned()),
                local_name: Some("helper".to_owned()),
                namespace: SymbolNamespace::Value,
                kind: ImportKind::ReExportNamed,
                request_kind: ModuleRequestKind::StaticImport,
                span: use_span.clone(),
            },
            outcome: ResolutionOutcome::Internal {
                target: prod_source.id.clone(),
            },
        };

        let graph = build(
            &[prod_source.clone(), test_source.clone()],
            &[prod_file_facts, test_file_facts],
            &[resolved_use],
            &[],
        );

        // The target export should have test_exact_fan_in == 1.
        let target_identity = ExportIdentity {
            source_id: prod_source.id.clone(),
            namespace: SymbolNamespace::Value,
            exported_name: "helper".to_owned(),
        };
        let target_export = graph
            .exports
            .get(&target_identity)
            .ok_or("target export not found in graph")?;
        assert_eq!(target_export.test_exact_fan_in, 1);
        assert_eq!(target_export.production_exact_fan_in, 0);

        // A GraphTestReExport should be recorded.
        assert_eq!(graph.test_re_exports.len(), 1);
        let re_export = &graph.test_re_exports[0];
        assert_eq!(re_export.importer_source_id, test_source.id);
        assert_eq!(re_export.importer_export.exported_name, "helper");
        assert_eq!(re_export.importer_export.span, use_span);
        assert_eq!(re_export.target, target_identity);
        Ok(())
    }

    #[test]
    fn production_re_export_named_does_not_populate_test_re_exports()
    -> Result<(), Box<dyn std::error::Error>> {
        let prod_source = make_source("src/lib.ts", false)?;
        let barrel_source = make_source("src/index.ts", false)?;

        let export_span = SourceSpan { start: 0, end: 20 };
        let use_span = SourceSpan { start: 0, end: 30 };

        let prod_export = ExportFact {
            source_id: prod_source.id.clone(),
            exported_name: "helper".to_owned(),
            local_name: Some("helper".to_owned()),
            namespace: SymbolNamespace::Value,
            span: export_span.clone(),
        };

        let barrel_export = ExportFact {
            source_id: barrel_source.id.clone(),
            exported_name: "helper".to_owned(),
            local_name: Some("helper".to_owned()),
            namespace: SymbolNamespace::Value,
            span: use_span.clone(),
        };

        let prod_file_facts = FileFacts {
            source_id: prod_source.id.clone(),
            source_unit: SourceUnitId::Logical(prod_source.id.clone()),
            exports: vec![prod_export],
            uses: Vec::new(),
            limitations: Vec::new(),
        };
        let barrel_file_facts = FileFacts {
            source_id: barrel_source.id.clone(),
            source_unit: SourceUnitId::Logical(barrel_source.id.clone()),
            exports: vec![barrel_export],
            uses: Vec::new(),
            limitations: Vec::new(),
        };

        let resolved_use = ResolvedSourceUse {
            source_use: SourceUseFact {
                importer: barrel_source.id.clone(),
                specifier: "./lib".to_owned(),
                imported_name: Some("helper".to_owned()),
                local_name: Some("helper".to_owned()),
                namespace: SymbolNamespace::Value,
                kind: ImportKind::ReExportNamed,
                request_kind: ModuleRequestKind::StaticImport,
                span: use_span.clone(),
            },
            outcome: ResolutionOutcome::Internal {
                target: prod_source.id.clone(),
            },
        };

        let graph = build(
            &[prod_source, barrel_source],
            &[prod_file_facts, barrel_file_facts],
            &[resolved_use],
            &[],
        );

        // Production re-export should NOT create test_re_exports.
        assert!(graph.test_re_exports.is_empty());
        Ok(())
    }

    #[test]
    fn test_re_export_without_matching_importer_export_is_not_collected()
    -> Result<(), Box<dyn std::error::Error>> {
        // If the importer file does not have a matching ExportFact (same span/namespace),
        // no GraphTestReExport should be created.
        let prod_source = make_source("src/lib.ts", false)?;
        let test_source = make_source("test/re-export.ts", true)?;

        let export_span = SourceSpan { start: 0, end: 20 };
        let use_span = SourceSpan { start: 0, end: 30 };

        let prod_export = ExportFact {
            source_id: prod_source.id.clone(),
            exported_name: "helper".to_owned(),
            local_name: Some("helper".to_owned()),
            namespace: SymbolNamespace::Value,
            span: export_span.clone(),
        };

        let prod_file_facts = FileFacts {
            source_id: prod_source.id.clone(),
            source_unit: SourceUnitId::Logical(prod_source.id.clone()),
            exports: vec![prod_export],
            uses: Vec::new(),
            limitations: Vec::new(),
        };
        // Test file has NO exports (so no matching ExportFact).
        let test_file_facts = FileFacts {
            source_id: test_source.id.clone(),
            source_unit: SourceUnitId::Logical(test_source.id.clone()),
            exports: Vec::new(),
            uses: Vec::new(),
            limitations: Vec::new(),
        };

        let resolved_use = ResolvedSourceUse {
            source_use: SourceUseFact {
                importer: test_source.id.clone(),
                specifier: "../src/lib".to_owned(),
                imported_name: Some("helper".to_owned()),
                local_name: Some("helper".to_owned()),
                namespace: SymbolNamespace::Value,
                kind: ImportKind::ReExportNamed,
                request_kind: ModuleRequestKind::StaticImport,
                span: use_span.clone(),
            },
            outcome: ResolutionOutcome::Internal {
                target: prod_source.id.clone(),
            },
        };

        let graph = build(
            &[prod_source, test_source],
            &[prod_file_facts, test_file_facts],
            &[resolved_use],
            &[],
        );

        assert!(graph.test_re_exports.is_empty());
        Ok(())
    }

    #[test]
    fn test_re_exports_are_deterministically_ordered_and_deduplicated()
    -> Result<(), Box<dyn std::error::Error>> {
        let prod_source = make_source("src/lib.ts", false)?;
        let test_source = make_source("test/re-export.ts", true)?;

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

        let prod_file_facts = FileFacts {
            source_id: prod_source.id.clone(),
            source_unit: SourceUnitId::Logical(prod_source.id.clone()),
            exports: vec![prod_export],
            uses: Vec::new(),
            limitations: Vec::new(),
        };
        let test_file_facts = FileFacts {
            source_id: test_source.id.clone(),
            source_unit: SourceUnitId::Logical(test_source.id.clone()),
            exports: vec![test_re_export_fact],
            uses: Vec::new(),
            limitations: Vec::new(),
        };

        // Duplicate resolved uses (same re-export twice).
        let resolved_use = ResolvedSourceUse {
            source_use: SourceUseFact {
                importer: test_source.id.clone(),
                specifier: "../src/lib".to_owned(),
                imported_name: Some("helper".to_owned()),
                local_name: Some("helper".to_owned()),
                namespace: SymbolNamespace::Value,
                kind: ImportKind::ReExportNamed,
                request_kind: ModuleRequestKind::StaticImport,
                span: use_span.clone(),
            },
            outcome: ResolutionOutcome::Internal {
                target: prod_source.id.clone(),
            },
        };

        let graph = build(
            &[prod_source, test_source],
            &[prod_file_facts, test_file_facts],
            &[resolved_use.clone(), resolved_use],
            &[],
        );

        // Deduplication should collapse identical re-exports.
        assert_eq!(graph.test_re_exports.len(), 1);
        Ok(())
    }
}
