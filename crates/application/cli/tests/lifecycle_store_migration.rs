use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

#[allow(dead_code)]
mod support;

#[path = "support/cache_cleanup_delivery_barrier.rs"]
mod cache_cleanup_delivery_barrier;
#[path = "support/cache_cleanup_state_barrier.rs"]
mod cache_cleanup_state_barrier;
#[path = "support/lifecycle_migration_barrier.rs"]
mod lifecycle_migration_barrier;
#[path = "support/publication_barrier.rs"]
mod publication_barrier;

use cache_cleanup_delivery_barrier::CacheCleanupDeliveryBarrier;
use cache_cleanup_state_barrier::CacheCleanupStateBarrier;
use lifecycle_migration_barrier::LifecycleMigrationBarrier;
use publication_barrier::PublicationBarrier;
use support::{assert_status, run, run_with_env};

const READY: &str = concat!(
    "{\"schemaVersion\":\"lumin.lifecycle-store-migration.v1\",",
    "\"storeSchema\":\"lumin-lifecycle-store-header.v13\",",
    "\"status\":\"ready\"}\n",
);

const MIGRATION_DEATH_STAGES: &[&str] = &[
    "after-pending-intent-create",
    "after-root-authorization",
    "after-pending-intent-sync",
    "after-root-write-start",
    "after-root-partial-write",
    "after-root-write",
    "after-root-name-publication",
    "after-root-reopen",
    "after-root-file-flush",
    "after-root-parent-flush",
    "after-intent-rename",
    "after-intent",
    "after-revision-candidate-create",
    "after-revision-write-start",
    "after-revision-partial-write",
    "after-revision-write",
    "after-revision-name-publication",
    "after-revision-reopen",
    "after-revision-file-flush",
    "after-revision-parent-flush",
    "after-validated-replacement",
    "after-target-name-publication",
    "after-target-reopen",
    "after-target-file-flush",
    "after-target-parent-flush",
    "after-target-publication",
    "before-exchange",
    "after-exchange-input-open",
    #[cfg(windows)]
    "after-source-retirement",
    "after-replace",
    "after-parent-flush",
    "after-intent-removal",
];

const CACHE_CLEANUP_CRASH_POINT: &str = "LUMIN_TEST_CACHE_CLEANUP_CRASH_POINT";
const CACHE_CLEANUP_INTERRUPTED_BARRIER: &str = "LUMIN_TEST_CACHE_CLEANUP_INTERRUPTED_BARRIER";
const DELIVERY_FAILURE_ENV: &str = "LUMIN_TEST_FAIL_RESULT_DELIVERY";
const MIGRATION_DELIVERY: &str = "lifecycle-store-migration";
const PUBLICATION_PREPARED_BARRIER: &str = "LUMIN_TEST_PUBLICATION_PREPARED_BARRIER";

#[test]
fn public_migration_refuses_absent_state_and_is_a_native_v13_noop()
-> Result<(), Box<dyn std::error::Error>> {
    let absent = fixture()?;
    let before = tree_snapshot(absent.path())?;
    let rejected = run(absent.path(), &["store", "migrate"])?;
    assert_status(&rejected, 1);
    assert!(rejected.stdout.is_empty());
    assert_eq!(
        rejected.stderr,
        "lumin: lifecycle store is not initialized\n"
    );
    assert_tree_eq(tree_snapshot(absent.path())?, before);
    assert!(!absent.path().join(".lumin").exists());

    let native = fixture()?;
    let initialized = run(native.path(), &["audit", "--jobs", "1"])?;
    assert_status(&initialized, 0);
    assert!(
        fs::read_dir(native.path().join(".lumin"))?.all(|entry| entry.is_ok_and(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            !name.starts_with("lifecycle-migration")
                && !name.starts_with("lifecycle.store.migration-")
        }))
    );
    let before = tree_snapshot(&native.path().join(".lumin"))?;
    let migrated = run(native.path(), &["store", "migrate"])?;
    assert_status(&migrated, 0);
    assert!(migrated.stderr.is_empty());
    assert_eq!(migrated.stdout, READY);
    assert_tree_eq(tree_snapshot(&native.path().join(".lumin"))?, before);
    Ok(())
}

