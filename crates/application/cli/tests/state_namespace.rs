use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

#[test]
fn fresh_namespace_initialization_publishes_the_exact_complete_binding()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let audited = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audited, 0);

    let state = root.path().join(".lumin");
    let marker = read_json(&state.join("repository.json"))?;
    assert_eq!(
        marker.get("schemaVersion").and_then(Value::as_str),
        Some("lumin-repository.v4")
    );
    assert_exact_object_keys(&marker, &["binding", "schemaVersion"]);
    let binding = marker
        .get("binding")
        .ok_or("repository binding is missing")?;
    assert_exact_object_keys(binding, &["cacheEvictions", "global", "managedParents"]);
    let global = binding.get("global").ok_or("global binding is missing")?;
    assert_exact_object_keys(
        global,
        &[
            "lifecycleLockIdentity",
            "namespaceNonce",
            "repositoryId",
            "repositoryRootCanonical",
            "repositoryRootPhysicalIdentity",
            "stateDirectoryIdentity",
        ],
    );

    let lock = read_json(&state.join("lifecycle.lock"))?;
    assert_eq!(
        lock.get("schemaVersion").and_then(Value::as_str),
        Some("lumin-lifecycle-lock.v2")
    );
    assert_exact_object_keys(&lock, &["global", "schemaVersion"]);
    assert_eq!(lock.get("global"), Some(global));

    let managed = binding
        .get("managedParents")
        .and_then(Value::as_array)
        .ok_or("managed parent bindings are missing")?;
    assert_eq!(managed.len(), 4);
    let mut nonces = vec![required_string(global, "namespaceNonce")?];
    for ((kind, directory), expected) in [
        ("attempts", "attempts"),
        ("runs", "runs"),
        ("trash", "trash"),
        ("cache", "cache"),
    ]
    .into_iter()
    .zip(managed)
    {
        assert_eq!(expected.get("kind").and_then(Value::as_str), Some(kind));
        assert_exact_object_keys(
            expected,
            &[
                "anchorPhysicalIdentity",
                "directoryPhysicalIdentity",
                "kind",
                "parentNonce",
            ],
        );
        nonces.push(required_string(expected, "parentNonce")?);
        let anchor = read_json(&state.join(directory).join("namespace.anchor"))?;
        assert_eq!(
            anchor.get("schemaVersion").and_then(Value::as_str),
            Some("lumin-managed-parent-anchor.v2")
        );
        assert_eq!(anchor.get("global"), Some(global));
        assert_eq!(anchor.get("binding"), Some(expected));
    }

    let cache_evictions = binding
        .get("cacheEvictions")
        .ok_or("cache-eviction binding is missing")?;
    assert_exact_object_keys(
        cache_evictions,
        &[
            "anchorPhysicalIdentity",
            "directoryPhysicalIdentity",
            "parentNonce",
        ],
    );
    nonces.push(required_string(cache_evictions, "parentNonce")?);
    let nested = read_json(&state.join("trash/cache-evictions/namespace.anchor"))?;
    assert_eq!(
        nested.get("schemaVersion").and_then(Value::as_str),
        Some("lumin-cache-eviction-parent-anchor.v1")
    );
    assert_eq!(nested.get("global"), Some(global));
    assert_eq!(nested.get("trashBinding"), managed.get(2));
    assert_eq!(nested.get("binding"), Some(cache_evictions));

    assert!(nonces.iter().all(|nonce| canonical_nonce(nonce)));
    let unique_nonces = nonces.iter().collect::<std::collections::BTreeSet<_>>();
    assert_eq!(unique_nonces.len(), nonces.len());
    assert!(state.join("lifecycle.store").is_file());
    assert!(fs::read_dir(&state)?.all(|entry| entry.is_ok_and(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        !name.starts_with(".lumin-unpublished-") && !name.starts_with(".tmp")
    })));

    let run_id = field(&audited.stdout, "runId")?;
    assert_status(&run(root.path(), &["overview", "--run", &run_id])?, 0);
    Ok(())
}

