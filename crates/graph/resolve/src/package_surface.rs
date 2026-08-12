mod exports;
mod fallback;
mod public;

use std::collections::{BTreeMap, BTreeSet};

use lumin_model::{
    ConfigDocument, ConfigObservation, ImportKind, Limitation, LogicalSourceId, ModuleRequestKind,
    PackageFact, PackageIdentityState, PackagePrivacy, PackageSurfaceDeclaration,
    PackageSurfaceLane, PackageSurfaceSource, PhysicalPathRedirect, PhysicalPathRedirectTarget,
    RepoPath, ResolutionOutcome, ResolutionProfile, SemanticConfigSnapshot, SourceSnapshot,
    SourceUseFact, SymbolNamespace, UnresolvedTargetScope,
};

use crate::candidates;
use crate::config::{ImporterSettings, PackageConditionMode};
use crate::generated_config_policy::{
    FieldClassification, PackageFieldApplicability, RESOLVER_PACKAGE_FIELD_APPLICABILITY,
    RESOLVER_PACKAGE_JSON_FIELDS,
};

pub(crate) struct PackageResolution {
    pub outcome: ResolutionOutcome,
    pub limitation: Option<Limitation>,
    pub declaration: Option<PackageSurfaceDeclaration>,
}

#[derive(Default)]
pub(crate) struct PublicSurfaceOutput {
    pub declarations: Vec<PackageSurfaceDeclaration>,
    pub limitations: Vec<Limitation>,
}

pub(super) struct PackageContext<'a> {
    pub package: &'a PackageFact,
    pub manifest: &'a ConfigDocument,
    pub sources: &'a BTreeMap<RepoPath, LogicalSourceId>,
    pub physical_path_redirects: &'a [PhysicalPathRedirect],
}

#[derive(Clone, Copy)]
pub(super) struct ResolutionRequest<'a> {
    pub specifier: &'a str,
    pub key: &'a str,
    pub namespace: SymbolNamespace,
    pub import_kind: ImportKind,
    pub lane: PackageSurfaceLane,
}

pub(super) struct TargetRequest<'a> {
    pub specifier: &'a str,
    pub namespace: SymbolNamespace,
    pub source: PackageSurfaceSource,
    pub base: RepoPath,
    pub allow_extensionless: bool,
    pub allow_directory: bool,
}

pub(crate) fn resolve(
    source_use: &SourceUseFact,
    sources: &BTreeMap<RepoPath, LogicalSourceId>,
    physical_path_redirects: &[PhysicalPathRedirect],
    settings: &ImporterSettings,
    config: &SemanticConfigSnapshot,
) -> Option<PackageResolution> {
    let request = PackageRequest::parse(&source_use.specifier)?;
    let package = workspace_package_for_importer(source_use, &request.name, config)?;
    let manifest = package_manifest(package, config)?;
    let context = PackageContext {
        package,
        manifest,
        sources,
        physical_path_redirects,
    };
    let mut result = resolve_request(
        &context,
        ResolutionRequest {
            specifier: &source_use.specifier,
            key: &request.key,
            namespace: source_use.namespace,
            import_kind: source_use.kind,
            lane: lane_for_use(settings, source_use.request_kind),
        },
    );
    if !matches!(
        package.privacy,
        PackagePrivacy::Public | PackagePrivacy::Unspecified
    ) {
        result.declaration = None;
    }
    Some(result)
}