#[test]
fn public_v12_route_migrates_once_and_retries_without_mutation()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let initialized = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&initialized, 0);

    let downgraded = run(root.path(), &["store", "test-downgrade-v12"])?;
    assert_status(&downgraded, 0);
    assert!(downgraded.stdout.is_empty());
    assert!(downgraded.stderr.is_empty());
    let prior = tree_snapshot(&root.path().join(".lumin"))?;

    let ordinary = run(root.path(), &["overview"])?;
    assert_status(&ordinary, 1);
    assert!(ordinary.stdout.is_empty());
    assert_eq!(
        ordinary.stderr,
        "lumin: lifecycle store migration requires 'lumin store migrate'\n"
    );
    assert_tree_eq(tree_snapshot(&root.path().join(".lumin"))?, prior);

    let migrated = run(root.path(), &["store", "migrate", "--format", "json"])?;
    assert_status(&migrated, 0);
    assert!(migrated.stderr.is_empty());
    assert_eq!(migrated.stdout, READY);

    let state = root.path().join(".lumin");
    let names = fs::read_dir(&state)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<Vec<_>, _>>()?;
    assert!(names.iter().any(|name| name == "lifecycle-migration.json"));
    assert!(names.iter().any(|name| {
        name.to_string_lossy()
            .starts_with("lifecycle-migration.revision-")
    }));
    assert!(names.iter().any(|name| {
        name.to_string_lossy()
            .starts_with("lifecycle.store.migration-")
    }));
    assert_eq!(
        names
            .iter()
            .filter(|name| {
                name.to_string_lossy()
                    .starts_with("lifecycle.store.migration-")
            })
            .count(),
        1,
    );

    let overview = run(root.path(), &["overview"])?;
    assert_status(&overview, 0);
    let immutable_before = migration_provenance_snapshot(&state)?;
    let canonical_before = fs::read(state.join("lifecycle.store"))?;
    let mutation = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&mutation, 0);
    assert_ne!(fs::read(state.join("lifecycle.store"))?, canonical_before);
    assert_tree_eq(migration_provenance_snapshot(&state)?, immutable_before);
    assert_status(&run(root.path(), &["overview"])?, 0);
    let before_retry = tree_snapshot(&state)?;
    let retry = run(root.path(), &["store", "migrate"])?;
    assert_status(&retry, 0);
    assert!(retry.stderr.is_empty());
    assert_eq!(retry.stdout, migrated.stdout);
    assert_tree_eq(tree_snapshot(&state)?, before_retry);
    Ok(())
}

#[test]
fn public_migration_recovers_a_post_exchange_output_failure_without_replacement()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    assert_status(&run(root.path(), &["audit", "--jobs", "1"])?, 0);
    assert_status(&run(root.path(), &["store", "test-downgrade-v12"])?, 0);

    let failed = run_with_env(
        root.path(),
        &["store", "migrate"],
        &[(DELIVERY_FAILURE_ENV, MIGRATION_DELIVERY)],
    )?;
    assert_status(&failed, 1);
    assert!(failed.stdout.is_empty());
    assert_eq!(
        failed.stderr,
        "lumin: injected result delivery failure after commit\n"
    );

    assert_status(&run(root.path(), &["overview"])?, 0);
    let state = root.path().join(".lumin");
    let before_retry = tree_snapshot(&state)?;
    let retry = run(root.path(), &["store", "migrate"])?;
    assert_status(&retry, 0);
    assert!(retry.stderr.is_empty());
    assert_eq!(retry.stdout, READY);
    assert_tree_eq(tree_snapshot(&state)?, before_retry);
    Ok(())
}

