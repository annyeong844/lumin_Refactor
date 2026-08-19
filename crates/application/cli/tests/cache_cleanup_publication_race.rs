use std::ffi::OsString;
use std::fs;

use serde_json::Value;

#[path = "support/cache_cleanup_barrier.rs"]
mod cache_cleanup_barrier;
#[path = "support/publication_barrier.rs"]
mod publication_barrier;
mod support;

use cache_cleanup_barrier::CacheCleanupBarrier;
use publication_barrier::PublicationBarrier;
use support::publication::{assert_no_attempt_liveness_files, baseline_repository, json, number};
use support::{assert_status, field, run};

const TARGET_ATTEMPT: &str = "attempt_0000000000000002";
const TARGET_RUN: &str = "run_0000000000000002";
const PREPARED_BARRIER_ENV: &str = "LUMIN_TEST_PUBLICATION_PREPARED_BARRIER";
const CONTENDED_BARRIER_ENV: &str = "LUMIN_TEST_PUBLICATION_CONTENDED_BARRIER";
const MOVE_BARRIER_ENV: &str = "LUMIN_TEST_CACHE_CLEANUP_MOVE_BARRIER";

#[test]
fn cache_cleanup_and_publication_serialize_through_one_exclusive_guard()
-> Result<(), Box<dyn std::error::Error>> {
    let (root, baseline) = baseline_repository()?;
    assert_eq!(number(&baseline.stdout, "sequence")?, 1);
    assert_eq!(field(&baseline.stdout, "runId")?, "run_0000000000000001");
    fs::write(root.path().join(".lumin/cache/race.bin"), b"cache-race")?;

    let prepared = PublicationBarrier::new(PREPARED_BARRIER_ENV, "prepared")?;
    let contended = PublicationBarrier::new(CONTENDED_BARRIER_ENV, "contended")?;
    let mut audit = prepared.spawn_audit(root.path(), &[&contended])?;
    let prepared_permit = prepared.accept(&mut audit, TARGET_ATTEMPT)?;

    let cleanup_barrier = CacheCleanupBarrier::new(MOVE_BARRIER_ENV, "before-move")?;
    let operation_id = "cache-publication-race";
    let mut cleanup = cleanup_barrier.spawn(root.path(), operation_id)?;
    let cleanup_permit = cleanup_barrier.accept(&mut cleanup, operation_id)?;

    prepared_permit.release()?;
    let contended_permit = contended.accept(&mut audit, TARGET_ATTEMPT)?;

    cleanup_permit.release()?;
    let cleaned = cleanup.finish()?;
    assert_status(&cleaned, 0);
    assert_eq!(field(&cleaned.stdout, "operationId")?, operation_id);

    contended_permit.release()?;
    let published = audit.finish()?;
    assert_status(&published, 0);
    assert_eq!(field(&published.stdout, "runId")?, TARGET_RUN);

    let operation = run(root.path(), &["operation", "show", operation_id])?;
    assert_status(&operation, 0);
    let operation = json(&operation.stdout)?;
    assert_eq!(
        operation.get("status").and_then(Value::as_str),
        Some("committed")
    );
    assert_eq!(
        operation.get("authorizedCount").and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        operation.get("validatedCount").and_then(Value::as_u64),
        Some(1)
    );

    let overview = run(root.path(), &["overview"])?;
    assert_status(&overview, 0);
    let overview = json(&overview.stdout)?;
    assert_eq!(
        overview.pointer("/scope/id").and_then(Value::as_str),
        Some(TARGET_RUN)
    );
    assert_eq!(
        fs::read_dir(root.path().join(".lumin/cache"))?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<Result<Vec<_>, _>>()?,
        [OsString::from("namespace.anchor")]
    );
    assert_eq!(
        fs::read_dir(root.path().join(".lumin/trash/cache-evictions"))?.count(),
        2
    );
    assert_no_attempt_liveness_files(root.path())?;
    Ok(())
}
