use lumin_model::{PackageSurfaceLane, PackageSurfaceSource, ResolutionOutcome, SymbolNamespace};

use crate::generated_config_policy::{FieldClassification, PackageFieldApplicability};

use super::{
    PackageContext, PackageResolution, ResolutionRequest, TargetRequest, package_field_policy,
    reject_applicable_unsupported_fields, resolve_base, unresolved, unsupported,
};

pub(super) fn resolve(
    context: &PackageContext<'_>,
    request: ResolutionRequest<'_>,
) -> PackageResolution {
    if request.namespace == SymbolNamespace::Type {
        return resolve_type(context, request);
    }
    if request.lane == PackageSurfaceLane::BundlerImport
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
    resolve_value(context, request)
}

fn resolve_value(
    context: &PackageContext<'_>,
    request: ResolutionRequest<'_>,
) -> PackageResolution {
    let mut consulted = Vec::new();
    let result = if request.lane == PackageSurfaceLane::BundlerImport {
        resolve_manifest_fields(
            context,
            request,
            &[
                (
                    "module",
                    PackageSurfaceSource::Module { lane: request.lane },
                ),
                ("main", PackageSurfaceSource::Main { lane: request.lane }),
            ],
            &mut consulted,
        )
    } else {
        resolve_manifest_fields(
            context,
            request,
            &[("main", PackageSurfaceSource::Main { lane: request.lane })],
            &mut consulted,
        )
    };
    if let Some(result) = result {
        return result;
    }
    if let Some(result) = resolve_index(
        context,
        request,
        PackageSurfaceSource::DirectoryIndex { lane: request.lane },
        &mut consulted,
    ) {
        return result;
    }
    unresolved(request.specifier, consulted)
}

fn resolve_type(context: &PackageContext<'_>, request: ResolutionRequest<'_>) -> PackageResolution {
    if let Some(result) = reject_applicable_unsupported_fields(
        context,
        request.specifier,
        &[PackageFieldApplicability::TypeFallbackWhenExportsAbsentOrNotConsulted],
    ) {
        return result;
    }
    let mut consulted = Vec::new();
    if let Some(result) = resolve_manifest_fields(
        context,
        request,
        &[
            (
                "typings",
                PackageSurfaceSource::Typings { lane: request.lane },
            ),
            ("types", PackageSurfaceSource::Types { lane: request.lane }),
        ],
        &mut consulted,
    ) {
        return result;
    }
    let result = if request.lane == PackageSurfaceLane::BundlerImport {
        resolve_manifest_fields(
            context,
            request,
            &[
                (
                    "module",
                    PackageSurfaceSource::DeclarationCompanion { lane: request.lane },
                ),
                (
                    "main",
                    PackageSurfaceSource::DeclarationCompanion { lane: request.lane },
                ),
            ],
            &mut consulted,
        )
    } else {
        resolve_manifest_fields(
            context,
            request,
            &[(
                "main",
                PackageSurfaceSource::DeclarationCompanion { lane: request.lane },
            )],
            &mut consulted,
        )
    };
    if let Some(result) = result {
        return result;
    }
    if let Some(result) = resolve_index(
        context,
        request,
        PackageSurfaceSource::DeclarationCompanion { lane: request.lane },
        &mut consulted,
    ) {
        return result;
    }
    unresolved(request.specifier, consulted)
}

fn resolve_manifest_fields(
    context: &PackageContext<'_>,
    request: ResolutionRequest<'_>,
    fields: &[(&str, PackageSurfaceSource)],
    consulted: &mut Vec<String>,
) -> Option<PackageResolution> {
    for (field, source) in fields {
        let Some(policy) = package_field_policy(field) else {
            return Some(unsupported(
                context.package,
                request.specifier,
                &format!("package {field} has no generated resolver policy"),
            ));
        };
        if policy.classification != FieldClassification::SupportedAndModeled {
            return Some(unsupported(
                context.package,
                request.specifier,
                &format!("package {field} is not a modeled resolver field"),
            ));
        }
        let Some(value) = context.manifest.root.get(field) else {
            continue;
        };
        let Some(value) = value.as_str().filter(|value| !value.is_empty()) else {
            return Some(unsupported(
                context.package,
                request.specifier,
                &format!("package {field} field must be a nonempty string"),
            ));
        };
        let base = match super::exports::lower_field_target(&context.package.root, value) {
            Ok(base) => base,
            Err(detail) => {
                return Some(unsupported(context.package, request.specifier, &detail));
            }
        };
        let result = resolve_base(
            context.package,
            TargetRequest {
                specifier: request.specifier,
                namespace: request.namespace,
                source: source.clone(),
                base,
                allow_extensionless: lane_allows_extensionless(request.lane),
                allow_directory: lane_allows_extensionless(request.lane),
            },
            context.sources,
            context.physical_path_redirects,
        );
        if let Some(result) = accept_or_collect(result, consulted) {
            return Some(result);
        }
    }
    None
}

fn resolve_index(
    context: &PackageContext<'_>,
    request: ResolutionRequest<'_>,
    source: PackageSurfaceSource,
    consulted: &mut Vec<String>,
) -> Option<PackageResolution> {
    if !lane_allows_extensionless(request.lane) {
        return None;
    }
    let Ok(base) = context.package.root.join_portable("index.js") else {
        return Some(unsupported(
            context.package,
            request.specifier,
            "package root cannot form index.js",
        ));
    };
    let result = resolve_base(
        context.package,
        TargetRequest {
            specifier: request.specifier,
            namespace: request.namespace,
            source,
            base,
            allow_extensionless: true,
            allow_directory: false,
        },
        context.sources,
        context.physical_path_redirects,
    );
    accept_or_collect(result, consulted)
}

fn accept_or_collect(
    result: PackageResolution,
    consulted: &mut Vec<String>,
) -> Option<PackageResolution> {
    match result.outcome {
        ResolutionOutcome::Unresolved { candidates, .. } => {
            consulted.extend(candidates);
            None
        }
        _ => Some(result),
    }
}

pub(super) fn lane_allows_extensionless(lane: PackageSurfaceLane) -> bool {
    !matches!(lane, PackageSurfaceLane::NodeImport)
}