#[test]
fn caller_state_paths_are_malformed_before_lifecycle_mutation()
-> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        vec!["audit", "--entry", ".lumin/cache/payload.ts", "--jobs", "1"],
        vec![
            "pre-write",
            "--operation-id",
            "op-reserved-write",
            "--path",
            ".lumin/cache/payload.ts",
            "--jobs",
            "1",
        ],
        vec![
            "pre-write",
            "--operation-id",
            "op-reserved-entry",
            "--path",
            "src/lib.ts",
            "--entry",
            ".lumin/cache/payload.ts",
            "--jobs",
            "1",
        ],
        vec![
            "pre-write",
            "--operation-id",
            "op-reserved-dependency",
            "--path",
            "src/lib.ts",
            "--dependency-at",
            ".lumin/cache",
            "dep",
            "--jobs",
            "1",
        ],
    ];
    for arguments in cases {
        let root = fixture()?;
        let rejected = run(root.path(), &arguments)?;
        assert_status(&rejected, 2);
        assert!(rejected.stdout.is_empty());
        assert!(rejected.stderr.contains("reserved .lumin namespace"));
        assert!(
            !root.path().join(".lumin").exists(),
            "malformed caller state input allocated lifecycle state: {arguments:?}",
        );
    }

    let escaping = fixture()?;
    let outside = tempfile::tempdir()?;
    fs::write(
        outside.path().join("entry.ts"),
        "export const outside = 1;\n",
    )?;
    create_directory_alias(outside.path(), &escaping.path().join("outside-alias"))?;
    let rejected = run(
        escaping.path(),
        &["audit", "--entry", "outside-alias/entry.ts", "--jobs", "1"],
    )?;
    assert_status(&rejected, 2);
    assert!(rejected.stdout.is_empty());
    assert!(rejected.stderr.contains("resolves outside repository root"));
    assert!(
        !escaping.path().join(".lumin").exists(),
        "invalid audit entry initialized reserved state",
    );
    remove_directory_alias(&escaping.path().join("outside-alias"))?;

    let root = fixture()?;
    let initialized = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&initialized, 0);
    let state = root.path().join(".lumin");
    let alias = root.path().join("state-alias");
    create_directory_alias(&state, &alias)?;

    let rejected = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-state-alias",
            "--path",
            "state-alias/cache/new.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&rejected, 2);
    assert!(rejected.stdout.is_empty());
    assert!(rejected.stderr.contains("reserved .lumin namespace"));

    remove_directory_alias(&alias)?;
    let operation = run(root.path(), &["operation", "show", "op-state-alias"])?;
    assert_status(&operation, 2);
    assert!(operation.stderr.contains("operation does not exist"));
    Ok(())
}

#[test]
fn committed_pre_write_retry_precedes_current_path_revalidation()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    fs::create_dir(root.path().join("planned"))?;
    let arguments = [
        "pre-write",
        "--operation-id",
        "op-committed-retry",
        "--path",
        "planned/output.ts",
        "--jobs",
        "1",
    ];
    let first = run(root.path(), &arguments)?;
    assert_status(&first, 0);

    fs::remove_dir(root.path().join("planned"))?;
    let alias = root.path().join("planned");
    create_directory_alias(&root.path().join(".lumin").join("cache"), &alias)?;
    let retry = run(root.path(), &arguments)?;
    assert_status(&retry, 0);
    assert_eq!(retry.stdout, first.stdout);
    assert!(retry.stderr.is_empty());
    remove_directory_alias(&alias)?;
    Ok(())
}

#[test]
fn public_process_rejects_state_directory_replacement() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let initial = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&initial, 0);
    let run_id = field(&initial.stdout, "runId")?;

    let state = root.path().join(".lumin");
    let displaced = root.path().join(".lumin.displaced");
    fs::rename(&state, &displaced)?;
    fs::create_dir(&state)?;

    let rejected = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&rejected, 1);
    assert!(
        rejected
            .stderr
            .contains("state namespace integrity failure")
    );

    fs::remove_dir(&state)?;
    fs::rename(displaced, &state)?;
    let recovered = run(root.path(), &["overview", "--run", &run_id])?;
    assert_status(&recovered, 0);
    Ok(())
}

