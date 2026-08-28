use std::ffi::OsStr;
use std::fs;

use serde_json::{Value, json};

mod support;

use support::{assert_status, field, run};

#[test]
fn public_cache_cleanup_quarantines_payloads_and_replays_one_committed_result()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(
        root.path().join("src/lib.ts"),
        "export const visible = 1;\n",
    )?;

    let initialized = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&initialized, 0);
    let run_id = field(&initialized.stdout, "runId")?;
    let state = root.path().join(".lumin");
    let cache = state.join("cache");
    let cache_anchor = cache.join("namespace.anchor");
    let cache_anchor_bytes = fs::read(&cache_anchor)?;
    fs::create_dir_all(cache.join("nested/deep"))?;
    fs::write(cache.join("nested/deep/payload.bin"), b"nested")?;
    fs::write(cache.join("direct.bin"), b"direct")?;

    let operation_id = "cache-clean-public-0001";
    let cleaned = run(
        root.path(),
        &["cache", "clean", "--operation-id", operation_id],
    )?;
    assert_status(&cleaned, 0);
    assert!(cleaned.stderr.is_empty());
    let request_digest = field(&cleaned.stdout, "requestDigest")?;
    assert_eq!(
        serde_json::from_str::<Value>(&cleaned.stdout)?,
        json!({
            "schemaVersion": "lumin.cache-cleanup.v2",
            "operationId": operation_id,
            "requestDigest": request_digest,
            "status": "clean",
        })
    );
    assert_eq!(
        fs::read_dir(&cache)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<Result<Vec<_>, _>>()?,
        [std::ffi::OsString::from("namespace.anchor")]
    );
    assert_eq!(fs::read(&cache_anchor)?, cache_anchor_bytes);

    let quarantine = state.join("trash/cache-evictions");
    let quarantine_anchor = quarantine.join("namespace.anchor");
    assert!(quarantine_anchor.is_file());
    let payloads = fs::read_dir(&quarantine)?
        .filter_map(|entry| match entry {
            Ok(entry) if entry.file_name() != OsStr::new("namespace.anchor") => Some(Ok(entry)),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect::<Result<Vec<_>, std::io::Error>>()?;
    assert_eq!(payloads.len(), 2);
    assert!(payloads.iter().any(|entry| {
        entry.file_type().is_ok_and(|kind| kind.is_file())
            && fs::read(entry.path()).is_ok_and(|bytes| bytes == b"direct")
    }));
    assert!(payloads.iter().any(|entry| {
        entry.file_type().is_ok_and(|kind| kind.is_dir())
            && fs::read(entry.path().join("deep/payload.bin")).is_ok_and(|bytes| bytes == b"nested")
    }));

    let shown = run(root.path(), &["operation", "show", operation_id])?;
    assert_status(&shown, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&shown.stdout)?,
        json!({
            "schemaVersion": "lumin.cache-cleanup-operation.v2",
            "operationId": operation_id,
            "kind": "cache-clean",
            "requestDigest": request_digest,
            "status": "committed",
            "interruptionCount": 0,
            "authorizedCount": 2,
            "validatedCount": 2,
            "result": {
                "schemaVersion": "lumin.cache-cleanup.v2",
                "operationId": operation_id,
                "requestDigest": request_digest,
                "status": "clean",
            },
            "lastDeliveryStatus": "succeeded",
        })
    );

    let repeated = run(
        root.path(),
        &[
            "cache",
            "clean",
            "--operation-id",
            operation_id,
            "--format",
            "json",
        ],
    )?;
    assert_status(&repeated, 0);
    assert_eq!(repeated.stdout, cleaned.stdout);
    assert_eq!(
        fs::read_dir(&quarantine)?.count(),
        3,
        "same-operation replay moved or deleted quarantine entries",
    );

    let empty_operation_id = "cache-clean-public-empty-0002";
    let empty_clean = run(
        root.path(),
        &[
            "cache",
            "clean",
            "--format",
            "json",
            "--operation-id",
            empty_operation_id,
        ],
    )?;
    assert_status(&empty_clean, 0);
    assert_eq!(
        fs::read_dir(&quarantine)?.count(),
        3,
        "an empty-cache cleanup changed prior authenticated quarantine",
    );
    let empty_shown = run(root.path(), &["operation", "show", empty_operation_id])?;
    assert_status(&empty_shown, 0);
    let empty_shown = serde_json::from_str::<Value>(&empty_shown.stdout)?;
    assert_eq!(
        empty_shown.get("authorizedCount").and_then(Value::as_u64),
        Some(0)
    );
    assert_eq!(
        empty_shown.get("validatedCount").and_then(Value::as_u64),
        Some(0)
    );
    assert_eq!(
        empty_shown.get("status").and_then(Value::as_str),
        Some("committed")
    );

    fs::write(cache.join("later.bin"), b"later")?;
    let empty_replay = run(
        root.path(),
        &["cache", "clean", "--operation-id", empty_operation_id],
    )?;
    assert_status(&empty_replay, 0);
    assert_eq!(empty_replay.stdout, empty_clean.stdout);
    assert_eq!(fs::read(cache.join("later.bin"))?, b"later");
    assert_eq!(fs::read_dir(&quarantine)?.count(), 3);

    let later_operation_id = "cache-clean-public-later-0003";
    let later_clean = run(
        root.path(),
        &["cache", "clean", "--operation-id", later_operation_id],
    )?;
    assert_status(&later_clean, 0);
    assert_eq!(fs::read_dir(&quarantine)?.count(), 4);
    assert!(!cache.join("later.bin").exists());

    let retained = run(root.path(), &["overview", "--run", &run_id])?;
    assert_status(&retained, 0);
    Ok(())
}

