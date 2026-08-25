use std::fs;
use std::path::Path;

use super::{
    MIGRATION_RESPONSE, expect_status, expect_string, expect_success, locate_binary, parse_json,
    run_binary, scratch_directory_for, validate_help_output,
};

const MAX_EXECUTABLE_BYTES: u64 = 12_582_912;

pub(super) fn check(target: &str) -> Result<(), String> {
    validate_host_target(target)?;
    let workspace = crate::metadata::find_workspace_root().map_err(|error| error.to_string())?;
    let binary = locate_binary(&workspace)?;
    let size = fs::metadata(&binary)
        .map_err(|error| format!("cannot inspect packaged lumin binary: {error}"))?
        .len();
    if size > MAX_EXECUTABLE_BYTES {
        return Err(format!(
            "packaged lumin binary is {size} bytes; limit is {MAX_EXECUTABLE_BYTES}"
        ));
    }

    let scratch = scratch_directory_for("platform")?;
    fs::create_dir(&scratch)
        .map_err(|error| format!("cannot create package-check scratch directory: {error}"))?;
    let result = validate_platform_contract(&binary, &scratch);
    let cleanup = fs::remove_dir_all(&scratch)
        .map_err(|error| format!("cannot remove package-check scratch directory: {error}"));
    result?;
    cleanup?;
    Ok(())
}

fn validate_host_target(target: &str) -> Result<(), String> {
    let host = match std::env::consts::OS {
        "windows" => "windows-x64",
        "linux" => "linux-x64",
        other => {
            return Err(format!(
                "package checks support only Windows and Linux; current OS is {other}"
            ));
        }
    };
    if std::env::consts::ARCH != "x86_64" {
        return Err(format!(
            "package checks require x86_64; current architecture is {}",
            std::env::consts::ARCH
        ));
    }
    if target != host {
        return Err(format!(
            "package target {target} cannot be executed on host {host}"
        ));
    }
    Ok(())
}

fn validate_platform_contract(binary: &Path, scratch: &Path) -> Result<(), String> {
    let capabilities_root = scratch.join("capabilities");
    fs::create_dir(&capabilities_root)
        .map_err(|error| format!("cannot create capabilities fixture: {error}"))?;
    let capabilities = expect_success(
        run_binary(
            binary,
            &capabilities_root,
            &["capabilities", "--format", "json"],
        ),
        "capabilities",
    )?;
    if capabilities_root.join(".lumin").exists() {
        return Err("binary capabilities created repository state".to_owned());
    }
    let capabilities_json = parse_json("capabilities", &capabilities.stdout)?;
    expect_string(&capabilities_json, "/schemaVersion", "lumin.collection.v1")?;
    expect_string(&capabilities_json, "/scope/kind", "binary")?;
    let build_id = capabilities_json
        .pointer("/scope/buildId")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "capabilities omitted scope.buildId".to_owned())?;
    if !build_id.starts_with("build_") {
        return Err(format!(
            "capabilities returned a malformed build ID: {build_id}"
        ));
    }

    let help = expect_success(
        run_binary(binary, &capabilities_root, &["help-agent"]),
        "help-agent",
    )?;
    validate_help_output(&help.stdout)?;

    let repository = scratch.join("repository");
    fs::create_dir_all(repository.join("src"))
        .map_err(|error| format!("cannot create audit fixture: {error}"))?;
    fs::write(
        repository.join("package.json"),
        br#"{"name":"lumin-package-fixture","private":true,"type":"module"}"#,
    )
    .map_err(|error| format!("cannot write audit fixture manifest: {error}"))?;
    fs::write(
        repository.join("src/lib.ts"),
        b"export const packageProbe = 1;\n",
    )
    .map_err(|error| format!("cannot write audit fixture source: {error}"))?;

    let audit = expect_success(
        run_binary(
            binary,
            &repository,
            &["audit", "--jobs", "1", "--format", "json"],
        ),
        "audit",
    )?;
    let audit_json = parse_json("audit", &audit.stdout)?;
    expect_string(&audit_json, "/schemaVersion", "lumin.audit.v2")?;
    let run_id = audit_json
        .get("runId")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "audit omitted runId".to_owned())?;

    let overview = expect_success(
        run_binary(binary, &repository, &["overview", "--format", "json"]),
        "overview",
    )?;
    let overview_json = parse_json("overview", &overview.stdout)?;
    expect_string(&overview_json, "/schemaVersion", "lumin.overview.v2")?;
    expect_string(&overview_json, "/scope/id", run_id)?;

    let cleanup_id = "package-check-cache-clean-0001";
    let cleaned = expect_success(
        run_binary(
            binary,
            &repository,
            &[
                "cache",
                "clean",
                "--operation-id",
                cleanup_id,
                "--format",
                "json",
            ],
        ),
        "cache clean",
    )?;
    let cleaned_json = parse_json("cache clean", &cleaned.stdout)?;
    expect_string(&cleaned_json, "/schemaVersion", "lumin.cache-cleanup.v2")?;
    expect_string(&cleaned_json, "/operationId", cleanup_id)?;

    let shown = expect_success(
        run_binary(
            binary,
            &repository,
            &["operation", "show", cleanup_id, "--format", "json"],
        ),
        "operation show",
    )?;
    let shown_json = parse_json("operation show", &shown.stdout)?;
    expect_string(
        &shown_json,
        "/schemaVersion",
        "lumin.cache-cleanup-operation.v2",
    )?;
    expect_string(&shown_json, "/operationId", cleanup_id)?;
    expect_string(&shown_json, "/lastDeliveryStatus", "succeeded")?;

    let migration = expect_success(
        run_binary(
            binary,
            &repository,
            &["store", "migrate", "--format", "json"],
        ),
        "current-store migration",
    )?;
    let migration_stdout = String::from_utf8_lossy(&migration.stdout);
    if migration_stdout.trim_end() != MIGRATION_RESPONSE {
        return Err(format!(
            "current-store migration returned unexpected stdout: {}",
            migration_stdout
        ));
    }

    validate_absent_store(binary, &scratch.join("absent"))?;
    validate_reserved_path(binary, &scratch.join("reserved"))?;
    Ok(())
}

fn validate_absent_store(binary: &Path, root: &Path) -> Result<(), String> {
    fs::create_dir(root).map_err(|error| format!("cannot create absent-store fixture: {error}"))?;
    let output = run_binary(binary, root, &["store", "migrate", "--format", "json"])?;
    expect_status(&output, Some(1), "absent-store migration")?;
    if !output.stdout.is_empty()
        || String::from_utf8_lossy(&output.stderr) != "lumin: lifecycle store is not initialized\n"
        || root.join(".lumin").exists()
    {
        return Err("absent-store migration mutated state or returned the wrong diagnostic".into());
    }
    Ok(())
}

fn validate_reserved_path(binary: &Path, root: &Path) -> Result<(), String> {
    fs::create_dir(root)
        .map_err(|error| format!("cannot create reserved-path fixture: {error}"))?;
    let output = run_binary(
        binary,
        root,
        &[
            "pre-write",
            "--operation-id",
            "package-check-reserved-0001",
            "--path",
            ".lumin/forbidden",
            "--format",
            "json",
        ],
    )?;
    expect_status(&output, Some(2), "reserved-path pre-write")?;
    if !output.stdout.is_empty()
        || !String::from_utf8_lossy(&output.stderr).contains("reserved .lumin namespace")
        || root.join(".lumin").exists()
    {
        return Err(
            "reserved-path pre-write allocated state or returned the wrong diagnostic".into(),
        );
    }
    Ok(())
}