pub(crate) fn resolve_relative_directory(
    base: &RepoPath,
    source_use: &SourceUseFact,
    sources: &BTreeMap<RepoPath, LogicalSourceId>,
    physical_path_redirects: &[PhysicalPathRedirect],
    settings: &ImporterSettings,
    config: &SemanticConfigSnapshot,
) -> PackageResolution {
    let manifest_path = match base.join_portable("package.json") {
        Ok(path) => path,
        Err(_) => {
            let detail = "directory cannot form package.json";
            return relative_directory_unsupported(
                &source_use.specifier,
                detail,
                Some(Limitation::AliasShapeUnsupported {
                    source_id: source_use.importer.clone(),
                    detail: detail.to_owned(),
                }),
            );
        }
    };
    match config.observations.get(&manifest_path) {
        Some(ConfigObservation::Present {
            document: manifest, ..
        }) => {
            let Some(package) = config
                .packages
                .iter()
                .find(|package| package.root == *base && package.manifest_path == manifest_path)
            else {
                let detail = "observed package manifest has no matching package fact";
                return relative_directory_unsupported(
                    &source_use.specifier,
                    detail,
                    Some(Limitation::PublicSurfaceUnsupported {
                        path: manifest_path.display_escaped(),
                        detail: detail.to_owned(),
                    }),
                );
            };
            let context = PackageContext {
                package,
                manifest,
                sources,
                physical_path_redirects,
            };
            let mut result = fallback::resolve(
                &context,
                ResolutionRequest {
                    specifier: &source_use.specifier,
                    key: ".",
                    namespace: source_use.namespace,
                    import_kind: source_use.kind,
                    lane: lane_for_use(settings, source_use.request_kind),
                },
            );
            result.declaration = None;
            result
        }
        Some(ConfigObservation::NonRegular { .. }) => relative_directory_unsupported(
            &source_use.specifier,
            "package.json is not a regular file",
            None,
        ),
        Some(ConfigObservation::Unreadable { .. }) => relative_directory_unsupported(
            &source_use.specifier,
            "package.json could not be read",
            None,
        ),
        Some(ConfigObservation::Missing { .. }) => {
            let index = match base.join_portable("index.js") {
                Ok(path) => path,
                Err(_) => {
                    let detail = "directory cannot form index.js";
                    return relative_directory_unsupported(
                        &source_use.specifier,
                        detail,
                        Some(Limitation::AliasShapeUnsupported {
                            source_id: source_use.importer.clone(),
                            detail: detail.to_owned(),
                        }),
                    );
                }
            };
            let paths = candidates(&index, source_use.namespace, true);
            if let Some(target) = paths.iter().find_map(|path| sources.get(path)) {
                return PackageResolution {
                    outcome: ResolutionOutcome::Internal {
                        target: target.clone(),
                    },
                    limitation: None,
                    declaration: None,
                };
            }
            unresolved(
                &source_use.specifier,
                paths.iter().map(RepoPath::display_escaped).collect(),
            )
        }
        None => {
            let detail = "package.json observation is missing after demand collection";
            relative_directory_unsupported(
                &source_use.specifier,
                detail,
                Some(Limitation::PublicSurfaceUnsupported {
                    path: manifest_path.display_escaped(),
                    detail: detail.to_owned(),
                }),
            )
        }
    }
}

fn relative_directory_unsupported(
    specifier: &str,
    detail: &str,
    limitation: Option<Limitation>,
) -> PackageResolution {
    PackageResolution {
        outcome: ResolutionOutcome::Unsupported {
            specifier: specifier.to_owned(),
            reason: detail.to_owned(),
        },
        limitation,
        declaration: None,
    }
}

pub(crate) fn package_imports_unsupported(
    source_use: &SourceUseFact,
    config: &SemanticConfigSnapshot,
) -> PackageResolution {
    let path = importer_package(source_use, config).map_or_else(
        || source_use.importer.as_str().to_owned(),
        |package| package.manifest_path.display_escaped(),
    );
    PackageResolution {
        outcome: ResolutionOutcome::Unsupported {
            specifier: source_use.specifier.clone(),
            reason: "package imports are unsupported".to_owned(),
        },
        limitation: Some(Limitation::PackageImportsUnsupported {
            path,
            detail: format!(
                "package imports specifier {} is unsupported",
                source_use.specifier
            ),
        }),
        declaration: None,
    }
}

pub(crate) fn collect_public_surfaces(
    sources: &[SourceSnapshot],
    physical_path_redirects: &[PhysicalPathRedirect],
    source_by_path: &BTreeMap<RepoPath, LogicalSourceId>,
    config: &SemanticConfigSnapshot,
) -> PublicSurfaceOutput {
    public::collect(sources, physical_path_redirects, source_by_path, config)
}

struct PackageRequest {
    name: String,
    key: String,
}

