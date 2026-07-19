mod exports;
mod fallback;
mod public;

use std::collections::BTreeMap;

use lumin_model::{
    ConfigDocument, ConfigObservation, ImportKind, Limitation, LogicalSourceId, ModuleRequestKind,
    PackageFact, PackageIdentityState, PackagePrivacy, PackageSurfaceDeclaration,
    PackageSurfaceLane, PackageSurfaceSource, RepoPath, ResolutionOutcome, ResolutionProfile,
    SemanticConfigSnapshot, SourceSnapshot, SourceUseFact, SymbolNamespace,
};

use crate::candidates;
use crate::config::{ImporterSettings, PackageConditionMode};

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
    source_by_path: &BTreeMap<RepoPath, LogicalSourceId>,
    config: &SemanticConfigSnapshot,
) -> PublicSurfaceOutput {
    public::collect(sources, source_by_path, config)
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
        ConfigObservation::Present(document) => Some(document),
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
        && context.manifest.root.get("sideEffects").is_some()
    {
        return unsupported(
            context.package,
            request.specifier,
            "package sideEffects semantics are unsupported",
        );
    }
    if request.lane != PackageSurfaceLane::LegacyNode
        && let Some(exports) = context.manifest.root.get("exports")
    {
        return exports::resolve(
            context.package,
            request.specifier,
            request.key,
            request.namespace,
            request.lane,
            exports,
            context.sources,
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
        && context.manifest.root.get("typesVersions").is_some()
    {
        return unsupported(
            context.package,
            request.specifier,
            "package typesVersions semantics are unsupported",
        );
    }
    if request.namespace == SymbolNamespace::Value
        && request.lane == PackageSurfaceLane::BundlerImport
    {
        for field in ["browser", "react-native"] {
            if context.manifest.root.get(field).is_some() {
                return unsupported(
                    context.package,
                    request.specifier,
                    &format!("package {field} semantics are unsupported"),
                );
            }
        }
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
    );
    result.declaration = None;
    result
}

pub(super) fn resolve_base(
    package: &PackageFact,
    request: TargetRequest<'_>,
    sources: &BTreeMap<RepoPath, LogicalSourceId>,
) -> PackageResolution {
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
    if let Some(target) = paths.iter().find_map(|path| sources.get(path)) {
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
    unresolved(
        request.specifier,
        paths.iter().map(RepoPath::display_escaped).collect(),
    )
}

pub(super) fn unresolved(specifier: &str, candidates: Vec<String>) -> PackageResolution {
    PackageResolution {
        outcome: ResolutionOutcome::Unresolved {
            specifier: specifier.to_owned(),
            candidates,
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
