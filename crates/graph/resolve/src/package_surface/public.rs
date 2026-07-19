use std::collections::{BTreeMap, BTreeSet};

use lumin_model::{
    ConfigDocument, ConfigValue, ImportKind, LogicalSourceId, PackageFact, PackageIdentityState,
    PackagePrivacy, PackageSurfaceLane, RepoPath, SemanticConfigSnapshot, SourceSnapshot,
    SymbolNamespace,
};

use super::{
    PackageContext, PublicSurfaceOutput, ResolutionRequest, package_manifest, resolve_request,
};

pub(super) fn collect(
    sources: &[SourceSnapshot],
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
        };
        for lane in [
            PackageSurfaceLane::BundlerImport,
            PackageSurfaceLane::LegacyNode,
            PackageSurfaceLane::NodeImport,
            PackageSurfaceLane::NodeRequire,
        ] {
            for namespace in [SymbolNamespace::Value, SymbolNamespace::Type] {
                for request in public_requests(package, manifest, lane, namespace, sources) {
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

fn public_requests(
    package: &PackageFact,
    manifest: &ConfigDocument,
    lane: PackageSurfaceLane,
    namespace: SymbolNamespace,
    sources: &[SourceSnapshot],
) -> Vec<PublicRequest> {
    let PackageIdentityState::Valid(identity) = &package.identity else {
        return Vec::new();
    };
    let mut keys = BTreeSet::from([".".to_owned()]);
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
            keys.extend(pattern_public_keys(
                package, entry, lane, namespace, sources,
            ));
        }
    }
    keys.into_iter()
        .filter_map(|key| {
            let specifier = if key == "." {
                identity.as_str().to_owned()
            } else {
                format!("{}/{}", identity.as_str(), key.strip_prefix("./")?)
            };
            Some(PublicRequest { specifier, key })
        })
        .collect()
}

fn pattern_public_keys(
    package: &PackageFact,
    entry: &lumin_model::ConfigEntry,
    lane: PackageSurfaceLane,
    namespace: SymbolNamespace,
    sources: &[SourceSnapshot],
) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    let Ok(Some(selected)) = super::exports::select_subpath_value(&entry.value, lane, namespace)
    else {
        return keys;
    };
    let Some(target) = selected.target else {
        return keys;
    };
    if !target.contains('*') {
        keys.insert(entry.key.replacen('*', "lumin-pattern", 1));
        return keys;
    }
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
                keys.insert(key);
            }
        }
    }
    keys
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
