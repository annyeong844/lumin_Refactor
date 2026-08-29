use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use super::{
    downgrade_store_as_prior, expect_migration_ready, expect_migration_required, expect_status,
    expect_string, expect_success, locate_binary, locate_fixture_binary, parse_json, run_binary,
    run_binary_with_broken_stdout, run_binary_with_stdout, scratch_directory_for,
    validate_help_output,
};

const MAX_EXECUTABLE_BYTES: u64 = 12_582_912;

pub(super) fn check(target: &str) -> Result<(), String> {
    validate_host_target(target)?;
    let workspace = crate::metadata::find_workspace_root().map_err(|error| error.to_string())?;
    let binary = locate_binary(&workspace)?;
    let fixture_binary = locate_fixture_binary()?;
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
    let result = validate_platform_contract(&binary, &fixture_binary, &scratch);
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

fn validate_platform_contract(
    binary: &Path,
    fixture_binary: &Path,
    scratch: &Path,
) -> Result<(), String> {
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

    downgrade_store_as_prior(fixture_binary, &repository, Some((cleanup_id, "succeeded")))?;
    let blocked = run_binary(binary, &repository, &["overview", "--format", "json"])?;
    expect_migration_required(&blocked, "v12 overview")?;
    expect_migration_ready(
        binary,
        &repository,
        &["store", "migrate", "--format", "json"],
        "v12 package migration",
    )?;

    let migrated_shown = expect_success(
        run_binary(
            binary,
            &repository,
            &["operation", "show", cleanup_id, "--format", "json"],
        ),
        "migrated operation show",
    )?;
    let migrated_shown_json = parse_json("migrated operation show", &migrated_shown.stdout)?;
    expect_string(
        &migrated_shown_json,
        "/schemaVersion",
        "lumin.cache-cleanup-operation.v2",
    )?;
    expect_string(&migrated_shown_json, "/lastDeliveryStatus", "unknown")?;

    let replayed = expect_success(
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
        "post-migration cleanup replay",
    )?;
    if replayed.stdout != cleaned.stdout {
        return Err(format!(
            "post-migration cleanup replay changed the committed result: {}",
            String::from_utf8_lossy(&replayed.stdout)
        ));
    }
    let replayed_show = expect_success(
        run_binary(
            binary,
            &repository,
            &["operation", "show", cleanup_id, "--format", "json"],
        ),
        "post-migration operation show",
    )?;
    let replayed_show_json = parse_json("post-migration operation show", &replayed_show.stdout)?;
    expect_string(&replayed_show_json, "/lastDeliveryStatus", "succeeded")?;

    let migrated_overview = expect_success(
        run_binary(binary, &repository, &["overview", "--format", "json"]),
        "post-migration overview",
    )?;
    let migrated_overview_json = parse_json("post-migration overview", &migrated_overview.stdout)?;
    expect_string(&migrated_overview_json, "/scope/id", run_id)?;
    expect_migration_ready(
        binary,
        &repository,
        &["store", "migrate", "--format", "json"],
        "current-store migration retry",
    )?;
    validate_corrupt_migration_anchor(binary, fixture_binary, &repository)?;

    validate_packaged_cleanup_contract(binary, fixture_binary, &scratch.join("cleanup-contract"))?;
    validate_absent_store(binary, &scratch.join("absent"))?;
    validate_reserved_path(binary, &scratch.join("reserved"))?;
    Ok(())
}

fn validate_packaged_cleanup_contract(
    binary: &Path,
    fixture_binary: &Path,
    root: &Path,
) -> Result<(), String> {
    fs::create_dir_all(root.join("src"))
        .map_err(|error| format!("cannot create cleanup package fixture: {error}"))?;
    fs::write(
        root.join("src/lib.ts"),
        b"export const cleanupPackageProbe = 1;\n",
    )
    .map_err(|error| format!("cannot write cleanup package fixture: {error}"))?;
    expect_success(
        run_binary(binary, root, &["audit", "--jobs", "1", "--format", "json"]),
        "cleanup package fixture audit",
    )?;

    seed_cache_payload(fixture_binary, root, "first.bin", "first")?;
    seed_cache_payload(fixture_binary, root, "second.bin", "second")?;
    let active_cache = root.join(".lumin/cache");
    let quarantine = root.join(".lumin/trash/cache-evictions");
    let active_before_clean = namespace_tree_snapshot(&active_cache)?;
    let quarantine_before_clean = namespace_tree_snapshot(&quarantine)?;
    let operation_id = "package-cache-contract-0001";
    let cleaned = expect_success(
        run_binary(
            binary,
            root,
            &[
                "cache",
                "clean",
                "--operation-id",
                operation_id,
                "--format",
                "json",
            ],
        ),
        "packaged cache cleanup",
    )?;
    let request_digest =
        expect_cleanup_response("packaged cache cleanup", &cleaned.stdout, operation_id)?;
    let shown = expect_success(
        run_binary(
            binary,
            root,
            &["operation", "show", operation_id, "--format", "json"],
        ),
        "packaged cache cleanup show",
    )?;
    expect_cleanup_operation(
        "packaged cache cleanup show",
        &shown.stdout,
        operation_id,
        &request_digest,
        2,
        2,
        "succeeded",
    )?;

    let active_after_clean = namespace_tree_snapshot(&active_cache)?;
    let quarantine_after_clean = namespace_tree_snapshot(&quarantine)?;
    validate_identity_preserving_quarantine_move(
        &active_before_clean,
        &active_after_clean,
        &quarantine_before_clean,
        &quarantine_after_clean,
    )?;
    let replay = expect_success(
        run_binary(
            binary,
            root,
            &["cache", "clean", "--operation-id", operation_id],
        ),
        "packaged cache cleanup replay",
    )?;
    if replay.stdout != cleaned.stdout
        || namespace_tree_snapshot(&quarantine)? != quarantine_after_clean
    {
        return Err(
            "packaged same-operation cleanup replay changed result bytes or quarantine objects"
                .to_owned(),
        );
    }

    let empty_operation_id = "package-cache-contract-empty-0002";
    let empty = expect_success(
        run_binary(
            binary,
            root,
            &["cache", "clean", "--operation-id", empty_operation_id],
        ),
        "packaged empty cache cleanup",
    )?;
    expect_cleanup_response(
        "packaged empty cache cleanup",
        &empty.stdout,
        empty_operation_id,
    )?;
    let empty_show = expect_success(
        run_binary(
            binary,
            root,
            &["operation", "show", empty_operation_id, "--format", "json"],
        ),
        "packaged empty cache cleanup show",
    )?;
    expect_cleanup_operation(
        "packaged empty cache cleanup show",
        &empty_show.stdout,
        empty_operation_id,
        &request_digest,
        0,
        0,
        "succeeded",
    )?;
    if namespace_tree_snapshot(&quarantine)? != quarantine_after_clean {
        return Err("packaged empty cleanup changed prior quarantine objects".to_owned());
    }
    drop(quarantine_after_clean);

    exercise_packaged_delivery_failure(
        binary,
        fixture_binary,
        root,
        "package-cache-broken-pipe-0003",
        "broken-pipe.bin",
        true,
    )?;
    exercise_packaged_delivery_failure(
        binary,
        fixture_binary,
        root,
        "package-cache-non-pipe-0004",
        "non-pipe.bin",
        false,
    )?;
    validate_malformed_cleanup(binary, &root.join("malformed"))
}

fn exercise_packaged_delivery_failure(
    binary: &Path,
    fixture_binary: &Path,
    root: &Path,
    operation_id: &str,
    payload_name: &str,
    broken_pipe: bool,
) -> Result<(), String> {
    seed_cache_payload(fixture_binary, root, payload_name, payload_name)?;
    let arguments = ["cache", "clean", "--operation-id", operation_id];
    let failed = if broken_pipe {
        run_binary_with_broken_stdout(binary, root, &arguments)?
    } else {
        let failure_stdout = non_pipe_failure_stdout(root)?;
        run_binary_with_stdout(binary, root, &arguments, Stdio::from(failure_stdout))?
    };
    expect_status(&failed, Some(1), "packaged cache cleanup delivery failure")?;
    let expected_stderr: &[u8] = if broken_pipe {
        b""
    } else {
        b"lumin: cannot write stdout\n"
    };
    if !failed.stdout.is_empty() || failed.stderr != expected_stderr {
        return Err(format!(
            "packaged delivery failure returned wrong bytes; stdout={} stderr={}",
            String::from_utf8_lossy(&failed.stdout),
            String::from_utf8_lossy(&failed.stderr)
        ));
    }

    let shown = expect_success(
        run_binary(
            binary,
            root,
            &["operation", "show", operation_id, "--format", "json"],
        ),
        "packaged failed-delivery operation show",
    )?;
    let shown_json = parse_json("packaged failed-delivery operation show", &shown.stdout)?;
    expect_string(
        &shown_json,
        "/schemaVersion",
        "lumin.cache-cleanup-operation.v2",
    )?;
    expect_string(&shown_json, "/operationId", operation_id)?;
    expect_string(&shown_json, "/status", "committed")?;
    expect_string(&shown_json, "/lastDeliveryStatus", "failed")?;
    let request_digest = shown_json
        .pointer("/requestDigest")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "failed-delivery operation omitted requestDigest".to_owned())?
        .to_owned();
    expect_cleanup_operation(
        "packaged failed-delivery operation show",
        &shown.stdout,
        operation_id,
        &request_digest,
        1,
        1,
        "failed",
    )?;

    let quarantine = root.join(".lumin/trash/cache-evictions");
    let before_replay = namespace_tree_snapshot(&quarantine)?;
    let replay = expect_success(
        run_binary(binary, root, &arguments),
        "packaged failed-delivery cleanup recovery",
    )?;
    expect_cleanup_response(
        "packaged failed-delivery cleanup recovery",
        &replay.stdout,
        operation_id,
    )?;
    if namespace_tree_snapshot(&quarantine)? != before_replay {
        return Err("packaged delivery recovery changed quarantine objects".to_owned());
    }
    let recovered = expect_success(
        run_binary(
            binary,
            root,
            &["operation", "show", operation_id, "--format", "json"],
        ),
        "packaged recovered-delivery operation show",
    )?;
    expect_cleanup_operation(
        "packaged recovered-delivery operation show",
        &recovered.stdout,
        operation_id,
        &request_digest,
        1,
        1,
        "succeeded",
    )
}

