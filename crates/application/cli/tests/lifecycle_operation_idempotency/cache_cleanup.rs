use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::Path;
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

#[path = "../support/cache_cleanup_barrier.rs"]
mod cache_cleanup_barrier;
#[path = "../support/cache_cleanup_delivery_barrier.rs"]
mod cache_cleanup_delivery_barrier;

use super::{
    assert_conflict, assert_delivery_failure, assert_status, field, fixture, open_gate,
    required_string, required_u64, required_value, run, run_with_env, show_operation,
};
use crate::support::lumin_command;
use cache_cleanup_barrier::{CacheCleanupBarrier, PausedCleanup};
use cache_cleanup_delivery_barrier::CacheCleanupDeliveryBarrier;

const CRASH_POINT_ENV: &str = "LUMIN_TEST_CACHE_CLEANUP_CRASH_POINT";
const INTERRUPTED_BARRIER_ENV: &str = "LUMIN_TEST_CACHE_CLEANUP_INTERRUPTED_BARRIER";
const PENDING_BARRIER_ENV: &str = "LUMIN_TEST_CACHE_CLEANUP_PENDING_BARRIER";
const MOVE_BARRIER_ENV: &str = "LUMIN_TEST_CACHE_CLEANUP_MOVE_BARRIER";
const DURABILITY_BARRIER_ENV: &str = "LUMIN_TEST_CACHE_CLEANUP_DURABILITY_BARRIER";
const CRASH_EXIT_CODE: i32 = 95;
const BARRIER_WAIT_LIMIT: Duration = Duration::from_secs(30);

#[test]
fn cache_cleanup_recovers_every_durable_boundary_with_the_same_operation_id()
-> Result<(), Box<dyn std::error::Error>> {
    for (index, (point, expected_validated)) in [
        ("after-authorization", 0),
        ("after-rename-visible:0", 0),
        ("after-physical-durability:0", 0),
        ("after-row-validation:0", 1),
        ("before-result-commit", 2),
    ]
    .into_iter()
    .enumerate()
    {
        let root = fixture()?;
        let initialized = run(root.path(), &["audit", "--jobs", "1"])?;
        assert_status(&initialized, 0);
        seed_cache(root.path())?;
        let operation_id = format!("cache-recovery-{index}");
        let crashed = run_with_env(
            root.path(),
            &["cache", "clean", "--operation-id", operation_id.as_str()],
            &[(CRASH_POINT_ENV, point)],
        )?;
        assert_status(&crashed, CRASH_EXIT_CODE);
        assert!(crashed.stdout.is_empty());
        assert!(crashed.stderr.is_empty());

        let first_show = show_operation(root.path(), &operation_id)?;
        assert_cleanup_projection(&first_show, "pending", 0, 2, expected_validated, true)?;
        let second_show = show_operation(root.path(), &operation_id)?;
        assert_eq!(
            second_show, first_show,
            "operation show changed cleanup state"
        );
        assert_active_cache_writer_blocked(
            root.path(),
            &format!("blocked-after-crash-{index}.bin"),
        )?;

        let foreign = run(
            root.path(),
            &[
                "cache",
                "clean",
                "--operation-id",
                &format!("foreign-cache-recovery-{index}"),
            ],
        )?;
        assert_status(&foreign, 4);
        assert!(foreign.stdout.is_empty());

        let recovered = run(
            root.path(),
            &["cache", "clean", "--operation-id", operation_id.as_str()],
        )?;
        assert_status(&recovered, 0);
        assert!(recovered.stderr.is_empty());
        assert_eq!(field(&recovered.stdout, "operationId")?, operation_id);

        let committed = show_operation(root.path(), &operation_id)?;
        assert_cleanup_projection(&committed, "committed", 1, 2, 2, false)?;
        assert_anchor_only(root.path())?;
        assert_eq!(quarantine_payload_count(root.path())?, 2);
    }
    Ok(())
}

