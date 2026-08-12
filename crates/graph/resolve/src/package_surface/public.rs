use std::collections::{BTreeMap, BTreeSet};

use lumin_model::{
    ConfigDocument, ConfigValue, ImportKind, Limitation, LogicalSourceId, PackageFact,
    PackageIdentityState, PackagePrivacy, PackageSurfaceLane, PhysicalPathRedirect, RepoPath,
    SemanticConfigSnapshot, SourceSnapshot, SymbolNamespace,
};

use super::{
    PackageContext, PublicSurfaceOutput, ResolutionRequest, package_manifest, resolve_request,
    verify_physical_package_containment,
};

pub(super) fn collect(
    sources: &[SourceSnapshot],
    physical_path_redirects: &[PhysicalPathRedirect],
    source_by_path: &BTreeMap<RepoPath, LogicalSourceId>,
    config: &SemanticConfigSnapshot,
) -> PublicSurfaceOutput {
    let mut output = PublicSurfaceOutput::default();
    for package in &config.packages {
        if !matches!(
            package.privacy,
            PackagePrivacy::Public | PackagePrivacy::Unspecified
        ) {
            continue;
        }
        let PackageIdentityState::Valid(_) = &package.identity else {
            continue;
        };
        let Some(manifest) = package_manifest(package, config) else {
            continue;
        };
        let context = PackageContext {
            package,
            manifest,
            sources: source_by_path,
            physical_path_redirects,
        };
        for lane in [
            PackageSurfaceLane::BundlerImport,
            PackageSurfaceLane::LegacyNode,
            PackageSurfaceLane::NodeImport,
            PackageSurfaceLane::NodeRequire,
        ] {
            for namespace in [SymbolNamespace::Value, SymbolNamespace::Type] {
                let requests = public_requests(
                    package,
                    manifest,
                    lane,
                    namespace,
                    sources,
                    physical_path_redirects,
                );
                output
                    .limitations
                    .extend(requests.physical_errors.into_iter().map(|detail| {
                        Limitation::PublicSurfaceUnsupported {
                            path: package.manifest_path.display_escaped(),
                            detail,
                        }
                    }));
                for request in requests.items {
                    let result = resolve_request(
                        &context,
                        ResolutionRequest {
                            specifier: &request.specifier,
                            key: &request.key,
                            namespace,
                            import_kind: ImportKind::Named,
                            lane,
                        },
                    );
                    if let Some(declaration) = result.declaration {
                        output.declarations.push(declaration);
                    }
                    if let Some(limitation) = result.limitation {
                        output.limitations.push(limitation);
                    }
                }
            }
        }
    }
    output.declarations.sort();
    output.declarations.dedup();
    output
}

struct PublicRequest {
    specifier: String,
    key: String,
}

#[derive(Default)]
struct PublicRequests {
    items: Vec<PublicRequest>,
    physical_errors: BTreeSet<String>,
}

fn public_requests(
    package: &PackageFact,
    manifest: &ConfigDocument,
    lane: PackageSurfaceLane,
    namespace: SymbolNamespace,
    sources: &[SourceSnapshot],
    physical_path_redirects: &[PhysicalPathRedirect],
) -> PublicRequests {
    let PackageIdentityState::Valid(identity) = &package.identity else {
        return PublicRequests::default();
    };
    let mut keys = BTreeSet::from([".".to_owned()]);
    let mut physical_errors = BTreeSet::new();
    if lane != PackageSurfaceLane::LegacyNode
        && let Some(ConfigValue::Object(entries)) = manifest.root.get("exports")
        && matches!(
            super::exports::object_kind(entries),
            Ok(super::exports::ObjectKind::Subpaths)
        )
    {
        for entry in entries {
            if !entry.key.contains('*') {
                keys.insert(entry.key.clone());
                continue;
            }
            let pattern = pattern_public_keys(
                package,
                entry,
                lane,
                namespace,
                sources,
                physical_path_redirects,
            );
            keys.extend(pattern.keys);
            physical_errors.extend(pattern.physical_errors);
        }
    }
    let items = keys
        .into_iter()
        .filter_map(|key| {
            let specifier = if key == "." {
                identity.as_str().to_owned()
            } else {
                format!("{}/{}", identity.as_str(), key.strip_prefix("./")?)
            };
            Some(PublicRequest { specifier, key })
        })
        .collect();
    PublicRequests {
        items,
        physical_errors,
    }
}

