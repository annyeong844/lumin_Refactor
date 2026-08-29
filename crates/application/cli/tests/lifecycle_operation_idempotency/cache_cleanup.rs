use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

use lumin_model::{OperationId, PhysicalFileIdentity, RepoPath, decode_native_path_component};
use serde_json::{Value, json};

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
const AUTHORIZED_BARRIER_ENV: &str = "LUMIN_TEST_CACHE_CLEANUP_AUTHORIZED_BARRIER";
const MOVE_BARRIER_ENV: &str = "LUMIN_TEST_CACHE_CLEANUP_MOVE_BARRIER";
const DURABILITY_BARRIER_ENV: &str = "LUMIN_TEST_CACHE_CLEANUP_DURABILITY_BARRIER";
const POST_MOVE_BARRIER_ENV: &str = "LUMIN_TEST_CACHE_CLEANUP_POST_MOVE_BARRIER";
const CRASH_EXIT_CODE: i32 = 95;
const BARRIER_WAIT_LIMIT: Duration = Duration::from_secs(30);

#[test]
fn cache_cleanup_recovers_every_durable_boundary_with_the_same_operation_id()
-> Result<(), Box<dyn std::error::Error>> {
    for (index, (point, expected_validated, at_destination)) in [
        ("after-authorization", 0, [false, false]),
        ("after-rename-visible:0", 0, [true, false]),
        ("after-physical-durability:0", 0, [true, false]),
        ("after-row-validation:0", 1, [true, false]),
        ("after-rename-visible:1", 1, [true, true]),
        ("after-physical-durability:1", 1, [true, true]),
        ("after-row-validation:1", 2, [true, true]),
        ("before-result-commit", 2, [true, true]),
    ]
    .into_iter()
    .enumerate()
    {
        let root = fixture()?;
        let initialized = run(root.path(), &["audit", "--jobs", "1"])?;
        assert_status(&initialized, 0);
        seed_cache(root.path())?;
        let cache = root.path().join(".lumin/cache");
        let source_payloads = [
            physical_tree_snapshot(&cache, &cache.join("first.bin"))?,
            physical_tree_snapshot(&cache, &cache.join("second"))?,
        ];
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
        let private_after_crash = cleanup_private_state(root.path(), &operation_id)?;
        assert_crashed_cleanup_state(
            root.path(),
            &private_after_crash,
            &operation_id,
            expected_validated,
            at_destination,
        )?;
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
        assert_eq!(
            cleanup_private_state(root.path(), &operation_id)?,
            private_after_crash,
            "read-only and conflicting commands changed the crashed cleanup snapshot"
        );

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
        let committed_private = cleanup_private_state(root.path(), &operation_id)?;
        let quarantine = root.path().join(".lumin/trash/cache-evictions");
        let destinations = cleanup_authorization_destinations(&committed_private)?;
        assert_eq!(destinations.len(), source_payloads.len());
        for (destination, source_payload) in destinations.iter().zip(&source_payloads) {
            assert_eq!(
                physical_tree_snapshot(&quarantine, &quarantine.join(destination))?,
                *source_payload,
                "cleanup recovery changed a quarantine object's identity or tree manifest"
            );
        }
        assert_eq!(
            committed_private,
            expected_recovered_cleanup_state(private_after_crash, &operation_id)?,
            "cleanup recovery changed durable state outside the exact owner transition"
        );
    }
    Ok(())
}

