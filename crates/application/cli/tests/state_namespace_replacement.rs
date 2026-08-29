use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

#[path = "support/namespace_barrier.rs"]
mod namespace_barrier;
mod support;

use namespace_barrier::NamespaceBarrier;
use support::{ProcessResult, assert_status, field, run};

const AFTER_PRE_ACQUIRE_VALIDATION: &str = "after-pre-acquire-validation";
const AFTER_COMPLETE_VALIDATION: &str = "after-complete-validation";
const BEFORE_STORE_COMMIT: &str = "before-store-commit";
const BEFORE_MIGRATION_STORE_COMMIT: &str = "before-migration-store-commit";
const BEFORE_LATEST_REPLACE: &str = "before-latest-replace";
const BEFORE_RETENTION_COMMIT: &str = "before-retention-commit";
const BEFORE_RUN_RENAME: &str = "before-run-rename";
const BEFORE_RETENTION_MOVE: &str = "before-retention-move";
const BEFORE_CACHE_MOVE: &str = "before-cache-move";

#[test]
fn lock_replacement_never_forms_two_accepted_guard_domains()
-> Result<(), Box<dyn std::error::Error>> {
    with_context(
        "pre-acquire migration lock replacement",
        migration_rejects_lock_replacement_after_pre_acquire_validation(),
    )?;
    with_context(
        "pre-commit migration lock replacement",
        migration_commit_rejects_lock_replacement(),
    )?;
    with_context(
        "post-validation state-directory swap",
        state_swap_is_rejected_after_complete_validation(),
    )?;
    with_context(
        "pre-replace latest-pointer lock replacement",
        latest_pointer_commit_rejects_lock_replacement(),
    )?;
    with_context(
        "pre-replace latest-pointer state-directory replacement",
        latest_pointer_replace_uses_held_state_directory(),
    )?;
    with_context(
        "pre-commit retention lock replacement",
        retention_commit_rejects_lock_replacement(),
    )?;
    with_context(
        "pre-commit lock replacement",
        gate_commit_rejects_lock_replacement(),
    )?;
    Ok(())
}

#[test]
fn managed_parent_replacement_stops_every_guarded_transition()
-> Result<(), Box<dyn std::error::Error>> {
    with_context(
        "pre-admission copied managed-parent replacements",
        copied_managed_parents_fail_closed_before_admission(),
    )?;
    with_context(
        "pre-admission cross-volume managed-parent replacement",
        cross_volume_managed_parent_fails_closed(),
    )?;
    with_context(
        "post-validation runs-parent swap",
        parent_swap_is_rejected_after_complete_validation(),
    )?;
    with_context(
        "pre-rename runs-parent swap",
        run_parent_swap_stops_before_publication_rename(),
    )?;
    with_context(
        "pre-cache-move quarantine swap",
        quarantine_swap_stops_before_cache_move(),
    )?;
    with_context(
        "multiply-linked managed cache child",
        managed_child_hard_link_stops_before_mutation(),
    )?;
    with_context(
        "post-validation retention trash swap",
        trash_swap_cannot_redirect_retention_move(),
    )?;
    with_context(
        "pre-commit attempts-parent swap",
        attempt_parent_swap_stops_before_gate_commit(),
    )?;
    Ok(())
}

fn copied_managed_parents_fail_closed_before_admission() -> Result<(), Box<dyn std::error::Error>> {
    for (relative, label) in [
        (".lumin/attempts", "attempts-before-admission"),
        (".lumin/runs", "runs-before-admission"),
        (".lumin/trash", "trash-before-admission"),
        (".lumin/cache", "cache-before-admission"),
        (
            ".lumin/trash/cache-evictions",
            "quarantine-before-admission",
        ),
    ] {
        let root = fixture()?;
        let baseline_run = initialize(root.path())?;
        let canonical = root.path().join(relative);
        let mut replacement = DirectoryReplacement::prepare(root.path(), &canonical, label)?;
        assert!(
            replacement.activate()?,
            "an idle managed parent must be replaceable for the fault fixture: {relative}"
        );
        let visible_before = tree_snapshot(&canonical)?;
        let authentic_before = tree_snapshot(replacement.authentic_path())?;

        assert_integrity_failure(&run(root.path(), &["audit", "--jobs", "1"])?);
        assert_eq!(tree_snapshot(&canonical)?, visible_before);
        assert_eq!(
            tree_snapshot(replacement.authentic_path())?,
            authentic_before
        );
        replacement.restore()?;
        assert_latest_run(root.path(), &baseline_run)?;
    }

    let root = fixture()?;
    let baseline_run = initialize(root.path())?;
    let cache = root.path().join(".lumin/cache");
    let authentic = root.path().join(".cache-empty.authentic");
    fs::rename(&cache, &authentic)?;
    fs::create_dir(&cache)?;
    assert_integrity_failure(&run(root.path(), &["audit", "--jobs", "1"])?);
    assert_eq!(fs::read_dir(&cache)?.count(), 0);
    fs::remove_dir(&cache)?;
    fs::rename(authentic, cache)?;
    assert_latest_run(root.path(), &baseline_run)?;
    Ok(())
}

fn cross_volume_managed_parent_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let baseline_run = initialize(root.path())?;
    let runs = root.path().join(".lumin/runs");
    let durable_before = durable_namespace_snapshot(root.path())?;
    let store_before = current_logical_store_snapshot(root.path())?;
    let mut replacement = CrossVolumeParentReplacement::install(root.path(), &runs)?;
    let authentic_before = tree_snapshot(&replacement.authentic)?;
    let foreign_before = tree_snapshot(&replacement.foreign_parent)?;

    let rejected = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_integrity_failure(&rejected);
    assert_eq!(tree_snapshot(&replacement.authentic)?, authentic_before);
    assert_eq!(tree_snapshot(&replacement.foreign_parent)?, foreign_before);
    replacement.restore()?;

    assert_eq!(durable_namespace_snapshot(root.path())?, durable_before);
    assert_eq!(current_logical_store_snapshot(root.path())?, store_before);
    assert_latest_run(root.path(), &baseline_run)?;
    assert_run_visible(root.path(), &baseline_run)?;
    Ok(())
}