impl PackageRequest {
    fn parse(specifier: &str) -> Option<Self> {
        if specifier.is_empty() || specifier.starts_with('#') {
            return None;
        }
        let (name, subpath) = if let Some(scoped) = specifier.strip_prefix('@') {
            let mut parts = scoped.split('/');
            let scope = parts.next()?;
            let package = parts.next()?;
            if scope.is_empty() || package.is_empty() {
                return None;
            }
            let name = format!("@{scope}/{package}");
            let rest = parts.collect::<Vec<_>>().join("/");
            (name, rest)
        } else {
            let mut parts = specifier.split('/');
            let name = parts.next()?.to_owned();
            let rest = parts.collect::<Vec<_>>().join("/");
            (name, rest)
        };
        if name.is_empty() {
            return None;
        }
        let key = if subpath.is_empty() {
            ".".to_owned()
        } else {
            format!("./{subpath}")
        };
        Some(Self { name, key })
    }
}

fn importer_package<'a>(
    source_use: &SourceUseFact,
    config: &'a SemanticConfigSnapshot,
) -> Option<&'a PackageFact> {
    let root = config.source_packages.get(&source_use.importer)?;
    config.packages.iter().find(|package| &package.root == root)
}

fn workspace_package_for_importer<'a>(
    source_use: &SourceUseFact,
    requested_name: &str,
    config: &'a SemanticConfigSnapshot,
) -> Option<&'a PackageFact> {
    let workspace_root = importer_package(source_use, config)?
        .workspace_root
        .as_ref()?;
    config.packages.iter().find(|package| {
        package.workspace_root.as_ref() == Some(workspace_root)
            && matches!(
                &package.identity,
                PackageIdentityState::Valid(identity) if identity.as_str() == requested_name
            )
    })
}

pub(super) fn package_manifest<'a>(
    package: &PackageFact,
    config: &'a SemanticConfigSnapshot,
) -> Option<&'a ConfigDocument> {
    match config.observations.get(&package.manifest_path)? {
        ConfigObservation::Present { document, .. } => Some(document),
        ConfigObservation::Missing { .. }
        | ConfigObservation::NonRegular { .. }
        | ConfigObservation::Unreadable { .. } => None,
    }
}

fn lane_for_use(
    settings: &ImporterSettings,
    request_kind: ModuleRequestKind,
) -> PackageSurfaceLane {
    match settings.profile {
        ResolutionProfile::Bundler => PackageSurfaceLane::BundlerImport,
        ResolutionProfile::Node => PackageSurfaceLane::LegacyNode,
        ResolutionProfile::Node16 | ResolutionProfile::NodeNext => match request_kind {
            ModuleRequestKind::DynamicImport => PackageSurfaceLane::NodeImport,
            ModuleRequestKind::Require => PackageSurfaceLane::NodeRequire,
            ModuleRequestKind::StaticImport => match settings.static_condition {
                PackageConditionMode::Import => PackageSurfaceLane::NodeImport,
                PackageConditionMode::Require => PackageSurfaceLane::NodeRequire,
            },
        },
    }
}

pub(super) fn resolve_request(
    context: &PackageContext<'_>,
    request: ResolutionRequest<'_>,
) -> PackageResolution {
    if request.import_kind == ImportKind::SideEffect
        && let Some(result) = reject_applicable_unsupported_fields(
            context,
            request.specifier,
            &[PackageFieldApplicability::SideEffectReachability],
        )
    {
        return result;
    }
    if request.lane != PackageSurfaceLane::LegacyNode
        && let Some(exports) = context.manifest.root.get("exports")
    {
        return exports::resolve(
            context,
            request.specifier,
            request.key,
            request.namespace,
            request.lane,
            exports,
        );
    }
    if request.key != "." {
        return resolve_legacy_subpath(context, request);
    }
    fallback::resolve(context, request)
}