#[test]
fn public_process_rejects_state_mount_crossing() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let initial = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&initial, 0);
    let run_id = field(&initial.stdout, "runId")?;
    let state = root.path().join(".lumin");

    let mut crossing = StateMountCrossing::install(&state)?;
    assert_public_integrity_failure(&run(root.path(), &["audit", "--jobs", "1"])?);
    crossing.remove()?;

    let recovered = run(root.path(), &["overview", "--run", &run_id])?;
    assert_status(&recovered, 0);

    #[cfg(target_os = "linux")]
    {
        let foreign = tempfile::tempdir()?;
        fs::write(foreign.path().join("payload"), b"foreign state payload")?;
        fs::write(
            root.path().join("src/shared.ts"),
            "export const shared = 1;\n",
        )?;
        fs::hard_link(
            root.path().join("src/shared.ts"),
            root.path().join("src/shared-alias.ts"),
        )?;
        let mut nested =
            DirectoryBindMount::install(foreign.path(), &state.join("cache/foreign-mount"))?;

        assert_public_integrity_failure(&run(
            root.path(),
            &["audit", "--entry", "src/shared.ts", "--jobs", "1"],
        )?);
        nested.remove()?;
    }
    Ok(())
}

#[test]
fn configured_state_entry_cannot_complete_an_audit() -> Result<(), Box<dyn std::error::Error>> {
    let lexical = fixture()?;
    fs::write(
        lexical.path().join("lumin.json"),
        r#"{"schemaVersion":"lumin-config.v1","entries":[".lumin/cache/namespace.anchor"]}"#,
    )?;
    let rejected = run(lexical.path(), &["audit", "--jobs", "1"])?;
    assert_status(&rejected, 2);
    assert!(rejected.stdout.is_empty());
    assert!(rejected.stderr.contains("reserved .lumin namespace"));
    Ok(())
}

#[test]
fn public_process_rejects_lifecycle_lock_replacement() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let initial = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&initial, 0);
    let run_id = field(&initial.stdout, "runId")?;

    let state = root.path().join(".lumin");
    let lock = state.join("lifecycle.lock");
    let displaced = state.join("lifecycle.lock.displaced");
    let bytes = fs::read(&lock)?;
    fs::rename(&lock, &displaced)?;
    fs::write(&lock, bytes)?;

    let rejected = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&rejected, 1);
    assert!(
        rejected
            .stderr
            .contains("state namespace integrity failure")
    );

    fs::remove_file(lock)?;
    fs::rename(displaced, state.join("lifecycle.lock"))?;
    let recovered = run(root.path(), &["overview", "--run", &run_id])?;
    assert_status(&recovered, 0);
    Ok(())
}

#[test]
fn public_process_rejects_foreign_and_redirected_state_namespaces()
-> Result<(), Box<dyn std::error::Error>> {
    let empty = fixture()?;
    fs::create_dir(empty.path().join(".lumin"))?;
    assert_public_integrity_failure(&run(empty.path(), &["audit", "--jobs", "1"])?);

    let file = fixture()?;
    fs::write(file.path().join(".lumin"), b"foreign state")?;
    assert_public_integrity_failure(&run(file.path(), &["audit", "--jobs", "1"])?);

    let redirected = fixture()?;
    let redirect_target = tempfile::tempdir()?;
    create_directory_alias(redirect_target.path(), &redirected.path().join(".lumin"))?;
    let result = run(redirected.path(), &["audit", "--jobs", "1"])?;
    assert_public_integrity_failure(&result);
    remove_directory_alias(&redirected.path().join(".lumin"))?;

    let source = fixture()?;
    let initialized = run(source.path(), &["audit", "--jobs", "1"])?;
    assert_status(&initialized, 0);
    let copied = fixture()?;
    copy_directory(&source.path().join(".lumin"), &copied.path().join(".lumin"))?;
    let result = run(copied.path(), &["audit", "--jobs", "1"])?;
    assert_status(&result, 1);
    assert!(result.stdout.is_empty());
    assert!(
        result
            .stderr
            .contains("repository marker belongs to a different canonical root")
    );
    Ok(())
}