#[test]
fn cache_cleanup_recovers_death_after_result_commit_without_duplicate_move()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    assert_status(&run(root.path(), &["audit", "--jobs", "1"])?, 0);
    seed_cache(root.path())?;
    let cache = root.path().join(".lumin/cache");
    let source_payloads = [
        physical_tree_snapshot(&cache, &cache.join("first.bin"))?,
        physical_tree_snapshot(&cache, &cache.join("second"))?,
    ];
    let operation_id = "cache-after-result-commit";

    let crashed = run_with_env(
        root.path(),
        &["cache", "clean", "--operation-id", operation_id],
        &[(CRASH_POINT_ENV, "after-result-commit")],
    )?;
    assert_status(&crashed, CRASH_EXIT_CODE);
    assert!(crashed.stdout.is_empty());
    assert!(crashed.stderr.is_empty());

    let committed = show_operation(root.path(), operation_id)?;
    assert_cleanup_projection(&committed, "committed", 0, 2, 2, false)?;
    assert_last_delivery(&committed, "not-attempted")?;
    let private_before_replay = cleanup_private_state(root.path(), operation_id)?;
    let quarantine = root.path().join(".lumin/trash/cache-evictions");
    let destinations = cleanup_authorization_destinations(&private_before_replay)?;
    assert_eq!(destinations.len(), source_payloads.len());
    for (destination, source_payload) in destinations.iter().zip(&source_payloads) {
        assert_eq!(
            physical_tree_snapshot(&quarantine, &quarantine.join(destination))?,
            *source_payload,
            "result commit changed an authorized quarantine payload"
        );
    }

    let later = run(root.path(), &["cache", "test-write", "later.bin", "later"])?;
    assert_status(&later, 0);
    let replay = run(
        root.path(),
        &["cache", "clean", "--operation-id", operation_id],
    )?;
    assert_status(&replay, 0);
    assert!(replay.stderr.is_empty());
    assert_eq!(field(&replay.stdout, "operationId")?, operation_id);
    assert_eq!(fs::read(cache.join("later.bin"))?, b"later");
    for (destination, source_payload) in destinations.iter().zip(&source_payloads) {
        assert_eq!(
            physical_tree_snapshot(&quarantine, &quarantine.join(destination))?,
            *source_payload,
            "same-ID delivery recovery replaced an authorized quarantine payload"
        );
    }
    assert_eq!(
        cleanup_private_state(root.path(), operation_id)?,
        expected_delivery_completion(private_before_replay, &[(1, "succeeded")])?,
        "same-ID recovery changed state outside the first delivery completion"
    );

    let later_cleanup = run(
        root.path(),
        &[
            "cache",
            "clean",
            "--operation-id",
            "cache-after-result-later",
        ],
    )?;
    assert_status(&later_cleanup, 0);
    assert!(!cache.join("later.bin").exists());
    assert_eq!(quarantine_payload_count(root.path())?, 3);
    Ok(())
}

#[test]
fn cache_cleanup_real_stream_failures_are_durable_and_recoverable()
-> Result<(), Box<dyn std::error::Error>> {
    for broken_pipe in [true, false] {
        let root = fixture()?;
        assert_status(&run(root.path(), &["audit", "--jobs", "1"])?, 0);
        seed_cache(root.path())?;
        let operation_id = if broken_pipe {
            "cache-real-broken-pipe"
        } else {
            "cache-real-non-pipe"
        };
        let barrier = CacheCleanupDeliveryBarrier::new("after-allocation")?;
        let mut cleanup = if broken_pipe {
            barrier.spawn(root.path(), operation_id)?
        } else {
            barrier.spawn_with_stdout(
                root.path(),
                operation_id,
                non_pipe_failure_stdout(root.path())?,
            )?
        };
        let (sequence, permit) = barrier.accept(&mut cleanup, operation_id)?;
        assert_eq!(sequence, 1);
        let allocated = show_operation(root.path(), operation_id)?;
        assert_cleanup_projection(&allocated, "committed", 0, 2, 2, false)?;
        assert_last_delivery(&allocated, "unknown")?;
        if broken_pipe {
            cleanup.close_stdout()?;
        }
        permit.release()?;

        let failed = cleanup.finish()?;
        assert_status(&failed, 1);
        assert!(failed.stdout.is_empty());
        assert_eq!(
            failed.stderr,
            if broken_pipe {
                ""
            } else {
                "lumin: cannot write stdout\n"
            }
        );
        let failed_show = show_operation(root.path(), operation_id)?;
        assert_cleanup_projection(&failed_show, "committed", 0, 2, 2, false)?;
        assert_last_delivery(&failed_show, "failed")?;
        let private_after_failure = cleanup_private_state(root.path(), operation_id)?;
        assert_eq!(
            required_value(&private_after_failure, "/operation/deliveryCompletions")?,
            &json!([{"sequence": 1, "outcome": "failed"}])
        );

        let quarantine = root.path().join(".lumin/trash/cache-evictions");
        let destinations = cleanup_authorization_destinations(&private_after_failure)?;
        let before_replay = destinations
            .iter()
            .map(|destination| physical_tree_snapshot(&quarantine, &quarantine.join(destination)))
            .collect::<Result<Vec<_>, _>>()?;
        let later = run(root.path(), &["cache", "test-write", "later.bin", "later"])?;
        assert_status(&later, 0);
        let replay = run(
            root.path(),
            &["cache", "clean", "--operation-id", operation_id],
        )?;
        assert_status(&replay, 0);
        assert_eq!(field(&replay.stdout, "operationId")?, operation_id);
        assert_eq!(
            fs::read(root.path().join(".lumin/cache/later.bin"))?,
            b"later"
        );
        for (destination, before) in destinations.iter().zip(&before_replay) {
            assert_eq!(
                physical_tree_snapshot(&quarantine, &quarantine.join(destination))?,
                *before,
                "delivery retry changed an authenticated quarantine object"
            );
        }
        assert_eq!(
            cleanup_private_state(root.path(), operation_id)?,
            expected_delivery_completion(
                private_after_failure,
                &[(1, "failed"), (2, "succeeded")],
            )?,
            "delivery retry changed state outside the ordered delivery ledger"
        );
    }
    Ok(())
}

