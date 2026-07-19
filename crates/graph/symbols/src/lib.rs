use std::collections::BTreeMap;

use lumin_model::{
    ExportFact, FileFacts, ImportKind, LogicalSourceId, PackageSurfaceDeclaration,
    ResolutionOutcome, ResolvedSourceUse, SourceRoles, SourceSnapshot, SymbolNamespace,
};

pub const GRAPH_VERSION: &str = "symbol-graph.v1";

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

#[derive(Clone, Debug, Default)]
pub struct SymbolGraph {
    pub exports: BTreeMap<ExportIdentity, GraphExport>,
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
            .is_some_and(|roles| roles.test_like.is_some());
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

    for surface in package_surfaces {
        for (identity, export) in &mut graph.exports {
            if identity.source_id == surface.target && identity.namespace == surface.namespace {
                export.public_surface_count += 1;
            }
        }
    }

    graph
}