fn migration_rejects_lock_replacement_after_pre_acquire_validation()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let baseline_run = initialize(root.path())?;
    let downgraded = run(root.path(), &["store", "test-downgrade-v12"])?;
    assert_status(&downgraded, 0);

    let barrier = NamespaceBarrier::new(AFTER_PRE_ACQUIRE_VALIDATION)?;
    let mut replacement = FileReplacement::prepare(
        &root.path().join(".lumin/lifecycle.lock"),
        root.path().join("lifecycle.lock.prepared"),
        root.path().join("lifecycle.lock.authentic"),
    )?;
    let mut migration = barrier.spawn(root.path(), &["store", "migrate", "--format", "json"])?;
    let permit = barrier.accept(&mut migration)?;
    assert!(
        replacement.activate()?,
        "platform must permit lock replacement"
    );

    assert_integrity_failure(&run(
        root.path(),
        &["store", "migrate", "--format", "json"],
    )?);
    permit.release()?;
    assert_integrity_failure(&migration.finish()?);
    replacement.restore()?;

    let migrated = run(root.path(), &["store", "migrate", "--format", "json"])?;
    assert_status(&migrated, 0);
    assert_eq!(
        json(&migrated.stdout)?
            .get("schemaVersion")
            .and_then(Value::as_str),
        Some("lumin.lifecycle-store-migration.v1")
    );
    assert_latest_run(root.path(), &baseline_run)?;
    assert_run_visible(root.path(), &baseline_run)?;
    Ok(())
}

fn migration_commit_rejects_lock_replacement() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let baseline_run = initialize(root.path())?;
    let downgraded = run(root.path(), &["store", "test-downgrade-v12"])?;
    assert_status(&downgraded, 0);

    let barrier = NamespaceBarrier::new(BEFORE_MIGRATION_STORE_COMMIT)?;
    let mut replacement = FileReplacement::prepare(
        &root.path().join(".lumin/lifecycle.lock"),
        root.path().join("lifecycle.lock.migration-commit-prepared"),
        root.path()
            .join("lifecycle.lock.migration-commit-authentic"),
    )?;
    let state = root.path().join(".lumin");
    let durable_before = durable_namespace_snapshot(root.path())?;
    let mut migration = barrier.spawn(root.path(), &["store", "migrate", "--format", "json"])?;
    let permit = barrier.accept(&mut migration)?;
    assert!(
        replacement.activate()?,
        "platform must permit lock replacement"
    );

    permit.release()?;
    assert_integrity_failure(&migration.finish()?);
    replacement.restore()?;
    assert!(
        durable_namespace_snapshot(root.path())? == durable_before,
        "failed migration exposed a namespace change before recovery"
    );

    let migrated = run(root.path(), &["store", "migrate", "--format", "json"])?;
    assert_status(&migrated, 0);
    let journal_path = state.join("lifecycle-migration.json");
    let journal_bytes = fs::read(&journal_path)?;
    let root_authorization: Value = serde_json::from_slice(&journal_bytes)?;
    assert_eq!(
        root_authorization
            .get("authorizationSequence")
            .and_then(Value::as_u64),
        Some(2),
        "migration retry did not supersede the committed pre-response authorization exactly once"
    );
    let retried = run_success(root.path(), &["store", "migrate", "--format", "json"])?;
    assert_eq!(retried.stdout, migrated.stdout);
    assert_eq!(fs::read(journal_path)?, journal_bytes);
    assert_latest_run(root.path(), &baseline_run)?;
    assert_run_visible(root.path(), &baseline_run)?;
    Ok(())
}

fn state_swap_is_rejected_after_complete_validation() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let baseline_run = initialize(root.path())?;
    let mut replacement = DirectoryReplacement::prepare(
        root.path(),
        &root.path().join(".lumin"),
        "state-after-validation",
    )?;
    let barrier = NamespaceBarrier::new(AFTER_COMPLETE_VALIDATION)?;
    let mut audit = barrier.spawn(root.path(), &["audit", "--jobs", "1"])?;
    let permit = barrier.accept(&mut audit)?;
    let replaced = replacement.activate()?;
    if replaced {
        assert_integrity_failure(&run(root.path(), &["audit", "--jobs", "1"])?);
        permit.release()?;
        assert_integrity_failure(&audit.finish()?);
        replacement.restore()?;
        assert_latest_run(root.path(), &baseline_run)?;
    } else {
        permit.release()?;
        let completed = audit.finish()?;
        assert_status(&completed, 0);
        let completed_run = field(&completed.stdout, "runId")?;
        replacement.restore()?;
        assert_latest_run(root.path(), &completed_run)?;
    }
    assert_run_visible(root.path(), &baseline_run)?;
    Ok(())
}

fn gate_commit_rejects_lock_replacement() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let baseline_run = initialize(root.path())?;
    let barrier = NamespaceBarrier::new(BEFORE_STORE_COMMIT)?;
    let mut replacement = FileReplacement::prepare(
        &root.path().join(".lumin/lifecycle.lock"),
        root.path().join("lifecycle.lock.commit-prepared"),
        root.path().join("lifecycle.lock.commit-authentic"),
    )?;
    let durable_before = durable_namespace_snapshot(root.path())?;
    let mut pre_write = barrier.spawn(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "lock-swap-before-commit",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    let permit = barrier.accept(&mut pre_write)?;
    assert!(
        replacement.activate()?,
        "platform must permit lock replacement"
    );

    assert_integrity_failure(&run(root.path(), &["audit", "--jobs", "1"])?);
    permit.release()?;
    assert_integrity_failure(&pre_write.finish()?);
    replacement.restore()?;

    assert!(
        durable_namespace_snapshot(root.path())? != durable_before,
        "gate result did not commit through the authenticated store handle"
    );
    assert_latest_run(root.path(), &baseline_run)?;
    assert_run_visible(root.path(), &baseline_run)?;
    assert_committed_pre_write_retry_and_abandon(root.path(), "lock-swap-before-commit")?;
    Ok(())
}