#[test]
fn public_migration_rejects_an_old_generation_late_publication()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    assert_status(&run(root.path(), &["audit", "--jobs", "1"])?, 0);

    let barrier = PublicationBarrier::new(PUBLICATION_PREPARED_BARRIER, "prepared")?;
    let mut old_generation = barrier.spawn_audit(root.path(), &[])?;
    let permit = barrier.accept(&mut old_generation, "attempt_0000000000000002")?;

    assert_status(&run(root.path(), &["store", "test-downgrade-v12"])?, 0);
    let migrated = run(root.path(), &["store", "migrate"])?;
    assert_status(&migrated, 0);
    assert_eq!(migrated.stdout, READY);
    let state = root.path().join(".lumin");
    let latest = state.join("latest.json");
    let latest_before_late_publication = fs::read(&latest)?;
    let journal_before_late_publication = migration_journal_snapshot(&state)?;

    permit.release()?;
    let rejected = old_generation.finish()?;
    assert_status(&rejected, 1);
    assert!(rejected.stdout.is_empty());
    assert!(rejected.stderr.contains("store generation changed"));
    assert_eq!(fs::read(&latest)?, latest_before_late_publication);
    assert_tree_eq(
        migration_journal_snapshot(&state)?,
        journal_before_late_publication,
    );
    assert_status(&run(root.path(), &["overview"])?, 0);
    Ok(())
}

#[test]
fn public_migration_maps_exact_legacy_cleanup_delivery_states_fail_closed()
-> Result<(), Box<dyn std::error::Error>> {
    for (index, (legacy_delivery, next_sequence)) in
        [("not-attempted", 2), ("succeeded", 3), ("failed", 3)]
            .into_iter()
            .enumerate()
    {
        let root = fixture()?;
        assert_status(&run(root.path(), &["audit", "--jobs", "1"])?, 0);
        let operation_id = format!("legacy-committed-{index}");
        assert_status(
            &run(
                root.path(),
                &["cache", "clean", "--operation-id", &operation_id],
            )?,
            0,
        );
        downgrade_cleanup_fixture(root.path(), &operation_id, legacy_delivery)?;
        assert_status(&run(root.path(), &["store", "migrate"])?, 0);

        let migrated = show_cleanup_operation(root.path(), &operation_id)?;
        assert_eq!(
            migrated.pointer("/status").and_then(Value::as_str),
            Some("committed")
        );
        assert_eq!(
            migrated
                .pointer("/lastDeliveryStatus")
                .and_then(Value::as_str),
            Some("unknown")
        );

        let barrier = CacheCleanupDeliveryBarrier::new("after-allocation")?;
        let mut retry = barrier.spawn(root.path(), &operation_id)?;
        let (sequence, permit) = barrier.accept(&mut retry, &operation_id)?;
        assert_eq!(sequence, next_sequence);
        permit.release()?;
        assert_status(&retry.finish()?, 0);
        assert_eq!(
            show_cleanup_operation(root.path(), &operation_id)?
                .pointer("/lastDeliveryStatus")
                .and_then(Value::as_str),
            Some("succeeded")
        );
    }

    let pending = fixture()?;
    assert_status(&run(pending.path(), &["audit", "--jobs", "1"])?, 0);
    let pending_id = "legacy-pending";
    assert_status(
        &run_with_env(
            pending.path(),
            &["cache", "clean", "--operation-id", pending_id],
            &[(CACHE_CLEANUP_CRASH_POINT, "after-authorization")],
        )?,
        95,
    );
    downgrade_cleanup_fixture(pending.path(), pending_id, "not-attempted")?;
    assert_status(&run(pending.path(), &["store", "migrate"])?, 0);
    assert_cleanup_state(pending.path(), pending_id, "pending", "not-attempted")?;
    assert_status(
        &run(
            pending.path(),
            &["cache", "clean", "--operation-id", pending_id],
        )?,
        0,
    );

    let interrupted = fixture()?;
    assert_status(&run(interrupted.path(), &["audit", "--jobs", "1"])?, 0);
    let interrupted_id = "legacy-interrupted";
    assert_status(
        &run_with_env(
            interrupted.path(),
            &["cache", "clean", "--operation-id", interrupted_id],
            &[(CACHE_CLEANUP_CRASH_POINT, "after-authorization")],
        )?,
        95,
    );
    let barrier = CacheCleanupStateBarrier::new(CACHE_CLEANUP_INTERRUPTED_BARRIER, "interrupted")?;
    let mut retry = barrier.spawn(interrupted.path(), interrupted_id)?;
    let permit = barrier.accept(&mut retry, interrupted_id)?;
    assert_cleanup_state(
        interrupted.path(),
        interrupted_id,
        "interrupted",
        "not-attempted",
    )?;
    downgrade_cleanup_fixture(interrupted.path(), interrupted_id, "not-attempted")?;
    assert_ne!(retry.terminate()?.status, 0);
    drop(permit);
    assert_status(&run(interrupted.path(), &["store", "migrate"])?, 0);
    assert_cleanup_state(
        interrupted.path(),
        interrupted_id,
        "interrupted",
        "not-attempted",
    )?;
    assert_status(
        &run(
            interrupted.path(),
            &["cache", "clean", "--operation-id", interrupted_id],
        )?,
        0,
    );

    let impossible = fixture()?;
    assert_status(&run(impossible.path(), &["audit", "--jobs", "1"])?, 0);
    let impossible_id = "legacy-impossible";
    assert_status(
        &run_with_env(
            impossible.path(),
            &["cache", "clean", "--operation-id", impossible_id],
            &[(CACHE_CLEANUP_CRASH_POINT, "after-authorization")],
        )?,
        95,
    );
    downgrade_cleanup_fixture(impossible.path(), impossible_id, "succeeded")?;
    let state = impossible.path().join(".lumin");
    let before = tree_snapshot_without_canonical_store(&state)?;
    let rejected = run(impossible.path(), &["store", "migrate"])?;
    assert_status(&rejected, 1);
    assert!(rejected.stdout.is_empty());
    assert!(rejected.stderr.contains("impossible delivery state"));
    assert_tree_eq(tree_snapshot_without_canonical_store(&state)?, before);
    assert!(fs::read_dir(&state)?.all(|entry| entry.is_ok_and(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        !name.starts_with("lifecycle-migration") && !name.starts_with("lifecycle.store.migration-")
    })));
    let ordinary = run(impossible.path(), &["overview"])?;
    assert_status(&ordinary, 1);
    assert_eq!(
        ordinary.stderr,
        "lumin: lifecycle store migration requires 'lumin store migrate'\n"
    );
    Ok(())
}

