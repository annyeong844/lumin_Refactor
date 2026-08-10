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
fn absolute_path_normalizes_parent_components() -> Result<(), Box<dyn std::error::Error>> {
    let with_parent = PathBuf::from("target")
        .join("..")
        .join("isolated-cargo-home");
    let normalized = absolute_path(&with_parent)?;
    let expected = absolute_path(Path::new("isolated-cargo-home"))?;

    assert_eq!(normalized, expected);
    assert!(
        !normalized
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    );
    Ok(())
}

#[test]
fn absolute_path_rejects_root_escape() -> Result<(), Box<dyn std::error::Error>> {
    let escaping = Path::new(std::path::MAIN_SEPARATOR_STR)
        .join("..")
        .join("outside");

    let error = match absolute_path(&escaping) {
        Ok(path) => return Err(format!("root escape was accepted as {}", path.display()).into()),
        Err(error) => error,
    };
    assert!(error.contains("escapes the filesystem root"), "{error}");
    Ok(())
}

#[cfg(unix)]
#[test]
fn registry_root_redirection_inside_cargo_home_is_rejected()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let repository = temporary.path().join("repo");
    let cargo_home = temporary.path().join("cargo-home");
    let registry_parent = cargo_home.join("registry");
    let redirected = cargo_home.join("redirected-source");
    std::fs::create_dir_all(&repository)?;
    std::fs::create_dir_all(&registry_parent)?;
    std::fs::create_dir_all(&redirected)?;
    std::os::unix::fs::symlink(&redirected, registry_parent.join("src"))?;
    let metadata = serde_json::json!({
        "workspace_members": [],
        "packages": []
    });

    let mut violations = Vec::new();
    validate_registry_locations_under(&metadata, &repository, &cargo_home, &mut violations)?;
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("REGISTRY PATH REDIRECTION")),
        "{violations:?}"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn registry_manifest_redirection_inside_registry_is_rejected()
-> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let repository = temporary.path().join("repo");
    let cargo_home = temporary.path().join("cargo-home");
    let registry = cargo_home.join("registry/src/index");
    let redirected = registry.join("redirected-1.0.0");
    std::fs::create_dir_all(&repository)?;
    std::fs::create_dir_all(&redirected)?;
    std::fs::write(
        redirected.join("Cargo.toml"),
        "[package]\nname='package'\nversion='1.0.0'\n",
    )?;
    let lexical_package = registry.join("package-1.0.0");
    std::os::unix::fs::symlink(&redirected, &lexical_package)?;
    let metadata = serde_json::json!({
        "workspace_members": [],
        "packages": [{
            "id": "registry",
            "name": "package",
            "version": "1.0.0",
            "source": REGISTRY_SOURCE,
            "manifest_path": lexical_package.join("Cargo.toml")
        }]
    });

    let mut violations = Vec::new();
    validate_registry_locations_under(&metadata, &repository, &cargo_home, &mut violations)?;
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("REGISTRY PATH REDIRECTION")),
        "{violations:?}"
    );
    Ok(())
}

#[test]
fn every_safety_dimension_is_part_of_policy_identity() -> Result<(), Box<dyn std::error::Error>> {
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
fn declared_rename_survives_normalized_binding_collision() -> Result<(), Box<dyn std::error::Error>>
{
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