#[test]
fn cleanup_retry_exposes_one_read_only_interrupted_barrier_before_reattachment()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    assert_status(&run(root.path(), &["audit", "--jobs", "1"])?, 0);
    seed_cache(root.path())?;
    let operation_id = "cache-interrupted-barrier";
    let crashed = run_with_env(
        root.path(),
        &["cache", "clean", "--operation-id", operation_id],
        &[(CRASH_POINT_ENV, "after-authorization")],
    )?;
    assert_status(&crashed, CRASH_EXIT_CODE);

    let interrupted_barrier = CleanupBarrier::new(INTERRUPTED_BARRIER_ENV, "interrupted")?;
    let pending_barrier = CleanupBarrier::new(PENDING_BARRIER_ENV, "pending")?;
    let mut retry =
        interrupted_barrier.spawn_retry_with(root.path(), operation_id, Some(&pending_barrier))?;
    let interrupted_permit = interrupted_barrier.accept(&mut retry, operation_id)?;

    let interrupted = show_operation(root.path(), operation_id)?;
    assert_cleanup_projection(&interrupted, "interrupted", 1, 2, 0, true)?;
    assert_eq!(show_operation(root.path(), operation_id)?, interrupted);
    let same_operation = run(
        root.path(),
        &["cache", "clean", "--operation-id", operation_id],
    )?;
    assert_status(&same_operation, 4);
    assert_eq!(show_operation(root.path(), operation_id)?, interrupted);
    let foreign = run(
        root.path(),
        &["cache", "clean", "--operation-id", "foreign-interrupted"],
    )?;
    assert_status(&foreign, 4);
    assert_active_cache_writer_blocked(root.path(), "blocked-while-interrupted.bin")?;
    assert_seed_cache_intact(root.path())?;

    interrupted_permit.release()?;
    let pending_permit = pending_barrier.accept(&mut retry, operation_id)?;
    let pending = show_operation(root.path(), operation_id)?;
    assert_cleanup_projection(&pending, "pending", 1, 2, 0, true)?;
    assert_active_cache_writer_blocked(root.path(), "blocked-after-reattachment.bin")?;
    assert_seed_cache_intact(root.path())?;

    pending_permit.release()?;
    let recovered = retry.finish()?;
    assert_status(&recovered, 0);
    let committed = show_operation(root.path(), operation_id)?;
    assert_cleanup_projection(&committed, "committed", 1, 2, 2, false)?;
    let writer = run(
        root.path(),
        &["cache", "test-write", "allowed-after-commit.bin", "allowed"],
    )?;
    assert_status(&writer, 0);
    assert!(writer.stdout.is_empty());
    assert_eq!(
        fs::read(root.path().join(".lumin/cache/allowed-after-commit.bin"))?,
        b"allowed"
    );
    Ok(())
}

#[test]
fn repeated_recovery_of_one_interrupted_attempt_does_not_increment_twice()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    assert_status(&run(root.path(), &["audit", "--jobs", "1"])?, 0);
    seed_cache(root.path())?;
    let operation_id = "cache-interrupted-retry-death";
    let crashed = run_with_env(
        root.path(),
        &["cache", "clean", "--operation-id", operation_id],
        &[(CRASH_POINT_ENV, "after-authorization")],
    )?;
    assert_status(&crashed, CRASH_EXIT_CODE);

    let barrier = CleanupBarrier::new(INTERRUPTED_BARRIER_ENV, "interrupted")?;
    let mut first_retry = barrier.spawn_retry(root.path(), operation_id)?;
    let permit = barrier.accept(&mut first_retry, operation_id)?;
    let interrupted = show_operation(root.path(), operation_id)?;
    assert_cleanup_projection(&interrupted, "interrupted", 1, 2, 0, true)?;
    drop(first_retry);
    drop(permit);

    let recovered = run(
        root.path(),
        &["cache", "clean", "--operation-id", operation_id],
    )?;
    assert_status(&recovered, 0);
    let committed = show_operation(root.path(), operation_id)?;
    assert_cleanup_projection(&committed, "committed", 1, 2, 2, false)?;
    Ok(())
}

#[test]
fn cache_cleanup_preserves_top_level_and_nested_substitutes_without_advancing_later_rows()
-> Result<(), Box<dyn std::error::Error>> {
    exercise_substitution(false)?;
    exercise_substitution(true)
}