#[test]
fn public_migration_routes_corrupt_bound_payload_only_to_migration_and_rejects_orphans()
-> Result<(), Box<dyn std::error::Error>> {
    let corrupt = fixture()?;
    assert_status(&run(corrupt.path(), &["audit", "--jobs", "1"])?, 0);
    let operation_id = "legacy-corrupt-payload";
    assert_status(
        &run(
            corrupt.path(),
            &["cache", "clean", "--operation-id", operation_id],
        )?,
        0,
    );
    downgrade_cleanup_fixture(corrupt.path(), operation_id, "not-attempted")?;
    let barrier = LifecycleMigrationBarrier::new("after-intent")?;
    let mut migration = barrier.spawn(corrupt.path())?;
    let permit = barrier.accept(&mut migration)?;
    assert_ne!(migration.terminate()?.status, 0);
    drop(permit);
    assert_status(
        &run(
            corrupt.path(),
            &["store", "test-corrupt-v12-cleanup", operation_id],
        )?,
        0,
    );

    let state = corrupt.path().join(".lumin");
    let before = tree_snapshot(&state)?;
    let ordinary = run(corrupt.path(), &["overview"])?;
    assert_status(&ordinary, 1);
    assert!(ordinary.stdout.is_empty());
    assert_eq!(
        ordinary.stderr,
        "lumin: lifecycle store migration requires 'lumin store migrate'\n"
    );
    assert_tree_eq(tree_snapshot(&state)?, before.clone());
    let rejected = run(corrupt.path(), &["store", "migrate"])?;
    assert_status(&rejected, 1);
    assert!(rejected.stdout.is_empty());
    assert!(
        rejected
            .stderr
            .contains("bound migration source payload changed")
    );
    assert_tree_eq(tree_snapshot(&state)?, before);

    let orphan = prior_fixture()?;
    let orphan_state = orphan.path().join(".lumin");
    let source_bytes = fs::read(orphan_state.join("lifecycle.store"))?;
    let foreign = orphan_state.join("lifecycle.store.migration-source-foreign");
    fs::write(&foreign, &source_bytes)?;
    let before = tree_snapshot(&orphan_state)?;
    let ordinary = run(orphan.path(), &["overview"])?;
    assert_status(&ordinary, 1);
    assert!(ordinary.stdout.is_empty());
    assert_ne!(
        ordinary.stderr,
        "lumin: lifecycle store migration requires 'lumin store migrate'\n"
    );
    assert_tree_eq(tree_snapshot(&orphan_state)?, before.clone());
    let migration = run(orphan.path(), &["store", "migrate"])?;
    assert_status(&migration, 1);
    assert!(migration.stdout.is_empty());
    assert_tree_eq(tree_snapshot(&orphan_state)?, before);
    assert_eq!(fs::read(foreign)?, source_bytes);
    Ok(())
}

