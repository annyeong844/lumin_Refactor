mod config;
mod generated_config_policy;
mod package_surface;

use std::collections::{BTreeMap, BTreeSet};

use lumin_model::{
    ConfigSyntax, FileFacts, InventoryBoundSourceUse, Limitation, LogicalSourceId,
    PackageSurfaceDeclaration, PhysicalPathRedirect, RepoPath, RepositoryRootIdentity,
    ResolutionOutcome, ResolutionProfile, ResolvedSourceUse, SelectedResolutionProfile,
    SemanticConfigSnapshot, SourceSnapshot, SourceUseFact, SymbolNamespace, UnresolvedTargetScope,
};
use thiserror::Error;

pub use generated_config_policy::{
    FieldClassification as ResolverConfigFieldClassification,
    FieldPolicy as ResolverConfigFieldPolicy, RESOLVER_COMPILER_OPTIONS,
    RESOLVER_CONFIG_ARTIFACT_SHA256, RESOLVER_CONFIG_TABLE_SHA256, RESOLVER_INVENTORY_OWNED_FIELDS,
    RESOLVER_PACKAGE_JSON_FIELDS, RESOLVER_TSCONFIG_TOP_LEVEL,
};

pub const RESOLVER_VERSION: &str = "config-package-resolution.v7";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImporterFormatClassification {
    CommonJs,
    EsModule,
    Unavailable,
    Unsupported { path: String, detail: String },
}