fn latest_pointer_commit_rejects_lock_replacement() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let baseline_run = initialize(root.path())?;
    fs::write(
        root.path().join("src/lib.ts"),
        "export const latestReplacement = 2;\n",
    )?;

    let state = root.path().join(".lumin");
    let latest_path = state.join("latest.json");
    let pending_path = state.join("latest.json.pending");
    let latest_before = fs::read(&latest_path)?;
    let barrier = NamespaceBarrier::new(BEFORE_LATEST_REPLACE)?;
    let mut replacement = FileReplacement::prepare(
        &state.join("lifecycle.lock"),
        root.path().join("lifecycle.lock.latest-prepared"),
        root.path().join("lifecycle.lock.latest-authentic"),
    )?;
    let mut audit = barrier.spawn(root.path(), &["audit", "--jobs", "1"])?;
    let permit = barrier.accept(&mut audit)?;
    let pending_bytes = fs::read(&pending_path)?;
    assert_ne!(pending_bytes, latest_before);
    let pending: Value = serde_json::from_slice(&pending_bytes)?;
    let completed_run = required_string(&pending, "/latestCompleted/runId")?.to_owned();
    let completed_sequence = pending
        .pointer("/latestAttempt/sequence")
        .and_then(Value::as_u64)
        .ok_or_else(|| std::io::Error::other("pending latest pointer omitted its sequence"))?;
    assert!(
        replacement.activate()?,
        "platform must permit lock replacement"
    );
    let foreign_identity = physical_identity(&replacement.canonical)?;
    let foreign_bytes = fs::read(&replacement.canonical)?;

    permit.release()?;
    assert_integrity_failure(&audit.finish()?);
    assert_eq!(physical_identity(&replacement.canonical)?, foreign_identity);
    assert_eq!(fs::read(&replacement.canonical)?, foreign_bytes);
    assert_eq!(fs::read(&latest_path)?, pending_bytes);
    assert!(
        !pending_path.exists(),
        "completed latest replacement retained its pending name"
    );
    replacement.restore()?;

    let recovered = run_success(root.path(), &["overview"])?;
    let recovered_body = json(&recovered.stdout)?;
    assert_eq!(
        recovered_body.pointer("/scope/id").and_then(Value::as_str),
        Some(completed_run.as_str())
    );
    assert_eq!(
        recovered_body
            .pointer("/latestAttempt/sequence")
            .and_then(Value::as_u64),
        Some(completed_sequence)
    );
    assert_eq!(
        recovered_body
            .pointer("/latestAttempt/status")
            .and_then(Value::as_str),
        Some("completed")
    );
    assert_run_visible(root.path(), &completed_run)?;
    assert_run_visible(root.path(), &baseline_run)?;
    let recovered_snapshot = durable_namespace_snapshot(root.path())?;
    let repeated = run_success(root.path(), &["overview"])?;
    assert_eq!(json(&repeated.stdout)?, recovered_body);
    assert!(
        durable_namespace_snapshot(root.path())? == recovered_snapshot,
        "repeated pointer recovery changed the settled durable snapshot"
    );
    Ok(())
}

fn latest_pointer_replace_uses_held_state_directory() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let baseline_run = initialize(root.path())?;
    fs::write(
        root.path().join("src/lib.ts"),
        "export const latestStateReplacement = 2;\n",
    )?;

    let state = root.path().join(".lumin");
    let mut replacement =
        DirectoryReplacement::prepare(root.path(), &state, "state-before-latest-replace")?;
    let pending_path = state.join("latest.json.pending");
    let barrier = NamespaceBarrier::new(BEFORE_LATEST_REPLACE)?;
    let mut audit = barrier.spawn(root.path(), &["audit", "--jobs", "1"])?;
    let permit = barrier.accept(&mut audit)?;
    let pending_bytes = fs::read(&pending_path)?;
    let pending: Value = serde_json::from_slice(&pending_bytes)?;
    let completed_run = required_string(&pending, "/latestCompleted/runId")?.to_owned();

    if !replacement.activate()? {
        permit.release()?;
        let completed = audit.finish()?;
        assert_status(&completed, 0);
        replacement.restore()?;
        assert_latest_run(root.path(), &completed_run)?;
        assert_run_visible(root.path(), &baseline_run)?;
        return Ok(());
    }

    let foreign_before = tree_snapshot(&state)?;
    let authentic_state = replacement.authentic_path().to_path_buf();
    permit.release()?;
    assert_integrity_failure(&audit.finish()?);
    assert_eq!(
        tree_snapshot(&state)?,
        foreign_before,
        "latest publication changed the foreign replacement tree"
    );
    assert_eq!(
        fs::read(authentic_state.join("latest.json"))?,
        pending_bytes,
        "latest publication did not replace the pointer in the held state directory"
    );
    assert!(
        !authentic_state.join("latest.json.pending").exists(),
        "held state directory retained the moved pending pointer"
    );
    replacement.restore()?;

    assert_latest_run(root.path(), &completed_run)?;
    assert_run_visible(root.path(), &completed_run)?;
    assert_run_visible(root.path(), &baseline_run)?;
    Ok(())
}

fn retention_commit_rejects_lock_replacement() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let first_run = initialize(root.path())?;
    fs::write(
        root.path().join("src/lib.ts"),
        "export const retentionReplacement = 2;\n",
    )?;
    let second_run = field(
        &run_success(root.path(), &["audit", "--jobs", "1"])?.stdout,
        "runId",
    )?;
    let plan = run_success(
        root.path(),
        &[
            "runs",
            "prune",
            "plan",
            "--before",
            "9000000000000",
            "--operation-id",
            "lock-swap-retention-plan",
        ],
    )?;
    let plan_id = required_string(&json(&plan.stdout)?, "/result/planId")?.to_owned();

    let state = root.path().join(".lumin");
    let barrier = NamespaceBarrier::new(BEFORE_RETENTION_COMMIT)?;
    let mut replacement = FileReplacement::prepare(
        &state.join("lifecycle.lock"),
        root.path().join("lifecycle.lock.retention-prepared"),
        root.path().join("lifecycle.lock.retention-authentic"),
    )?;
    let mut confirm = barrier.spawn(
        root.path(),
        &[
            "runs",
            "prune",
            "confirm",
            &plan_id,
            "--operation-id",
            "lock-swap-retention-confirm",
        ],
    )?;
    let permit = barrier.accept(&mut confirm)?;
    assert!(
        !state.join("runs").join(&first_run).exists(),
        "retention reached its final commit before moving the selected run"
    );
    assert!(
        replacement.activate()?,
        "platform must permit lock replacement"
    );
    let foreign_identity = physical_identity(&replacement.canonical)?;
    let foreign_bytes = fs::read(&replacement.canonical)?;

    permit.release()?;
    assert_integrity_failure(&confirm.finish()?);
    assert_eq!(physical_identity(&replacement.canonical)?, foreign_identity);
    assert_eq!(fs::read(&replacement.canonical)?, foreign_bytes);
    replacement.restore()?;

    let committed_plan = run_success(root.path(), &["runs", "prune", "plan", "show", &plan_id])?;
    let committed_plan = json(&committed_plan.stdout)?;
    assert_eq!(
        committed_plan.get("state").and_then(Value::as_str),
        Some("pruned")
    );
    assert_eq!(
        committed_plan
            .get("physicalReclamationPending")
            .and_then(Value::as_bool),
        Some(true)
    );
    let committed_operation = run_success(
        root.path(),
        &["operation", "show", "lock-swap-retention-confirm"],
    )?;
    let committed_operation = json(&committed_operation.stdout)?;
    assert_eq!(
        committed_operation
            .pointer("/operation/status")
            .and_then(Value::as_str),
        Some("committed")
    );
    assert_eq!(
        committed_operation
            .pointer("/operation/result/result/planId")
            .and_then(Value::as_str),
        Some(plan_id.as_str())
    );
    assert_eq!(
        committed_operation
            .pointer("/operation/result/result/physicalReclamationPending")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        committed_operation
            .get("currentPhysicalReclamationPending")
            .and_then(Value::as_bool),
        Some(true)
    );

    let recovered = run_success(
        root.path(),
        &[
            "runs",
            "prune",
            "confirm",
            &plan_id,
            "--operation-id",
            "lock-swap-retention-confirm",
        ],
    )?;
    let recovered_body = json(&recovered.stdout)?;
    assert_eq!(
        recovered_body
            .pointer("/result/physicalReclamationPending")
            .and_then(Value::as_bool),
        Some(true)
    );
    let recovered_plan = run_success(root.path(), &["runs", "prune", "plan", "show", &plan_id])?;
    assert_eq!(
        json(&recovered_plan.stdout)?
            .get("physicalReclamationPending")
            .and_then(Value::as_bool),
        Some(false)
    );
    let recovered_operation = run_success(
        root.path(),
        &["operation", "show", "lock-swap-retention-confirm"],
    )?;
    let recovered_operation = json(&recovered_operation.stdout)?;
    assert_eq!(
        recovered_operation
            .pointer("/operation/result/result/physicalReclamationPending")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        recovered_operation
            .get("currentPhysicalReclamationPending")
            .and_then(Value::as_bool),
        Some(false)
    );
    let recovered_snapshot = durable_namespace_snapshot(root.path())?;
    let recovered_store = current_logical_store_snapshot(root.path())?;
    let repeated = run_success(
        root.path(),
        &[
            "runs",
            "prune",
            "confirm",
            &plan_id,
            "--operation-id",
            "lock-swap-retention-confirm",
        ],
    )?;
    assert_eq!(repeated.stdout, recovered.stdout);
    assert!(
        durable_namespace_snapshot(root.path())? == recovered_snapshot,
        "repeated retention recovery changed the settled durable snapshot"
    );
    assert_eq!(
        current_logical_store_snapshot(root.path())?,
        recovered_store,
        "repeated retention recovery changed lifecycle-store rows or revisions"
    );
    assert_latest_run(root.path(), &second_run)?;
    assert_run_visible(root.path(), &second_run)?;
    assert!(!state.join("runs").join(first_run).exists());
    Ok(())
}

