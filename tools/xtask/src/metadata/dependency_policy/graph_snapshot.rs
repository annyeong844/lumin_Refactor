//! Complete canonical Cargo package-definition and resolved-graph snapshot.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use super::{
    canonical_feature_policy, canonical_string_array, dependency_kind, optional_string,
    required_array, required_bool, required_field, required_string, stable_workspace_path,
};

pub(super) struct GraphSnapshot {
    pub(super) package_definitions: Vec<serde_json::Value>,
    pub(super) resolved_graph: serde_json::Value,
}

pub(super) fn build(
    packages: &[serde_json::Value],
    nodes: &[serde_json::Value],
    root: &serde_json::Value,
    package_by_id: &HashMap<String, &serde_json::Value>,
    member_ids: &BTreeSet<String>,
    workspace_root: &Path,
    violations: &mut Vec<String>,
) -> Result<GraphSnapshot, String> {
    let identities = stable_identities(packages, member_ids, workspace_root)?;
    if identities.len() != package_by_id.len() {
        return Err(format!(
            "stable package identity count {} differs from metadata package count {}",
            identities.len(),
            package_by_id.len()
        ));
    }

    let mut package_definitions = packages
        .iter()
        .map(|package| {
            package_definition(package, member_ids, workspace_root, &identities, violations)
        })
        .collect::<Result<Vec<_>, _>>()?;
    sort_unique_values(&mut package_definitions, "package definition")?;

    let resolved_graph = resolved_graph(nodes, root, package_by_id, &identities, violations)?;
    Ok(GraphSnapshot {
        package_definitions,
        resolved_graph,
    })
}

fn stable_identities(
    packages: &[serde_json::Value],
    member_ids: &BTreeSet<String>,
    workspace_root: &Path,
) -> Result<HashMap<String, serde_json::Value>, String> {
    let mut identities = HashMap::new();
    let mut stable_keys = BTreeSet::new();
    for package in packages {
        let id = required_string(package, "id", "metadata package")?;
        let name = required_string(package, "name", "metadata package")?;
        let version = required_string(package, "version", "metadata package")?;
        let identity = if member_ids.contains(id) {
            serde_json::json!({
                "kind": "workspace",
                "name": name,
                "version": version,
                "manifest": stable_workspace_path(
                    required_string(package, "manifest_path", "workspace package")?,
                    workspace_root,
                    &format!("workspace package {name} manifest"),
                )?,
            })
        } else {
            let source = required_string(package, "source", "non-workspace package")?;
            serde_json::json!({
                "kind": "registry",
                "name": name,
                "version": version,
                "source": source,
            })
        };
        let stable_key = value_key(&identity)?;
        if !stable_keys.insert(stable_key.clone()) {
            return Err(format!("duplicate stable package identity: {stable_key}"));
        }
        if identities.insert(id.to_owned(), identity).is_some() {
            return Err(format!("duplicate metadata package id: {id}"));
        }
    }
    Ok(identities)
}