#[cfg(unix)]
fn non_pipe_failure_stdout(_root: &Path) -> std::io::Result<Stdio> {
    fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .map(Stdio::from)
}

#[cfg(windows)]
fn non_pipe_failure_stdout(root: &Path) -> std::io::Result<Stdio> {
    let read_only_path = root.join("read-only-stdout.bin");
    fs::write(&read_only_path, b"sentinel")?;
    fs::File::open(read_only_path).map(Stdio::from)
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
    let mut retry = interrupted_barrier.spawn_retry_with(
        root.path(),
        operation_id,
        Some(PENDING_BARRIER_ENV),
    )?;
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
    let interrupted_private_before_writer = cleanup_private_state(root.path(), operation_id)?;
    assert_active_cache_writer_blocked(root.path(), "blocked-while-interrupted.bin")?;
    assert_eq!(
        cleanup_private_state(root.path(), operation_id)?,
        interrupted_private_before_writer,
        "rejected writer changed the interrupted cleanup snapshot"
    );
    assert_seed_cache_intact(root.path())?;

    let recovery_permit = interrupted_permit.release_for_next()?;
    let pending_permit = recovery_permit.wait_for_stage("pending", operation_id)?;
    let pending = show_operation(root.path(), operation_id)?;
    assert_cleanup_projection(&pending, "pending", 1, 2, 0, true)?;
    let pending_private_before_writer = cleanup_private_state(root.path(), operation_id)?;
    assert_active_cache_writer_blocked(root.path(), "blocked-after-reattachment.bin")?;
    assert_eq!(
        cleanup_private_state(root.path(), operation_id)?,
        pending_private_before_writer,
        "rejected writer changed the reattached cleanup snapshot"
    );
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
    exercise_dirty_payload(false)?;
    exercise_dirty_payload(true)?;
    exercise_post_move_dirty_payload(false)?;
    exercise_post_move_dirty_payload(true)
}

fn exercise_dirty_payload(nested: bool) -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    assert_status(&run(root.path(), &["audit", "--jobs", "1"])?, 0);
    let cache = root.path().join(".lumin/cache");
    let target = if nested {
        fs::create_dir(cache.join("a-tree"))?;
        cache.join("a-tree/child.bin")
    } else {
        cache.join("a-file.bin")
    };
    fs::write(&target, b"initial")?;
    fs::write(cache.join("z-later.bin"), b"later")?;
    let operation_id = if nested {
        "cache-dirty-tree"
    } else {
        "cache-dirty-file"
    };

    let barrier = CacheCleanupBarrier::new(DURABILITY_BARRIER_ENV, "after-initial-flush")?;
    let mut cleanup = barrier.spawn(root.path(), operation_id)?;
    let permit = barrier.accept(&mut cleanup, operation_id)?;
    fs::write(&target, b"changed after flush")?;
    permit.release()?;

    let failed = cleanup.finish()?;
    assert_status(&failed, 1);
    assert!(failed.stdout.is_empty());
    assert!(failed.stderr.contains("changed while becoming durable"));
    assert_eq!(fs::read(&target)?, b"changed after flush");
    assert_eq!(fs::read(cache.join("z-later.bin"))?, b"later");
    assert_eq!(quarantine_payload_count(root.path())?, 0);
    let operation = show_operation(root.path(), operation_id)?;
    assert_cleanup_projection(&operation, "pending", 0, 0, 0, true)?;
    Ok(())
}