#[test]
fn public_migration_authenticates_the_terminal_target_anchor()
-> Result<(), Box<dyn std::error::Error>> {
    let root = prior_fixture()?;
    assert_status(&run(root.path(), &["store", "migrate"])?, 0);
    let state = root.path().join(".lumin");
    let canonical = state.join("lifecycle.store");
    let canonical_before = fs::read(&canonical)?;
    let provenance_before = migration_provenance_snapshot(&state)?;

    assert_status(&run(root.path(), &["store", "test-corrupt-v13-anchor"])?, 0);
    assert_tree_eq(
        migration_provenance_snapshot(&state)?,
        provenance_before.clone(),
    );
    let ordinary = run(root.path(), &["overview"])?;
    assert_status(&ordinary, 1);
    assert!(ordinary.stdout.is_empty());
    let migration = run(root.path(), &["store", "migrate"])?;
    assert_status(&migration, 1);
    assert!(migration.stdout.is_empty());

    fs::write(&canonical, &canonical_before)?;
    assert_status(&run(root.path(), &["overview"])?, 0);
    assert_tree_eq(migration_provenance_snapshot(&state)?, provenance_before);
    Ok(())
}

#[test]
fn public_migration_rejects_a_self_consistent_root_without_v12_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let root = prior_fixture()?;
    let barrier = LifecycleMigrationBarrier::new("after-intent")?;
    let mut migration = barrier.spawn(root.path())?;
    let permit = barrier.accept(&mut migration)?;
    assert_ne!(migration.terminate()?.status, 0);
    drop(permit);
    assert_status(
        &run(
            root.path(),
            &["store", "test-remove-v12-root-authorization"],
        )?,
        0,
    );

    let state = root.path().join(".lumin");
    let before = tree_snapshot(&state)?;
    let ordinary = run(root.path(), &["overview"])?;
    assert_status(&ordinary, 1);
    assert!(ordinary.stdout.is_empty());
    assert_ne!(
        ordinary.stderr,
        "lumin: lifecycle store migration requires 'lumin store migrate'\n"
    );
    assert_tree_eq(tree_snapshot(&state)?, before.clone());
    let migration = run(root.path(), &["store", "migrate"])?;
    assert_status(&migration, 1);
    assert!(migration.stdout.is_empty());
    assert_tree_eq(tree_snapshot(&state)?, before);
    assert!(state.join("lifecycle-migration.json").is_file());
    assert!(state.join("lifecycle.store").is_file());
    Ok(())
}