fn parent_swap_is_rejected_after_complete_validation() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let baseline_run = initialize(root.path())?;
    let runs = root.path().join(".lumin/runs");
    let mut replacement =
        ManagedParentReplacement::prepare(root.path(), &runs, "runs-after-validation")?;
    let barrier = NamespaceBarrier::new(AFTER_COMPLETE_VALIDATION)?;
    let mut audit = barrier.spawn(root.path(), &["audit", "--jobs", "1"])?;
    let permit = barrier.accept(&mut audit)?;
    replacement.activate()?;
    let (visible_before, authentic_before) = replacement.snapshots()?;

    permit.release()?;
    assert_integrity_failure(&audit.finish()?);
    replacement.assert_unchanged(&visible_before, authentic_before.as_deref())?;
    replacement.restore()?;

    assert_latest_run(root.path(), &baseline_run)?;
    assert_run_visible(root.path(), &baseline_run)?;
    Ok(())
}

fn run_parent_swap_stops_before_publication_rename() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let baseline_run = initialize(root.path())?;
    fs::write(
        root.path().join("src/lib.ts"),
        "export const visible = 2;\n",
    )?;
    let runs = root.path().join(".lumin/runs");
    let mut replacement =
        ManagedParentReplacement::prepare(root.path(), &runs, "runs-before-rename")?;
    let barrier = NamespaceBarrier::new(BEFORE_RUN_RENAME)?;
    let mut audit = barrier.spawn(root.path(), &["audit", "--jobs", "1"])?;
    let permit = barrier.accept(&mut audit)?;
    replacement.activate()?;
    let foreign_before = replacement.foreign_binding_snapshot()?;
    let move_parent = replacement.move_destination_root().to_path_buf();
    if !cfg!(windows) && move_parent == runs {
        return Err(std::io::Error::other(
            "platform did not replace the runs parent at the rename barrier",
        )
        .into());
    }
    let before_move = tree_snapshot(&move_parent)?;
    let staging = before_move
        .iter()
        .find(|entry| {
            entry.kind == "directory"
                && !entry.relative.contains('/')
                && entry.relative.starts_with(".run_")
                && entry.relative.ends_with(".staging")
        })
        .cloned()
        .ok_or_else(|| {
            std::io::Error::other(
                "audit reached the rename barrier without a durable staging directory",
            )
        })?;
    permit.release()?;
    assert_integrity_failure(&audit.finish()?);
    let after_move = tree_snapshot(&move_parent)?;
    let published = after_move
        .iter()
        .find(|entry| {
            entry.kind == "directory"
                && !entry.relative.contains('/')
                && entry.relative.starts_with("run_")
                && entry.physical_identity == staging.physical_identity
        })
        .cloned()
        .ok_or_else(|| {
            std::io::Error::other("run did not move within the authentic held parent")
        })?;
    assert_eq!(
        tree_without_subtree(&before_move, &staging.relative),
        tree_without_subtree(&after_move, &published.relative)
    );
    assert_eq!(
        rebased_subtree(&before_move, &staging.relative),
        rebased_subtree(&after_move, &published.relative),
        "run publication changed the staged payload while moving it"
    );
    replacement.assert_foreign_binding_unchanged(&foreign_before)?;
    replacement.restore()?;

    assert_latest_run(root.path(), &baseline_run)?;
    assert_run_visible(root.path(), &baseline_run)?;
    let recovered_run = field(
        &run_success(root.path(), &["audit", "--jobs", "1"])?.stdout,
        "runId",
    )?;
    assert_ne!(recovered_run, baseline_run);
    assert_latest_run(root.path(), &recovered_run)?;
    assert_run_visible(root.path(), &recovered_run)?;
    Ok(())
}