fn seed_cache_payload(
    fixture_binary: &Path,
    root: &Path,
    name: &str,
    payload: &str,
) -> Result<(), String> {
    let output = expect_success(
        run_binary(
            fixture_binary,
            root,
            &["cache", "test-write", name, payload],
        ),
        "seed packaged cache payload",
    )?;
    if !output.stdout.is_empty() {
        return Err("cache fixture writer emitted stdout".to_owned());
    }
    Ok(())
}

fn expect_cleanup_response(
    label: &str,
    bytes: &[u8],
    operation_id: &str,
) -> Result<String, String> {
    let value = parse_json(label, bytes)?;
    expect_string(&value, "/schemaVersion", "lumin.cache-cleanup.v2")?;
    expect_string(&value, "/operationId", operation_id)?;
    expect_string(&value, "/status", "clean")?;
    let request_digest = value
        .pointer("/requestDigest")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("{label} omitted requestDigest"))?;
    if request_digest.len() != 64 || !request_digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("{label} returned malformed requestDigest"));
    }
    let expected = format!(
        concat!(
            "{{\"schemaVersion\":\"lumin.cache-cleanup.v2\",",
            "\"operationId\":\"{operation_id}\",",
            "\"requestDigest\":\"{request_digest}\",",
            "\"status\":\"clean\"}}\n"
        ),
        operation_id = operation_id,
        request_digest = request_digest,
    );
    if bytes != expected.as_bytes() {
        return Err(format!("{label} did not emit canonical response bytes"));
    }
    Ok(request_digest.to_owned())
}

