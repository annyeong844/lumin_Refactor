use tempfile::TempDir;

use super::*;

#[test]
fn both_source_and_trash_payloads_are_an_integrity_hard_stop()
-> Result<(), Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    let store = open_store(root.path())?;
    let (run_id, plan_id, confirm_id) = admit_run_pruning(&store, "both")?;
    let (source, trash) = first_move_paths(&store, &plan_id, &confirm_id)?;
    assert!(source.is_dir());
    std::fs::create_dir(&trash)?;

    assert_integrity_error(
        store.confirm_retention_plan(&plan_id, &confirm_id),
        "both source and trash",
    )?;
    assert_pruning_truth(&store, &plan_id, &run_id)?;
    Ok(())
}

#[test]
fn missing_source_and_trash_payloads_are_an_integrity_hard_stop()
-> Result<(), Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    let store = open_store(root.path())?;
    let (run_id, plan_id, confirm_id) = admit_run_pruning(&store, "neither")?;
    let (source, trash) = first_move_paths(&store, &plan_id, &confirm_id)?;
    assert!(!trash.exists());
    std::fs::rename(&source, root.path().join("displaced-retention-payload"))?;

    assert_integrity_error(
        store.confirm_retention_plan(&plan_id, &confirm_id),
        "missing from source and trash",
    )?;
    assert_pruning_truth(&store, &plan_id, &run_id)?;
    Ok(())
}

#[test]
fn changed_payload_is_rejected_before_trash_move() -> Result<(), Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    let store = open_store(root.path())?;
    let (run_id, plan_id, confirm_id) = admit_run_pruning(&store, "changed")?;
    let (source, trash) = first_move_paths(&store, &plan_id, &confirm_id)?;
    assert!(!trash.exists());
    std::fs::write(source.join("attempt.json"), b"{}")?;

    assert_integrity_error(
        store.confirm_retention_plan(&plan_id, &confirm_id),
        "changed before retention move",
    )?;
    assert!(!trash.exists());
    assert_pruning_truth(&store, &plan_id, &run_id)?;
    Ok(())
}

#[test]
fn migration_preserves_inflight_pruning_state() -> Result<(), Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    let store = open_store(root.path())?;
    let (run_id, plan_id, confirm_id) = admit_run_pruning(&store, "migration")?;

    store.migrate_lifecycle_store()?;
    assert_pruning_truth(&store, &plan_id, &run_id)?;
    let result = store.confirm_retention_plan(&plan_id, &confirm_id)?;
    assert!(matches!(result, RetentionMutationResult::Pruned { .. }));
    assert!(matches!(
        store.lookup_run(&run_id)?,
        RecordLookup::Pruned(_)
    ));
    Ok(())
}

#[test]
fn reclaim_retries_after_payload_removal_completed_before_store_mark()
-> Result<(), Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    let store = open_store(root.path())?;
    let (run_id, plan_id, confirm_id) = admit_run_pruning(&store, "reclaim-before-mark")?;
    store.with_exclusive_lock(|guard| {
        let plan = confirmation::commit_pruned_without_reclaim(guard, &plan_id, &confirm_id)?;
        assert_eq!(plan.record.state, RetentionPlanState::Pruned);
        assert!(plan.record.physical_reclamation_pending);
        Ok(())
    })?;
    store.migrate_lifecycle_store()?;
    store.with_exclusive_lock(|guard| {
        confirmation::reclaim_without_mark(guard, &plan_id, &confirm_id)
    })?;
    assert!(
        store
            .load_retention_plan(&plan_id)?
            .physical_reclamation_pending
    );
    drop(store);

    let reopened = open_store(root.path())?;
    assert!(matches!(
        reopened.confirm_retention_plan(&plan_id, &confirm_id)?,
        RetentionMutationResult::Pruned {
            physical_reclamation_pending: true,
            ..
        }
    ));
    assert!(
        !reopened
            .load_retention_plan(&plan_id)?
            .physical_reclamation_pending
    );
    assert!(matches!(
        reopened.lookup_run(&run_id)?,
        RecordLookup::Pruned(_)
    ));
    Ok(())
}

#[test]
fn reclaim_retries_after_anchor_removal_completed_before_directory_removal()
-> Result<(), Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    let store = open_store(root.path())?;
    let (_run_id, plan_id, confirm_id) = admit_run_pruning(&store, "anchor-before-directory")?;
    let (_source, trash_child) = first_move_paths(&store, &plan_id, &confirm_id)?;
    store.with_exclusive_lock(|guard| {
        let plan = confirmation::commit_pruned_without_reclaim(guard, &plan_id, &confirm_id)?;
        assert_eq!(plan.record.state, RetentionPlanState::Pruned);
        Ok(())
    })?;
    let trash_directory = trash_child
        .parent()
        .ok_or("trash movement has no plan directory")?;
    for entry in std::fs::read_dir(trash_directory)? {
        let entry = entry?;
        if entry.file_name() != ".plan-anchor" {
            std::fs::remove_dir_all(entry.path())?;
        }
    }
    std::fs::remove_file(trash_directory.join(".plan-anchor"))?;
    assert!(trash_directory.is_dir());

    assert!(matches!(
        store.confirm_retention_plan(&plan_id, &confirm_id)?,
        RetentionMutationResult::Pruned {
            physical_reclamation_pending: true,
            ..
        }
    ));
    assert!(
        !store
            .load_retention_plan(&plan_id)?
            .physical_reclamation_pending
    );
    assert!(!trash_directory.exists());
    Ok(())
}

#[test]
fn committed_pending_reclamation_result_is_stable_after_cleanup_retry()
-> Result<(), Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    let store = open_store(root.path())?;
    let (_run_id, plan_id, confirm_id) = admit_run_pruning(&store, "reclaim-io-retry")?;

    let committed = confirmation::confirm_with_reclaim_io_error(&store, &plan_id, &confirm_id)?;
    assert!(matches!(
        committed,
        RetentionMutationResult::Pruned {
            physical_reclamation_pending: true,
            ..
        }
    ));
    assert!(
        store
            .load_retention_plan(&plan_id)?
            .physical_reclamation_pending
    );
    store.migrate_lifecycle_store()?;

    assert_eq!(
        store.confirm_retention_plan(&plan_id, &confirm_id)?,
        committed
    );
    assert!(
        !store
            .load_retention_plan(&plan_id)?
            .physical_reclamation_pending
    );
    store.migrate_lifecycle_store()?;
    assert_eq!(
        store.confirm_retention_plan(&plan_id, &confirm_id)?,
        committed
    );
    Ok(())
}

fn assert_integrity_error(
    result: Result<RetentionMutationResult, StoreError>,
    expected_message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    match result {
        Err(StoreError::Integrity(message)) if message.contains(expected_message) => Ok(()),
        Err(error) => Err(format!("unexpected retention error: {error}").into()),
        Ok(result) => Err(format!("retention unexpectedly succeeded: {result:?}").into()),
    }
}