fn exercise_post_move_dirty_payload(nested: bool) -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    assert_status(&run(root.path(), &["audit", "--jobs", "1"])?, 0);
    let cache = root.path().join(".lumin/cache");
    let source_name = if nested { "a-tree" } else { "a-file.bin" };
    let source = cache.join(source_name);
    let source_payload = if nested {
        fs::create_dir(&source)?;
        source.join("child.bin")
    } else {
        source.clone()
    };
    fs::write(&source_payload, b"initial")?;
    fs::write(cache.join("z-later.bin"), b"later")?;
    let operation_id = if nested {
        "cache-dirty-tree-after-move"
    } else {
        "cache-dirty-file-after-move"
    };

    let barrier = CacheCleanupBarrier::new(POST_MOVE_BARRIER_ENV, "after-move")?;
    let mut cleanup = barrier.spawn(root.path(), operation_id)?;
    let permit = barrier.accept(&mut cleanup, operation_id)?;
    let quarantine = root.path().join(".lumin/trash/cache-evictions");
    let mut moved = fs::read_dir(&quarantine)?
        .filter_map(|entry| match entry {
            Ok(entry) if entry.file_name() != "namespace.anchor" => Some(Ok(entry.path())),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect::<Result<Vec<_>, std::io::Error>>()?;
    assert_eq!(moved.len(), 1);
    let moved = moved
        .pop()
        .ok_or_else(|| std::io::Error::other("moved cache payload disappeared"))?;
    let moved_payload = if nested {
        moved.join("child.bin")
    } else {
        moved.clone()
    };
    fs::write(&moved_payload, b"changed after move")?;
    permit.release()?;

    let failed = cleanup.finish()?;
    assert_status(&failed, 1);
    assert!(failed.stdout.is_empty());
    assert!(
        failed
            .stderr
            .contains("moved cache payload disagrees with its authorization")
    );
    assert!(!source.exists());
    assert_eq!(fs::read(&moved_payload)?, b"changed after move");
    assert_eq!(fs::read(cache.join("z-later.bin"))?, b"later");

    let operation = show_operation(root.path(), operation_id)?;
    assert_cleanup_projection(&operation, "pending", 0, 2, 0, true)?;
    let private = cleanup_private_state(root.path(), operation_id)?;
    let destinations = assert_pending_cleanup_authorizations(
        &private,
        operation_id,
        &[source_name, "z-later.bin"],
        0,
    )?;
    assert_eq!(destinations.len(), 2);
    assert_eq!(quarantine.join(&destinations[0]), moved);
    assert!(quarantine.join(&destinations[0]).exists());
    assert!(!quarantine.join(&destinations[1]).exists());
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
        let before_greater_private = cleanup_private_state(root.path(), operation_id)?;
        assert_eq!(
            before_greater_private
                .pointer("/operation/deliveryCompletions")
                .ok_or_else(|| std::io::Error::other("missing lower-first delivery ledger"))?,
            &json!([
                {"sequence": 1, "outcome": "succeeded"},
                {"sequence": lower_sequence, "outcome": "succeeded"},
            ]),
        );
        greater_permit.release()?;
        assert_status(&greater.finish()?, 0);
        assert_last_delivery(&show_operation(root.path(), operation_id)?, "succeeded")?;
        let after_greater_private = cleanup_private_state(root.path(), operation_id)?;
        let mut expected_after_greater = before_greater_private;
        *expected_after_greater
            .pointer_mut("/operation/greatestCompletedDeliverySequence")
            .ok_or_else(|| std::io::Error::other("missing completed delivery sequence"))? =
            json!(greater_sequence);
        *expected_after_greater
            .pointer_mut("/operation/deliveryCompletions")
            .ok_or_else(|| std::io::Error::other("missing expected delivery ledger"))? = json!([
            {"sequence": 1, "outcome": "succeeded"},
            {"sequence": lower_sequence, "outcome": "succeeded"},
            {"sequence": greater_sequence, "outcome": "succeeded"},
        ]);
        assert_eq!(
            after_greater_private, expected_after_greater,
            "greater completion changed state outside the ordered private ledger"
        );
    } else {
        greater_permit.release()?;
        assert_status(&greater.finish()?, 0);
        let before_late = run(root.path(), &["operation", "show", operation_id])?;
        assert_status(&before_late, 0);
        let before_late_value: Value = serde_json::from_str(&before_late.stdout)?;
        assert_last_delivery(&before_late_value, "succeeded")?;
        let before_late_private = cleanup_private_state(root.path(), operation_id)?;

        lower_permit.release()?;
        assert_status(&lower.finish()?, 0);
        let after_late = run(root.path(), &["operation", "show", operation_id])?;
        assert_status(&after_late, 0);
        assert_eq!(after_late.stdout, before_late.stdout);
        let after_late_private = cleanup_private_state(root.path(), operation_id)?;
        assert_eq!(
            before_late_private
                .pointer("/operation/deliveryCompletions")
                .ok_or_else(|| std::io::Error::other("missing prior delivery ledger"))?,
            &json!([
                {"sequence": 1, "outcome": "succeeded"},
                {"sequence": greater_sequence, "outcome": "succeeded"},
            ]),
        );
        let mut expected_after_late = before_late_private;
        *expected_after_late
            .pointer_mut("/operation/deliveryCompletions")
            .ok_or_else(|| std::io::Error::other("missing expected delivery ledger"))? = json!([
            {"sequence": 1, "outcome": "succeeded"},
            {"sequence": lower_sequence, "outcome": "succeeded"},
            {"sequence": greater_sequence, "outcome": "succeeded"},
        ]);
        assert_eq!(
            after_late_private, expected_after_late,
            "late lower completion was not inserted at its exact ordered ledger position"
        );
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

    let authorized_barrier = CacheCleanupBarrier::new(AUTHORIZED_BARRIER_ENV, "authorized")?;
    let move_barrier = CacheCleanupBarrier::new(MOVE_BARRIER_ENV, "before-move")?;
    let mut cleanup =
        authorized_barrier.spawn_with_barrier(root.path(), operation_id, Some(&move_barrier))?;
    let authorized_permit = authorized_barrier.accept(&mut cleanup, operation_id)?;
    let private_before_substitution = cleanup_private_state(root.path(), operation_id)?;
    authorized_permit.release()?;
    let permit = move_barrier.accept(&mut cleanup, operation_id)?;
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
    assert_eq!(fs::read(&saved)?, original);
    assert_eq!(fs::read(&target)?, b"substitute");
    assert_eq!(fs::read(cache.join("z-later.bin"))?, b"later");
    let quarantine = root.path().join(".lumin/trash/cache-evictions");
    assert_eq!(quarantine_payload_count(root.path())?, 0);
    let operation = show_operation(root.path(), operation_id)?;
    assert_cleanup_projection(&operation, "pending", 0, 2, 0, true)?;
    let private = cleanup_private_state(root.path(), operation_id)?;
    assert_eq!(
        private, private_before_substitution,
        "substitution rejection changed the cleanup operation or authorization records"
    );
    let destinations = assert_pending_cleanup_authorizations(
        &private,
        operation_id,
        &[if nested { "a-tree" } else { "a-first.bin" }, "z-later.bin"],
        0,
    )?;
    assert_eq!(destinations.len(), 2);
    assert!(
        destinations
            .iter()
            .all(|destination| !quarantine.join(destination).exists())
    );
    assert!(cache.join("namespace.anchor").is_file());
    assert!(quarantine.join("namespace.anchor").is_file());
    Ok(())
}

fn cleanup_private_state(
    root: &Path,
    operation_id: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let operation_id = OperationId::from_string(operation_id.to_owned());
    let (operation, authorizations) =
        lumin_engine::cache_cleanup_state_for_test(root, &operation_id)?;
    Ok(json!({
        "operation": operation,
        "authorizations": authorizations,
    }))
}

fn cleanup_authorization_destinations(
    state: &Value,
) -> Result<Vec<OsString>, Box<dyn std::error::Error>> {
    required_value(state, "/authorizations")?
        .as_array()
        .ok_or_else(|| std::io::Error::other("cleanup authorizations are not an array"))?
        .iter()
        .map(|authorization| authorization_component(authorization, "destinationComponent"))
        .collect()
}

#[derive(Debug, Eq, PartialEq)]
struct PhysicalTreeSnapshotRow {
    relative_path: PathBuf,
    kind: PhysicalTreeEntryKind,
    physical_identity: PhysicalFileIdentity,
    payload: Option<Vec<u8>>,
}

#[derive(Debug, Eq, PartialEq)]
enum PhysicalTreeEntryKind {
    Directory,
    RegularFile,
}

fn physical_tree_snapshot(
    namespace_root: &Path,
    payload_root: &Path,
) -> Result<Vec<PhysicalTreeSnapshotRow>, Box<dyn std::error::Error>> {
    let mut rows = Vec::new();
    snapshot_physical_tree_entry(namespace_root, payload_root, payload_root, &mut rows)?;
    Ok(rows)
}

fn snapshot_physical_tree_entry(
    namespace_root: &Path,
    payload_root: &Path,
    path: &Path,
    rows: &mut Vec<PhysicalTreeSnapshotRow>,
) -> Result<(), Box<dyn std::error::Error>> {
    let metadata = fs::symlink_metadata(path)?;
    let file_type = metadata.file_type();
    let namespace_relative = path.strip_prefix(namespace_root)?;
    let payload_relative = path.strip_prefix(payload_root)?;
    let logical = RepoPath::from_native_relative(namespace_relative)?;
    let physical_identity =
        lumin_engine::path_physical_identity_for_test(namespace_root, &logical)?;
    let (kind, payload) = if file_type.is_dir() {
        (PhysicalTreeEntryKind::Directory, None)
    } else if file_type.is_file() {
        (PhysicalTreeEntryKind::RegularFile, Some(fs::read(path)?))
    } else {
        return Err(std::io::Error::other(format!(
            "physical tree snapshot contains unsupported entry: {}",
            path.display()
        ))
        .into());
    };
    rows.push(PhysicalTreeSnapshotRow {
        relative_path: payload_relative.to_path_buf(),
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
        for (_, child) in children {
            snapshot_physical_tree_entry(namespace_root, payload_root, &child, rows)?;
        }
    }
    Ok(())
}

fn assert_pending_cleanup_authorizations(
    state: &Value,
    operation_id: &str,
    expected_sources: &[&str],
    expected_validated: u64,
) -> Result<Vec<OsString>, Box<dyn std::error::Error>> {
    assert_eq!(
        required_string(state, "/operation/schemaVersion")?,
        "lumin-cache-cleanup-operation.v2"
    );
    assert_eq!(
        required_string(state, "/operation/operationId")?,
        operation_id
    );
    assert_eq!(required_string(state, "/operation/status")?, "pending");
    assert_eq!(required_u64(state, "/operation/interruptionCount")?, 0);
    assert_eq!(
        required_u64(state, "/operation/initialAuthorizationCount")?,
        0
    );
    assert_eq!(
        required_value(state, "/operation/planInitialized")?.as_bool(),
        Some(true)
    );
    assert_eq!(
        required_u64(state, "/operation/validatedCount")?,
        expected_validated
    );
    assert!(!required_value(state, "/operation/executionLease")?.is_null());
    assert!(required_value(state, "/operation/recoveryReservation")?.is_null());
    assert!(required_value(state, "/operation/result")?.is_null());
    assert_eq!(
        required_u64(state, "/operation/greatestAllocatedDeliverySequence")?,
        0
    );
    assert!(required_value(state, "/operation/greatestCompletedDeliverySequence")?.is_null());
    assert_eq!(
        required_value(state, "/operation/deliveryCompletions")?
            .as_array()
            .map(Vec::len),
        Some(0)
    );

    let keys = required_value(state, "/operation/authorizationKeys")?
        .as_array()
        .ok_or_else(|| std::io::Error::other("cleanup authorization keys are not an array"))?;
    let authorizations = required_value(state, "/authorizations")?
        .as_array()
        .ok_or_else(|| std::io::Error::other("cleanup authorizations are not an array"))?;
    assert_eq!(keys.len(), expected_sources.len());
    assert_eq!(authorizations.len(), expected_sources.len());
    let mut destinations = Vec::with_capacity(authorizations.len());
    for (index, (authorization, expected_source)) in authorizations
        .iter()
        .zip(expected_sources.iter())
        .enumerate()
    {
        assert_eq!(
            required_string(authorization, "/operationId")?,
            operation_id
        );
        assert_eq!(required_u64(authorization, "/ordinal")?, index as u64);
        assert_eq!(
            required_string(authorization, "/state")?,
            if (index as u64) < expected_validated {
                "validated"
            } else {
                "authorized"
            }
        );
        assert_eq!(
            authorization_component(authorization, "sourceComponent")?,
            OsString::from(*expected_source)
        );
        let destination = authorization_component(authorization, "destinationComponent")?;
        assert_eq!(
            keys[index].as_str(),
            destination.to_str(),
            "authorization key and destination component diverged"
        );
        destinations.push(destination);
    }
    Ok(destinations)
}

fn assert_crashed_cleanup_state(
    root: &Path,
    state: &Value,
    operation_id: &str,
    expected_validated: u64,
    at_destination: [bool; 2],
) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        required_string(state, "/operation/schemaVersion")?,
        "lumin-cache-cleanup-operation.v2"
    );
    assert_eq!(
        required_string(state, "/operation/operationId")?,
        operation_id
    );
    assert_eq!(required_string(state, "/operation/status")?, "pending");
    assert_eq!(required_u64(state, "/operation/interruptionCount")?, 0);
    assert_eq!(
        required_u64(state, "/operation/initialAuthorizationCount")?,
        0
    );
    assert_eq!(
        required_value(state, "/operation/planInitialized")?.as_bool(),
        Some(true)
    );
    assert_eq!(
        required_u64(state, "/operation/validatedCount")?,
        expected_validated
    );
    assert!(!required_value(state, "/operation/executionLease")?.is_null());
    assert!(required_value(state, "/operation/recoveryReservation")?.is_null());
    assert!(required_value(state, "/operation/result")?.is_null());
    assert_eq!(
        required_u64(state, "/operation/greatestAllocatedDeliverySequence")?,
        0
    );
    assert!(required_value(state, "/operation/greatestCompletedDeliverySequence")?.is_null());
    assert_eq!(
        required_value(state, "/operation/deliveryCompletions")?
            .as_array()
            .map(Vec::len),
        Some(0)
    );

    let keys = required_value(state, "/operation/authorizationKeys")?
        .as_array()
        .ok_or_else(|| std::io::Error::other("cleanup authorization keys are not an array"))?;
    let authorizations = required_value(state, "/authorizations")?
        .as_array()
        .ok_or_else(|| std::io::Error::other("cleanup authorizations are not an array"))?;
    assert_eq!(keys.len(), 2);
    assert_eq!(authorizations.len(), 2);
    for (index, authorization) in authorizations.iter().enumerate() {
        assert_eq!(
            required_string(authorization, "/operationId")?,
            operation_id
        );
        assert_eq!(required_u64(authorization, "/ordinal")?, index as u64);
        assert_eq!(
            required_string(authorization, "/state")?,
            if (index as u64) < expected_validated {
                "validated"
            } else {
                "authorized"
            }
        );
        let source = authorization_component(authorization, "sourceComponent")?;
        let destination = authorization_component(authorization, "destinationComponent")?;
        assert_eq!(
            source,
            std::ffi::OsString::from(if index == 0 { "first.bin" } else { "second" })
        );
        assert_eq!(
            keys[index].as_str(),
            destination.to_str(),
            "authorization key and destination component diverged"
        );
        let source_path = root.join(".lumin/cache").join(&source);
        let destination_path = root.join(".lumin/trash/cache-evictions").join(&destination);
        assert_eq!(source_path.exists(), !at_destination[index]);
        assert_eq!(destination_path.exists(), at_destination[index]);
        let payload_path = if at_destination[index] {
            destination_path
        } else {
            source_path
        };
        if index == 0 {
            assert_eq!(fs::read(payload_path)?, b"first");
        } else {
            assert_eq!(fs::read(payload_path.join("nested.bin"))?, b"second");
        }
    }
    Ok(())
}