#[test]
fn dirty_cache_tree_hard_stops_before_plan_authorization() -> Result<(), Box<dyn std::error::Error>>
{
    let root = fixture()?;
    assert_status(&run(root.path(), &["audit", "--jobs", "1"])?, 0);
    let cache = root.path().join(".lumin/cache");
    fs::create_dir(cache.join("a-tree"))?;
    fs::write(cache.join("a-tree/child.bin"), b"initial")?;
    fs::write(cache.join("z-later.bin"), b"later")?;
    let operation_id = "cache-dirty-tree";

    let barrier = CacheCleanupBarrier::new(DURABILITY_BARRIER_ENV, "after-initial-flush")?;
    let mut cleanup = barrier.spawn(root.path(), operation_id)?;
    let permit = barrier.accept(&mut cleanup, operation_id)?;
    fs::write(cache.join("a-tree/child.bin"), b"changed after flush")?;
    permit.release()?;

    let failed = cleanup.finish()?;
    assert_status(&failed, 1);
    assert!(failed.stdout.is_empty());
    assert!(failed.stderr.contains("changed while becoming durable"));
    assert_eq!(
        fs::read(cache.join("a-tree/child.bin"))?,
        b"changed after flush"
    );
    assert_eq!(fs::read(cache.join("z-later.bin"))?, b"later");
    assert_eq!(quarantine_payload_count(root.path())?, 0);
    let operation = show_operation(root.path(), operation_id)?;
    assert_cleanup_projection(&operation, "pending", 0, 0, 0, true)?;
    Ok(())
}

#[test]
fn committed_cache_cleanup_recovers_a_failed_delivery_without_another_move()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    assert_status(&run(root.path(), &["audit", "--jobs", "1"])?, 0);
    seed_cache(root.path())?;
    let operation_id = "cache-delivery-recovery";
    assert_delivery_failure(
        root.path(),
        &["cache", "clean", "--operation-id", operation_id],
        operation_id,
    )?;

    let failed_delivery = show_operation(root.path(), operation_id)?;
    assert_cleanup_projection(&failed_delivery, "committed", 0, 2, 2, false)?;
    assert_eq!(
        required_string(&failed_delivery, "/lastDeliveryStatus")?,
        "failed"
    );
    assert_eq!(quarantine_payload_count(root.path())?, 2);

    let replay = run(
        root.path(),
        &["cache", "clean", "--operation-id", operation_id],
    )?;
    assert_status(&replay, 0);
    assert_eq!(field(&replay.stdout, "operationId")?, operation_id);
    assert_eq!(quarantine_payload_count(root.path())?, 2);
    let delivered = show_operation(root.path(), operation_id)?;
    assert_eq!(
        required_string(&delivered, "/lastDeliveryStatus")?,
        "succeeded"
    );
    Ok(())
}

#[test]
fn cleanup_delivery_death_after_allocation_or_stdout_remains_unknown_until_retry()
-> Result<(), Box<dyn std::error::Error>> {
    for (index, stage) in [
        "after-allocation",
        "after-partial-stdout",
        "after-complete-stdout",
    ]
    .into_iter()
    .enumerate()
    {
        let root = fixture()?;
        assert_status(&run(root.path(), &["audit", "--jobs", "1"])?, 0);
        seed_cache(root.path())?;
        let operation_id = format!("cache-delivery-death-{index}");
        let barrier = CacheCleanupDeliveryBarrier::new(stage)?;
        let mut cleanup = barrier.spawn(root.path(), &operation_id)?;
        let (sequence, permit) = barrier.accept(&mut cleanup, &operation_id)?;
        assert_eq!(sequence, 1);

        let unfinished = show_operation(root.path(), &operation_id)?;
        assert_cleanup_projection(&unfinished, "committed", 0, 2, 2, false)?;
        assert_last_delivery(&unfinished, "unknown")?;
        let killed = cleanup.terminate()?;
        drop(permit);
        match stage {
            "after-allocation" => assert!(killed.stdout.is_empty()),
            "after-partial-stdout" => {
                assert!(!killed.stdout.is_empty());
                assert!(!killed.stdout.ends_with('\n'));
                assert!(serde_json::from_str::<Value>(&killed.stdout).is_err());
            }
            "after-complete-stdout" => {
                assert!(killed.stdout.ends_with('\n'));
                let delivered: Value = serde_json::from_str(killed.stdout.trim_end())?;
                assert_eq!(required_string(&delivered, "/operationId")?, operation_id);
            }
            _ => unreachable!(),
        }
        let still_unfinished = show_operation(root.path(), &operation_id)?;
        assert_last_delivery(&still_unfinished, "unknown")?;

        let recovered = run(
            root.path(),
            &["cache", "clean", "--operation-id", &operation_id],
        )?;
        assert_status(&recovered, 0);
        let completed = show_operation(root.path(), &operation_id)?;
        assert_last_delivery(&completed, "succeeded")?;
        assert_anchor_only(root.path())?;
        assert_eq!(quarantine_payload_count(root.path())?, 2);
    }
    Ok(())
}