fn resolve_legacy_subpath(
    context: &PackageContext<'_>,
    request: ResolutionRequest<'_>,
) -> PackageResolution {
    if request.namespace == SymbolNamespace::Type
        && let Some(result) = reject_applicable_unsupported_fields(
            context,
            request.specifier,
            &[PackageFieldApplicability::TypeFallbackWhenExportsAbsentOrNotConsulted],
        )
    {
        return result;
    }
    if request.namespace == SymbolNamespace::Value
        && request.lane == PackageSurfaceLane::BundlerImport
        && let Some(result) = reject_applicable_unsupported_fields(
            context,
            request.specifier,
            &[
                PackageFieldApplicability::BundlerValueWhenExportsAbsentOrNotConsulted,
                PackageFieldApplicability::BundlerValueFallbackWhenExportsAbsent,
            ],
        )
    {
        return result;
    }
    let base = match exports::lower_target(&context.package.root, request.key, None) {
        Ok(base) => base,
        Err(detail) => return unsupported(context.package, request.specifier, &detail),
    };
    let allow_extensionless = fallback::lane_allows_extensionless(request.lane);
    let mut result = resolve_base(
        context.package,
        TargetRequest {
            specifier: request.specifier,
            namespace: request.namespace,
            source: PackageSurfaceSource::DirectoryIndex { lane: request.lane },
            base,
            allow_extensionless,
            allow_directory: allow_extensionless,
        },
        context.sources,
        context.physical_path_redirects,
    );
    result.declaration = None;
    result
}

pub(super) fn package_field_policy(
    path: &str,
) -> Option<&'static crate::generated_config_policy::FieldPolicy> {
    RESOLVER_PACKAGE_JSON_FIELDS
        .iter()
        .find(|policy| policy.path == path)
}

pub(super) fn reject_applicable_unsupported_fields(
    context: &PackageContext<'_>,
    specifier: &str,
    applicability: &[PackageFieldApplicability],
) -> Option<PackageResolution> {
    RESOLVER_PACKAGE_FIELD_APPLICABILITY
        .iter()
        .filter(|policy| applicability.contains(&policy.applicability))
        .find_map(|applicability_policy| {
            let policy = package_field_policy(applicability_policy.path)?;
            (policy.classification == FieldClassification::UnsupportedResolutionAffecting
                && context.manifest.root.get(policy.path).is_some())
            .then(|| {
                unsupported(
                    context.package,
                    specifier,
                    &format!("package {} semantics are unsupported", policy.path),
                )
            })
        })
}

pub(super) fn resolve_base(
    package: &PackageFact,
    request: TargetRequest<'_>,
    sources: &BTreeMap<RepoPath, LogicalSourceId>,
    physical_path_redirects: &[PhysicalPathRedirect],
) -> PackageResolution {
    if let Err(detail) =
        verify_physical_package_containment(&package.root, &request.base, physical_path_redirects)
    {
        return unsupported(package, request.specifier, &detail);
    }
    let mut paths = candidates(
        &request.base,
        request.namespace,
        request.allow_extensionless,
    );
    if request.allow_directory
        && let Ok(index) = request.base.join_portable("index.js")
    {
        paths.extend(candidates(&index, request.namespace, true));
    }
    paths.dedup();
    for path in &paths {
        if let Err(detail) =
            verify_physical_package_containment(&package.root, path, physical_path_redirects)
        {
            return unsupported(package, request.specifier, &detail);
        }
        if let Some(target) = sources.get(path) {
            return PackageResolution {
                outcome: ResolutionOutcome::Internal {
                    target: target.clone(),
                },
                limitation: None,
                declaration: Some(PackageSurfaceDeclaration {
                    package_root: package.root.clone(),
                    manifest_path: package.manifest_path.clone(),
                    request: request.specifier.to_owned(),
                    namespace: request.namespace,
                    source: request.source,
                    target: target.clone(),
                }),
            };
        }
    }
    unresolved(
        request.specifier,
        paths.iter().map(RepoPath::display_escaped).collect(),
    )
}