fn authorization_component(
    authorization: &Value,
    field: &str,
) -> Result<std::ffi::OsString, Box<dyn std::error::Error>> {
    let pointer = format!("/{field}/canonical");
    let canonical = required_value(authorization, &pointer)?
        .as_array()
        .ok_or_else(|| std::io::Error::other("authorization component is not a byte array"))?;
    let bytes = canonical
        .iter()
        .map(|byte| {
            byte.as_u64()
                .ok_or_else(|| std::io::Error::other("authorization component byte is invalid"))
                .and_then(|byte| u8::try_from(byte).map_err(std::io::Error::other))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(decode_native_path_component(&bytes)?)
}

fn expected_recovered_cleanup_state(
    mut state: Value,
    operation_id: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let request_digest = required_string(&state, "/operation/requestDigest")?.to_owned();
    let operation = state
        .get_mut("operation")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| std::io::Error::other("missing private cleanup operation"))?;
    operation.insert("status".to_owned(), json!("committed"));
    operation.insert("interruptionCount".to_owned(), json!(1));
    operation.insert("validatedCount".to_owned(), json!(2));
    operation.insert("executionLease".to_owned(), Value::Null);
    operation.insert("recoveryReservation".to_owned(), Value::Null);
    operation.insert(
        "result".to_owned(),
        json!({"operationId": operation_id, "requestDigest": request_digest}),
    );
    operation.insert("greatestAllocatedDeliverySequence".to_owned(), json!(1));
    operation.insert("greatestCompletedDeliverySequence".to_owned(), json!(1));
    operation.insert(
        "deliveryCompletions".to_owned(),
        json!([{"sequence": 1, "outcome": "succeeded"}]),
    );
    let authorizations = state
        .get_mut("authorizations")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| std::io::Error::other("missing private cleanup authorizations"))?;
    for authorization in authorizations {
        let authorization = authorization
            .as_object_mut()
            .ok_or_else(|| std::io::Error::other("invalid private cleanup authorization"))?;
        authorization.insert("state".to_owned(), json!("validated"));
    }
    Ok(state)
}

