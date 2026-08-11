//! Exact workspace dependency-surface and registry-location policy.

mod graph_snapshot;

use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use super::PRODUCTION_NAMES;

const POLICY_PATH: &str = "tools/xtask/dependency-surface-policy.v1.json";
const REGISTRY_SOURCE: &str = "registry+https://github.com/rust-lang/crates.io-index";
const OWNER_DEPS: &[(&str, &str)] = &[
    ("redb", "lumin-store"),
    ("oxc_allocator", "lumin-js"),
    ("oxc_ast_visit", "lumin-js"),
    ("oxc_ast", "lumin-js"),
    ("oxc_parser", "lumin-js"),
    ("oxc_span", "lumin-js"),
];

pub(super) fn validate_dependency_surface(
    metadata: &serde_json::Value,
    workspace_root: &Path,
    violations: &mut Vec<String>,
) -> Result<(), String> {
    let observed = build_observed_policy(metadata, workspace_root, violations)?;
    compare_checked_policy(workspace_root, &observed, violations)?;
    validate_registry_locations(metadata, workspace_root, violations)
}

fn build_observed_policy(
    metadata: &serde_json::Value,
    workspace_root: &Path,
    violations: &mut Vec<String>,
) -> Result<serde_json::Value, String> {
    // The isolated Python guard strict-parses the semantic TOML value immediately before
    // metadata acquisition. Cargo metadata does not expose the workspace resolver.
    let resolver = "3";
    let packages = required_array(metadata, "packages", "metadata")?;
    let member_values = required_array(metadata, "workspace_members", "metadata")?;
    let resolve = required_field(metadata, "resolve", "metadata")?;
    let nodes = required_array(resolve, "nodes", "metadata.resolve")?;

    let mut package_by_id = HashMap::new();
    for package in packages {
        let id = required_string(package, "id", "metadata package")?;
        if package_by_id.insert(id.to_owned(), package).is_some() {
            return Err(format!("duplicate package id in cargo metadata: {id}"));
        }
    }

    let mut member_ids = BTreeSet::new();
    for value in member_values {
        let id = value
            .as_str()
            .ok_or_else(|| "workspace member id is not a string".to_owned())?;
        if !member_ids.insert(id.to_owned()) {
            violations.push(format!("DUPLICATE workspace member id: {id}"));
        }
    }

    let mut node_by_id = HashMap::new();
    for node in nodes {
        let id = required_string(node, "id", "resolve node")?;
        if node_by_id.insert(id.to_owned(), node).is_some() {
            return Err(format!("duplicate resolve node id: {id}"));
        }
    }

    let graph_snapshot = graph_snapshot::build(
        packages,
        nodes,
        required_field(resolve, "root", "metadata.resolve")?,
        &package_by_id,
        &member_ids,
        workspace_root,
        violations,
    )?;

    let mut member_packages = member_ids
        .iter()
        .map(|id| {
            package_by_id
                .get(id)
                .copied()
                .ok_or_else(|| format!("workspace member package is missing: {id}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    member_packages
        .sort_by(|left, right| string_or_empty(left, "name").cmp(string_or_empty(right, "name")));

    let mut consumed = BTreeSet::new();
    let mut policy_packages = Vec::new();
    for package in member_packages {
        let owner_id = required_string(package, "id", "workspace package")?;
        let owner = required_string(package, "name", "workspace package")?;
        let node = node_by_id
            .get(owner_id)
            .copied()
            .ok_or_else(|| format!("resolve node missing for workspace member {owner}"))?;
        let resolve_deps = required_array(node, "deps", &format!("resolve node {owner}"))?;

        let features = canonical_feature_policy(package, owner)?;
        let declarations = required_array(
            package,
            "dependencies",
            &format!("workspace package {owner}"),
        )?;
        let mut dependency_policies = Vec::new();
        for declaration in declarations {
            dependency_policies.push(build_dependency_policy(
                owner,
                owner_id,
                declaration,
                resolve_deps,
                &package_by_id,
                &member_ids,
                &mut consumed,
                violations,
            )?);
        }
        dependency_policies.sort_by_key(dependency_sort_key);
        validate_unconsumed_resolutions(owner, owner_id, resolve_deps, &consumed, violations)?;

        let class = if owner == "lumin-xtask" {
            "development-tool"
        } else if PRODUCTION_NAMES.contains(&owner) {
            "production"
        } else {
            violations.push(format!("UNKNOWN workspace package policy class: {owner}"));
            "unknown"
        };
        policy_packages.push(serde_json::json!({
            "name": owner,
            "class": class,
            "definition": workspace_package_definition(package, workspace_root)?,
            "features": features,
            "dependencies": dependency_policies,
        }));
    }
    policy_packages
        .sort_by(|left, right| string_or_empty(left, "name").cmp(string_or_empty(right, "name")));

    Ok(serde_json::json!({
        "schemaVersion": 1,
        "workspaceResolver": resolver,
        "cargoLockSha256": file_sha256(&workspace_root.join("Cargo.lock"))?,
        "packageDefinitions": graph_snapshot.package_definitions,
        "resolvedGraph": graph_snapshot.resolved_graph,
        "packages": policy_packages,
    }))
}

fn workspace_package_definition(
    package: &serde_json::Value,
    workspace_root: &Path,
) -> Result<serde_json::Value, String> {
    let name = required_string(package, "name", "workspace package")?;
    let version = required_string(package, "version", "workspace package")?;
    let manifest = stable_workspace_path(
        required_string(package, "manifest_path", "workspace package")?,
        workspace_root,
        &format!("workspace package {name} manifest"),
    )?;
    let readme = stable_optional_workspace_path(
        required_field(package, "readme", "workspace package")?,
        workspace_root,
        &format!("workspace package {name} readme"),
    )?;
    let license_file = stable_optional_workspace_path(
        required_field(package, "license_file", "workspace package")?,
        workspace_root,
        &format!("workspace package {name} license file"),
    )?;

    Ok(serde_json::json!({
        "identity": {
            "name": name,
            "version": version,
            "manifest": manifest,
        },
        "edition": required_field(package, "edition", "workspace package")?.clone(),
        "rustVersion": required_field(package, "rust_version", "workspace package")?.clone(),
        "authors": required_field(package, "authors", "workspace package")?.clone(),
        "description": required_field(package, "description", "workspace package")?.clone(),
        "homepage": required_field(package, "homepage", "workspace package")?.clone(),
        "documentation": required_field(package, "documentation", "workspace package")?.clone(),
        "readme": readme,
        "keywords": required_field(package, "keywords", "workspace package")?.clone(),
        "categories": required_field(package, "categories", "workspace package")?.clone(),
        "license": required_field(package, "license", "workspace package")?.clone(),
        "licenseFile": license_file,
        "repository": required_field(package, "repository", "workspace package")?.clone(),
        "links": required_field(package, "links", "workspace package")?.clone(),
        "publish": required_field(package, "publish", "workspace package")?.clone(),
        "defaultRun": required_field(package, "default_run", "workspace package")?.clone(),
        "metadata": required_field(package, "metadata", "workspace package")?.clone(),
    }))
}

fn stable_workspace_path(
    raw: &str,
    workspace_root: &Path,
    context: &str,
) -> Result<String, String> {
    let path = Path::new(raw);
    let canonical_root = std::fs::canonicalize(workspace_root).map_err(|error| {
        format!(
            "cannot canonicalize workspace root {}: {error}",
            workspace_root.display()
        )
    })?;
    let canonical = std::fs::canonicalize(path)
        .map_err(|error| format!("cannot canonicalize {context} {}: {error}", path.display()))?;
    if !canonical.starts_with(&canonical_root) {
        return Err(format!(
            "{context} escapes workspace {}: {}",
            canonical_root.display(),
            canonical.display()
        ));
    }
    Ok(super::relative_display(&canonical_root, &canonical))
}

fn stable_optional_workspace_path(
    value: &serde_json::Value,
    workspace_root: &Path,
    context: &str,
) -> Result<serde_json::Value, String> {
    match value {
        serde_json::Value::Null => Ok(serde_json::Value::Null),
        serde_json::Value::String(path) => Ok(serde_json::Value::String(stable_workspace_path(
            path,
            workspace_root,
            context,
        )?)),
        _ => Err(format!("{context} is neither null nor a string")),
    }
}

#[allow(clippy::too_many_arguments)]
fn build_dependency_policy(
    owner: &str,
    owner_id: &str,
    declaration: &serde_json::Value,
    resolve_deps: &[serde_json::Value],
    package_by_id: &HashMap<String, &serde_json::Value>,
    member_ids: &BTreeSet<String>,
    consumed: &mut BTreeSet<(String, usize, usize)>,
    violations: &mut Vec<String>,
) -> Result<serde_json::Value, String> {
    let package_name = required_string(declaration, "name", "dependency declaration")?;
    let rename = optional_string(declaration, "rename", "dependency declaration")?;
    let requirement = required_string(declaration, "req", "dependency declaration")?;
    let kind = dependency_kind(
        required_field(declaration, "kind", "dependency declaration")?,
        violations,
    )?;
    let target = optional_string(declaration, "target", "dependency declaration")?;
    let optional = required_bool(declaration, "optional", "dependency declaration")?;
    let default_features = required_bool(
        declaration,
        "uses_default_features",
        "dependency declaration",
    )?;
    let features = canonical_string_array(
        required_field(declaration, "features", "dependency declaration")?,
        "dependency declaration features",
    )?;
    let expected_binding = rename.unwrap_or(package_name).replace('-', "_");

    let mut candidates = Vec::new();
    for (dep_index, resolved) in resolve_deps.iter().enumerate() {
        if required_string(resolved, "name", "resolved dependency")? != expected_binding {
            continue;
        }
        let target_id = required_string(resolved, "pkg", "resolved dependency")?;
        let target_package = package_by_id
            .get(target_id)
            .copied()
            .ok_or_else(|| format!("resolved dependency package is missing: {target_id}"))?;
        if required_string(target_package, "name", "resolved package")? != package_name {
            continue;
        }
        for (kind_index, resolved_kind) in
            required_array(resolved, "dep_kinds", "resolved dependency")?
                .iter()
                .enumerate()
        {
            let resolved_kind_name = dependency_kind(
                required_field(resolved_kind, "kind", "resolved dependency kind")?,
                violations,
            )?;
            let resolved_target =
                optional_string(resolved_kind, "target", "resolved dependency kind")?;
            if resolved_kind_name == kind && resolved_target == target {
                candidates.push((dep_index, kind_index, resolved, target_package));
            }
        }
    }

    let (dep_index, kind_index, resolved, target_package) = match candidates.as_slice() {
        [candidate] => *candidate,
        [] => {
            violations.push(format!(
                "MISSING dependency join: {owner} -> {package_name} ({kind}, target={target:?}, rename={rename:?})"
            ));
            return Ok(unresolved_policy(
                package_name,
                rename,
                requirement,
                kind,
                target,
                optional,
                default_features,
                features,
                &expected_binding,
            ));
        }
        _ => {
            violations.push(format!(
                "AMBIGUOUS dependency join: {owner} -> {package_name} ({kind}, target={target:?}, rename={rename:?}) has {} matches",
                candidates.len()
            ));
            return Ok(unresolved_policy(
                package_name,
                rename,
                requirement,
                kind,
                target,
                optional,
                default_features,
                features,
                &expected_binding,
            ));
        }
    };
    let consumption = (owner_id.to_owned(), dep_index, kind_index);
    if !consumed.insert(consumption) {
        violations.push(format!(
            "DUPLICATE dependency join consumption: {owner} -> {package_name} ({kind})"
        ));
    }

    let target_id = required_string(resolved, "pkg", "resolved dependency")?;
    let resolved_name = required_string(target_package, "name", "resolved package")?;
    let resolution = if member_ids.contains(target_id) {
        if resolved_name == "lumin-xtask" && PRODUCTION_NAMES.contains(&owner) {
            violations.push(format!(
                "FORBIDDEN: production crate {owner} depends on lumin-xtask"
            ));
        }
        serde_json::json!({
            "kind": "workspace",
            "package": resolved_name,
        })
    } else {
        let version = required_string(target_package, "version", "resolved package")?;
        let source = optional_string(target_package, "source", "resolved package")?;
        if source.is_none() {
            violations.push(format!(
                "NON-WORKSPACE PATH PACKAGE: {owner} -> {resolved_name} {version} has no source"
            ));
        }
        for (prefix, allowed_owner) in OWNER_DEPS {
            if resolved_name.starts_with(prefix) && owner != *allowed_owner {
                violations.push(format!(
                    "OWNER VIOLATION: {owner} uses third-party {resolved_name} but only {allowed_owner} may own it"
                ));
            }
        }
        serde_json::json!({
            "kind": "third-party",
            "package": resolved_name,
            "version": version,
            "source": source,
        })
    };

    Ok(serde_json::json!({
        "package": package_name,
        "rename": rename,
        "requirement": requirement,
        "kind": kind,
        "target": target,
        "optional": optional,
        "usesDefaultFeatures": default_features,
        "features": features,
        "binding": required_string(resolved, "name", "resolved dependency")?,
        "resolution": resolution,
    }))
}

#[allow(clippy::too_many_arguments)]
fn unresolved_policy(
    package_name: &str,
    rename: Option<&str>,
    requirement: &str,
    kind: &str,
    target: Option<&str>,
    optional: bool,
    default_features: bool,
    features: Vec<String>,
    binding: &str,
) -> serde_json::Value {
    serde_json::json!({
        "package": package_name,
        "rename": rename,
        "requirement": requirement,
        "kind": kind,
        "target": target,
        "optional": optional,
        "usesDefaultFeatures": default_features,
        "features": features,
        "binding": binding,
        "resolution": { "kind": "unresolved" },
    })
}

fn validate_unconsumed_resolutions(
    owner: &str,
    owner_id: &str,
    resolve_deps: &[serde_json::Value],
    consumed: &BTreeSet<(String, usize, usize)>,
    violations: &mut Vec<String>,
) -> Result<(), String> {
    for (dep_index, resolved) in resolve_deps.iter().enumerate() {
        let binding = required_string(resolved, "name", "resolved dependency")?;
        let kinds = required_array(resolved, "dep_kinds", "resolved dependency")?;
        if kinds.is_empty() {
            violations.push(format!(
                "EMPTY resolved dependency kinds: {owner} -> {binding}"
            ));
        }
        for kind_index in 0..kinds.len() {
            if !consumed.contains(&(owner_id.to_owned(), dep_index, kind_index)) {
                violations.push(format!(
                    "UNMATCHED resolved dependency: {owner} -> {binding} kind index {kind_index}"
                ));
            }
        }
    }
    Ok(())
}

fn canonical_feature_policy(
    package: &serde_json::Value,
    owner: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let object = required_object(
        required_field(package, "features", &format!("workspace package {owner}"))?,
        &format!("workspace package {owner} features"),
    )?;
    let mut features = Vec::new();
    for (name, activations) in object {
        features.push(serde_json::json!({
            "name": name,
            "activations": canonical_string_array(
                activations,
                &format!("workspace feature {owner}/{name}"),
            )?,
        }));
    }
    features
        .sort_by(|left, right| string_or_empty(left, "name").cmp(string_or_empty(right, "name")));
    Ok(features)
}

fn compare_checked_policy(
    workspace_root: &Path,
    observed: &serde_json::Value,
    violations: &mut Vec<String>,
) -> Result<(), String> {
    let path = workspace_root.join(POLICY_PATH);
    let bytes = std::fs::read(&path).map_err(|error| {
        format!(
            "cannot read checked dependency policy {}: {error}",
            path.display()
        )
    })?;
    let expected: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "cannot parse checked dependency policy {}: {error}",
            path.display()
        )
    })?;
    let expected = rust_policy_view(expected)?;
    if expected == *observed {
        return Ok(());
    }
    let expected_digest = json_digest(&expected)?;
    let observed_digest = json_digest(observed)?;
    let difference = first_difference(&expected, observed, "$")
        .unwrap_or_else(|| "unknown structural difference".to_owned());
    violations.push(format!(
        "DEPENDENCY SURFACE POLICY DRIFT: {difference}; expected digest {expected_digest}, observed {observed_digest}"
    ));
    Ok(())
}

fn rust_policy_view(mut policy: serde_json::Value) -> Result<serde_json::Value, String> {
    let root = policy
        .as_object_mut()
        .ok_or_else(|| "checked dependency policy root is not an object".to_owned())?;
    // The digest-pinned Python bootstrap owns strict authored-TOML comparison before Cargo.
    // Rust owns the independent Cargo metadata projection and must not pretend metadata can
    // reproduce source spelling or the root profile table that Cargo omits.
    root.remove("rootProfiles");
    root.remove("workspacePackage");
    let packages = root
        .get_mut("packages")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| "checked dependency policy packages is not an array".to_owned())?;
    for package in packages {
        package
            .as_object_mut()
            .ok_or_else(|| "checked dependency policy package is not an object".to_owned())?
            .remove("authoredPackage");
    }
    Ok(policy)
}

