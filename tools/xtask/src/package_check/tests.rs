use super::skills::{
    CODEX_SKILL, MIGRATION_WORKFLOW, stage_skill_sources, validate_adapter, validate_skill_sources,
};

#[test]
fn repository_skills_share_one_thin_contract() -> Result<(), String> {
    let workspace = crate::metadata::find_workspace_root().map_err(|error| error.to_string())?;
    validate_skill_sources(&workspace)
}

#[test]
fn staged_skill_package_is_a_separate_exact_copy() -> Result<(), String> {
    let workspace = crate::metadata::find_workspace_root().map_err(|error| error.to_string())?;
    let scratch = tempfile::tempdir().map_err(|error| error.to_string())?;
    let package_root = scratch.path().join("package");
    stage_skill_sources(&workspace, &package_root)?;
    validate_skill_sources(&package_root)?;
    assert_ne!(
        package_root
            .canonicalize()
            .map_err(|error| error.to_string())?,
        workspace
            .canonicalize()
            .map_err(|error| error.to_string())?
    );
    Ok(())
}

#[test]
fn adapter_rejects_embedded_private_contracts() {
    let source = format!("{}schemaVersion", valid_adapter_source());
    let result = validate_adapter(CODEX_SKILL, &source);
    assert!(result.is_err(), "schema must be rejected");
    if let Err(error) = result {
        assert!(error.contains("schemaVersion"));
    }
}

#[test]
fn adapter_rejects_reordered_migration_workflow() {
    let source = valid_adapter_source().replace(
        "  2. Run `lumin store migrate --format json` and no other migration command.\n",
        "  4. Run `lumin store migrate --format json` and no other migration command.\n",
    );
    let result = validate_adapter(CODEX_SKILL, &source);
    assert!(
        result.is_err(),
        "reordered migration workflow must be rejected"
    );
    if let Err(error) = result {
        assert!(error.contains("canonical migration recovery workflow"));
    }
}

fn valid_adapter_source() -> String {
    format!(
        "name: lumin\ndescription: x\nlumin help-agent\nunique operation ID\noperation show\n{MIGRATION_WORKFLOW}Never read, edit, infer, or repair `.lumin` internals\n"
    )
}