fn expected_delivery_completion(
    mut state: Value,
    completions: &[(u64, &str)],
) -> Result<Value, Box<dyn std::error::Error>> {
    let greatest = completions
        .last()
        .map(|(sequence, _)| *sequence)
        .ok_or_else(|| std::io::Error::other("expected delivery ledger is empty"))?;
    let operation = state
        .get_mut("operation")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| std::io::Error::other("missing private cleanup operation"))?;
    operation.insert(
        "greatestAllocatedDeliverySequence".to_owned(),
        json!(greatest),
    );
    operation.insert(
        "greatestCompletedDeliverySequence".to_owned(),
        json!(greatest),
    );
    operation.insert(
        "deliveryCompletions".to_owned(),
        Value::Array(
            completions
                .iter()
                .map(|(sequence, outcome)| json!({"sequence": sequence, "outcome": outcome}))
                .collect(),
        ),
    );
    Ok(state)
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
        additional_environment: Option<&'static str>,
    ) -> Result<PausedCleanup, Box<dyn std::error::Error>> {
        let mut command = lumin_command(root)?;
        let address = self.listener.local_addr()?.to_string();
        command
            .args(["cache", "clean", "--operation-id", operation_id])
            .env(self.environment, &address)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(environment) = additional_environment {
            command.env(environment, &address);
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
        Self { stream }.wait_for_stage(stage, operation_id)
    }

    fn wait_for_stage(
        self,
        stage: &str,
        operation_id: &str,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut frame = String::new();
        BufReader::new(self.stream.try_clone()?).read_line(&mut frame)?;
        assert_eq!(frame.trim_end(), format!("{stage} {operation_id}"));
        Ok(self)
    }

    fn release_for_next(mut self) -> Result<Self, Box<dyn std::error::Error>> {
        self.stream.write_all(b"release\n")?;
        Ok(self)
    }

    fn release(mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.stream.write_all(b"release\n")?;
        Ok(())
    }
}