fn validate_registry_locations(
    metadata: &serde_json::Value,
    workspace_root: &Path,
    violations: &mut Vec<String>,
) -> Result<(), String> {
    let cargo_home = cargo_home()?;
    validate_registry_locations_under(metadata, workspace_root, &cargo_home, violations)
}

fn validate_registry_locations_under(
    metadata: &serde_json::Value,
    workspace_root: &Path,
    cargo_home: &Path,
    violations: &mut Vec<String>,
) -> Result<(), String> {
    let canonical_root = std::fs::canonicalize(workspace_root)
        .map_err(|error| format!("cannot canonicalize {}: {error}", workspace_root.display()))?;
    let lexical_home = absolute_path(cargo_home)?;
    let canonical_home = std::fs::canonicalize(&lexical_home).map_err(|error| {
        format!(
            "cannot canonicalize Cargo home {}: {error}",
            lexical_home.display()
        )
    })?;
    let lexical_registry = lexical_home.join("registry").join("src");
    let canonical_registry = std::fs::canonicalize(&lexical_registry).map_err(|error| {
        format!(
            "cannot canonicalize Cargo registry source root {}: {error}",
            lexical_registry.display()
        )
    })?;
    validate_unredirected_path(
        &lexical_home,
        &canonical_home,
        &lexical_registry,
        "Cargo registry source root",
        violations,
    )?;
    if !canonical_registry.starts_with(&canonical_home)
        || canonical_registry.starts_with(&canonical_root)
    {
        violations.push(format!(
            "CARGO REGISTRY ROOT ESCAPE: lexical={} physical={}",
            lexical_registry.display(),
            canonical_registry.display()
        ));
    }

    let member_values = required_array(metadata, "workspace_members", "metadata")?;
    let member_ids = member_values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "workspace member id is not a string".to_owned())
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    for package in required_array(metadata, "packages", "metadata")? {
        let id = required_string(package, "id", "metadata package")?;
        if member_ids.contains(id) {
            continue;
        }
        let name = required_string(package, "name", "metadata package")?;
        let version = required_string(package, "version", "metadata package")?;
        let source = optional_string(package, "source", "metadata package")?;
        if source != Some(REGISTRY_SOURCE) {
            violations.push(format!(
                "UNAPPROVED PACKAGE SOURCE: {name} {version} reports {source:?}"
            ));
        }
        let manifest = PathBuf::from(required_string(
            package,
            "manifest_path",
            "metadata package",
        )?);
        let is_lexically_contained =
            manifest.is_absolute() && manifest.starts_with(&lexical_registry);
        if !is_lexically_contained {
            violations.push(format!(
                "REGISTRY MANIFEST LEXICAL ESCAPE: {name} {version} at {}",
                manifest.display()
            ));
        }
        match std::fs::canonicalize(&manifest) {
            Ok(canonical_manifest) => {
                if is_lexically_contained {
                    validate_unredirected_path(
                        &lexical_registry,
                        &canonical_registry,
                        &manifest,
                        &format!("registry manifest {name} {version}"),
                        violations,
                    )?;
                }
                if !canonical_manifest.starts_with(&canonical_registry)
                    || canonical_manifest.starts_with(&canonical_root)
                {
                    violations.push(format!(
                        "REGISTRY MANIFEST PHYSICAL ESCAPE: {name} {version} lexical={} physical={}",
                        manifest.display(),
                        canonical_manifest.display()
                    ));
                }
            }
            Err(error) => violations.push(format!(
                "REGISTRY MANIFEST UNAVAILABLE: {name} {version} at {}: {error}",
                manifest.display()
            )),
        }
    }
    Ok(())
}