#[test]
fn public_process_rejects_managed_parent_anchor_and_marker_replacement()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let initialized = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&initialized, 0);
    let run_id = field(&initialized.stdout, "runId")?;
    let state = root.path().join(".lumin");

    let runs = state.join("runs");
    let runs_original = state.join("runs.original");
    fs::rename(&runs, &runs_original)?;
    copy_directory(&runs_original, &runs)?;
    assert_public_integrity_failure(&run(root.path(), &["audit", "--jobs", "1"])?);
    fs::remove_dir_all(&runs)?;
    fs::rename(&runs_original, &runs)?;

    let cache = state.join("cache");
    let cache_original = state.join("cache.original");
    fs::rename(&cache, &cache_original)?;
    create_directory_alias(&cache_original, &cache)?;
    assert_public_integrity_failure(&run(root.path(), &["audit", "--jobs", "1"])?);
    remove_directory_alias(&cache)?;
    fs::rename(&cache_original, &cache)?;

    let quarantine = state.join("trash").join("cache-evictions");
    let quarantine_original = root.path().join("cache-evictions.original");
    let quarantine_anchor = quarantine.join("namespace.anchor");
    let quarantine_anchor_bytes = fs::read(&quarantine_anchor)?;
    fs::rename(&quarantine, &quarantine_original)?;
    fs::create_dir(&quarantine)?;
    fs::rename(
        quarantine_original.join("namespace.anchor"),
        &quarantine_anchor,
    )?;
    assert_public_integrity_failure(&run(root.path(), &["audit", "--jobs", "1"])?);
    assert_eq!(fs::read(&quarantine_anchor)?, quarantine_anchor_bytes);
    assert_eq!(fs::read_dir(&quarantine)?.count(), 1);
    fs::rename(
        &quarantine_anchor,
        quarantine_original.join("namespace.anchor"),
    )?;
    fs::remove_dir(&quarantine)?;
    fs::rename(&quarantine_original, &quarantine)?;

    let quarantine_redirect_original = root.path().join("cache-evictions.redirect-original");
    fs::rename(&quarantine, &quarantine_redirect_original)?;
    create_directory_alias(&quarantine_redirect_original, &quarantine)?;
    assert_public_integrity_failure(&run(
        root.path(),
        &[
            "cache",
            "clean",
            "--operation-id",
            "cache-nested-parent-redirect",
        ],
    )?);
    assert_eq!(fs::read(&quarantine_anchor)?, quarantine_anchor_bytes);
    assert_eq!(fs::read_dir(&quarantine)?.count(), 1);
    remove_directory_alias(&quarantine)?;
    fs::rename(&quarantine_redirect_original, &quarantine)?;
    let absent_operation = run(
        root.path(),
        &["operation", "show", "cache-nested-parent-redirect"],
    )?;
    assert_status(&absent_operation, 2);
    assert!(absent_operation.stderr.contains("operation does not exist"));

    let quarantine_anchor = quarantine.join("namespace.anchor");
    let quarantine_anchor_original = root.path().join("cache-evictions-anchor.original");
    let quarantine_anchor_bytes = fs::read(&quarantine_anchor)?;
    fs::rename(&quarantine_anchor, &quarantine_anchor_original)?;
    fs::write(&quarantine_anchor, &quarantine_anchor_bytes)?;
    assert_public_integrity_failure(&run(
        root.path(),
        &[
            "cache",
            "clean",
            "--operation-id",
            "cache-nested-anchor-replacement",
        ],
    )?);
    assert_eq!(fs::read(&quarantine_anchor)?, quarantine_anchor_bytes);
    assert_eq!(fs::read_dir(&quarantine)?.count(), 1);
    fs::remove_file(&quarantine_anchor)?;
    fs::rename(&quarantine_anchor_original, &quarantine_anchor)?;
    let absent_operation = run(
        root.path(),
        &["operation", "show", "cache-nested-anchor-replacement"],
    )?;
    assert_status(&absent_operation, 2);
    assert!(absent_operation.stderr.contains("operation does not exist"));

    let anchor = state.join("trash/namespace.anchor");
    let extra_anchor = state.join("trash/namespace.anchor.extra");
    fs::hard_link(&anchor, &extra_anchor)?;
    assert_public_integrity_failure(&run(root.path(), &["audit", "--jobs", "1"])?);
    fs::remove_file(extra_anchor)?;

    let anchor = state.join("attempts/namespace.anchor");
    let anchor_original = state.join("attempts/namespace.anchor.original");
    let anchor_bytes = fs::read(&anchor)?;
    fs::rename(&anchor, &anchor_original)?;
    fs::write(&anchor, anchor_bytes)?;
    assert_public_integrity_failure(&run(root.path(), &["audit", "--jobs", "1"])?);
    fs::remove_file(anchor)?;
    fs::rename(anchor_original, state.join("attempts/namespace.anchor"))?;

    let marker = state.join("repository.json");
    let marker_bytes = fs::read(&marker)?;
    fs::write(&marker, b"{}")?;
    assert_public_integrity_failure(&run(root.path(), &["audit", "--jobs", "1"])?);
    fs::write(marker, marker_bytes)?;

    let recovered = run(root.path(), &["overview", "--run", &run_id])?;
    assert_status(&recovered, 0);
    Ok(())
}