#[test]
fn self_hashed_unauthorized_quarantine_is_rejected_without_disposition()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(
        root.path().join("src/lib.ts"),
        "export const visible = 1;\n",
    )?;
    assert_status(&run(root.path(), &["audit", "--jobs", "1"])?, 0);

    let cache = root.path().join(".lumin/cache");
    fs::write(cache.join("payload.bin"), b"payload")?;
    assert_status(
        &run(
            root.path(),
            &["cache", "clean", "--operation-id", "cache-clean-authorized"],
        )?,
        0,
    );

    let quarantine = root.path().join(".lumin/trash/cache-evictions");
    let mut authorized_payloads = fs::read_dir(&quarantine)?
        .filter_map(|entry| match entry {
            Ok(entry) if entry.file_name() != OsStr::new("namespace.anchor") => Some(Ok(entry)),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect::<Result<Vec<_>, std::io::Error>>()?;
    assert_eq!(authorized_payloads.len(), 1);
    let authorized = authorized_payloads
        .pop()
        .ok_or_else(|| std::io::Error::other("authorized quarantine payload disappeared"))?;
    let authorized_path = authorized.path();
    assert_eq!(fs::read(&authorized_path)?, b"payload");

    let foreign_root = tempfile::tempdir()?;
    fs::create_dir(foreign_root.path().join("src"))?;
    fs::write(
        foreign_root.path().join("src/lib.ts"),
        "export const visible = 1;\n",
    )?;
    assert_status(&run(foreign_root.path(), &["audit", "--jobs", "1"])?, 0);
    fs::write(
        foreign_root.path().join(".lumin/cache/foreign.bin"),
        b"foreign",
    )?;
    assert_status(
        &run(
            foreign_root.path(),
            &[
                "cache",
                "clean",
                "--operation-id",
                "cache-clean-foreign-owner",
            ],
        )?,
        0,
    );
    let foreign_quarantine = foreign_root.path().join(".lumin/trash/cache-evictions");
    let mut foreign_payloads = fs::read_dir(&foreign_quarantine)?
        .filter_map(|entry| match entry {
            Ok(entry) if entry.file_name() != OsStr::new("namespace.anchor") => Some(Ok(entry)),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect::<Result<Vec<_>, std::io::Error>>()?;
    assert_eq!(foreign_payloads.len(), 1);
    let foreign_payload = foreign_payloads
        .pop()
        .ok_or_else(|| std::io::Error::other("foreign quarantine payload disappeared"))?;
    let foreign_path = quarantine.join(foreign_payload.file_name());
    assert!(!foreign_path.exists());
    fs::rename(foreign_payload.path(), &foreign_path)?;

    let rejected = run(
        root.path(),
        &[
            "cache",
            "clean",
            "--operation-id",
            "cache-clean-unauthorized",
        ],
    )?;
    assert_status(&rejected, 1);
    assert!(rejected.stdout.is_empty());
    assert!(rejected.stderr.contains("cache quarantine"));
    assert_eq!(fs::read(&authorized_path)?, b"payload");
    assert_eq!(fs::read(&foreign_path)?, b"foreign");
    assert_eq!(
        fs::read_dir(&quarantine)?
            .filter(|entry| entry
                .as_ref()
                .is_ok_and(|entry| { entry.file_name() != OsStr::new("namespace.anchor") }))
            .count(),
        2,
    );
    Ok(())
}

#[test]
fn malformed_cache_cleanup_arguments_do_not_initialize_state()
-> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        vec!["cache", "clean"],
        vec!["cache", "clean", "--operation-id"],
        vec!["cache", "clean", "--operation-id=operation-1"],
        vec!["cache", "clean", "--format=json"],
        vec!["cache", "clean", "--format"],
        vec![
            "cache",
            "clean",
            "--operation-id",
            "operation-1",
            "--operation-id",
            "operation-2",
        ],
        vec![
            "cache",
            "clean",
            "--operation-id",
            "operation-1",
            "--format",
            "yaml",
        ],
        vec![
            "cache",
            "clean",
            "--operation-id",
            "operation-1",
            "--format",
            "json",
            "--format",
            "json",
        ],
        vec![
            "cache",
            "clean",
            "--operation-id",
            "operation-1",
            "--jobs",
            "1",
        ],
        vec![
            "cache",
            "clean",
            "--operation-id",
            "operation-1",
            "unexpected",
        ],
    ];
    for arguments in cases {
        let root = tempfile::tempdir()?;
        let result = run(root.path(), &arguments)?;
        assert_status(&result, 2);
        assert!(result.stdout.is_empty());
        assert!(result.stderr.starts_with("lumin: "));
        assert!(!root.path().join(".lumin").exists());
    }
    Ok(())
}
