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