fn quarantine_swap_stops_before_cache_move() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let baseline_run = initialize(root.path())?;
    let written = run(
        root.path(),
        &["cache", "test-write", "move.bin", "must-stay-active"],
    )?;
    assert_status(&written, 0);
    let source = root.path().join(".lumin/cache/move.bin");
    let source_identity = physical_identity(&source)?;
    let quarantine = root.path().join(".lumin/trash/cache-evictions");
    let mut replacement =
        ManagedParentReplacement::prepare(root.path(), &quarantine, "quarantine-before-move")?;
    let barrier = NamespaceBarrier::new(BEFORE_CACHE_MOVE)?;
    let mut cleanup = barrier.spawn(
        root.path(),
        &[
            "cache",
            "clean",
            "--operation-id",
            "parent-swap-cache-clean",
        ],
    )?;
    let permit = barrier.accept(&mut cleanup)?;
    replacement.activate()?;
    let foreign_before = replacement.foreign_binding_snapshot()?;
    let move_destination = replacement.move_destination_root().to_path_buf();
    let destination_before = tree_snapshot(&move_destination)?;

    permit.release()?;
    assert_integrity_failure(&cleanup.finish()?);
    assert!(
        !source.exists(),
        "the handle-bound move must remove the authentic source entry"
    );
    let destination_after = tree_snapshot(&move_destination)?;
    let moved = destination_after
        .iter()
        .find(|entry| entry.kind == "file" && entry.physical_identity == source_identity)
        .ok_or_else(|| {
            std::io::Error::other("cache payload did not move into the authentic quarantine")
        })?;
    assert_eq!(moved.bytes, b"must-stay-active");
    assert_eq!(
        tree_without_subtree(&destination_after, &moved.relative),
        destination_before,
        "the authentic quarantine changed beyond the one authorized move"
    );
    replacement.assert_foreign_binding_unchanged(&foreign_before)?;
    replacement.restore()?;

    assert_cleanup_not_committed(root.path(), "parent-swap-cache-clean")?;
    let restored_quarantine = tree_snapshot(&quarantine)?;
    let recovered = run_success(
        root.path(),
        &[
            "cache",
            "clean",
            "--operation-id",
            "parent-swap-cache-clean",
        ],
    )?;
    assert_eq!(
        json(&recovered.stdout)?
            .get("schemaVersion")
            .and_then(Value::as_str),
        Some("lumin.cache-cleanup.v2")
    );
    assert_eq!(tree_snapshot(&quarantine)?, restored_quarantine);
    assert_latest_run(root.path(), &baseline_run)?;
    assert_run_visible(root.path(), &baseline_run)?;
    Ok(())
}

fn managed_child_hard_link_stops_before_mutation() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let baseline_run = initialize(root.path())?;
    let written = run(
        root.path(),
        &["cache", "test-write", "linked.bin", "managed-child"],
    )?;
    assert_status(&written, 0);

    let cache = root.path().join(".lumin/cache");
    let quarantine = root.path().join(".lumin/trash/cache-evictions");
    let source = cache.join("linked.bin");
    let external_link = root.path().join("linked-cache-payload.bin");
    fs::hard_link(&source, &external_link)?;
    let source_identity = physical_identity(&source)?;
    assert_eq!(physical_identity(&external_link)?, source_identity);
    let cache_before = tree_snapshot(&cache)?;
    let quarantine_before = tree_snapshot(&quarantine)?;

    let rejected = run(
        root.path(),
        &[
            "cache",
            "clean",
            "--operation-id",
            "managed-child-hard-link",
        ],
    )?;
    assert_integrity_failure(&rejected);
    assert_eq!(tree_snapshot(&cache)?, cache_before);
    assert_eq!(tree_snapshot(&quarantine)?, quarantine_before);
    assert_eq!(physical_identity(&source)?, source_identity);
    assert_eq!(physical_identity(&external_link)?, source_identity);

    let operation = run_success(
        root.path(),
        &["operation", "show", "managed-child-hard-link"],
    )?;
    let operation = json(&operation.stdout)?;
    assert_eq!(
        operation.get("status").and_then(Value::as_str),
        Some("pending")
    );
    assert_eq!(
        operation.get("authorizedCount").and_then(Value::as_u64),
        Some(0)
    );
    assert_eq!(
        operation.get("validatedCount").and_then(Value::as_u64),
        Some(0)
    );

    fs::remove_file(&external_link)?;
    let recovered = run_success(
        root.path(),
        &[
            "cache",
            "clean",
            "--operation-id",
            "managed-child-hard-link",
        ],
    )?;
    assert_eq!(
        json(&recovered.stdout)?
            .get("schemaVersion")
            .and_then(Value::as_str),
        Some("lumin.cache-cleanup.v2")
    );
    assert!(!source.exists());
    assert_latest_run(root.path(), &baseline_run)?;
    assert_run_visible(root.path(), &baseline_run)?;
    Ok(())
}

fn trash_swap_cannot_redirect_retention_move() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let first_run = initialize(root.path())?;
    fs::write(
        root.path().join("src/lib.ts"),
        "export const visible = 2;\n",
    )?;
    let second_run = field(
        &run_success(root.path(), &["audit", "--jobs", "1"])?.stdout,
        "runId",
    )?;
    let plan = run_success(
        root.path(),
        &[
            "runs",
            "prune",
            "plan",
            "--before",
            "9000000000000",
            "--operation-id",
            "parent-swap-plan",
        ],
    )?;
    let plan_id = required_string(&json(&plan.stdout)?, "/result/planId")?.to_owned();
    let trash = root.path().join(".lumin/trash");
    let mut replacement =
        ManagedParentReplacement::prepare(root.path(), &trash, "trash-before-retention-move")?;
    let barrier = NamespaceBarrier::new(BEFORE_RETENTION_MOVE)?;
    let mut confirm = barrier.spawn(
        root.path(),
        &[
            "runs",
            "prune",
            "confirm",
            &plan_id,
            "--operation-id",
            "parent-swap-confirm",
        ],
    )?;
    let permit = barrier.accept(&mut confirm)?;
    let attempts_before = tree_snapshot(&root.path().join(".lumin/attempts"))?;
    replacement.activate()?;
    let foreign_before = replacement.foreign_binding_snapshot()?;
    let move_destination = replacement.move_destination_root().to_path_buf();

    permit.release()?;
    assert_integrity_failure(&confirm.finish()?);
    let attempts_after = tree_snapshot(&root.path().join(".lumin/attempts"))?;
    let removed_attempts = attempts_before
        .iter()
        .filter(|entry| {
            entry.kind == "directory"
                && entry.relative != "."
                && !entry.relative.contains('/')
                && !attempts_after
                    .iter()
                    .any(|current| current.relative == entry.relative)
        })
        .collect::<Vec<_>>();
    assert_eq!(removed_attempts.len(), 1);
    let moved_attempt = removed_attempts[0];
    assert_eq!(
        attempts_after,
        tree_without_subtree(&attempts_before, &moved_attempt.relative)
    );
    assert!(
        tree_snapshot(&move_destination)?.iter().any(|entry| {
            entry.kind == "directory" && entry.physical_identity == moved_attempt.physical_identity
        }),
        "the moved attempt must remain bound to the authentic held trash parent"
    );
    replacement.assert_foreign_binding_unchanged(&foreign_before)?;
    replacement.restore()?;

    let shown = run_success(root.path(), &["runs", "prune", "plan", "show", &plan_id])?;
    assert_ne!(
        json(&shown.stdout)?.get("state").and_then(Value::as_str),
        Some("pruned")
    );
    let operation = run_success(root.path(), &["operation", "show", "parent-swap-confirm"])?;
    assert_ne!(
        json(&operation.stdout)?
            .pointer("/operation/status")
            .and_then(Value::as_str),
        Some("committed")
    );
    let recovered = run_success(
        root.path(),
        &[
            "runs",
            "prune",
            "confirm",
            &plan_id,
            "--operation-id",
            "parent-swap-confirm",
        ],
    )?;
    assert_eq!(
        json(&recovered.stdout)?
            .get("schemaVersion")
            .and_then(Value::as_str),
        Some("lumin.retention-mutation.v1")
    );
    let recovered_plan = run_success(root.path(), &["runs", "prune", "plan", "show", &plan_id])?;
    assert_eq!(
        json(&recovered_plan.stdout)?
            .get("state")
            .and_then(Value::as_str),
        Some("pruned")
    );
    let recovered_operation =
        run_success(root.path(), &["operation", "show", "parent-swap-confirm"])?;
    assert_eq!(
        json(&recovered_operation.stdout)?
            .pointer("/operation/status")
            .and_then(Value::as_str),
        Some("committed")
    );
    assert!(!root.path().join(".lumin/runs").join(&first_run).exists());
    assert_latest_run(root.path(), &second_run)?;
    assert_run_visible(root.path(), &second_run)?;
    Ok(())
}

