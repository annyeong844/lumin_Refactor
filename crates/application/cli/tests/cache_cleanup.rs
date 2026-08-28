use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use lumin_model::{PhysicalFileIdentity, RepoPath};

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
        cleaned.stdout,
        cleanup_response_bytes(operation_id, &request_digest)
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
    let quarantine_after_clean = quarantine_tree_snapshot(&quarantine)?;

    let shown = run(root.path(), &["operation", "show", operation_id])?;
    assert_status(&shown, 0);
    assert_eq!(
        shown.stdout,
        cleanup_operation_bytes(operation_id, &request_digest, 2, 2)
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
        quarantine_tree_snapshot(&quarantine)?,
        quarantine_after_clean,
        "same-operation replay changed authenticated quarantine identities or payloads",
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
        empty_clean.stdout,
        cleanup_response_bytes(empty_operation_id, &request_digest)
    );
    assert_eq!(
        quarantine_tree_snapshot(&quarantine)?,
        quarantine_after_clean,
        "an empty-cache cleanup changed prior authenticated quarantine identities or payloads",
    );
    let empty_shown = run(root.path(), &["operation", "show", empty_operation_id])?;
    assert_status(&empty_shown, 0);
    assert_eq!(
        empty_shown.stdout,
        cleanup_operation_bytes(empty_operation_id, &request_digest, 0, 0)
    );

    fs::write(cache.join("later.bin"), b"later")?;
    let quarantine_before_empty_replay = quarantine_tree_snapshot(&quarantine)?;
    let empty_replay = run(
        root.path(),
        &["cache", "clean", "--operation-id", empty_operation_id],
    )?;
    assert_status(&empty_replay, 0);
    assert_eq!(empty_replay.stdout, empty_clean.stdout);
    assert_eq!(fs::read(cache.join("later.bin"))?, b"later");
    assert_eq!(
        quarantine_tree_snapshot(&quarantine)?,
        quarantine_before_empty_replay,
        "same-operation empty replay changed authenticated quarantine identities or payloads",
    );

    let later_operation_id = "cache-clean-public-later-0003";
    let later_clean = run(
        root.path(),
        &["cache", "clean", "--operation-id", later_operation_id],
    )?;
    assert_status(&later_clean, 0);
    assert_eq!(
        later_clean.stdout,
        cleanup_response_bytes(later_operation_id, &request_digest)
    );
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
    let foreign_original_path = foreign_payload.path();
    let foreign_path = quarantine.join(foreign_payload.file_name());
    assert!(!foreign_path.exists());
    fs::rename(&foreign_original_path, &foreign_path)?;

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
    fs::rename(&foreign_path, &foreign_original_path)?;
    let absent = run(
        root.path(),
        &["operation", "show", "cache-clean-unauthorized"],
    )?;
    assert_status(&absent, 2);
    assert!(absent.stdout.is_empty());
    assert!(absent.stderr.contains("operation does not exist"));

    fs::write(cache.join("after-rejection.bin"), b"after rejection")?;
    let subsequent = run(
        root.path(),
        &[
            "cache",
            "clean",
            "--operation-id",
            "cache-clean-after-unauthorized",
        ],
    )?;
    assert_status(&subsequent, 0);
    assert!(!cache.join("after-rejection.bin").exists());
    Ok(())
}

fn cleanup_response_bytes(operation_id: &str, request_digest: &str) -> String {
    format!(
        concat!(
            "{{\"schemaVersion\":\"lumin.cache-cleanup.v2\",",
            "\"operationId\":\"{operation_id}\",",
            "\"requestDigest\":\"{request_digest}\",",
            "\"status\":\"clean\"}}\n"
        ),
        operation_id = operation_id,
        request_digest = request_digest,
    )
}

fn cleanup_operation_bytes(
    operation_id: &str,
    request_digest: &str,
    authorized_count: u64,
    validated_count: u64,
) -> String {
    format!(
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
            "\"lastDeliveryStatus\":\"succeeded\"}}\n"
        ),
        operation_id = operation_id,
        request_digest = request_digest,
        authorized_count = authorized_count,
        validated_count = validated_count,
    )
}

#[derive(Debug, Eq, PartialEq)]
struct QuarantineSnapshotRow {
    relative_path: PathBuf,
    kind: QuarantineSnapshotKind,
    physical_identity: PhysicalFileIdentity,
    payload: Option<Vec<u8>>,
}

#[derive(Debug, Eq, PartialEq)]
enum QuarantineSnapshotKind {
    Directory,
    RegularFile,
}

fn quarantine_tree_snapshot(
    root: &Path,
) -> Result<Vec<QuarantineSnapshotRow>, Box<dyn std::error::Error>> {
    let mut rows = Vec::new();
    snapshot_quarantine_entry(root, root, Path::new(""), &mut rows)?;
    Ok(rows)
}

fn snapshot_quarantine_entry(
    root: &Path,
    path: &Path,
    relative_path: &Path,
    rows: &mut Vec<QuarantineSnapshotRow>,
) -> Result<(), Box<dyn std::error::Error>> {
    let metadata = fs::symlink_metadata(path)?;
    let file_type = metadata.file_type();
    let logical = RepoPath::from_native_relative(relative_path)?;
    let physical_identity = lumin_engine::path_physical_identity_for_test(root, &logical)?;
    let (kind, payload) = if file_type.is_dir() {
        (QuarantineSnapshotKind::Directory, None)
    } else if file_type.is_file() {
        (QuarantineSnapshotKind::RegularFile, Some(fs::read(path)?))
    } else {
        return Err(std::io::Error::other(format!(
            "quarantine snapshot contains unsupported entry: {}",
            path.display()
        ))
        .into());
    };
    rows.push(QuarantineSnapshotRow {
        relative_path: relative_path.to_path_buf(),
        kind,
        physical_identity,
        payload,
    });

    if file_type.is_dir() {
        let mut children = fs::read_dir(path)?
            .map(|entry| {
                let entry = entry?;
                Ok::<_, std::io::Error>((entry.file_name(), entry.path()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        children.sort_by(|left, right| left.0.cmp(&right.0));
        for (name, child) in children {
            snapshot_quarantine_entry(root, &child, &relative_path.join(name), rows)?;
        }
    }
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