fn package_definition(
    package: &serde_json::Value,
    member_ids: &BTreeSet<String>,
    workspace_root: &Path,
    identities: &HashMap<String, serde_json::Value>,
    violations: &mut Vec<String>,
) -> Result<serde_json::Value, String> {
    let id = required_string(package, "id", "metadata package")?;
    let name = required_string(package, "name", "metadata package")?;
    let identity = identities
        .get(id)
        .ok_or_else(|| format!("stable package identity is missing: {id}"))?
        .clone();

    let mut dependencies = required_array(package, "dependencies", "metadata package")?
        .iter()
        .map(|dependency| package_dependency(dependency, name, violations))
        .collect::<Result<Vec<_>, _>>()?;
    sort_unique_values(
        &mut dependencies,
        &format!("package dependency definition for {name}"),
    )?;

    let mut targets = required_array(package, "targets", "metadata package")?
        .iter()
        .map(|target| {
            package_target(
                package,
                target,
                member_ids.contains(id),
                workspace_root,
                name,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    sort_unique_values(
        &mut targets,
        &format!("package target definition for {name}"),
    )?;

    Ok(serde_json::json!({
        "identity": identity,
        "links": required_field(package, "links", "metadata package")?.clone(),
        "rustVersion": required_field(package, "rust_version", "metadata package")?.clone(),
        "features": canonical_feature_policy(package, name)?,
        "dependencies": dependencies,
        "targets": targets,
    }))
}

fn package_dependency(
    dependency: &serde_json::Value,
    owner: &str,
    violations: &mut Vec<String>,
) -> Result<serde_json::Value, String> {
    let context = format!("package definition dependency for {owner}");
    Ok(serde_json::json!({
        "name": required_string(dependency, "name", &context)?,
        "rename": optional_string(dependency, "rename", &context)?,
        "requirement": required_string(dependency, "req", &context)?,
        "source": required_field(dependency, "source", &context)?.clone(),
        "registry": required_field(dependency, "registry", &context)?.clone(),
        "kind": dependency_kind(required_field(dependency, "kind", &context)?, violations)?,
        "target": optional_string(dependency, "target", &context)?,
        "optional": required_bool(dependency, "optional", &context)?,
        "usesDefaultFeatures": required_bool(
            dependency,
            "uses_default_features",
            &context,
        )?,
        "features": canonical_string_array(
            required_field(dependency, "features", &context)?,
            &format!("{context} features"),
        )?,
    }))
}

fn package_target(
    package: &serde_json::Value,
    target: &serde_json::Value,
    is_workspace: bool,
    workspace_root: &Path,
    owner: &str,
) -> Result<serde_json::Value, String> {
    let context = format!("package target for {owner}");
    let source = stable_target_source(package, target, is_workspace, workspace_root, &context)?;
    let required_features = match target.get("required-features") {
        Some(value) => canonical_string_array(value, &format!("{context} required features"))?,
        None => Vec::new(),
    };
    Ok(serde_json::json!({
        "name": required_string(target, "name", &context)?,
        "edition": required_string(target, "edition", &context)?,
        "doc": required_bool(target, "doc", &context)?,
        "doctest": required_bool(target, "doctest", &context)?,
        "test": required_bool(target, "test", &context)?,
        "kind": canonical_string_array(
            required_field(target, "kind", &context)?,
            &format!("{context} kind"),
        )?,
        "crateTypes": canonical_string_array(
            required_field(target, "crate_types", &context)?,
            &format!("{context} crate types"),
        )?,
        "requiredFeatures": required_features,
        "source": source,
    }))
}

fn stable_target_source(
    package: &serde_json::Value,
    target: &serde_json::Value,
    is_workspace: bool,
    workspace_root: &Path,
    context: &str,
) -> Result<String, String> {
    let raw_source = required_string(target, "src_path", context)?;
    if is_workspace {
        return stable_workspace_path(raw_source, workspace_root, context);
    }

    let manifest = Path::new(required_string(
        package,
        "manifest_path",
        "registry package",
    )?);
    let package_root = manifest
        .parent()
        .ok_or_else(|| format!("registry manifest has no parent: {}", manifest.display()))?;
    let canonical_root = std::fs::canonicalize(package_root).map_err(|error| {
        format!(
            "cannot canonicalize registry package root {}: {error}",
            package_root.display()
        )
    })?;
    let source = Path::new(raw_source);
    let canonical_source = std::fs::canonicalize(source).map_err(|error| {
        format!(
            "cannot canonicalize {context} {}: {error}",
            source.display()
        )
    })?;
    if !canonical_source.starts_with(&canonical_root) {
        return Err(format!(
            "{context} escapes registry package {}: {}",
            canonical_root.display(),
            canonical_source.display()
        ));
    }
    Ok(super::super::relative_display(
        &canonical_root,
        &canonical_source,
    ))
}

fn resolved_graph(
    nodes: &[serde_json::Value],
    root: &serde_json::Value,
    package_by_id: &HashMap<String, &serde_json::Value>,
    identities: &HashMap<String, serde_json::Value>,
    violations: &mut Vec<String>,
) -> Result<serde_json::Value, String> {
    let root = match root {
        serde_json::Value::Null => serde_json::Value::Null,
        serde_json::Value::String(id) => identities
            .get(id)
            .ok_or_else(|| format!("resolve root package is missing: {id}"))?
            .clone(),
        _ => return Err("metadata.resolve.root is neither null nor a string".to_owned()),
    };

    let mut seen_node_ids = BTreeSet::new();
    let mut canonical_nodes = Vec::new();
    for node in nodes {
        let id = required_string(node, "id", "resolve node")?;
        if !seen_node_ids.insert(id.to_owned()) {
            return Err(format!("duplicate resolve node id: {id}"));
        }
        if !package_by_id.contains_key(id) {
            return Err(format!("resolve node package is missing: {id}"));
        }
        canonical_nodes.push(resolved_node(node, identities, violations)?);
    }
    let package_ids = package_by_id.keys().cloned().collect::<BTreeSet<_>>();
    if seen_node_ids != package_ids {
        return Err("resolve node ids do not equal the complete package id set".to_owned());
    }
    sort_unique_values(&mut canonical_nodes, "resolve node")?;
    Ok(serde_json::json!({"root": root, "nodes": canonical_nodes}))
}

fn resolved_node(
    node: &serde_json::Value,
    identities: &HashMap<String, serde_json::Value>,
    violations: &mut Vec<String>,
) -> Result<serde_json::Value, String> {
    let id = required_string(node, "id", "resolve node")?;
    let identity = identities
        .get(id)
        .ok_or_else(|| format!("resolve node identity is missing: {id}"))?
        .clone();

    let (dependencies, dependency_keys) = resolved_dependency_identities(
        required_array(node, "dependencies", "resolve node")?,
        identities,
        id,
    )?;
    let mut bindings = Vec::new();
    let mut binding_destinations = BTreeSet::new();
    for dependency in required_array(node, "deps", "resolve node")? {
        let target_id = required_string(dependency, "pkg", "resolved dependency")?;
        let target = identities
            .get(target_id)
            .ok_or_else(|| format!("resolved dependency identity is missing: {target_id}"))?
            .clone();
        binding_destinations.insert(value_key(&target)?);
        let mut kinds = required_array(dependency, "dep_kinds", "resolved dependency")?
            .iter()
            .map(|kind| {
                Ok(serde_json::json!({
                    "kind": dependency_kind(
                        required_field(kind, "kind", "resolved dependency kind")?,
                        violations,
                    )?,
                    "target": optional_string(kind, "target", "resolved dependency kind")?,
                }))
            })
            .collect::<Result<Vec<_>, String>>()?;
        if kinds.is_empty() {
            return Err(format!(
                "resolved dependency has zero kinds: {id} -> {target_id}"
            ));
        }
        sort_unique_values(&mut kinds, "resolved dependency kind")?;
        bindings.push(serde_json::json!({
            "binding": required_string(dependency, "name", "resolved dependency")?,
            "package": target,
            "kinds": kinds,
        }));
    }
    if dependency_keys != binding_destinations {
        return Err(format!(
            "resolve node dependency ids and bindings disagree for {id}"
        ));
    }
    sort_unique_values(&mut bindings, "resolved dependency binding")?;

    Ok(serde_json::json!({
        "package": identity,
        "features": canonical_string_array(
            required_field(node, "features", "resolve node")?,
            "resolve node features",
        )?,
        "dependencies": dependencies,
        "bindings": bindings,
    }))
}

fn resolved_dependency_identities(
    values: &[serde_json::Value],
    identities: &HashMap<String, serde_json::Value>,
    owner: &str,
) -> Result<(Vec<serde_json::Value>, BTreeSet<String>), String> {
    let mut keys = BTreeSet::new();
    let mut dependencies = Vec::new();
    for value in values {
        let id = value
            .as_str()
            .ok_or_else(|| format!("resolve dependency id is not a string for {owner}"))?;
        let identity = identities
            .get(id)
            .ok_or_else(|| format!("resolve dependency package is missing: {id}"))?
            .clone();
        let key = value_key(&identity)?;
        if !keys.insert(key) {
            return Err(format!("duplicate resolve dependency id for {owner}: {id}"));
        }
        dependencies.push(identity);
    }
    dependencies.sort_by_key(|value| value_key(value).unwrap_or_default());
    Ok((dependencies, keys))
}

fn sort_unique_values(values: &mut [serde_json::Value], context: &str) -> Result<(), String> {
    let mut keyed = values
        .iter()
        .cloned()
        .map(|value| Ok((value_key(&value)?, value)))
        .collect::<Result<Vec<_>, String>>()?;
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    for pair in keyed.windows(2) {
        if pair[0].0 == pair[1].0 {
            return Err(format!("duplicate {context}: {}", pair[0].0));
        }
    }
    for (slot, (_, value)) in values.iter_mut().zip(keyed) {
        *slot = value;
    }
    Ok(())
}

fn value_key(value: &serde_json::Value) -> Result<String, String> {
    serde_json::to_string(value)
        .map_err(|error| format!("cannot serialize graph identity: {error}"))
}
