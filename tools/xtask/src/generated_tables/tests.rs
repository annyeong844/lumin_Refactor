use super::*;

fn workspace_root() -> Result<PathBuf, std::io::Error> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
}

#[test]
fn reviewed_artifacts_validate_and_render() -> Result<(), Box<dyn std::error::Error>> {
    let artifacts = load_artifacts(&workspace_root()?).map_err(std::io::Error::other)?;
    assert_eq!(validate_artifacts(&artifacts), Vec::<String>::new());
    let files = render::expected_files(&artifacts).map_err(std::io::Error::other)?;
    assert_eq!(files.len(), 2);
    assert!(files.iter().all(|file| file.content.contains("@generated")));
    let resolver = files
        .iter()
        .find(|file| file.relative_path == RESOLVER_OUTPUT)
        .ok_or_else(|| std::io::Error::other("resolver generated file is missing"))?;
    assert!(
        resolver
            .content
            .contains("pub(crate) fn package_json_field_for_rule")
    );
    Ok(())
}

#[test]
fn owner_partition_mutation_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let mut artifacts = load_artifacts(&workspace_root()?).map_err(std::io::Error::other)?;
    artifacts.inventory_resolver_owned.pop();
    let violations = validate_artifacts(&artifacts);
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("OWNER DRIFT"))
    );
    Ok(())
}

#[test]
fn duplicate_owner_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let mut artifacts = load_artifacts(&workspace_root()?).map_err(std::io::Error::other)?;
    let duplicate = artifacts.resolver_inventory_owned[0].clone();
    artifacts.resolver_inventory_owned.push(duplicate);
    let violations = validate_artifacts(&artifacts);
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("OWNER DUPLICATE"))
    );
    Ok(())
}

#[test]
fn unknown_classification_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let mut artifacts = load_artifacts(&workspace_root()?).map_err(std::io::Error::other)?;
    artifacts.resolver_compiler_options[0].classification = "MagicClean".to_owned();
    let violations = validate_artifacts(&artifacts);
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("UNKNOWN CLASSIFICATION"))
    );
    Ok(())
}

#[test]
fn missing_package_applicability_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let mut artifacts = load_artifacts(&workspace_root()?).map_err(std::io::Error::other)?;
    artifacts.resolver_package_fields[0].applies_when = None;
    let violations = validate_artifacts(&artifacts);
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("MISSING APPLICABILITY"))
    );
    Ok(())
}

#[test]
fn unknown_package_applicability_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let mut artifacts = load_artifacts(&workspace_root()?).map_err(std::io::Error::other)?;
    artifacts.resolver_package_fields[0].applies_when = Some("whenever".to_owned());
    let violations = validate_artifacts(&artifacts);
    assert!(
        violations
            .iter()
            .any(|violation| violation.contains("UNKNOWN APPLICABILITY"))
    );
    Ok(())
}

#[test]
fn generation_is_idempotent_and_drift_is_visible() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    for relative in [RESOLVER_SPEC, INVENTORY_SPEC] {
        let source = workspace_path(&workspace_root()?, relative);
        let target = workspace_path(temp.path(), relative);
        let parent = target
            .parent()
            .ok_or_else(|| std::io::Error::other("spec path has no parent"))?;
        std::fs::create_dir_all(parent)?;
        std::fs::copy(source, target)?;
    }
    for relative in [RESOLVER_OUTPUT, INVENTORY_OUTPUT] {
        let target = workspace_path(temp.path(), relative);
        let parent = target
            .parent()
            .ok_or_else(|| std::io::Error::other("output path has no parent"))?;
        std::fs::create_dir_all(parent)?;
    }

    write_generated_tables(temp.path()).map_err(std::io::Error::other)?;
    let resolver_path = workspace_path(temp.path(), RESOLVER_OUTPUT);
    let first = std::fs::read(&resolver_path)?;
    write_generated_tables(temp.path()).map_err(std::io::Error::other)?;
    let second = std::fs::read(&resolver_path)?;
    assert_eq!(first, second);

    std::fs::write(&resolver_path, b"drift\n")?;
    let result = check_generated_tables(temp.path());
    assert!(result.tool_errors.is_empty());
    assert!(
        result
            .violations
            .iter()
            .any(|violation| violation.contains("GENERATED TABLE DRIFT"))
    );
    Ok(())
}

#[test]
fn artifact_entry_and_generated_row_mutations_are_rejected()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = workspace_root()?;
    let temp = tempfile::tempdir()?;
    for relative in [
        RESOLVER_SPEC,
        INVENTORY_SPEC,
        RESOLVER_OUTPUT,
        INVENTORY_OUTPUT,
    ] {
        let target = workspace_path(temp.path(), relative);
        let parent = target
            .parent()
            .ok_or_else(|| std::io::Error::other("contract path has no parent"))?;
        std::fs::create_dir_all(parent)?;
        std::fs::copy(workspace_path(&workspace, relative), target)?;
    }

    let baseline = check_generated_tables(temp.path());
    assert!(baseline.tool_errors.is_empty());
    assert!(baseline.violations.is_empty(), "{:?}", baseline.violations);

    let resolver_spec = workspace_path(temp.path(), RESOLVER_SPEC);
    let mut artifact: Value = serde_json::from_slice(&std::fs::read(&resolver_spec)?)?;
    let source_hash = artifact
        .pointer_mut("/nodePackageBaseline/resolverSourceSha256")
        .ok_or_else(|| std::io::Error::other("resolver source hash is missing"))?;
    *source_hash = Value::String("0".repeat(64));
    std::fs::write(&resolver_spec, serde_json::to_vec_pretty(&artifact)?)?;
    let artifact_drift = check_generated_tables(temp.path());
    assert!(artifact_drift.tool_errors.is_empty());
    assert!(
        artifact_drift
            .violations
            .iter()
            .any(|violation| violation.contains("GENERATED TABLE DRIFT")),
        "{:?}",
        artifact_drift.violations,
    );

    std::fs::copy(workspace_path(&workspace, RESOLVER_SPEC), &resolver_spec)?;
    let resolver_output = workspace_path(temp.path(), RESOLVER_OUTPUT);
    let compiled = std::fs::read_to_string(&resolver_output)?;
    let mutated = compiled.replacen(
        "        path: \"browser\",",
        "        path: \"browser-tampered\",",
        1,
    );
    assert_ne!(mutated, compiled);
    std::fs::write(&resolver_output, mutated)?;
    let generated_drift = check_generated_tables(temp.path());
    assert!(generated_drift.tool_errors.is_empty());
    assert!(
        generated_drift
            .violations
            .iter()
            .any(|violation| violation.contains("GENERATED TABLE DRIFT")),
        "{:?}",
        generated_drift.violations,
    );
    Ok(())
}