#[allow(clippy::too_many_arguments)]
fn expect_cleanup_operation(
    label: &str,
    bytes: &[u8],
    operation_id: &str,
    request_digest: &str,
    authorized_count: u64,
    validated_count: u64,
    last_delivery_status: &str,
) -> Result<(), String> {
    let expected = format!(
        concat!(
            "{{\"schemaVersion\":\"lumin.cache-cleanup-operation.v2\",",
            "\"operationId\":\"{operation_id}\",",
            "\"kind\":\"cache-clean\",",
            "\"requestDigest\":\"{request_digest}\",",
            "\"status\":\"committed\",",
            "\"interruptionCount\":0,",
            "\"authorizedCount\":{authorized_count},",
            "\"validatedCount\":{validated_count},",
            "\"result\":{{\"schemaVersion\":\"lumin.cache-cleanup.v2\",",
            "\"operationId\":\"{operation_id}\",",
            "\"requestDigest\":\"{request_digest}\",",
            "\"status\":\"clean\"}},",
            "\"lastDeliveryStatus\":\"{last_delivery_status}\"}}\n"
        ),
        operation_id = operation_id,
        request_digest = request_digest,
        authorized_count = authorized_count,
        validated_count = validated_count,
        last_delivery_status = last_delivery_status,
    );
    if bytes != expected.as_bytes() {
        return Err(format!(
            "{label} did not emit canonical operation bytes: {}",
            String::from_utf8_lossy(bytes)
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn non_pipe_failure_stdout(_root: &Path) -> Result<fs::File, String> {
    fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .map_err(|error| format!("cannot open Linux stdout failure device: {error}"))
}

#[cfg(windows)]
fn non_pipe_failure_stdout(root: &Path) -> Result<fs::File, String> {
    let read_only_path = root.join("read-only-stdout.bin");
    fs::write(&read_only_path, b"sentinel")
        .map_err(|error| format!("cannot create read-only stdout fixture: {error}"))?;
    fs::File::open(&read_only_path)
        .map_err(|error| format!("cannot open read-only stdout fixture: {error}"))
}

fn validate_malformed_cleanup(binary: &Path, root: &Path) -> Result<(), String> {
    fs::create_dir(root)
        .map_err(|error| format!("cannot create malformed cleanup fixture: {error}"))?;
    let output = run_binary(binary, root, &["cache", "clean", "--operation-id"])?;
    expect_status(&output, Some(2), "malformed packaged cache cleanup")?;
    if !output.stdout.is_empty() || root.join(".lumin").exists() {
        return Err("malformed packaged cache cleanup initialized state".to_owned());
    }
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
struct NamespaceSnapshotRow {
    relative_path: PathBuf,
    kind: NamespaceSnapshotKind,
    identity: same_file::Handle,
    payload: Option<Vec<u8>>,
}

#[derive(Debug, Eq, PartialEq)]
enum NamespaceSnapshotKind {
    Directory,
    RegularFile,
}

fn validate_identity_preserving_quarantine_move(
    active_before: &[NamespaceSnapshotRow],
    active_after: &[NamespaceSnapshotRow],
    quarantine_before: &[NamespaceSnapshotRow],
    quarantine_after: &[NamespaceSnapshotRow],
) -> Result<(), String> {
    let source_payloads = active_before
        .iter()
        .filter(|row| !is_namespace_control_row(row))
        .collect::<Vec<_>>();
    if source_payloads.len() != 2 {
        return Err(format!(
            "packaged cleanup fixture expected 2 active payload rows, found {}",
            source_payloads.len()
        ));
    }
    let expected_active = active_before
        .iter()
        .filter(|row| is_namespace_control_row(row))
        .collect::<Vec<_>>();
    let observed_active = active_after.iter().collect::<Vec<_>>();
    if observed_active != expected_active {
        return Err(
            "packaged cleanup changed active-cache controls or retained payloads".to_owned(),
        );
    }
    if quarantine_before
        .iter()
        .any(|row| !quarantine_after.contains(row))
    {
        return Err("packaged cleanup changed a preexisting quarantine object".to_owned());
    }
    let added_destinations = quarantine_after
        .iter()
        .filter(|row| !quarantine_before.contains(row))
        .collect::<Vec<_>>();
    if added_destinations.len() != source_payloads.len() {
        return Err(format!(
            "packaged cleanup created {} quarantine rows for {} active payload rows",
            added_destinations.len(),
            source_payloads.len()
        ));
    }
    for source in &source_payloads {
        let matches = added_destinations
            .iter()
            .filter(|destination| same_physical_payload(source, destination))
            .count();
        if matches != 1 {
            return Err(format!(
                "packaged cleanup source {} has {matches} identity-preserving quarantine matches",
                source.relative_path.display()
            ));
        }
    }
    for destination in &added_destinations {
        let matches = source_payloads
            .iter()
            .filter(|source| same_physical_payload(source, destination))
            .count();
        if matches != 1 {
            return Err(format!(
                "packaged cleanup destination {} has {matches} authenticated source matches",
                destination.relative_path.display()
            ));
        }
    }
    Ok(())
}

fn is_namespace_control_row(row: &NamespaceSnapshotRow) -> bool {
    row.relative_path.as_os_str().is_empty()
        || row.relative_path.as_path() == Path::new("namespace.anchor")
}

fn same_physical_payload(
    source: &NamespaceSnapshotRow,
    destination: &NamespaceSnapshotRow,
) -> bool {
    source.kind == destination.kind
        && source.identity == destination.identity
        && source.payload == destination.payload
}

fn namespace_tree_snapshot(root: &Path) -> Result<Vec<NamespaceSnapshotRow>, String> {
    let mut rows = Vec::new();
    snapshot_namespace_entry(root, root, &mut rows)?;
    Ok(rows)
}

fn snapshot_namespace_entry(
    root: &Path,
    path: &Path,
    rows: &mut Vec<NamespaceSnapshotRow>,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    let file_type = metadata.file_type();
    let (kind, payload) = if file_type.is_dir() {
        (NamespaceSnapshotKind::Directory, None)
    } else if file_type.is_file() {
        (
            NamespaceSnapshotKind::RegularFile,
            Some(
                fs::read(path)
                    .map_err(|error| format!("cannot read {}: {error}", path.display()))?,
            ),
        )
    } else {
        return Err(format!(
            "quarantine snapshot contains unsupported entry {}",
            path.display()
        ));
    };
    let identity = same_file::Handle::from_path(path)
        .map_err(|error| format!("cannot identify {}: {error}", path.display()))?;
    rows.push(NamespaceSnapshotRow {
        relative_path: path
            .strip_prefix(root)
            .map_err(|error| format!("cannot project quarantine path: {error}"))?
            .to_path_buf(),
        kind,
        identity,
        payload,
    });
    if file_type.is_dir() {
        let mut children = fs::read_dir(path)
            .map_err(|error| format!("cannot enumerate {}: {error}", path.display()))?
            .map(|entry| entry.map(|entry| (entry.file_name(), entry.path())))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("cannot enumerate {}: {error}", path.display()))?;
        children.sort_by(|left, right| left.0.cmp(&right.0));
        for (_, child) in children {
            snapshot_namespace_entry(root, &child, rows)?;
        }
    }
    Ok(())
}

fn validate_corrupt_migration_anchor(
    binary: &Path,
    fixture_binary: &Path,
    root: &Path,
) -> Result<(), String> {
    let corrupted = expect_success(
        run_binary(fixture_binary, root, &["store", "test-corrupt-v13-anchor"]),
        "corrupt migrated provenance anchor",
    )?;
    if !corrupted.stdout.is_empty() {
        return Err("migration-anchor fixture wrote stdout".to_owned());
    }

    for (label, arguments) in [
        (
            "overview with corrupted migration anchor",
            &["overview", "--format", "json"][..],
        ),
        (
            "migration retry with corrupted migration anchor",
            &["store", "migrate", "--format", "json"][..],
        ),
    ] {
        let output = run_binary(binary, root, arguments)?;
        expect_status(&output, Some(1), label)?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.stdout.is_empty()
            || !stderr.starts_with("lumin: state namespace integrity failure: ")
        {
            return Err(format!(
                "{label} did not hard-stop on the corrupted anchor; stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                stderr
            ));
        }
    }
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