#[derive(Default)]
struct PatternPublicKeys {
    keys: BTreeSet<String>,
    physical_errors: BTreeSet<String>,
}

fn pattern_public_keys(
    package: &PackageFact,
    entry: &lumin_model::ConfigEntry,
    lane: PackageSurfaceLane,
    namespace: SymbolNamespace,
    sources: &[SourceSnapshot],
    physical_path_redirects: &[PhysicalPathRedirect],
) -> PatternPublicKeys {
    let mut output = PatternPublicKeys::default();
    let Ok(Some(selected)) = super::exports::select_subpath_value(&entry.value, lane, namespace)
    else {
        return output;
    };
    let Some(target) = selected.target else {
        return output;
    };
    if !target.contains('*') {
        output
            .keys
            .insert(entry.key.replacen('*', "lumin-pattern", 1));
        return output;
    }
    output.physical_errors.extend(pattern_physical_errors(
        package,
        &target,
        physical_path_redirects,
    ));
    for source in sources {
        let Some(relative) = source.path.portable_relative_to(&package.root) else {
            continue;
        };
        for host in host_variants(
            &relative,
            namespace,
            super::fallback::lane_allows_extensionless(lane),
        ) {
            let candidate = format!("./{host}");
            let Some(capture) = super::exports::pattern_capture(&target, &candidate) else {
                continue;
            };
            let key = entry.key.replacen('*', &capture, 1);
            if super::exports::validate_subpath_key(&key).is_ok() {
                output.keys.insert(key);
            }
        }
    }
    output
}

fn pattern_physical_errors(
    package: &PackageFact,
    target: &str,
    physical_path_redirects: &[PhysicalPathRedirect],
) -> BTreeSet<String> {
    let Some((_, suffix)) = target.split_once('*') else {
        return BTreeSet::new();
    };
    let mut errors = BTreeSet::new();
    for redirect in physical_path_redirects
        .iter()
        .filter(|redirect| redirect.path.is_within(&package.root))
    {
        let Some(relative) = redirect.path.portable_relative_to(&package.root) else {
            continue;
        };
        let candidates = [
            format!("./{relative}"),
            format!("./{relative}/lumin-pattern{suffix}"),
        ];
        for candidate in candidates {
            let Some(capture) = super::exports::pattern_capture(target, &candidate) else {
                continue;
            };
            let Ok(lowered) = super::exports::lower_target(&package.root, target, Some(&capture))
            else {
                continue;
            };
            if let Err(detail) = verify_physical_package_containment(
                &package.root,
                &lowered,
                physical_path_redirects,
            ) {
                errors.insert(detail);
            }
        }
    }
    errors
}

fn host_variants(
    path: &str,
    namespace: SymbolNamespace,
    allow_extensionless: bool,
) -> BTreeSet<String> {
    let mut variants = BTreeSet::from([path.to_owned()]);
    let mappings: &[(&str, &[&str])] = match namespace {
        SymbolNamespace::Value => &[
            (".tsx", &[".js", ".jsx"]),
            (".ts", &[".js"]),
            (".jsx", &[".js"]),
            (".mts", &[".mjs"]),
            (".cts", &[".cjs"]),
        ],
        SymbolNamespace::Type => &[
            (".d.mts", &[".mjs"]),
            (".d.cts", &[".cjs"]),
            (".d.ts", &[".js", ".jsx"]),
            (".tsx", &[".js", ".jsx"]),
            (".ts", &[".js"]),
            (".mts", &[".mjs"]),
            (".cts", &[".cjs"]),
        ],
    };
    for (source_extension, host_extensions) in mappings {
        let Some(stem) = path.strip_suffix(source_extension) else {
            continue;
        };
        variants.extend(
            host_extensions
                .iter()
                .map(|extension| format!("{stem}{extension}")),
        );
        if allow_extensionless && matches!(*source_extension, ".ts" | ".tsx" | ".d.ts") {
            variants.insert(stem.to_owned());
        }
        break;
    }
    variants
}