#[test]
fn public_migration_rejects_live_binding_substitution_without_disposition()
-> Result<(), Box<dyn std::error::Error>> {
    for (stage, target_kind) in [
        ("after-intent", SubstitutionTarget::Root),
        ("after-target-publication", SubstitutionTarget::Predecessor),
        ("after-target-publication", SubstitutionTarget::Target),
        ("before-exchange", SubstitutionTarget::CanonicalSource),
    ] {
        let root = prior_fixture()?;
        let barrier = LifecycleMigrationBarrier::new(stage)?;
        let mut migration = barrier.spawn(root.path())?;
        let permit = barrier.accept(&mut migration)?;
        let state = root.path().join(".lumin");
        let target = match target_kind {
            SubstitutionTarget::Root => state.join("lifecycle-migration.json"),
            SubstitutionTarget::Predecessor => journal_head_path(&state)?,
            SubstitutionTarget::Target => pending_target_path(&state)?,
            SubstitutionTarget::CanonicalSource => state.join("lifecycle.store"),
        };
        let displaced = root.path().join(format!("displaced-{target_kind:?}"));
        let original = substitute_with_same_bytes(&target, &displaced)?;
        permit.release()?;
        let rejected = migration.finish()?;
        assert_status(&rejected, 1);
        assert!(rejected.stdout.is_empty());
        assert!(
            target.is_file(),
            "substitute disappeared for {target_kind:?}"
        );
        assert!(
            displaced.is_file(),
            "original disappeared for {target_kind:?}"
        );
        assert_eq!(fs::read(&target)?, original);
        assert_eq!(fs::read(&displaced)?, original);

        fs::remove_file(&target)?;
        fs::rename(&displaced, &target)?;
        let recovered = run(root.path(), &["store", "migrate"])?;
        assert_status(&recovered, 0);
        assert_eq!(recovered.stdout, READY);
    }
    Ok(())
}

#[test]
fn public_migration_hard_link_race_never_disposes_a_published_object()
-> Result<(), Box<dyn std::error::Error>> {
    let root = prior_fixture()?;
    let barrier = LifecycleMigrationBarrier::new("before-exchange")?;
    let mut migration = barrier.spawn(root.path())?;
    let permit = barrier.accept(&mut migration)?;
    let target = pending_target_path(&root.path().join(".lumin"))?;
    let foreign_link = root.path().join("foreign-target-link");
    fs::hard_link(&target, &foreign_link)?;
    permit.release()?;

    let rejected = migration.finish()?;
    assert_status(&rejected, 1);
    assert!(rejected.stdout.is_empty());
    assert!(foreign_link.is_file());
    assert!(root.path().join(".lumin/lifecycle.store").is_file());
    assert!(
        root.path()
            .join(".lumin/lifecycle-migration.json")
            .is_file()
    );

    fs::remove_file(&foreign_link)?;
    let recovered = run(root.path(), &["store", "migrate"])?;
    assert_status(&recovered, 0);
    assert_eq!(recovered.stdout, READY);
    Ok(())
}

#[test]
fn public_migration_rechecks_hard_links_after_movement_handles_open()
-> Result<(), Box<dyn std::error::Error>> {
    for binding in ["source", "target"] {
        let root = prior_fixture()?;
        let barrier = LifecycleMigrationBarrier::new("after-exchange-input-open")?;
        let mut migration = barrier.spawn(root.path())?;
        let permit = barrier.accept(&mut migration)?;
        let state = root.path().join(".lumin");
        let protected = if binding == "source" {
            state.join("lifecycle.store")
        } else {
            pending_target_path(&state)?
        };
        let foreign_link = root.path().join(format!("foreign-{binding}-link"));
        fs::hard_link(&protected, &foreign_link)?;
        permit.release()?;

        let rejected = migration.finish()?;
        assert_status(&rejected, 1);
        assert!(rejected.stdout.is_empty());
        assert!(foreign_link.is_file());
        assert!(state.join("lifecycle.store").is_file());
        assert!(state.join("lifecycle-migration.json").is_file());

        fs::remove_file(&foreign_link)?;
        let recovered = run(root.path(), &["store", "migrate"])?;
        assert_status(&recovered, 0);
        assert_eq!(recovered.stdout, READY);
    }
    Ok(())
}

