use std::fs;

use lumin_evidence::{
    GateRecord, RetentionMutationResult, RetentionOperationRecord, RetentionOperationResult,
    RetentionPlanScope,
};
use lumin_model::{AttemptId, OperationId, RunId};
use redb::{Database, ReadableTable};

use crate::gate::GATES;
use crate::retention::RETENTION_OPERATIONS;
use crate::{RUN_CATALOG, RunCatalogRecord, StoreError};

use super::super::open_store;
use super::{current_generation, evidence, open_active_gate_for};

#[test]
fn unpublished_intent_bytes_are_discarded_before_reopen() -> Result<(), Box<dyn std::error::Error>>
{
    for bytes in [b"".as_slice(), b"{\"fromGeneration\":1".as_slice()] {
        let root = tempfile::tempdir()?;
        drop(open_store(root.path())?);
        let pending = root.path().join(".lumin/lifecycle-migration.json.pending");
        fs::write(&pending, bytes)?;

        let reopened = open_store(root.path())?;
        assert_eq!(
            current_generation(&reopened)?,
            crate::StoreGeneration::INITIAL
        );
        assert!(!pending.exists());
        assert!(!root.path().join(".lumin/lifecycle-migration.json").exists());
    }
    Ok(())
}

#[test]
fn malformed_published_intent_remains_a_hard_stop() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    drop(open_store(root.path())?);
    fs::write(root.path().join(".lumin/lifecycle-migration.json"), b"")?;

    assert!(matches!(
        open_store(root.path()),
        Err(StoreError::Integrity(_))
    ));
    Ok(())
}

#[test]
fn migration_rejects_run_ids_that_escape_the_managed_parent()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    drop(open_store(root.path())?);
    let escaped_id = "../../outside";
    let outside = root.path().join("outside");
    fs::create_dir(&outside)?;
    fs::write(outside.join("evidence.store"), b"")?;
    let record = RunCatalogRecord {
        attempt_id: AttemptId::from_string("attempt_0000000000000001".to_owned()),
        run_id: RunId::from_string(escaped_id.to_owned()),
        sequence: 1,
        evidence_store_sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
            .to_owned(),
        evidence_store_size: 0,
    };
    fs::write(outside.join("run.json"), serde_json::to_vec(&record)?)?;

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(RUN_CATALOG)?;
        let bytes = serde_json::to_vec(&record)?;
        table.insert(escaped_id, bytes.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(_))
    ));
    Ok(())
}

#[test]
fn migration_rejects_hard_linked_run_evidence() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let attempt = store.begin_attempt()?;
    let published = store.publish_run(&attempt, &evidence())?;
    drop(store);

    let evidence_path = root
        .path()
        .join(".lumin/runs")
        .join(published.run_id.as_str())
        .join("evidence.store");
    let outside = root.path().join("outside-evidence.store");
    fs::copy(&evidence_path, &outside)?;
    fs::remove_file(&evidence_path)?;
    fs::hard_link(&outside, &evidence_path)?;

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(_))
    ));
    Ok(())
}

#[test]
fn migration_rejects_revision_owned_by_another_gate_operation()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let first_gate = open_active_gate_for(&store, "op-cross-a", "src/a.ts")?;
    let second_operation = OperationId::from_string("op-cross-b".to_owned());
    open_active_gate_for(&store, second_operation.as_str(), "src/b.ts")?;
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(GATES)?;
        let bytes = table
            .get(first_gate.as_str())?
            .ok_or("first gate is missing")?
            .value()
            .to_vec();
        let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
        gate.revisions
            .first_mut()
            .ok_or("first gate revision is missing")?
            .operation_id = second_operation;
        let changed = serde_json::to_vec(&gate)?;
        table.insert(first_gate.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message)) if message.contains("not owned by that gate")
    ));
    Ok(())
}

#[test]
fn migration_rejects_retention_result_owned_by_another_plan()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let first = store.prepare_retention_plan(&crate::RetentionPlanRequest {
        scope: RetentionPlanScope::Runs {
            before_unix_millis: u64::MAX,
        },
        operation_id: OperationId::from_string("retention-plan-first".to_owned()),
    })?;
    let second = store.prepare_retention_plan(&crate::RetentionPlanRequest {
        scope: RetentionPlanScope::Runs {
            before_unix_millis: u64::MAX,
        },
        operation_id: OperationId::from_string("retention-plan-second".to_owned()),
    })?;
    let first_plan = prepared_plan_id(first)?;
    let second_plan = prepared_plan_id(second)?;
    let confirmation_id = OperationId::from_string("retention-confirm-first".to_owned());
    store.confirm_retention_plan(&first_plan, &confirmation_id)?;
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(RETENTION_OPERATIONS)?;
        let bytes = table
            .get(confirmation_id.as_str())?
            .ok_or("retention confirmation is missing")?
            .value()
            .to_vec();
        let mut operation = serde_json::from_slice::<RetentionOperationRecord>(&bytes)?;
        match &mut operation.result {
            RetentionOperationResult::Retention {
                result: RetentionMutationResult::Pruned { plan_id, .. },
            } => *plan_id = second_plan,
            result => return Err(format!("unexpected confirmation result: {result:?}").into()),
        }
        let changed = serde_json::to_vec(&operation)?;
        table.insert(confirmation_id.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message)) if message.contains("incoherent kind, status, plan, or result")
    ));
    Ok(())
}

fn prepared_plan_id(
    result: RetentionMutationResult,
) -> Result<lumin_model::RetentionPlanId, Box<dyn std::error::Error>> {
    match result {
        RetentionMutationResult::Prepared { plan_id, .. } => Ok(plan_id),
        other => Err(format!("unexpected retention plan result: {other:?}").into()),
    }
}
