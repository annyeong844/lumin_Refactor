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

use super::{
    assert_conflict, assert_delivery_failure, assert_status, field, fixture, open_gate,
    required_string, required_u64, required_value, run, run_with_env, show_operation,
};
use crate::support::lumin_command;
use cache_cleanup_barrier::{CacheCleanupBarrier, PausedCleanup};

const CRASH_POINT_ENV: &str = "LUMIN_TEST_CACHE_CLEANUP_CRASH_POINT";
const INTERRUPTED_BARRIER_ENV: &str = "LUMIN_TEST_CACHE_CLEANUP_INTERRUPTED_BARRIER";
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

    let barrier = CleanupBarrier::new()?;
    let mut retry = barrier.spawn_retry(root.path(), operation_id)?;
    let permit = barrier.accept(&mut retry, operation_id)?;

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

    permit.release()?;
    let recovered = retry.finish()?;
    assert_status(&recovered, 0);
    let committed = show_operation(root.path(), operation_id)?;
    assert_cleanup_projection(&committed, "committed", 1, 2, 2, false)?;
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

    let barrier = CleanupBarrier::new()?;
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
        "lumin.cache-cleanup-operation.v1"
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
}

impl CleanupBarrier {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        Ok(Self { listener })
    }

    fn spawn_retry(
        &self,
        root: &Path,
        operation_id: &str,
    ) -> Result<PausedCleanup, Box<dyn std::error::Error>> {
        let mut command = lumin_command(root)?;
        command
            .args(["cache", "clean", "--operation-id", operation_id])
            .env(
                INTERRUPTED_BARRIER_ENV,
                self.listener.local_addr()?.to_string(),
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
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
                    return CleanupPermit::new(stream, operation_id);
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
                    "cleanup exited before interrupted barrier: status={:?}\nstdout={}\nstderr={}",
                    output.status.code(),
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr),
                ))
                .into());
            }
            if started.elapsed() >= BARRIER_WAIT_LIMIT {
                return Err(
                    std::io::Error::other("cleanup did not reach the interrupted barrier").into(),
                );
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

struct CleanupPermit {
    stream: TcpStream,
}

impl CleanupPermit {
    fn new(stream: TcpStream, operation_id: &str) -> Result<Self, Box<dyn std::error::Error>> {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(BARRIER_WAIT_LIMIT))?;
        let mut frame = String::new();
        BufReader::new(stream.try_clone()?).read_line(&mut frame)?;
        assert_eq!(frame.trim_end(), format!("interrupted {operation_id}"));
        Ok(Self { stream })
    }

    fn release(mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.stream.write_all(b"release\n")?;
        Ok(())
    }
}
