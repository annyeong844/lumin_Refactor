use std::fs;

use lumin_evidence::GateRecord;
use lumin_model::{AttemptId, OperationId, RunId};
use redb::{Database, ReadableTable};

use crate::gate::GATES;
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