fn attempt_parent_swap_stops_before_gate_commit() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let baseline_run = initialize(root.path())?;
    let attempts = root.path().join(".lumin/attempts");
    let mut replacement =
        ManagedParentReplacement::prepare(root.path(), &attempts, "attempts-before-commit")?;
    let durable_before = durable_namespace_snapshot(root.path())?;
    let barrier = NamespaceBarrier::new(BEFORE_STORE_COMMIT)?;
    let mut pre_write = barrier.spawn(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "parent-swap-before-commit",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    let permit = barrier.accept(&mut pre_write)?;
    replacement.activate()?;
    let (visible_before, authentic_before) = replacement.snapshots()?;
    // Windows may fence the parent rename through its held namespace handles;
    // Linux must reach the exact parent-replacement turn.
    if !cfg!(windows) && authentic_before.is_none() {
        return Err(std::io::Error::other(
            "platform did not replace the attempts parent at the commit barrier",
        )
        .into());
    }

    permit.release()?;
    assert_integrity_failure(&pre_write.finish()?);
    replacement.assert_unchanged(&visible_before, authentic_before.as_deref())?;
    replacement.restore()?;

    assert!(
        durable_namespace_snapshot(root.path())? != durable_before,
        "gate result did not commit while the attempts binding was replaced"
    );
    assert_latest_run(root.path(), &baseline_run)?;
    assert_run_visible(root.path(), &baseline_run)?;
    assert_committed_pre_write_retry_and_abandon(root.path(), "parent-swap-before-commit")?;
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

fn initialize(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let output = run_success(root, &["audit", "--jobs", "1"])?;
    field(&output.stdout, "runId")
}

fn run_success(
    root: &Path,
    arguments: &[&str],
) -> Result<ProcessResult, Box<dyn std::error::Error>> {
    let output = run(root, arguments)?;
    assert_status(&output, 0);
    Ok(output)
}

fn assert_run_visible(root: &Path, run_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let overview = run(root, &["overview", "--run", run_id])?;
    assert_status(&overview, 0);
    assert_eq!(
        json(&overview.stdout)?
            .pointer("/scope/id")
            .and_then(Value::as_str),
        Some(run_id)
    );
    Ok(())
}

fn assert_latest_run(root: &Path, run_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let overview = run(root, &["overview"])?;
    assert_status(&overview, 0);
    assert_eq!(
        json(&overview.stdout)?
            .pointer("/scope/id")
            .and_then(Value::as_str),
        Some(run_id)
    );
    Ok(())
}

fn assert_committed_pre_write_retry_and_abandon(
    root: &Path,
    operation_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let committed_snapshot = durable_namespace_snapshot(root)?;
    let committed_store = current_logical_store_snapshot(root)?;
    let shown = run_success(root, &["operation", "show", operation_id])?;
    let shown = json(&shown.stdout)?;
    assert_eq!(
        shown.get("status").and_then(Value::as_str),
        Some("committed")
    );
    let committed_result = shown
        .get("result")
        .cloned()
        .ok_or_else(|| std::io::Error::other("committed pre-write result is missing"))?;
    let retried = run_success(
        root,
        &[
            "pre-write",
            "--operation-id",
            operation_id,
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_eq!(json(&retried.stdout)?, committed_result);
    assert!(
        durable_namespace_snapshot(root)? == committed_snapshot,
        "same-ID retry changed the already committed durable result"
    );
    assert_eq!(
        current_logical_store_snapshot(root)?,
        committed_store,
        "same-ID retry changed lifecycle-store rows or revisions"
    );
    let gate_id = field(&retried.stdout, "gateId")?;
    let abandon_operation = format!("{operation_id}-abandon");
    let abandoned = run(
        root,
        &[
            "gate",
            "abandon",
            &gate_id,
            "--operation-id",
            &abandon_operation,
            "--reason",
            "replacement barrier regression complete",
        ],
    )?;
    assert_status(&abandoned, 3);
    Ok(())
}

fn assert_cleanup_not_committed(
    root: &Path,
    operation_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let operation = run(root, &["operation", "show", operation_id])?;
    assert_status(&operation, 0);
    assert_ne!(
        json(&operation.stdout)?
            .get("status")
            .and_then(Value::as_str),
        Some("committed")
    );
    Ok(())
}

fn assert_integrity_failure(result: &ProcessResult) {
    assert_status(result, 1);
    assert!(result.stdout.is_empty());
    assert!(
        result.stderr.contains("state namespace integrity failure"),
        "unexpected integrity diagnostic: {}",
        result.stderr
    );
}

fn json(value: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(value)
}

fn required_string<'a>(
    value: &'a Value,
    pointer: &str,
) -> Result<&'a str, Box<dyn std::error::Error>> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other(format!("missing string field {pointer}")).into())
}

fn with_context<T>(
    context: &str,
    result: Result<T, Box<dyn std::error::Error>>,
) -> Result<T, Box<dyn std::error::Error>> {
    result.map_err(|error| std::io::Error::other(format!("{context}: {error}")).into())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TreeEntry {
    relative: String,
    kind: &'static str,
    physical_identity: String,
    bytes: Vec<u8>,
}

type ReplacementSnapshots = (Vec<TreeEntry>, Option<Vec<TreeEntry>>);
type DurableNamespaceSnapshot = (String, Vec<TreeEntry>);

#[derive(Debug, Eq, PartialEq)]
enum ForeignBindingSnapshot {
    Parent(Vec<TreeEntry>),
    Anchor {
        physical_identity: String,
        bytes: Vec<u8>,
    },
}

fn tree_snapshot(root: &Path) -> Result<Vec<TreeEntry>, Box<dyn std::error::Error>> {
    let mut entries = vec![TreeEntry {
        relative: ".".to_owned(),
        kind: "directory",
        physical_identity: physical_identity(root)?,
        bytes: Vec::new(),
    }];
    collect_tree(root, root, &mut entries)?;
    entries.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok(entries)
}

fn durable_namespace_snapshot(
    root: &Path,
) -> Result<DurableNamespaceSnapshot, Box<dyn std::error::Error>> {
    let state = root.join(".lumin");
    Ok((
        physical_identity(&state.join("lifecycle.store"))?,
        tree_without_subtree(&tree_snapshot(&state)?, "lifecycle.store"),
    ))
}

fn current_logical_store_snapshot(root: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    lumin_engine::current_logical_store_snapshot_for_test(root).map_err(Into::into)
}

fn tree_without_subtree(entries: &[TreeEntry], subtree: &str) -> Vec<TreeEntry> {
    let child_prefix = format!("{subtree}/");
    entries
        .iter()
        .filter(|entry| entry.relative != subtree && !entry.relative.starts_with(&child_prefix))
        .cloned()
        .collect()
}

fn rebased_subtree(entries: &[TreeEntry], subtree: &str) -> Vec<TreeEntry> {
    let child_prefix = format!("{subtree}/");
    entries
        .iter()
        .filter_map(|entry| {
            let relative = if entry.relative == subtree {
                ".".to_owned()
            } else {
                entry
                    .relative
                    .strip_prefix(&child_prefix)
                    .map(str::to_owned)?
            };
            Some(TreeEntry {
                relative,
                kind: entry.kind,
                physical_identity: entry.physical_identity.clone(),
                bytes: entry.bytes.clone(),
            })
        })
        .collect()
}

fn collect_tree(
    root: &Path,
    directory: &Path,
    entries: &mut Vec<TreeEntry>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut children = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(fs::DirEntry::file_name);
    for child in children {
        let path = child.path();
        let relative = path
            .strip_prefix(root)?
            .to_str()
            .ok_or_else(|| std::io::Error::other("test state path is not UTF-8"))?
            .replace('\\', "/");
        let file_type = child.file_type()?;
        if file_type.is_dir() {
            entries.push(TreeEntry {
                relative,
                kind: "directory",
                physical_identity: physical_identity(&path)?,
                bytes: Vec::new(),
            });
            collect_tree(root, &path, entries)?;
        } else if file_type.is_file() {
            entries.push(TreeEntry {
                relative,
                kind: "file",
                physical_identity: physical_identity(&path)?,
                bytes: fs::read(path)?,
            });
        } else {
            return Err(std::io::Error::other("test state tree contains a redirect").into());
        }
    }
    Ok(())
}

fn physical_identity(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    lumin_engine::state_entry_physical_identity_for_test(path)
        .map(|identity| format!("{identity:?}"))
        .map_err(Into::into)
}

struct FileReplacement {
    canonical: PathBuf,
    prepared: PathBuf,
    authentic: PathBuf,
    active: bool,
}

impl FileReplacement {
    fn prepare(
        canonical: &Path,
        prepared: PathBuf,
        authentic: PathBuf,
    ) -> Result<Self, std::io::Error> {
        fs::copy(canonical, &prepared)?;
        Ok(Self {
            canonical: canonical.to_path_buf(),
            prepared,
            authentic,
            active: false,
        })
    }

    fn activate(&mut self) -> Result<bool, std::io::Error> {
        match fs::rename(&self.canonical, &self.authentic) {
            Ok(()) => {}
            Err(error) if replacement_blocked_by_open_handle(&error) => {
                return Ok(false);
            }
            Err(error) => return Err(error),
        }
        if let Err(error) = fs::rename(&self.prepared, &self.canonical) {
            fs::rename(&self.authentic, &self.canonical)?;
            return Err(error);
        }
        self.active = true;
        Ok(true)
    }

    fn restore(mut self) -> Result<(), std::io::Error> {
        if self.active {
            fs::rename(&self.canonical, &self.prepared)?;
            fs::rename(&self.authentic, &self.canonical)?;
            self.active = false;
        }
        fs::remove_file(self.prepared)
    }
}

struct DirectoryReplacement {
    canonical: PathBuf,
    prepared: PathBuf,
    authentic: PathBuf,
    active: bool,
}

impl DirectoryReplacement {
    fn prepare(root: &Path, canonical: &Path, label: &str) -> Result<Self, std::io::Error> {
        let prepared = root.join(format!(".{label}.prepared"));
        let authentic = root.join(format!(".{label}.authentic"));
        copy_directory(canonical, &prepared)?;
        Ok(Self {
            canonical: canonical.to_path_buf(),
            prepared,
            authentic,
            active: false,
        })
    }

    fn activate(&mut self) -> Result<bool, std::io::Error> {
        match fs::rename(&self.canonical, &self.authentic) {
            Ok(()) => {}
            Err(error) if replacement_blocked_by_open_handle(&error) => {
                return Ok(false);
            }
            Err(error) => return Err(error),
        }
        if let Err(error) = fs::rename(&self.prepared, &self.canonical) {
            fs::rename(&self.authentic, &self.canonical)?;
            return Err(error);
        }
        self.active = true;
        Ok(true)
    }

    fn authentic_path(&self) -> &Path {
        &self.authentic
    }

    fn restore(mut self) -> Result<(), std::io::Error> {
        if self.active {
            fs::rename(&self.canonical, &self.prepared)?;
            fs::rename(&self.authentic, &self.canonical)?;
            self.active = false;
        }
        fs::remove_dir_all(self.prepared)
    }
}

fn replacement_blocked_by_open_handle(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::PermissionDenied
        || (cfg!(windows) && matches!(error.raw_os_error(), Some(5 | 32 | 33)))
}

struct ManagedParentReplacement {
    directory: DirectoryReplacement,
    anchor: FileReplacement,
    directory_active: bool,
}

impl ManagedParentReplacement {
    fn prepare(root: &Path, canonical: &Path, label: &str) -> Result<Self, std::io::Error> {
        Ok(Self {
            directory: DirectoryReplacement::prepare(root, canonical, label)?,
            anchor: FileReplacement::prepare(
                &canonical.join("namespace.anchor"),
                root.join(format!(".{label}-anchor.prepared")),
                root.join(format!(".{label}-anchor.authentic")),
            )?,
            directory_active: false,
        })
    }

    fn activate(&mut self) -> Result<(), std::io::Error> {
        self.directory_active = self.directory.activate()?;
        if !self.directory_active && !self.anchor.activate()? {
            return Err(std::io::Error::other(
                "platform permitted neither managed-parent nor anchor replacement",
            ));
        }
        Ok(())
    }

    fn snapshots(&self) -> Result<ReplacementSnapshots, Box<dyn std::error::Error>> {
        let visible = tree_snapshot(&self.directory.canonical)?;
        let authentic = self
            .directory_active
            .then(|| tree_snapshot(self.directory.authentic_path()))
            .transpose()?;
        Ok((visible, authentic))
    }

    fn move_destination_root(&self) -> &Path {
        if self.directory_active {
            self.directory.authentic_path()
        } else {
            &self.directory.canonical
        }
    }

    fn foreign_binding_snapshot(
        &self,
    ) -> Result<ForeignBindingSnapshot, Box<dyn std::error::Error>> {
        if self.directory_active {
            Ok(ForeignBindingSnapshot::Parent(tree_snapshot(
                &self.directory.canonical,
            )?))
        } else {
            Ok(ForeignBindingSnapshot::Anchor {
                physical_identity: physical_identity(&self.anchor.canonical)?,
                bytes: fs::read(&self.anchor.canonical)?,
            })
        }
    }

    fn assert_foreign_binding_unchanged(
        &self,
        expected: &ForeignBindingSnapshot,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let observed = self.foreign_binding_snapshot()?;
        assert_eq!(&observed, expected);
        Ok(())
    }

    fn assert_unchanged(
        &self,
        visible: &[TreeEntry],
        authentic: Option<&[TreeEntry]>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(tree_snapshot(&self.directory.canonical)?, visible);
        if let Some(authentic) = authentic {
            assert_eq!(tree_snapshot(self.directory.authentic_path())?, authentic);
        }
        Ok(())
    }

    fn restore(self) -> Result<(), std::io::Error> {
        self.anchor.restore()?;
        self.directory.restore()
    }
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

#[derive(Debug, Eq, PartialEq)]
enum VolumeIdentity {
    Unix(u64),
    Windows(u32),
}

fn volume_identity(path: &Path) -> Result<VolumeIdentity, Box<dyn std::error::Error>> {
    match lumin_engine::state_entry_physical_identity_for_test(path)? {
        lumin_model::PhysicalFileIdentity::Unix { device, .. } => Ok(VolumeIdentity::Unix(device)),
        lumin_model::PhysicalFileIdentity::Windows { volume_serial, .. } => {
            Ok(VolumeIdentity::Windows(volume_serial))
        }
    }
}

fn cross_volume_tempdir(reference: &Path) -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let reference_volume = volume_identity(reference)?;
    #[cfg(target_os = "linux")]
    let candidates = ["/dev/shm", "/run", "/var/tmp", "/tmp"]
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    #[cfg(windows)]
    let candidates = (b'A'..=b'Z')
        .map(|letter| PathBuf::from(format!("{}:\\", char::from(letter))))
        .collect::<Vec<_>>();
    #[cfg(not(any(target_os = "linux", windows)))]
    let candidates = Vec::<PathBuf>::new();

    for candidate in candidates {
        if !candidate.is_dir()
            || volume_identity(&candidate).ok().as_ref() == Some(&reference_volume)
        {
            continue;
        }
        if let Ok(directory) = tempfile::Builder::new()
            .prefix("lumin-cross-volume-")
            .tempdir_in(candidate)
        {
            return Ok(directory);
        }
    }
    Err(std::io::Error::other("no writable second device or volume is available").into())
}

struct CrossVolumeParentReplacement {
    canonical: PathBuf,
    authentic: PathBuf,
    _foreign_root: tempfile::TempDir,
    foreign_parent: PathBuf,
    active: bool,
}

impl CrossVolumeParentReplacement {
    fn install(root: &Path, canonical: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let foreign_root = cross_volume_tempdir(canonical)?;
        let foreign_parent = foreign_root.path().join("runs");
        copy_directory(canonical, &foreign_parent)?;
        assert_ne!(
            volume_identity(canonical)?,
            volume_identity(&foreign_parent)?,
            "cross-volume fixture remained on the canonical volume"
        );

        let authentic = root.join(".runs-cross-volume.authentic");
        fs::rename(canonical, &authentic)?;
        if let Err(error) = install_cross_volume_alias(&foreign_parent, canonical) {
            fs::rename(&authentic, canonical)?;
            return Err(error.into());
        }
        Ok(Self {
            canonical: canonical.to_path_buf(),
            authentic,
            _foreign_root: foreign_root,
            foreign_parent,
            active: true,
        })
    }

    fn restore(&mut self) -> Result<(), std::io::Error> {
        if self.active {
            remove_cross_volume_alias(&self.canonical)?;
            fs::rename(&self.authentic, &self.canonical)?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for CrossVolumeParentReplacement {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[cfg(target_os = "linux")]
fn install_cross_volume_alias(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::create_dir(target)?;
    if let Err(error) = run_linux_mount("mount", &["--bind"], source, Some(target)) {
        fs::remove_dir(target)?;
        return Err(error);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn remove_cross_volume_alias(target: &Path) -> std::io::Result<()> {
    run_linux_mount("umount", &[], target, None)?;
    fs::remove_dir(target)
}

#[cfg(target_os = "linux")]
fn run_linux_mount(
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
fn install_cross_volume_alias(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;

    let source = source.display().to_string().replace('/', "\\");
    let target = target.display().to_string().replace('/', "\\");
    let command_line = format!("mklink /J \"{target}\" \"{source}\"");
    let mut command = std::process::Command::new("cmd");
    command.args(["/d", "/c"]);
    command.raw_arg(&command_line);
    let status = command.status()?;
    status.success().then_some(()).ok_or_else(|| {
        std::io::Error::other(format!(
            "cross-volume mklink /J failed with {status}: {command_line}"
        ))
    })
}

#[cfg(windows)]
fn remove_cross_volume_alias(target: &Path) -> std::io::Result<()> {
    fs::remove_dir(target)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn install_cross_volume_alias(_source: &Path, _target: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "cross-volume fixture supports Windows and Linux",
    ))
}

#[cfg(not(any(target_os = "linux", windows)))]
fn remove_cross_volume_alias(_target: &Path) -> std::io::Result<()> {
    Ok(())
}