#[cfg(windows)]
#[test]
fn public_migration_revalidates_the_target_after_source_retirement()
-> Result<(), Box<dyn std::error::Error>> {
    for mutation in ["payload", "link"] {
        let root = prior_fixture()?;
        let barrier = LifecycleMigrationBarrier::new("after-source-retirement")?;
        let mut migration = barrier.spawn(root.path())?;
        let permit = barrier.accept(&mut migration)?;
        let state = root.path().join(".lumin");
        let target = pending_target_path(&state)?;
        let original = fs::read(&target)?;
        let foreign_link = root.path().join("foreign-post-retirement-target-link");
        match mutation {
            "payload" => fs::write(&target, b"changed after source retirement")?,
            "link" => fs::hard_link(&target, &foreign_link)?,
            _ => unreachable!(),
        }
        permit.release()?;

        let rejected = migration.finish()?;
        assert_status(&rejected, 1);
        assert!(rejected.stdout.is_empty());
        assert!(!state.join("lifecycle.store").exists());
        assert!(target.is_file());

        match mutation {
            "payload" => fs::write(&target, original)?,
            "link" => fs::remove_file(&foreign_link)?,
            _ => unreachable!(),
        }
        let recovered = run(root.path(), &["store", "migrate"])?;
        assert_status(&recovered, 0);
        assert_eq!(recovered.stdout, READY);
    }
    Ok(())
}

#[test]
fn public_migration_recovers_every_durable_process_death_boundary()
-> Result<(), Box<dyn std::error::Error>> {
    for stage in MIGRATION_DEATH_STAGES {
        let root = fixture()?;
        assert_status(&run(root.path(), &["audit", "--jobs", "1"])?, 0);
        assert_status(&run(root.path(), &["store", "test-downgrade-v12"])?, 0);

        let barrier = LifecycleMigrationBarrier::new(stage)?;
        let mut migration = barrier.spawn(root.path())?;
        let permit = barrier.accept(&mut migration)?;
        let terminated = migration.terminate()?;
        assert_ne!(terminated.status, 0, "{stage} child unexpectedly succeeded");
        drop(permit);

        let ordinary = run(root.path(), &["overview"])?;
        if *stage == "after-intent-removal" {
            assert_status(&ordinary, 0);
        } else {
            assert_status(&ordinary, 1);
            assert!(ordinary.stdout.is_empty());
            assert_eq!(
                ordinary.stderr,
                "lumin: lifecycle store migration requires 'lumin store migrate'\n",
                "unexpected ordinary recovery route after {stage}",
            );
        }

        let migrated = run(root.path(), &["store", "migrate"])?;
        assert_status(&migrated, 0);
        assert!(migrated.stderr.is_empty());
        assert_eq!(migrated.stdout, READY);
        let before_retry = tree_snapshot(&root.path().join(".lumin"))?;
        let retry = run(root.path(), &["store", "migrate"])?;
        assert_status(&retry, 0);
        assert_eq!(retry.stdout, READY);
        assert_tree_eq(tree_snapshot(&root.path().join(".lumin"))?, before_retry);
    }
    Ok(())
}

fn fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(
        root.path().join("src/lib.ts"),
        "export const migrationFixture = 1;\n",
    )?;
    Ok(root)
}

fn prior_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = fixture()?;
    assert_status(&run(root.path(), &["audit", "--jobs", "1"])?, 0);
    assert_status(&run(root.path(), &["store", "test-downgrade-v12"])?, 0);
    Ok(root)
}

fn downgrade_cleanup_fixture(
    root: &Path,
    operation_id: &str,
    legacy_delivery: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let downgraded = run(
        root,
        &[
            "store",
            "test-downgrade-v12",
            "--cleanup-operation",
            operation_id,
            "--legacy-delivery",
            legacy_delivery,
        ],
    )?;
    assert_status(&downgraded, 0);
    assert!(downgraded.stdout.is_empty());
    assert!(downgraded.stderr.is_empty());
    Ok(())
}

fn show_cleanup_operation(
    root: &Path,
    operation_id: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let shown = run(root, &["operation", "show", operation_id])?;
    assert_status(&shown, 0);
    Ok(serde_json::from_str(&shown.stdout)?)
}