fn validate_unredirected_path(
    lexical_base: &Path,
    canonical_base: &Path,
    lexical_target: &Path,
    label: &str,
    violations: &mut Vec<String>,
) -> Result<(), String> {
    let relative = lexical_target.strip_prefix(lexical_base).map_err(|_| {
        format!(
            "{label} {} is not under lexical base {}",
            lexical_target.display(),
            lexical_base.display()
        )
    })?;
    let mut lexical_child = lexical_base.to_path_buf();
    let mut canonical_parent = canonical_base.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(format!(
                "{label} contains a non-canonical component: {}",
                lexical_target.display()
            ));
        };
        lexical_child.push(name);
        let canonical_child = std::fs::canonicalize(&lexical_child).map_err(|error| {
            format!(
                "cannot canonicalize {label} component {}: {error}",
                lexical_child.display()
            )
        })?;
        if canonical_child.parent() != Some(canonical_parent.as_path())
            || canonical_child.file_name() != Some(name)
        {
            violations.push(format!(
                "REGISTRY PATH REDIRECTION: {label} lexical={} physical={}",
                lexical_child.display(),
                canonical_child.display()
            ));
            return Ok(());
        }
        canonical_parent = canonical_child;
    }
    Ok(())
}

fn cargo_home() -> Result<PathBuf, String> {
    if let Some(configured) = std::env::var_os("CARGO_HOME") {
        return absolute_path(Path::new(&configured));
    }
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME");
    let home = home.ok_or_else(|| "cannot determine active Cargo home".to_owned())?;
    Ok(PathBuf::from(home).join(".cargo"))
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("cannot resolve current directory: {error}"))?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    let mut normal_depth = 0usize;
    for component in absolute.components() {
        match component {
            std::path::Component::Prefix(_)
            | std::path::Component::RootDir
            | std::path::Component::Normal(_) => {
                if matches!(component, std::path::Component::Normal(_)) {
                    normal_depth += 1;
                }
                normalized.push(component.as_os_str());
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if normal_depth == 0 || !normalized.pop() {
                    return Err(format!(
                        "path escapes the filesystem root: {}",
                        path.display()
                    ));
                }
                normal_depth -= 1;
            }
        }
    }
    if !normalized.is_absolute() {
        return Err(format!("path is not absolute: {}", path.display()));
    }
    Ok(normalized)
}