#[test]
fn concurrent_cleanup_deliveries_obey_allocation_order_in_both_completion_orders()
-> Result<(), Box<dyn std::error::Error>> {
    exercise_delivery_completion_order(true)?;
    exercise_delivery_completion_order(false)
}

fn exercise_delivery_completion_order(lower_first: bool) -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    assert_status(&run(root.path(), &["audit", "--jobs", "1"])?, 0);
    seed_cache(root.path())?;
    let operation_id = if lower_first {
        "cache-delivery-lower-first"
    } else {
        "cache-delivery-greater-first"
    };
    let initial = run(
        root.path(),
        &["cache", "clean", "--operation-id", operation_id],
    )?;
    assert_status(&initial, 0);

    let lower_barrier = CacheCleanupDeliveryBarrier::new("after-allocation")?;
    let mut lower = lower_barrier.spawn(root.path(), operation_id)?;
    let (lower_sequence, lower_permit) = lower_barrier.accept(&mut lower, operation_id)?;
    let greater_barrier = CacheCleanupDeliveryBarrier::new("after-allocation")?;
    let mut greater = greater_barrier.spawn(root.path(), operation_id)?;
    let (greater_sequence, greater_permit) = greater_barrier.accept(&mut greater, operation_id)?;
    assert_eq!(lower_sequence, 2);
    assert_eq!(greater_sequence, 3);
    assert_last_delivery(&show_operation(root.path(), operation_id)?, "unknown")?;

    if lower_first {
        lower_permit.release()?;
        assert_status(&lower.finish()?, 0);
        assert_last_delivery(&show_operation(root.path(), operation_id)?, "unknown")?;
        greater_permit.release()?;
        assert_status(&greater.finish()?, 0);
        assert_last_delivery(&show_operation(root.path(), operation_id)?, "succeeded")?;
    } else {
        greater_permit.release()?;
        assert_status(&greater.finish()?, 0);
        let before_late = run(root.path(), &["operation", "show", operation_id])?;
        assert_status(&before_late, 0);
        let before_late_value: Value = serde_json::from_str(&before_late.stdout)?;
        assert_last_delivery(&before_late_value, "succeeded")?;

        lower_permit.release()?;
        assert_status(&lower.finish()?, 0);
        let after_late = run(root.path(), &["operation", "show", operation_id])?;
        assert_status(&after_late, 0);
        assert_eq!(after_late.stdout, before_late.stdout);
    }
    assert_anchor_only(root.path())?;
    assert_eq!(quarantine_payload_count(root.path())?, 2);
    Ok(())
}

#[test]
fn cache_cleanup_operation_ids_cannot_cross_gate_or_retention_owners()
-> Result<(), Box<dyn std::error::Error>> {
    let gate_first = fixture()?;
    assert_status(&run(gate_first.path(), &["audit", "--jobs", "1"])?, 0);
    open_gate(
        gate_first.path(),
        "cache-global-id-gate-first",
        "src/other.ts",
    )?;
    assert_conflict(run(
        gate_first.path(),
        &[
            "cache",
            "clean",
            "--operation-id",
            "cache-global-id-gate-first",
        ],
    )?);

    let cache_first = fixture()?;
    assert_status(&run(cache_first.path(), &["audit", "--jobs", "1"])?, 0);
    assert_status(
        &run(
            cache_first.path(),
            &[
                "cache",
                "clean",
                "--operation-id",
                "cache-global-id-cache-first-gate",
            ],
        )?,
        0,
    );
    assert_conflict(run(
        cache_first.path(),
        &[
            "pre-write",
            "--operation-id",
            "cache-global-id-cache-first-gate",
            "--path",
            "src/other.ts",
            "--jobs",
            "1",
        ],
    )?);

    let retention_first = fixture()?;
    let audited = run(retention_first.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audited, 0);
    let run_id = field(&audited.stdout, "runId")?;
    assert_status(
        &run(
            retention_first.path(),
            &[
                "runs",
                "pin",
                &run_id,
                "--operation-id",
                "cache-global-id-retention-first",
                "--reason",
                "cross-owner collision",
            ],
        )?,
        0,
    );
    assert_conflict(run(
        retention_first.path(),
        &[
            "cache",
            "clean",
            "--operation-id",
            "cache-global-id-retention-first",
        ],
    )?);

    let cache_then_retention = fixture()?;
    let audited = run(cache_then_retention.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audited, 0);
    let run_id = field(&audited.stdout, "runId")?;
    assert_status(
        &run(
            cache_then_retention.path(),
            &[
                "cache",
                "clean",
                "--operation-id",
                "cache-global-id-cache-first-retention",
            ],
        )?,
        0,
    );
    assert_conflict(run(
        cache_then_retention.path(),
        &[
            "runs",
            "pin",
            &run_id,
            "--operation-id",
            "cache-global-id-cache-first-retention",
            "--reason",
            "cross-owner collision",
        ],
    )?);
    Ok(())
}