pub fn classify_importer_format(
    source: &SourceSnapshot,
    config: &SemanticConfigSnapshot,
) -> ImporterFormatClassification {
    config::classify_importer_format(source, config)
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ConfigDemand {
    pub path: RepoPath,
    pub syntax: ConfigSyntax,
}

#[derive(Clone, Debug)]
pub struct ResolverOutput {
    pub resolved: Vec<ResolvedSourceUse>,
    pub package_surfaces: Vec<PackageSurfaceDeclaration>,
    pub profiles: Vec<SelectedResolutionProfile>,
    pub configured_sources: BTreeMap<LogicalSourceId, BTreeSet<RepoPath>>,
    pub limitations: Vec<Limitation>,
    pub demands: Vec<ConfigDemand>,
}

#[derive(Clone, Debug)]
pub struct ResolutionProfileSelection {
    pub profiles: Vec<SelectedResolutionProfile>,
    pub demands: Vec<ConfigDemand>,
}

#[derive(Debug, Error)]
pub enum ResolverError {
    #[error("resolver generated policy is invalid: {0}")]
    Policy(String),
    #[error("resolver configuration is invalid: {0}")]
    Configuration(String),
}

pub fn select_resolution_profiles(
    sources: &[SourceSnapshot],
    semantic_config: &SemanticConfigSnapshot,
    repository_root: &RepositoryRootIdentity,
    override_profile: Option<ResolutionProfile>,
) -> Result<ResolutionProfileSelection, ResolverError> {
    let selection = config::select(sources, semantic_config, repository_root, override_profile)?;
    Ok(ResolutionProfileSelection {
        profiles: selection.profiles,
        demands: selection.demands,
    })
}

pub fn resolve_all(
    sources: &[SourceSnapshot],
    physical_path_redirects: &[PhysicalPathRedirect],
    facts: &[FileFacts],
    inventory_bound_uses: &[InventoryBoundSourceUse],
    semantic_config: &SemanticConfigSnapshot,
    repository_root: &RepositoryRootIdentity,
    override_profile: Option<ResolutionProfile>,
) -> Result<ResolverOutput, ResolverError> {
    let mut selection =
        config::select(sources, semantic_config, repository_root, override_profile)?;
    if !selection.demands.is_empty() {
        return Ok(ResolverOutput {
            resolved: Vec::new(),
            package_surfaces: Vec::new(),
            profiles: selection.profiles,
            configured_sources: selection.configured_sources,
            limitations: selection.limitations,
            demands: selection.demands,
        });
    }
    let source_by_path = sources
        .iter()
        .map(|source| (source.path.clone(), source.id.clone()))
        .collect::<BTreeMap<_, _>>();
    let path_by_source = sources
        .iter()
        .map(|source| (source.id.clone(), source.path.clone()))
        .collect::<BTreeMap<_, _>>();
    let demands = collect_relative_directory_demands(
        facts,
        &path_by_source,
        &source_by_path,
        &selection.settings,
        semantic_config,
    );
    if !demands.is_empty() {
        return Ok(ResolverOutput {
            resolved: Vec::new(),
            package_surfaces: Vec::new(),
            profiles: selection.profiles,
            configured_sources: selection.configured_sources,
            limitations: selection.limitations,
            demands,
        });
    }

    let public_surfaces = package_surface::collect_public_surfaces(
        sources,
        physical_path_redirects,
        &source_by_path,
        semantic_config,
    );
    let mut package_surfaces = public_surfaces.declarations;
    selection.limitations.extend(public_surfaces.limitations);
    let mut resolved = Vec::new();
    for bound in inventory_bound_uses {
        let Some(settings) = selection.settings.get(&bound.source_use.importer) else {
            continue;
        };
        let (outcome, limitation) = resolve_inventory_bound_use(bound, &path_by_source, settings);
        if let Some(limitation) = limitation {
            selection.limitations.push(limitation);
        }
        resolved.push(ResolvedSourceUse {
            source_use: bound.source_use.clone(),
            outcome,
        });
    }
    for file in facts {
        let Some(importer_path) = path_by_source.get(&file.source_id) else {
            continue;
        };
        let Some(settings) = selection.settings.get(&file.source_id) else {
            continue;
        };
        for source_use in &file.uses {
            let (outcome, limitation, declaration) = resolve_one(
                importer_path,
                source_use,
                &source_by_path,
                physical_path_redirects,
                settings,
                semantic_config,
            );
            let limitation = limitation.or_else(|| match &outcome {
                ResolutionOutcome::Unresolved {
                    specifier,
                    candidates,
                    target_scope,
                } => Some(Limitation::InternalSpecifierUnresolved {
                    importer: source_use.importer.clone(),
                    specifier: specifier.clone(),
                    candidates: candidates.clone(),
                    target_scope: target_scope.clone(),
                }),
                ResolutionOutcome::Internal { .. }
                | ResolutionOutcome::External { .. }
                | ResolutionOutcome::NonSourceAsset { .. }
                | ResolutionOutcome::Unsupported { .. } => None,
            });
            if let Some(limitation) = limitation {
                selection.limitations.push(limitation);
            }
            if let Some(declaration) = declaration {
                package_surfaces.push(declaration);
            }
            resolved.push(ResolvedSourceUse {
                source_use: source_use.clone(),
                outcome,
            });
        }
    }
    resolved.sort_by(|left, right| {
        left.source_use
            .importer
            .cmp(&right.source_use.importer)
            .then_with(|| left.source_use.span.start.cmp(&right.source_use.span.start))
            .then_with(|| left.source_use.specifier.cmp(&right.source_use.specifier))
    });
    package_surfaces.sort();
    package_surfaces.dedup();
    Ok(ResolverOutput {
        resolved,
        package_surfaces,
        profiles: selection.profiles,
        configured_sources: selection.configured_sources,
        limitations: selection.limitations,
        demands: Vec::new(),
    })
}

fn resolve_inventory_bound_use(
    bound: &InventoryBoundSourceUse,
    path_by_source: &BTreeMap<LogicalSourceId, RepoPath>,
    settings: &config::ImporterSettings,
) -> (ResolutionOutcome, Option<Limitation>) {
    if settings.blocked {
        return (
            ResolutionOutcome::Unsupported {
                specifier: bound.source_use.specifier.clone(),
                reason: "the importer's semantic configuration is incomplete".to_owned(),
            },
            None,
        );
    }
    if !path_by_source.contains_key(&bound.target) {
        let detail = "inventory-bound source target is unavailable".to_owned();
        return (
            ResolutionOutcome::Unsupported {
                specifier: bound.source_use.specifier.clone(),
                reason: detail.clone(),
            },
            Some(Limitation::AliasShapeUnsupported {
                source_id: bound.source_use.importer.clone(),
                detail,
            }),
        );
    }
    (
        ResolutionOutcome::Internal {
            target: bound.target.clone(),
        },
        None,
    )
}

fn collect_relative_directory_demands(
    facts: &[FileFacts],
    path_by_source: &BTreeMap<LogicalSourceId, RepoPath>,
    sources: &BTreeMap<RepoPath, LogicalSourceId>,
    settings_by_source: &BTreeMap<LogicalSourceId, config::ImporterSettings>,
    semantic_config: &SemanticConfigSnapshot,
) -> Vec<ConfigDemand> {
    let mut demands = Vec::new();
    for file in facts {
        let Some(importer_path) = path_by_source.get(&file.source_id) else {
            continue;
        };
        let Some(settings) = settings_by_source.get(&file.source_id) else {
            continue;
        };
        if settings.blocked {
            continue;
        }
        for source_use in &file.uses {
            if !settings.allows_extensionless_for(source_use.request_kind) {
                continue;
            }
            let specifier = source_use.specifier.as_str();
            if !specifier.starts_with("./") && !specifier.starts_with("../") {
                continue;
            }
            let Some(base) = importer_path.resolve_portable_relative(specifier) else {
                continue;
            };
            if base
                .file_name_portable()
                .is_none_or(|name| name.contains('.'))
                || candidates(&base, source_use.namespace, true)
                    .iter()
                    .any(|candidate| sources.contains_key(candidate))
            {
                continue;
            }
            let Ok(manifest_path) = base.join_portable("package.json") else {
                continue;
            };
            if !semantic_config.observations.contains_key(&manifest_path) {
                demands.push(ConfigDemand {
                    path: manifest_path,
                    syntax: ConfigSyntax::StrictJson,
                });
            }
        }
    }
    demands.sort();
    demands.dedup();
    demands
}

fn resolve_one(
    importer_path: &RepoPath,
    source_use: &SourceUseFact,
    sources: &BTreeMap<RepoPath, LogicalSourceId>,
    physical_path_redirects: &[PhysicalPathRedirect],
    settings: &config::ImporterSettings,
    semantic_config: &SemanticConfigSnapshot,
) -> (
    ResolutionOutcome,
    Option<Limitation>,
    Option<PackageSurfaceDeclaration>,
) {
    let specifier = source_use.specifier.as_str();
    if settings.blocked {
        return (
            ResolutionOutcome::Unsupported {
                specifier: specifier.to_owned(),
                reason: "the importer's semantic configuration is incomplete".to_owned(),
            },
            None,
            None,
        );
    }
    if specifier.starts_with('/') || specifier.starts_with('\\') {
        return unsupported_with_limitation(
            source_use,
            "root-absolute internal-looking specifier".to_owned(),
            |source_id, detail| Limitation::AbsoluteInternalSpecifierUnsupported {
                source_id,
                detail,
            },
        );
    }
    if !specifier.starts_with("./") && !specifier.starts_with("../") {
        return resolve_bare_specifier(
            specifier,
            source_use,
            sources,
            physical_path_redirects,
            settings,
            semantic_config,
        );
    }
    resolve_relative_specifier(
        specifier,
        source_use,
        importer_path,
        sources,
        physical_path_redirects,
        settings,
        semantic_config,
    )
}

fn resolve_bare_specifier(
    specifier: &str,
    source_use: &SourceUseFact,
    sources: &BTreeMap<RepoPath, LogicalSourceId>,
    physical_path_redirects: &[PhysicalPathRedirect],
    settings: &config::ImporterSettings,
    semantic_config: &SemanticConfigSnapshot,
) -> (
    ResolutionOutcome,
    Option<Limitation>,
    Option<PackageSurfaceDeclaration>,
) {
    if specifier.starts_with('#') {
        if settings.profile == ResolutionProfile::Node {
            return (
                ResolutionOutcome::External {
                    package: specifier.to_owned(),
                },
                None,
                None,
            );
        }
        let result = package_surface::package_imports_unsupported(source_use, semantic_config);
        return (result.outcome, result.limitation, result.declaration);
    }
    match resolve_paths(specifier, source_use, sources, settings) {
        Ok(Some(outcome)) => return (outcome, None, None),
        Ok(None) => {}
        Err(reason) => {
            return unsupported_with_limitation(source_use, reason, |source_id, detail| {
                Limitation::AliasShapeUnsupported { source_id, detail }
            });
        }
    }
    if let Some(base_url) = &settings.base_url
        && let Some(base) = config::normalize_from(base_url, specifier)
    {
        let candidates = candidates(
            &base,
            source_use.namespace,
            settings.allows_extensionless_for(source_use.request_kind),
        );
        if let Some(target) = candidates.iter().find_map(|path| sources.get(path)) {
            return (
                ResolutionOutcome::Internal {
                    target: target.clone(),
                },
                None,
                None,
            );
        }
    }
    if let Some(result) = package_surface::resolve(
        source_use,
        sources,
        physical_path_redirects,
        settings,
        semantic_config,
    ) {
        return (result.outcome, result.limitation, result.declaration);
    }
    let bare_identity = package_name(specifier);
    (
        ResolutionOutcome::External {
            package: bare_identity,
        },
        None,
        None,
    )
}

fn resolve_relative_specifier(
    specifier: &str,
    source_use: &SourceUseFact,
    importer_path: &RepoPath,
    sources: &BTreeMap<RepoPath, LogicalSourceId>,
    physical_path_redirects: &[PhysicalPathRedirect],
    settings: &config::ImporterSettings,
    semantic_config: &SemanticConfigSnapshot,
) -> (
    ResolutionOutcome,
    Option<Limitation>,
    Option<PackageSurfaceDeclaration>,
) {
    let Some(base) = importer_path.resolve_portable_relative(specifier) else {
        return unsupported_with_limitation(
            source_use,
            "relative specifier escapes the canonical root".to_owned(),
            |source_id, detail| Limitation::AliasShapeUnsupported { source_id, detail },
        );
    };
    let allow_extensionless = settings.allows_extensionless_for(source_use.request_kind);
    if !allow_extensionless
        && base
            .file_name_portable()
            .is_some_and(|name| !name.contains('.'))
    {
        return unsupported_with_limitation(
            source_use,
            format!(
                "{} import-mode resolution requires an explicit relative extension",
                settings.profile.as_str()
            ),
            |source_id, detail| Limitation::AliasShapeUnsupported { source_id, detail },
        );
    }
    let candidates = candidates(&base, source_use.namespace, allow_extensionless);
    for candidate in &candidates {
        if let Some(target) = sources.get(candidate) {
            return (
                ResolutionOutcome::Internal {
                    target: target.clone(),
                },
                None,
                None,
            );
        }
    }

    if has_unsupported_explicit_extension(&base) {
        return (
            ResolutionOutcome::NonSourceAsset {
                specifier: specifier.to_owned(),
            },
            None,
            None,
        );
    }

    let mut unresolved_candidates = candidates
        .iter()
        .map(RepoPath::display_escaped)
        .collect::<Vec<_>>();
    if allow_extensionless
        && base
            .file_name_portable()
            .is_some_and(|name| !name.contains('.'))
    {
        let directory = package_surface::resolve_relative_directory(
            &base,
            source_use,
            sources,
            physical_path_redirects,
            settings,
            semantic_config,
        );
        match directory.outcome {
            ResolutionOutcome::Unresolved { candidates, .. } => {
                for candidate in candidates {
                    if !unresolved_candidates.contains(&candidate) {
                        unresolved_candidates.push(candidate);
                    }
                }
            }
            outcome => return (outcome, directory.limitation, directory.declaration),
        }
    }

    (
        ResolutionOutcome::Unresolved {
            specifier: specifier.to_owned(),
            candidates: unresolved_candidates,
            target_scope: Some(UnresolvedTargetScope::ExplicitTargets),
        },
        None,
        None,
    )
}

fn unsupported_with_limitation(
    source_use: &SourceUseFact,
    reason: String,
    make_limitation: impl FnOnce(LogicalSourceId, String) -> Limitation,
) -> (
    ResolutionOutcome,
    Option<Limitation>,
    Option<PackageSurfaceDeclaration>,
) {
    let specifier = source_use.specifier.clone();
    let detail = format!("unsupported specifier {specifier}: {reason}");
    (
        ResolutionOutcome::Unsupported { specifier, reason },
        Some(make_limitation(source_use.importer.clone(), detail)),
        None,
    )
}

pub(crate) fn candidates(
    base: &RepoPath,
    namespace: SymbolNamespace,
    allow_extensionless: bool,
) -> Vec<RepoPath> {
    let Some(file_name) = base.file_name_portable() else {
        return vec![base.clone()];
    };
    let Some(parent) = base.parent() else {
        return vec![base.clone()];
    };

    let names: Vec<String> = if let Some(stem) = file_name.strip_suffix(".js") {
        let extensions = if namespace == SymbolNamespace::Type {
            vec![".ts", ".tsx", ".d.ts", ".js", ".jsx"]
        } else {
            vec![".ts", ".tsx", ".js", ".jsx"]
        };
        extensions
            .into_iter()
            .map(|extension| format!("{stem}{extension}"))
            .collect()
    } else if let Some(stem) = file_name.strip_suffix(".jsx") {
        let extensions = if namespace == SymbolNamespace::Type {
            vec![".tsx", ".d.ts", ".jsx"]
        } else {
            vec![".tsx", ".jsx"]
        };
        extensions
            .into_iter()
            .map(|extension| format!("{stem}{extension}"))
            .collect()
    } else if let Some(stem) = file_name.strip_suffix(".mjs") {
        let extensions = if namespace == SymbolNamespace::Type {
            vec![".mts", ".d.mts", ".mjs"]
        } else {
            vec![".mts", ".mjs"]
        };
        extensions
            .into_iter()
            .map(|extension| format!("{stem}{extension}"))
            .collect()
    } else if let Some(stem) = file_name.strip_suffix(".cjs") {
        let extensions = if namespace == SymbolNamespace::Type {
            vec![".cts", ".d.cts", ".cjs"]
        } else {
            vec![".cts", ".cjs"]
        };
        extensions
            .into_iter()
            .map(|extension| format!("{stem}{extension}"))
            .collect()
    } else if file_name.contains('.') {
        return vec![base.clone()];
    } else if allow_extensionless {
        let extensions = if namespace == SymbolNamespace::Type {
            vec![".ts", ".tsx", ".d.ts", ".js", ".jsx"]
        } else {
            vec![".ts", ".tsx", ".js", ".jsx"]
        };
        extensions
            .into_iter()
            .map(|extension| format!("{file_name}{extension}"))
            .collect()
    } else {
        return vec![base.clone()];
    };

    names
        .into_iter()
        .filter_map(|name| parent.join_portable(&name).ok())
        .collect()
}

fn resolve_paths(
    specifier: &str,
    source_use: &SourceUseFact,
    sources: &BTreeMap<RepoPath, LogicalSourceId>,
    settings: &config::ImporterSettings,
) -> Result<Option<ResolutionOutcome>, String> {
    let Some(mappings) = settings.paths.as_ref() else {
        return Ok(None);
    };
    let Some(mapping) = mappings
        .entries
        .iter()
        .find(|mapping| !mapping.pattern.contains('*') && mapping.pattern == specifier)
        .or_else(|| {
            mappings
                .entries
                .iter()
                .filter_map(|mapping| {
                    let (prefix, suffix) = mapping.pattern.split_once('*')?;
                    if specifier.starts_with(prefix)
                        && specifier.ends_with(suffix)
                        && specifier.len() >= prefix.len() + suffix.len()
                    {
                        Some((mapping, prefix.len()))
                    } else {
                        None
                    }
                })
                .max_by(|(left, left_prefix), (right, right_prefix)| {
                    left_prefix
                        .cmp(right_prefix)
                        .then_with(|| right.source_order.cmp(&left.source_order))
                })
                .map(|(mapping, _)| mapping)
        })
    else {
        return Ok(None);
    };
    let capture = mapping.pattern.split_once('*').map(|(prefix, suffix)| {
        &specifier[prefix.len()..specifier.len().saturating_sub(suffix.len())]
    });
    for target in &mapping.targets {
        let target = match capture {
            Some(capture) => target.replacen('*', capture, 1),
            None => target.clone(),
        };
        let Some(base) = config::normalize_from(&mappings.base, &target) else {
            return Err(format!(
                "paths mapping for {specifier} escapes the canonical repository root"
            ));
        };
        for candidate in candidates(
            &base,
            source_use.namespace,
            settings.allows_extensionless_for(source_use.request_kind),
        ) {
            if let Some(target) = sources.get(&candidate) {
                return Ok(Some(ResolutionOutcome::Internal {
                    target: target.clone(),
                }));
            }
        }
    }
    Ok(None)
}

fn has_supported_explicit_extension(file_name: &str) -> bool {
    [
        ".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".mts", ".cts", ".vue", ".svelte", ".astro",
        ".d.ts", ".d.mts", ".d.cts",
    ]
    .iter()
    .any(|extension| file_name.ends_with(extension))
}

fn has_unsupported_explicit_extension(path: &RepoPath) -> bool {
    path.file_name_portable()
        .is_some_and(|name| name.contains('.') && !has_supported_explicit_extension(name))
}

fn package_name(specifier: &str) -> String {
    if let Some(scoped) = specifier.strip_prefix('@') {
        let mut parts = scoped.split('/');
        let scope = parts.next().unwrap_or_default();
        let package = parts.next().unwrap_or_default();
        if package.is_empty() {
            format!("@{scope}")
        } else {
            format!("@{scope}/{package}")
        }
    } else {
        specifier.split('/').next().unwrap_or(specifier).to_owned()
    }
}

#[cfg(test)]
mod tests {
    use lumin_model::{
        ConfigDocument, ConfigObservation, ConfigValue, ImportKind, ModuleRequestKind, SourceKind,
        SourceRoles, SourceSpan, SourceUseFact, SymbolNamespace,
    };

    use super::*;

    fn test_repository_root() -> Result<RepositoryRootIdentity, Box<dyn std::error::Error>> {
        const UNIX_ROOT_VECTOR: &str =
            "4c554d52524f4f54000101010000000101000000047265706f0100000000000000010000000000000002";
        let bytes = (0..UNIX_ROOT_VECTOR.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&UNIX_ROOT_VECTOR[index..index + 2], 16))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(RepositoryRootIdentity::from_canonical_bytes(&bytes)?)
    }

    #[test]
    fn js_candidate_prefers_typescript_source() -> Result<(), Box<dyn std::error::Error>> {
        let importer = SourceSnapshot::new(
            RepoPath::from_portable("src/main.ts")?,
            SourceKind::TypeScript,
            SourceRoles::default(),
            lumin_model::PhysicalFileIdentity::Unix {
                device: 1,
                inode: 1,
            },
            Vec::new(),
        );
        let target = SourceSnapshot::new(
            RepoPath::from_portable("src/lib.ts")?,
            SourceKind::TypeScript,
            SourceRoles::default(),
            lumin_model::PhysicalFileIdentity::Unix {
                device: 1,
                inode: 2,
            },
            Vec::new(),
        );
        let source_use = SourceUseFact {
            importer: importer.id.clone(),
            specifier: "./lib.js".to_owned(),
            imported_name: Some("used".to_owned()),
            local_name: Some("used".to_owned()),
            namespace: SymbolNamespace::Value,
            kind: ImportKind::Named,
            request_kind: ModuleRequestKind::StaticImport,
            span: SourceSpan { start: 0, end: 10 },
        };
        let config = SemanticConfigSnapshot::default();
        let settings = config::ImporterSettings {
            profile: ResolutionProfile::Bundler,
            static_condition: config::PackageConditionMode::Import,
            base_url: None,
            paths: None,
            blocked: false,
        };
        let (outcome, limitation, declaration) = resolve_one(
            &importer.path,
            &source_use,
            &[(target.path.clone(), target.id.clone())]
                .into_iter()
                .collect(),
            &[],
            &settings,
            &config,
        );
        assert!(limitation.is_none());
        assert!(declaration.is_none());
        assert_eq!(outcome, ResolutionOutcome::Internal { target: target.id });
        Ok(())
    }

    #[test]
    fn inventory_bound_glob_target_bypasses_typescript_substitution()
    -> Result<(), Box<dyn std::error::Error>> {
        let importer = SourceSnapshot::new(
            RepoPath::from_portable("src/main.ts")?,
            SourceKind::TypeScript,
            SourceRoles::default(),
            lumin_model::PhysicalFileIdentity::Unix {
                device: 1,
                inode: 1,
            },
            Vec::new(),
        );
        let javascript = SourceSnapshot::new(
            RepoPath::from_portable("src/pages/foo.js")?,
            SourceKind::JavaScript,
            SourceRoles::default(),
            lumin_model::PhysicalFileIdentity::Unix {
                device: 1,
                inode: 2,
            },
            Vec::new(),
        );
        let typescript = SourceSnapshot::new(
            RepoPath::from_portable("src/pages/foo.ts")?,
            SourceKind::TypeScript,
            SourceRoles::default(),
            lumin_model::PhysicalFileIdentity::Unix {
                device: 1,
                inode: 3,
            },
            Vec::new(),
        );
        let facts = FileFacts::physical(importer.id.clone());
        let bound = InventoryBoundSourceUse {
            source_use: SourceUseFact {
                importer: importer.id.clone(),
                specifier: "./pages/foo.js".to_owned(),
                imported_name: None,
                local_name: None,
                namespace: SymbolNamespace::Value,
                kind: ImportKind::DynamicBroad,
                request_kind: ModuleRequestKind::ImportMetaGlob,
                span: SourceSpan { start: 0, end: 10 },
            },
            target: javascript.id.clone(),
        };
        let expected = javascript.id.clone();
        let output = resolve_all(
            &[importer, javascript, typescript],
            &[],
            &[facts],
            &[bound],
            &SemanticConfigSnapshot::default(),
            &test_repository_root()?,
            None,
        )?;

        assert_eq!(output.resolved.len(), 1);
        assert_eq!(
            output.resolved[0].outcome,
            ResolutionOutcome::Internal { target: expected }
        );
        Ok(())
    }

    #[test]
    fn extensionless_directory_manifest_is_demanded_before_index_resolution()
    -> Result<(), Box<dyn std::error::Error>> {
        let importer = SourceSnapshot::new(
            RepoPath::from_portable("src/main.ts")?,
            SourceKind::TypeScript,
            SourceRoles::default(),
            lumin_model::PhysicalFileIdentity::Unix {
                device: 1,
                inode: 1,
            },
            Vec::new(),
        );
        let index = SourceSnapshot::new(
            RepoPath::from_portable("src/lib/index.ts")?,
            SourceKind::TypeScript,
            SourceRoles::default(),
            lumin_model::PhysicalFileIdentity::Unix {
                device: 1,
                inode: 2,
            },
            Vec::new(),
        );
        let mut facts = FileFacts::physical(importer.id.clone());
        facts.uses.push(SourceUseFact {
            importer: importer.id.clone(),
            specifier: "./lib".to_owned(),
            imported_name: Some("used".to_owned()),
            local_name: Some("used".to_owned()),
            namespace: SymbolNamespace::Value,
            kind: ImportKind::Named,
            request_kind: ModuleRequestKind::StaticImport,
            span: SourceSpan { start: 0, end: 10 },
        });
        let sources = [importer, index.clone()];
        let mut config = SemanticConfigSnapshot::default();
        let manifest_path = RepoPath::from_portable("src/lib/package.json")?;

        let repository_root = test_repository_root()?;
        let first = resolve_all(
            &sources,
            &[],
            std::slice::from_ref(&facts),
            &[],
            &config,
            &repository_root,
            None,
        )?;
        assert_eq!(
            first.demands,
            vec![ConfigDemand {
                path: manifest_path.clone(),
                syntax: ConfigSyntax::StrictJson,
            }]
        );
        assert!(first.resolved.is_empty());

        config.observations.insert(
            manifest_path.clone(),
            ConfigObservation::Missing {
                path: manifest_path,
                parent: lumin_model::ConfigAbsenceParent {
                    path: RepoPath::from_portable("src/lib")?,
                    physical_identity: lumin_model::PhysicalFileIdentity::Unix {
                        device: 1,
                        inode: 3,
                    },
                },
            },
        );
        let second = resolve_all(
            &sources,
            &[],
            &[facts],
            &[],
            &config,
            &repository_root,
            None,
        )?;
        assert!(second.demands.is_empty());
        assert_eq!(second.resolved.len(), 1);
        assert_eq!(
            second.resolved[0].outcome,
            ResolutionOutcome::Internal { target: index.id }
        );
        Ok(())
    }

    #[test]
    fn observed_directory_manifest_without_package_fact_is_typed_incomplete()
    -> Result<(), Box<dyn std::error::Error>> {
        let importer = SourceSnapshot::new(
            RepoPath::from_portable("src/main.ts")?,
            SourceKind::TypeScript,
            SourceRoles::default(),
            lumin_model::PhysicalFileIdentity::Unix {
                device: 1,
                inode: 1,
            },
            Vec::new(),
        );
        let mut facts = FileFacts::physical(importer.id.clone());
        facts.uses.push(SourceUseFact {
            importer: importer.id.clone(),
            specifier: "./lib".to_owned(),
            imported_name: Some("used".to_owned()),
            local_name: Some("used".to_owned()),
            namespace: SymbolNamespace::Value,
            kind: ImportKind::Named,
            request_kind: ModuleRequestKind::StaticImport,
            span: SourceSpan { start: 0, end: 10 },
        });
        let manifest_path = RepoPath::from_portable("src/lib/package.json")?;
        let mut config = SemanticConfigSnapshot::default();
        config.observations.insert(
            manifest_path.clone(),
            ConfigObservation::Present {
                document: ConfigDocument {
                    path: manifest_path.clone(),
                    payload_sha256: "digest".to_owned(),
                    root: ConfigValue::Object(Vec::new()),
                },
                physical_identity: lumin_model::PhysicalFileIdentity::Unix {
                    device: 1,
                    inode: 4,
                },
            },
        );

        let repository_root = test_repository_root()?;
        let selection = resolve_all(
            &[importer],
            &[],
            &[facts],
            &[],
            &config,
            &repository_root,
            None,
        )?;

        assert!(selection.demands.is_empty());
        assert!(matches!(
            selection.resolved[0].outcome,
            ResolutionOutcome::Unsupported { .. }
        ));
        assert!(selection.limitations.iter().any(|limitation| matches!(
            limitation,
            Limitation::PublicSurfaceUnsupported { path, detail, .. }
                if path == &manifest_path.display_escaped()
                    && detail.contains("no matching package fact")
        )));
        Ok(())
    }
}