fn dependency_kind<'a>(
    value: &'a serde_json::Value,
    violations: &mut Vec<String>,
) -> Result<&'a str, String> {
    match value {
        serde_json::Value::Null => Ok("normal"),
        serde_json::Value::String(kind) if kind == "dev" || kind == "build" => Ok(kind),
        serde_json::Value::String(kind) => {
            violations.push(format!("UNKNOWN DEPENDENCY KIND: {kind}"));
            Ok(kind)
        }
        _ => Err("dependency kind is neither null nor a string".to_owned()),
    }
}

fn canonical_string_array(value: &serde_json::Value, context: &str) -> Result<Vec<String>, String> {
    let array = value
        .as_array()
        .ok_or_else(|| format!("{context} is not an array"))?;
    let mut values = array
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{context} contains a non-string"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    values.sort();
    values.dedup();
    Ok(values)
}

fn dependency_sort_key(value: &serde_json::Value) -> String {
    ["package", "rename", "kind", "target", "binding"]
        .iter()
        .map(|field| {
            value
                .get(field)
                .map(serde_json::Value::to_string)
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

fn json_digest(value: &serde_json::Value) -> Result<String, String> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| format!("cannot serialize dependency policy: {error}"))?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

fn file_sha256(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|error| {
        format!(
            "cannot read {} for dependency policy: {error}",
            path.display()
        )
    })?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

fn first_difference(
    expected: &serde_json::Value,
    observed: &serde_json::Value,
    path: &str,
) -> Option<String> {
    match (expected, observed) {
        (serde_json::Value::Object(left), serde_json::Value::Object(right)) => {
            let keys = left.keys().chain(right.keys()).collect::<BTreeSet<_>>();
            for key in keys {
                match (left.get(key), right.get(key)) {
                    (Some(left_value), Some(right_value)) => {
                        if let Some(difference) =
                            first_difference(left_value, right_value, &format!("{path}.{key}"))
                        {
                            return Some(difference);
                        }
                    }
                    (Some(_), None) => return Some(format!("{path}.{key} is missing")),
                    (None, Some(_)) => return Some(format!("{path}.{key} is unexpected")),
                    (None, None) => {}
                }
            }
            None
        }
        (serde_json::Value::Array(left), serde_json::Value::Array(right)) => {
            if left.len() != right.len() {
                return Some(format!(
                    "{path} length expected {} observed {}",
                    left.len(),
                    right.len()
                ));
            }
            for (index, (left_value, right_value)) in left.iter().zip(right).enumerate() {
                if let Some(difference) =
                    first_difference(left_value, right_value, &format!("{path}[{index}]"))
                {
                    return Some(difference);
                }
            }
            None
        }
        _ if expected == observed => None,
        _ => Some(format!(
            "{path} expected {} observed {}",
            display_value(expected),
            display_value(observed)
        )),
    }
}

fn display_value(value: &serde_json::Value) -> String {
    let rendered = value.to_string();
    if rendered.chars().count() <= 160 {
        rendered
    } else {
        format!("{}...", rendered.chars().take(160).collect::<String>())
    }
}

fn required_field<'a>(
    value: &'a serde_json::Value,
    field: &str,
    context: &str,
) -> Result<&'a serde_json::Value, String> {
    value
        .get(field)
        .ok_or_else(|| format!("{context} is missing {field}"))
}