#[test]
fn state_payload_aliases_never_enter_source_evidence_or_gate_writes()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let initialized = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&initialized, 0);
    let initialized_run_id = field(&initialized.stdout, "runId")?;
    let state = root.path().join(".lumin");
    let state_payload = state
        .join("runs")
        .join(&initialized_run_id)
        .join("evidence.store");
    let alias = root.path().join("src/state-payload-alias.ts");
    fs::hard_link(&state_payload, &alias)?;

    let audited = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audited, 0);
    let run_id = field(&audited.stdout, "runId")?;
    let files = run(
        root.path(),
        &["files", "--run", &run_id, "src/state-payload-alias.ts"],
    )?;
    assert_status(&files, 0);
    let response: Value = serde_json::from_str(&files.stdout)?;
    assert_eq!(response.get("total").and_then(Value::as_u64), Some(0));

    let rejected = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-state-payload-alias",
            "--path",
            "src/state-payload-alias.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&rejected, 2);
    assert!(rejected.stdout.is_empty());
    assert!(rejected.stderr.contains("reserved .lumin namespace"));
    let operation = run(
        root.path(),
        &["operation", "show", "op-state-payload-alias"],
    )?;
    assert_status(&operation, 2);
    assert!(operation.stderr.contains("operation does not exist"));

    for semantic_input in ["lumin.json", ".gitignore", "package.json", "tsconfig.json"] {
        let semantic_alias = root.path().join(semantic_input);
        fs::hard_link(&state_payload, &semantic_alias)?;
        let rejected = run(root.path(), &["audit", "--jobs", "1"])?;
        assert_status(&rejected, 1);
        assert!(rejected.stdout.is_empty());
        assert!(
            rejected
                .stderr
                .contains("semantic input aliases the reserved .lumin namespace"),
            "unexpected semantic-input diagnostic for {semantic_input}: {}",
            rejected.stderr,
        );
        fs::remove_file(semantic_alias)?;
    }
    fs::remove_file(&alias)?;

    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;

        assert_eq!(fs::metadata(&state_payload)?.nlink(), 1);
        let bind_alias = root.path().join("src/state-payload-bind.ts");
        let mut bind = FileBindMount::install(&state_payload, &bind_alias)?;
        let audited = run(root.path(), &["audit", "--jobs", "1"])?;
        assert_status(&audited, 0);
        let bind_run_id = field(&audited.stdout, "runId")?;
        let files = run(
            root.path(),
            &["files", "--run", &bind_run_id, "src/state-payload-bind.ts"],
        )?;
        assert_status(&files, 0);
        let response: Value = serde_json::from_str(&files.stdout)?;
        assert_eq!(response.get("total").and_then(Value::as_u64), Some(0));

        let rejected = run(
            root.path(),
            &[
                "pre-write",
                "--operation-id",
                "op-state-payload-bind",
                "--path",
                "src/state-payload-bind.ts",
                "--jobs",
                "1",
            ],
        )?;
        assert_status(&rejected, 2);
        assert!(rejected.stdout.is_empty());
        assert!(rejected.stderr.contains("reserved .lumin namespace"));
        let operation = run(root.path(), &["operation", "show", "op-state-payload-bind"])?;
        assert_status(&operation, 2);
        assert!(operation.stderr.contains("operation does not exist"));
        bind.remove()?;

        let directory_alias = root.path().join("state-cache-bind");
        let mut directory_bind =
            DirectoryBindMount::install(&state.join("cache"), &directory_alias)?;
        let rejected = run(
            root.path(),
            &[
                "pre-write",
                "--operation-id",
                "op-state-cache-bind-child",
                "--path",
                "state-cache-bind/new.ts",
                "--jobs",
                "1",
            ],
        )?;
        assert_status(&rejected, 2);
        assert!(rejected.stdout.is_empty());
        assert!(rejected.stderr.contains("reserved .lumin namespace"));
        let operation = run(
            root.path(),
            &["operation", "show", "op-state-cache-bind-child"],
        )?;
        assert_status(&operation, 2);
        assert!(operation.stderr.contains("operation does not exist"));
        directory_bind.remove()?;
    }

    for parent in ["attempts", "runs", "trash", "cache"] {
        assert!(
            state.join(parent).join("namespace.anchor").is_file(),
            "managed state anchor disappeared for {parent}",
        );
    }
    Ok(())
}