fn exercise_substitution(nested: bool) -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    assert_status(&run(root.path(), &["audit", "--jobs", "1"])?, 0);
    let cache = root.path().join(".lumin/cache");
    let operation_id = if nested {
        fs::create_dir(cache.join("a-tree"))?;
        fs::write(cache.join("a-tree/child.bin"), b"nested-original")?;
        fs::write(cache.join("z-later.bin"), b"later")?;
        "cache-nested-substitute"
    } else {
        fs::write(cache.join("a-first.bin"), b"top-original")?;
        fs::write(cache.join("z-later.bin"), b"later")?;
        "cache-top-substitute"
    };

    let barrier = CacheCleanupBarrier::new(MOVE_BARRIER_ENV, "before-move")?;
    let mut cleanup = barrier.spawn(root.path(), operation_id)?;
    let permit = barrier.accept(&mut cleanup, operation_id)?;
    let saved = root.path().join(if nested {
        "saved-nested-original.bin"
    } else {
        "saved-top-original.bin"
    });
    let target = if nested {
        cache.join("a-tree/child.bin")
    } else {
        cache.join("a-first.bin")
    };
    fs::rename(&target, &saved)?;
    fs::write(&target, b"substitute")?;
    permit.release()?;

    let failed = cleanup.finish()?;
    assert_status(&failed, 1);
    assert!(failed.stdout.is_empty());
    assert!(failed.stderr.starts_with("lumin: "));
    let original: &[u8] = if nested {
        b"nested-original"
    } else {
        b"top-original"
    };
    assert!(tree_contains_bytes(root.path(), original)?);
    assert_eq!(fs::read(cache.join("z-later.bin"))?, b"later");
    assert!(
        tree_contains_bytes(
            &root.path().join(".lumin/trash/cache-evictions"),
            b"substitute",
        )? || tree_contains_bytes(&cache, b"substitute")?
    );
    let operation = show_operation(root.path(), operation_id)?;
    assert_cleanup_projection(&operation, "pending", 0, 2, 0, true)?;
    assert!(cache.join("namespace.anchor").is_file());
    assert!(
        root.path()
            .join(".lumin/trash/cache-evictions/namespace.anchor")
            .is_file()
    );
    Ok(())
}

fn seed_cache(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let cache = root.join(".lumin/cache");
    fs::write(cache.join("first.bin"), b"first")?;
    fs::create_dir(cache.join("second"))?;
    fs::write(cache.join("second/nested.bin"), b"second")?;
    Ok(())
}

fn assert_seed_cache_intact(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(fs::read(root.join(".lumin/cache/first.bin"))?, b"first");
    assert_eq!(
        fs::read(root.join(".lumin/cache/second/nested.bin"))?,
        b"second"
    );
    Ok(())
}

fn assert_active_cache_writer_blocked(
    root: &Path,
    name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let result = run(root, &["cache", "test-write", name, "must-not-be-written"])?;
    assert_status(&result, 4);
    assert!(result.stdout.is_empty());
    assert!(!root.join(".lumin/cache").join(name).exists());
    Ok(())
}

