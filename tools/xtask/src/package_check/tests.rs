use super::skills::{CODEX_SKILL, validate_adapter, validate_skill_sources};

#[test]
fn repository_skills_share_one_thin_contract() -> Result<(), String> {
    let workspace = crate::metadata::find_workspace_root().map_err(|error| error.to_string())?;
    validate_skill_sources(&workspace)
}

#[test]
fn adapter_rejects_embedded_private_contracts() {
    let source = concat!(
        "name: lumin\ndescription: x\n",
        "lumin help-agent unique operation ID operation show lumin store migrate ",
        "retry the original command unchanged ",
        "Never read, edit, infer, or repair `.lumin` internals schemaVersion",
    );
    let result = validate_adapter(CODEX_SKILL, source);
    assert!(result.is_err(), "schema must be rejected");
    if let Err(error) = result {
        assert!(error.contains("schemaVersion"));
    }
}