pub(super) fn verify_physical_package_containment(
    package_root: &RepoPath,
    candidate: &RepoPath,
    physical_path_redirects: &[PhysicalPathRedirect],
) -> Result<(), String> {
    let mut current = candidate.clone();
    let mut visited = BTreeSet::new();
    loop {
        reject_hard_excluded_package_target(package_root, &current)?;
        if !visited.insert(current.clone()) {
            return Err("package target physical redirect cycle is unsupported".to_owned());
        }
        let Some(redirect) = physical_path_redirects
            .iter()
            .filter(|redirect| current.is_within(&redirect.path))
            .max_by_key(|redirect| redirect.path.components_len())
        else {
            return current
                .is_within(package_root)
                .then_some(())
                .ok_or_else(|| "package target physically escapes the package root".to_owned());
        };
        let target = match &redirect.target {
            PhysicalPathRedirectTarget::Repository(target) => target,
            PhysicalPathRedirectTarget::OutsideRepository => {
                return Err("package target physically escapes the repository root".to_owned());
            }
            PhysicalPathRedirectTarget::Unavailable => {
                return Err("package target physical containment is unavailable".to_owned());
            }
        };
        if !target.is_within(package_root) {
            return Err("package target physically escapes the package root".to_owned());
        }
        let suffix = current
            .portable_relative_to(&redirect.path)
            .ok_or_else(|| "package target physical suffix is not portable".to_owned())?;
        let mut resolved = target.clone();
        for component in suffix.split('/').filter(|component| !component.is_empty()) {
            resolved = resolved
                .join_portable(component)
                .map_err(|_| "package target physical suffix is invalid".to_owned())?;
        }
        current = resolved;
    }
}

fn reject_hard_excluded_package_target(
    package_root: &RepoPath,
    candidate: &RepoPath,
) -> Result<(), String> {
    let relative = candidate
        .portable_relative_to(package_root)
        .ok_or_else(|| {
            "package target is not representable relative to the package root".to_owned()
        })?;
    if relative
        .split('/')
        .any(|component| matches!(component, ".git" | ".lumin" | "node_modules"))
    {
        return Err("package target enters a hard-excluded source namespace".to_owned());
    }
    Ok(())
}

pub(super) fn unresolved(specifier: &str, candidates: Vec<String>) -> PackageResolution {
    PackageResolution {
        outcome: ResolutionOutcome::Unresolved {
            specifier: specifier.to_owned(),
            candidates,
            target_scope: Some(UnresolvedTargetScope::ExplicitTargets),
        },
        limitation: None,
        declaration: None,
    }
}

pub(super) fn unresolved_no_target(specifier: &str) -> PackageResolution {
    PackageResolution {
        outcome: ResolutionOutcome::Unresolved {
            specifier: specifier.to_owned(),
            candidates: Vec::new(),
            target_scope: Some(UnresolvedTargetScope::KnownNoTarget {
                package: crate::package_name(specifier),
            }),
        },
        limitation: None,
        declaration: None,
    }
}

pub(super) fn unsupported(
    package: &PackageFact,
    specifier: &str,
    detail: &str,
) -> PackageResolution {
    PackageResolution {
        outcome: ResolutionOutcome::Unsupported {
            specifier: specifier.to_owned(),
            reason: detail.to_owned(),
        },
        limitation: Some(Limitation::PublicSurfaceUnsupported {
            path: package.manifest_path.display_escaped(),
            detail: detail.to_owned(),
        }),
        declaration: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_redirects_must_remain_inside_the_package() -> Result<(), Box<dyn std::error::Error>>
    {
        let package = RepoPath::from_portable("packages/lib")?;
        let candidate = RepoPath::from_portable("packages/lib/link/index.ts")?;
        let link = RepoPath::from_portable("packages/lib/link")?;

        let contained = PhysicalPathRedirect {
            path: link.clone(),
            target: PhysicalPathRedirectTarget::Repository(RepoPath::from_portable(
                "packages/lib/real",
            )?),
            kind: lumin_model::PhysicalPathRedirectKind::Directory,
            entry_physical_identity: None,
            target_physical_identity: None,
            target_identity_sha256: "contained".to_owned(),
        };
        assert!(verify_physical_package_containment(&package, &candidate, &[contained]).is_ok());

        for target in [
            PhysicalPathRedirectTarget::Repository(RepoPath::from_portable("shared")?),
            PhysicalPathRedirectTarget::OutsideRepository,
            PhysicalPathRedirectTarget::Unavailable,
        ] {
            let redirect = PhysicalPathRedirect {
                path: link.clone(),
                target,
                kind: lumin_model::PhysicalPathRedirectKind::Directory,
                entry_physical_identity: None,
                target_physical_identity: None,
                target_identity_sha256: "rejected".to_owned(),
            };
            assert!(
                verify_physical_package_containment(&package, &candidate, &[redirect]).is_err()
            );
        }
        Ok(())
    }
}
