//! Exact workspace dependency-surface and registry-location policy.

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
    let resolver = workspace_resolver(workspace_root, violations)?;
    let packages = required_array(metadata, "packages", "metadata")?;
    let member_values = required_array(metadata, "workspace_members", "metadata")?;
    let nodes = required_array(
        required_field(metadata, "resolve", "metadata")?,
        "nodes",
        "metadata.resolve",
    )?;

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
            "features": features,
            "dependencies": dependency_policies,
        }));
    }
    policy_packages
        .sort_by(|left, right| string_or_empty(left, "name").cmp(string_or_empty(right, "name")));

    Ok(serde_json::json!({
        "schemaVersion": 1,
        "workspaceResolver": resolver,
        "packages": policy_packages,
    }))
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

fn workspace_resolver(
    workspace_root: &Path,
    violations: &mut Vec<String>,
) -> Result<&'static str, String> {
    let path = workspace_root.join("Cargo.toml");
    let source = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut in_workspace = false;
    let mut workspace_tables = 0_usize;
    let mut resolver_lines = Vec::new();
    for raw_line in source.lines() {
        let line = raw_line.trim();
        if line.starts_with('[') {
            in_workspace = line == "[workspace]";
            if in_workspace {
                workspace_tables += 1;
            }
            continue;
        }
        if in_workspace && line.starts_with("resolver") {
            resolver_lines.push(line.to_owned());
        }
    }
    if workspace_tables != 1 || resolver_lines != ["resolver = \"3\""] {
        violations.push(format!(
            "WORKSPACE RESOLVER DRIFT: expected one exact `[workspace]` `resolver = \"3\"`, found tables={workspace_tables} values={resolver_lines:?}"
        ));
        return Ok("invalid");
    }
    Ok("3")
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
        if !manifest.is_absolute() || !manifest.starts_with(&lexical_registry) {
            violations.push(format!(
                "REGISTRY MANIFEST LEXICAL ESCAPE: {name} {version} at {}",
                manifest.display()
            ));
        }
        match std::fs::canonicalize(&manifest) {
            Ok(canonical_manifest) => {
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
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .map_err(|error| format!("cannot resolve current directory: {error}"))
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
mod tests {
    use super::*;

    fn checked_policy(root: &Path) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        let bytes = std::fs::read(root.join(POLICY_PATH))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn package_mut<'a>(
        policy: &'a mut serde_json::Value,
        name: &str,
    ) -> Result<&'a mut serde_json::Value, String> {
        policy
            .get_mut("packages")
            .and_then(serde_json::Value::as_array_mut)
            .and_then(|packages| {
                packages.iter_mut().find(|package| {
                    package.get("name").and_then(serde_json::Value::as_str) == Some(name)
                })
            })
            .ok_or_else(|| format!("policy package is missing: {name}"))
    }

    fn dependency_mut<'a>(
        policy: &'a mut serde_json::Value,
        owner: &str,
        package_name: &str,
    ) -> Result<&'a mut serde_json::Value, String> {
        package_mut(policy, owner)?
            .get_mut("dependencies")
            .and_then(serde_json::Value::as_array_mut)
            .and_then(|dependencies| {
                dependencies.iter_mut().find(|dependency| {
                    dependency
                        .get("package")
                        .and_then(serde_json::Value::as_str)
                        == Some(package_name)
                })
            })
            .ok_or_else(|| format!("policy dependency is missing: {owner} -> {package_name}"))
    }

    fn replace_field(
        value: &mut serde_json::Value,
        field: &str,
        replacement: serde_json::Value,
    ) -> Result<(), String> {
        value
            .as_object_mut()
            .ok_or_else(|| "policy row is not an object".to_owned())?
            .insert(field.to_owned(), replacement);
        Ok(())
    }

    fn assert_policy_drift(root: &Path, observed: &serde_json::Value) -> Result<(), String> {
        let mut violations = Vec::new();
        compare_checked_policy(root, observed, &mut violations)?;
        if violations
            .iter()
            .any(|violation| violation.contains("DEPENDENCY SURFACE POLICY DRIFT"))
        {
            Ok(())
        } else {
            Err(format!(
                "expected dependency policy drift, got {violations:?}"
            ))
        }
    }

    #[test]
    fn first_difference_names_exact_dimension() {
        let expected = serde_json::json!({"optional": false, "features": ["a"]});
        let observed = serde_json::json!({"optional": true, "features": ["a"]});
        let difference = first_difference(&expected, &observed, "$");
        assert_eq!(
            difference.as_deref(),
            Some("$.optional expected false observed true")
        );
    }

    #[test]
    fn dependency_kind_rejects_unknown_wire_values() -> Result<(), String> {
        let mut violations = Vec::new();
        let value = serde_json::json!("future");
        let kind = dependency_kind(&value, &mut violations)?;
        assert_eq!(kind, "future");
        assert_eq!(violations, ["UNKNOWN DEPENDENCY KIND: future"]);
        Ok(())
    }

    #[test]
    fn registry_location_rejects_repository_and_source_less_packages()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let repository = temporary.path().join("repo");
        let cargo_home = temporary.path().join("cargo-home");
        let registry = cargo_home.join("registry/src/index/package-1.0.0");
        std::fs::create_dir_all(&repository)?;
        std::fs::create_dir_all(&registry)?;
        let valid_manifest = registry.join("Cargo.toml");
        std::fs::write(
            &valid_manifest,
            "[package]\nname='package'\nversion='1.0.0'\n",
        )?;
        let invalid_manifest = repository.join("Cargo.toml");
        std::fs::write(&invalid_manifest, "[workspace]\n")?;
        let metadata = serde_json::json!({
            "workspace_members": [],
            "packages": [
                {
                    "id": "registry",
                    "name": "package",
                    "version": "1.0.0",
                    "source": REGISTRY_SOURCE,
                    "manifest_path": valid_manifest,
                },
                {
                    "id": "path",
                    "name": "replacement",
                    "version": "1.0.0",
                    "source": null,
                    "manifest_path": invalid_manifest,
                }
            ]
        });
        let mut violations = Vec::new();
        validate_registry_locations_under(&metadata, &repository, &cargo_home, &mut violations)?;
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("UNAPPROVED PACKAGE SOURCE")),
            "{violations:?}"
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("LEXICAL ESCAPE")),
            "{violations:?}"
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("PHYSICAL ESCAPE")),
            "{violations:?}"
        );
        Ok(())
    }

    #[test]
    fn every_safety_dimension_is_part_of_policy_identity() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = super::super::find_workspace_root()?;
        let expected = checked_policy(&root)?;

        let mut resolver = expected.clone();
        replace_field(&mut resolver, "workspaceResolver", serde_json::json!("1"))?;
        assert_policy_drift(&root, &resolver)?;

        let mut feature_map = expected.clone();
        let feature = package_mut(&mut feature_map, "lumin-cli")?
            .get_mut("features")
            .and_then(serde_json::Value::as_array_mut)
            .and_then(|features| features.first_mut())
            .ok_or("lumin-cli feature policy is empty")?;
        replace_field(
            feature,
            "activations",
            serde_json::json!(["dep:unreviewed"]),
        )?;
        assert_policy_drift(&root, &feature_map)?;

        for (field, replacement) in [
            ("rename", serde_json::json!("same_file")),
            ("target", serde_json::json!("cfg(unix)")),
            ("optional", serde_json::json!(true)),
            ("usesDefaultFeatures", serde_json::json!(false)),
            ("features", serde_json::json!(["unreviewed"])),
            ("requirement", serde_json::json!(">=1")),
            ("binding", serde_json::json!("renamed_binding")),
        ] {
            let mut changed = expected.clone();
            replace_field(
                dependency_mut(&mut changed, "lumin-inventory", "windows-sys")?,
                field,
                replacement,
            )?;
            assert_policy_drift(&root, &changed)?;
        }

        for (field, replacement) in [
            ("version", serde_json::json!("999.0.0")),
            ("source", serde_json::json!("path+file:///replacement")),
        ] {
            let mut changed = expected.clone();
            let resolution = dependency_mut(&mut changed, "lumin-engine", "rayon")?
                .get_mut("resolution")
                .ok_or("rayon resolution is missing")?;
            replace_field(resolution, field, replacement)?;
            assert_policy_drift(&root, &changed)?;
        }

        let mut xtask = expected.clone();
        replace_field(
            dependency_mut(&mut xtask, "lumin-xtask", "syn")?,
            "features",
            serde_json::json!(["full"]),
        )?;
        assert_policy_drift(&root, &xtask)?;

        let mut duplicate = expected.clone();
        let dependencies = package_mut(&mut duplicate, "lumin-cli")?
            .get_mut("dependencies")
            .and_then(serde_json::Value::as_array_mut)
            .ok_or("lumin-cli dependencies are missing")?;
        let first = dependencies
            .first()
            .cloned()
            .ok_or("lumin-cli dependency policy is empty")?;
        dependencies.push(first);
        assert_policy_drift(&root, &duplicate)?;
        Ok(())
    }

    #[test]
    fn declared_rename_survives_normalized_binding_collision()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        std::fs::write(
            temporary.path().join("Cargo.toml"),
            "[workspace]\nresolver = \"3\"\nmembers = []\n",
        )?;
        let base = serde_json::json!({
            "workspace_members": ["owner-id"],
            "packages": [
                {
                    "id": "owner-id",
                    "name": "lumin-inventory",
                    "version": "0.1.0",
                    "manifest_path": temporary.path().join("owner/Cargo.toml"),
                    "features": {},
                    "dependencies": [{
                        "name": "same-file",
                        "rename": null,
                        "req": "=1.0.6",
                        "kind": null,
                        "target": null,
                        "optional": false,
                        "uses_default_features": true,
                        "features": []
                    }]
                },
                {
                    "id": "registry-id",
                    "name": "same-file",
                    "version": "1.0.6",
                    "source": REGISTRY_SOURCE,
                    "manifest_path": temporary.path().join("registry/same-file/Cargo.toml")
                }
            ],
            "resolve": {"nodes": [{
                "id": "owner-id",
                "deps": [{
                    "name": "same_file",
                    "pkg": "registry-id",
                    "dep_kinds": [{"kind": null, "target": null}]
                }]
            }]}
        });
        let mut violations = Vec::new();
        let authored = build_observed_policy(&base, temporary.path(), &mut violations)?;
        assert!(violations.is_empty(), "{violations:?}");

        let mut renamed_metadata = base;
        let declaration = renamed_metadata
            .get_mut("packages")
            .and_then(serde_json::Value::as_array_mut)
            .and_then(|packages| packages.first_mut())
            .and_then(|package| package.get_mut("dependencies"))
            .and_then(serde_json::Value::as_array_mut)
            .and_then(|dependencies| dependencies.first_mut())
            .ok_or("fixture dependency is missing")?;
        replace_field(declaration, "rename", serde_json::json!("same_file"))?;
        let mut renamed_violations = Vec::new();
        let renamed =
            build_observed_policy(&renamed_metadata, temporary.path(), &mut renamed_violations)?;
        assert!(renamed_violations.is_empty(), "{renamed_violations:?}");
        assert_ne!(authored, renamed);

        let kinds = renamed_metadata
            .get_mut("resolve")
            .and_then(|resolve| resolve.get_mut("nodes"))
            .and_then(serde_json::Value::as_array_mut)
            .and_then(|nodes| nodes.first_mut())
            .and_then(|node| node.get_mut("deps"))
            .and_then(serde_json::Value::as_array_mut)
            .and_then(|dependencies| dependencies.first_mut())
            .and_then(|dependency| dependency.get_mut("dep_kinds"))
            .and_then(serde_json::Value::as_array_mut)
            .ok_or("fixture resolution kinds are missing")?;
        let duplicate_kind = kinds
            .first()
            .cloned()
            .ok_or("fixture resolution kind is missing")?;
        kinds.push(duplicate_kind);
        let mut ambiguous_violations = Vec::new();
        let _ = build_observed_policy(
            &renamed_metadata,
            temporary.path(),
            &mut ambiguous_violations,
        )?;
        assert!(
            ambiguous_violations
                .iter()
                .any(|violation| violation.contains("AMBIGUOUS dependency join")),
            "{ambiguous_violations:?}"
        );
        Ok(())
    }
}