fn assert_cleanup_projection(
    operation: &Value,
    status: &str,
    interruptions: u64,
    authorized: u64,
    validated: u64,
    result_is_null: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        required_string(operation, "/schemaVersion")?,
        "lumin.cache-cleanup-operation.v2"
    );
    assert_eq!(required_string(operation, "/kind")?, "cache-clean");
    assert_eq!(required_string(operation, "/status")?, status);
    assert_eq!(
        required_u64(operation, "/interruptionCount")?,
        interruptions
    );
    assert_eq!(required_u64(operation, "/authorizedCount")?, authorized);
    assert_eq!(required_u64(operation, "/validatedCount")?, validated);
    assert_eq!(
        required_value(operation, "/result")?.is_null(),
        result_is_null
    );
    Ok(())
}

fn assert_last_delivery(
    operation: &Value,
    expected: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(required_string(operation, "/lastDeliveryStatus")?, expected);
    Ok(())
}

fn assert_anchor_only(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let names = fs::read_dir(root.join(".lumin/cache"))?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(names, [std::ffi::OsString::from("namespace.anchor")]);
    Ok(())
}

fn quarantine_payload_count(root: &Path) -> Result<usize, Box<dyn std::error::Error>> {
    Ok(fs::read_dir(root.join(".lumin/trash/cache-evictions"))?
        .filter(|entry| {
            entry
                .as_ref()
                .is_ok_and(|entry| entry.file_name() != "namespace.anchor")
        })
        .count())
}

fn tree_contains_bytes(root: &Path, expected: &[u8]) -> Result<bool, Box<dyn std::error::Error>> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            if tree_contains_bytes(&entry.path(), expected)? {
                return Ok(true);
            }
        } else if kind.is_file() && fs::read(entry.path())? == expected {
            return Ok(true);
        }
    }
    Ok(false)
}

struct CleanupBarrier {
    listener: TcpListener,
    environment: &'static str,
    stage: &'static str,
}

impl CleanupBarrier {
    fn new(
        environment: &'static str,
        stage: &'static str,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            environment,
            stage,
        })
    }

    fn spawn_retry(
        &self,
        root: &Path,
        operation_id: &str,
    ) -> Result<PausedCleanup, Box<dyn std::error::Error>> {
        self.spawn_retry_with(root, operation_id, None)
    }

    fn spawn_retry_with(
        &self,
        root: &Path,
        operation_id: &str,
        additional: Option<&Self>,
    ) -> Result<PausedCleanup, Box<dyn std::error::Error>> {
        let mut command = lumin_command(root)?;
        command
            .args(["cache", "clean", "--operation-id", operation_id])
            .env(self.environment, self.listener.local_addr()?.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(additional) = additional {
            command.env(
                additional.environment,
                additional.listener.local_addr()?.to_string(),
            );
        }
        Ok(PausedCleanup::from_child(command.spawn()?))
    }

    fn accept(
        &self,
        process: &mut PausedCleanup,
        operation_id: &str,
    ) -> Result<CleanupPermit, Box<dyn std::error::Error>> {
        let started = Instant::now();
        loop {
            match self.listener.accept() {
                Ok((stream, peer)) if peer.ip().is_loopback() => {
                    return CleanupPermit::new(stream, self.stage, operation_id);
                }
                Ok(_) => {
                    return Err(std::io::Error::other(
                        "cleanup barrier accepted a non-loopback peer",
                    )
                    .into());
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
            if process.has_exited()? {
                let output = process.take_output()?;
                return Err(std::io::Error::other(format!(
                    "cleanup exited before {} barrier: status={:?}\nstdout={}\nstderr={}",
                    self.stage,
                    output.status.code(),
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr),
                ))
                .into());
            }
            if started.elapsed() >= BARRIER_WAIT_LIMIT {
                return Err(std::io::Error::other(format!(
                    "cleanup did not reach {} barrier",
                    self.stage
                ))
                .into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

struct CleanupPermit {
    stream: TcpStream,
}

impl CleanupPermit {
    fn new(
        stream: TcpStream,
        stage: &str,
        operation_id: &str,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(BARRIER_WAIT_LIMIT))?;
        let mut frame = String::new();
        BufReader::new(stream.try_clone()?).read_line(&mut frame)?;
        assert_eq!(frame.trim_end(), format!("{stage} {operation_id}"));
        Ok(Self { stream })
    }

    fn release(mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.stream.write_all(b"release\n")?;
        Ok(())
    }
}