fn assert_cleanup_state(
    root: &Path,
    operation_id: &str,
    status: &str,
    delivery: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let operation = show_cleanup_operation(root, operation_id)?;
    assert_eq!(
        operation.pointer("/status").and_then(Value::as_str),
        Some(status)
    );
    assert_eq!(
        operation
            .pointer("/lastDeliveryStatus")
            .and_then(Value::as_str),
        Some(delivery)
    );
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum SubstitutionTarget {
    Root,
    Predecessor,
    Target,
    CanonicalSource,
}

fn pending_target_path(state: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    for revision in journal_revision_paths(state)?.into_iter().rev() {
        let value = serde_json::from_slice::<Value>(&fs::read(revision)?)?;
        let Some(events) = value.get("events").and_then(Value::as_array) else {
            continue;
        };
        for event in events.iter().rev() {
            if event.get("kind").and_then(Value::as_str) == Some("pendingPublication") {
                let name = event
                    .pointer("/binding/preExchangeName")
                    .and_then(Value::as_str)
                    .ok_or("pending migration target omitted its path")?;
                return Ok(state.join(name));
            }
        }
    }
    Err("pending migration target binding was not journaled".into())
}

fn journal_head_path(state: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    journal_revision_paths(state)?
        .into_iter()
        .next_back()
        .ok_or_else(|| "migration journal omitted its successor".into())
}

fn journal_revision_paths(
    state: &Path,
) -> Result<std::collections::BTreeSet<PathBuf>, std::io::Error> {
    fs::read_dir(state)?
        .filter_map(|entry| match entry {
            Ok(entry)
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("lifecycle-migration.revision-") =>
            {
                Some(Ok(entry.path()))
            }
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

fn substitute_with_same_bytes(
    target: &Path,
    displaced: &Path,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let bytes = fs::read(target)?;
    fs::rename(target, displaced)?;
    fs::write(target, &bytes)?;
    Ok(bytes)
}

fn migration_provenance_snapshot(root: &Path) -> Result<Vec<(PathBuf, Vec<u8>)>, std::io::Error> {
    Ok(tree_snapshot(root)?
        .into_iter()
        .filter(|(path, _)| {
            path.file_name().is_some_and(|name| {
                let name = name.to_string_lossy();
                name.starts_with("lifecycle-migration")
                    || name.starts_with("lifecycle.store.migration-")
            })
        })
        .collect())
}

fn migration_journal_snapshot(root: &Path) -> Result<Vec<(PathBuf, Vec<u8>)>, std::io::Error> {
    let mut paths = journal_revision_paths(root)?
        .into_iter()
        .collect::<Vec<_>>();
    paths.push(root.join("lifecycle-migration.json"));
    paths
        .into_iter()
        .map(|path| {
            let relative = path
                .strip_prefix(root)
                .map_err(std::io::Error::other)?
                .to_owned();
            Ok((relative, fs::read(path)?))
        })
        .collect()
}

fn tree_snapshot(root: &Path) -> Result<Vec<(PathBuf, Vec<u8>)>, std::io::Error> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut pending = vec![root.to_owned()];
    let mut snapshot = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .map_err(std::io::Error::other)?
                .to_owned();
            if entry.file_type()?.is_dir() {
                snapshot.push((relative.clone(), Vec::new()));
                pending.push(path);
            } else {
                snapshot.push((relative, fs::read(path)?));
            }
        }
    }
    snapshot.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(snapshot)
}

fn tree_snapshot_without_canonical_store(
    root: &Path,
) -> Result<Vec<(PathBuf, Vec<u8>)>, std::io::Error> {
    Ok(tree_snapshot(root)?
        .into_iter()
        .filter(|(path, _)| path != Path::new("lifecycle.store"))
        .collect())
}

fn assert_tree_eq(actual: Vec<(PathBuf, Vec<u8>)>, expected: Vec<(PathBuf, Vec<u8>)>) {
    let actual = actual
        .into_iter()
        .collect::<std::collections::BTreeMap<_, _>>();
    let expected = expected
        .into_iter()
        .collect::<std::collections::BTreeMap<_, _>>();
    let paths = actual
        .keys()
        .chain(expected.keys())
        .collect::<std::collections::BTreeSet<_>>();
    let changed = paths
        .into_iter()
        .filter(|path| actual.get(*path) != expected.get(*path))
        .cloned()
        .collect::<Vec<_>>();
    assert!(changed.is_empty(), "state paths changed: {changed:?}");
}