#[test]
fn irrelevant_files_are_filtered_before_identity_capture() -> Result<(), Box<dyn std::error::Error>>
{
    let root = fixture()?;
    let irrelevant = root.path().join("README.locked");
    fs::write(&irrelevant, "not source or configuration evidence\n")?;
    #[cfg(windows)]
    let _exclusive = {
        use std::os::windows::fs::OpenOptionsExt;

        fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&irrelevant)?
    };

    let audited = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audited, 0);
    Ok(())
}

fn fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(
        root.path().join("src/lib.ts"),
        "export const visible = 1;\n",
    )?;
    Ok(root)
}

fn read_json(path: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn assert_exact_object_keys(value: &Value, expected: &[&str]) {
    assert!(value.is_object(), "expected JSON object, got {value}");
    let mut actual = value
        .as_object()
        .into_iter()
        .flat_map(serde_json::Map::keys)
        .map(String::as_str)
        .collect::<Vec<_>>();
    actual.sort_unstable();
    let mut expected = expected.to_vec();
    expected.sort_unstable();
    assert_eq!(actual, expected);
}

fn required_string<'a>(
    value: &'a Value,
    field: &str,
) -> Result<&'a str, Box<dyn std::error::Error>> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string field {field}").into())
}

fn canonical_nonce(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn assert_public_integrity_failure(result: &support::ProcessResult) {
    assert_status(result, 1);
    assert!(result.stdout.is_empty());
    assert!(
        result.stderr.contains("state namespace integrity failure")
            || result
                .stderr
                .contains("reserved .lumin namespace is not a real directory"),
        "unexpected state-integrity diagnostic: {}",
        result.stderr,
    );
}

fn copy_directory(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::create_dir(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory(&source_path, &target_path)?;
        } else {
            fs::copy(source_path, target_path)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn create_directory_alias(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(unix)]
fn remove_directory_alias(link: &Path) -> std::io::Result<()> {
    fs::remove_file(link)
}

#[cfg(windows)]
fn create_directory_alias(target: &Path, link: &Path) -> std::io::Result<()> {
    let status = std::process::Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .status()?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| std::io::Error::other(format!("mklink /J failed with {status}")))
}

#[cfg(windows)]
fn remove_directory_alias(link: &Path) -> std::io::Result<()> {
    fs::remove_dir(link)
}

#[cfg(target_os = "linux")]
struct StateMountCrossing {
    state: std::path::PathBuf,
    active: bool,
}

#[cfg(target_os = "linux")]
impl StateMountCrossing {
    fn install(state: &Path) -> std::io::Result<Self> {
        run_linux_mount_command("mount", &["--bind"], state, Some(state))?;
        Ok(Self {
            state: state.to_owned(),
            active: true,
        })
    }

    fn remove(&mut self) -> std::io::Result<()> {
        if self.active {
            run_linux_mount_command("umount", &[], &self.state, None)?;
            self.active = false;
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl Drop for StateMountCrossing {
    fn drop(&mut self) {
        let _ = self.remove();
    }
}

#[cfg(target_os = "linux")]
struct FileBindMount {
    target: std::path::PathBuf,
    active: bool,
}

#[cfg(target_os = "linux")]
impl FileBindMount {
    fn install(source: &Path, target: &Path) -> std::io::Result<Self> {
        fs::write(target, b"mount target")?;
        if let Err(error) = run_linux_mount_command("mount", &["--bind"], source, Some(target)) {
            fs::remove_file(target)?;
            return Err(error);
        }
        Ok(Self {
            target: target.to_owned(),
            active: true,
        })
    }

    fn remove(&mut self) -> std::io::Result<()> {
        if self.active {
            run_linux_mount_command("umount", &[], &self.target, None)?;
            fs::remove_file(&self.target)?;
            self.active = false;
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl Drop for FileBindMount {
    fn drop(&mut self) {
        let _ = self.remove();
    }
}

#[cfg(target_os = "linux")]
struct DirectoryBindMount {
    target: std::path::PathBuf,
    active: bool,
}

#[cfg(target_os = "linux")]
impl DirectoryBindMount {
    fn install(source: &Path, target: &Path) -> std::io::Result<Self> {
        fs::create_dir(target)?;
        if let Err(error) = run_linux_mount_command("mount", &["--bind"], source, Some(target)) {
            fs::remove_dir(target)?;
            return Err(error);
        }
        Ok(Self {
            target: target.to_owned(),
            active: true,
        })
    }

    fn remove(&mut self) -> std::io::Result<()> {
        if self.active {
            run_linux_mount_command("umount", &[], &self.target, None)?;
            fs::remove_dir(&self.target)?;
            self.active = false;
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl Drop for DirectoryBindMount {
    fn drop(&mut self) {
        let _ = self.remove();
    }
}

#[cfg(target_os = "linux")]
fn run_linux_mount_command(
    program: &str,
    arguments: &[&str],
    path: &Path,
    second_path: Option<&Path>,
) -> std::io::Result<()> {
    let mut diagnostics = Vec::new();
    for privileged in [false, true] {
        let mut command = if privileged {
            let mut command = std::process::Command::new("sudo");
            command.args(["-n", program]);
            command
        } else {
            std::process::Command::new(program)
        };
        command.args(arguments).arg(path);
        if let Some(second_path) = second_path {
            command.arg(second_path);
        }
        match command.output() {
            Ok(output) if output.status.success() => return Ok(()),
            Ok(output) => diagnostics.push(format!(
                "{}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )),
            Err(error) => diagnostics.push(error.to_string()),
        }
    }
    Err(std::io::Error::other(format!(
        "cannot execute Linux {program} fixture: {}",
        diagnostics.join("; ")
    )))
}

#[cfg(windows)]
struct StateMountCrossing {
    state: std::path::PathBuf,
    target: std::path::PathBuf,
    active: bool,
}

#[cfg(windows)]
impl StateMountCrossing {
    fn install(state: &Path) -> std::io::Result<Self> {
        let target = state.with_extension("mount-target");
        fs::rename(state, &target)?;
        if let Err(error) = create_directory_alias(&target, state) {
            fs::rename(&target, state)?;
            return Err(error);
        }
        Ok(Self {
            state: state.to_owned(),
            target,
            active: true,
        })
    }

    fn remove(&mut self) -> std::io::Result<()> {
        if self.active {
            remove_directory_alias(&self.state)?;
            fs::rename(&self.target, &self.state)?;
            self.active = false;
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for StateMountCrossing {
    fn drop(&mut self) {
        let _ = self.remove();
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
struct StateMountCrossing;

#[cfg(not(any(target_os = "linux", windows)))]
impl StateMountCrossing {
    fn install(_state: &Path) -> std::io::Result<Self> {
        Err(std::io::Error::other(
            "state mount fixture supports Windows and Linux",
        ))
    }

    fn remove(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