fn required_array<'a>(
    value: &'a serde_json::Value,
    field: &str,
    context: &str,
) -> Result<&'a [serde_json::Value], String> {
    required_field(value, field, context)?
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| format!("{context}.{field} is not an array"))
}

fn required_object<'a>(
    value: &'a serde_json::Value,
    context: &str,
) -> Result<&'a serde_json::Map<String, serde_json::Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{context} is not an object"))
}

fn required_string<'a>(
    value: &'a serde_json::Value,
    field: &str,
    context: &str,
) -> Result<&'a str, String> {
    required_field(value, field, context)?
        .as_str()
        .ok_or_else(|| format!("{context}.{field} is not a string"))
}

fn optional_string<'a>(
    value: &'a serde_json::Value,
    field: &str,
    context: &str,
) -> Result<Option<&'a str>, String> {
    match required_field(value, field, context)? {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(text) => Ok(Some(text)),
        _ => Err(format!("{context}.{field} is neither null nor a string")),
    }
}

fn required_bool(value: &serde_json::Value, field: &str, context: &str) -> Result<bool, String> {
    required_field(value, field, context)?
        .as_bool()
        .ok_or_else(|| format!("{context}.{field} is not a boolean"))
}

fn string_or_empty<'a>(value: &'a serde_json::Value, field: &str) -> &'a str {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
}

#[cfg(test)]
mod tests;
