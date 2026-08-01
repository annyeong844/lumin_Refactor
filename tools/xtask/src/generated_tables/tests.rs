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
