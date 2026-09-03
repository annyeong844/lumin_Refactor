use super::artifact::{validate_test_package, write_test_package};
use super::skills::{
    CODEX_SKILL, MIGRATION_WORKFLOW, OPERATION_RECOVERY_WORKFLOW, stage_skill_sources,
    validate_adapter, validate_skill_sources,
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
fn package_manifest_binds_the_exact_staged_payloads() -> Result<(), String> {
    let scratch = tempfile::tempdir().map_err(|error| error.to_string())?;
    let package = scratch.path().join("package");
    write_test_package(&package, test_target()?, b"release-binary")?;
    validate_test_package(&package, test_target()?)?;

    std::fs::write(package.join(test_binary_path()?), b"changed-binary")
        .map_err(|error| error.to_string())?;
    let error = rejection(
        validate_test_package(&package, test_target()?),
        "changed package payload",
    )?;
    assert!(error.contains("identity differs from package manifest"));
    Ok(())
}

#[test]
fn package_inventory_rejects_unbound_files() -> Result<(), String> {
    let scratch = tempfile::tempdir().map_err(|error| error.to_string())?;
    let package = scratch.path().join("package");
    write_test_package(&package, test_target()?, b"release-binary")?;
    std::fs::write(package.join("source.rs"), b"fn fallback() {}")
        .map_err(|error| error.to_string())?;
    let error = rejection(
        validate_test_package(&package, test_target()?),
        "unbound package child",
    )?;
    assert!(error.contains("package root inventory differs"));

    std::fs::remove_file(package.join("source.rs")).map_err(|error| error.to_string())?;
    std::fs::write(
        package.join("skills/codex/fallback.rs"),
        b"fn fallback() {}",
    )
    .map_err(|error| error.to_string())?;
    let error = rejection(
        validate_test_package(&package, test_target()?),
        "unbound nested package child",
    )?;
    assert!(error.contains("package codex skill directory inventory differs"));
    Ok(())
}

#[test]
fn package_manifest_rejects_opaque_or_noncanonical_fields() -> Result<(), String> {
    let scratch = tempfile::tempdir().map_err(|error| error.to_string())?;
    let package = scratch.path().join("package");
    write_test_package(&package, test_target()?, b"release-binary")?;
    let manifest_path = package.join("lumin-package.json");
    let manifest = std::fs::read_to_string(&manifest_path).map_err(|error| error.to_string())?;
    let changed = manifest.replacen(
        "{\"schemaVersion\"",
        "{\"opaque\":true,\"schemaVersion\"",
        1,
    );
    std::fs::write(&manifest_path, changed).map_err(|error| error.to_string())?;
    let error = rejection(
        validate_test_package(&package, test_target()?),
        "opaque package manifest field",
    )?;
    assert!(error.contains("package manifest fields differ"));
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

#[test]
fn adapter_rejects_reordered_operation_recovery_workflow() {
    let source = valid_adapter_source().replace(
        "  2. Run `lumin operation show <operation-id> --format json` before any cleanup retry.\n",
        "  3. Run `lumin operation show <operation-id> --format json` before any cleanup retry.\n",
    );
    let result = validate_adapter(CODEX_SKILL, &source);
    assert!(
        result.is_err(),
        "reordered operation recovery workflow must be rejected"
    );
    if let Err(error) = result {
        assert!(error.contains("canonical operation recovery workflow"));
    }
}

fn valid_adapter_source() -> String {
    format!(
        "name: lumin\ndescription: x\nlumin help-agent\nunique operation ID\noperation show\n{OPERATION_RECOVERY_WORKFLOW}{MIGRATION_WORKFLOW}Never read, edit, infer, or repair `.lumin` internals\n"
    )
}

fn test_target() -> Result<&'static str, String> {
    match std::env::consts::OS {
        "windows" => Ok("windows-x64"),
        "linux" => Ok("linux-x64"),
        other => Err(format!(
            "package tests require Windows or Linux, found {other}"
        )),
    }
}

fn test_binary_path() -> Result<&'static str, String> {
    match std::env::consts::OS {
        "windows" => Ok("bin/lumin.exe"),
        "linux" => Ok("bin/lumin"),
        other => Err(format!(
            "package tests require Windows or Linux, found {other}"
        )),
    }
}

fn rejection(result: Result<(), String>, label: &str) -> Result<String, String> {
    match result {
        Ok(()) => Err(format!("{label} was accepted")),
        Err(error) => Ok(error),
    }
}
