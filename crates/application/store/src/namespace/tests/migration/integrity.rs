use std::fs;

use lumin_evidence::{
    ActualWriteSet, GateDecision, GateLifecycle, GateObservationBinding, GateOperationStatus,
    GateRecord, GateSignal, OperationRecord, RetentionMutationResult, RetentionOperationRecord,
    RetentionOperationResult, RetentionPlanScope, WorktreeTransition,
};
use lumin_model::{
    AnalysisInputId, AttemptId, DeltaFactFamily, DeltaKey, GateDeltaClassification,
    GateDeltaRecord, ObservationBinding, OperationId, ResolutionProfile, RunId,
    UnsealedObservationReason,
};
use redb::{Database, ReadableTable};

use crate::gate::{
    GATES, OPERATIONS, TRANSITIONS, records::ACTIVE_GATE_CATALOG_SEQUENCE_KEY, transition_key,
};
use crate::retention::RETENTION_OPERATIONS;
use crate::{RUN_CATALOG, RunCatalogRecord, SEQUENCES, StoreError};

use super::super::open_store;
use super::{
    append_non_authorizing_close_for_migration, append_unsealed_close_for_migration,
    close_active_gate_for_migration, current_generation, evidence, open_active_gate_for,
    open_active_gate_for_with_protected_inputs, path, semantic_input,
};

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
    let mut attempt = store.begin_attempt()?;
    let published = store.publish_run(&mut attempt, &evidence(), |_| Ok(()))?;
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
        Err(StoreError::Integrity(message))
            if message.contains("not owned by committed gate operations")
    ));
    Ok(())
}

#[test]
fn migration_reconstructs_gate_observations_with_their_catalog_revision()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate_for(&store, "op-catalog-binding", "src/catalog.ts")?;
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(GATES)?;
        let bytes = table
            .get(gate_id.as_str())?
            .ok_or("catalog-bound gate is missing")?
            .value()
            .to_vec();
        let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
        let baseline = gate
            .baseline
            .as_mut()
            .ok_or("catalog-bound gate omitted its baseline")?;
        baseline.catalog_revision = baseline.catalog_revision.saturating_add(1);
        gate.revisions
            .first_mut()
            .ok_or("catalog-bound gate omitted its opening revision")?
            .catalog_revision = Some(baseline.catalog_revision);
        let changed = serde_json::to_vec(&gate)?;
        table.insert(gate_id.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("baseline observation cannot be reconstructed")
    ));
    Ok(())
}

#[test]
fn migration_binds_analysis_options_to_the_opening_operation_and_sealed_baseline()
-> Result<(), Box<dyn std::error::Error>> {
    for corruption in ["gate-and-operation", "operation-only"] {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        let operation_id = OperationId::from_string(format!("op-analysis-options-{corruption}"));
        let gate_id = open_active_gate_for(
            &store,
            operation_id.as_str(),
            &format!("src/analysis-options-{corruption}.ts"),
        )?;
        let mut forged_options = store.load_gate(&gate_id)?.analysis_options;
        forged_options.resolution_profile = Some(ResolutionProfile::Node16);
        forged_options.scan_invocation.resolution_profile = Some(ResolutionProfile::Node16);
        drop(store);

        let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
        let write = database.begin_write()?;
        if corruption == "gate-and-operation" {
            let mut table = write.open_table(GATES)?;
            let bytes = table
                .get(gate_id.as_str())?
                .ok_or("analysis-options gate is missing")?
                .value()
                .to_vec();
            let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
            gate.analysis_options = forged_options.clone();
            let changed = serde_json::to_vec(&gate)?;
            table.insert(gate_id.as_str(), changed.as_slice())?;
        }
        {
            let mut table = write.open_table(OPERATIONS)?;
            let bytes = table
                .get(operation_id.as_str())?
                .ok_or("analysis-options opening operation is missing")?
                .value()
                .to_vec();
            let mut operation = serde_json::from_slice::<OperationRecord>(&bytes)?;
            operation.analysis_options = Some(forged_options);
            let changed = serde_json::to_vec(&operation)?;
            table.insert(operation_id.as_str(), changed.as_slice())?;
        }
        write.commit()?;
        drop(database);

        let store = open_store(root.path())?;
        assert!(matches!(
            store.migrate_lifecycle_store(),
            Err(StoreError::Integrity(message))
                if message.contains("analysis invocation disagrees with its sealed baseline")
                    || message.contains("analysis options disagree with its opening operation")
        ));
    }
    Ok(())
}

#[test]
fn migration_rejects_close_only_payloads_on_the_opening_revision()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate_for(&store, "op-opening-payload", "src/opening-payload.ts")?;
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(GATES)?;
        let bytes = table
            .get(gate_id.as_str())?
            .ok_or("opening-payload gate is missing")?
            .value()
            .to_vec();
        let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
        let forged_snapshot = gate
            .baseline
            .as_ref()
            .ok_or("opening-payload gate omitted its baseline")?
            .snapshot
            .clone();
        gate.revisions
            .first_mut()
            .ok_or("opening-payload gate omitted its opening revision")?
            .snapshot = Some(forged_snapshot);
        let changed = serde_json::to_vec(&gate)?;
        table.insert(gate_id.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("opening revision retained close-only payloads")
    ));
    Ok(())
}

#[test]
fn migration_rejects_an_active_lease_domain_weaker_than_its_sealed_baseline()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate_for(&store, "op-active-domain", "src/active-domain.ts")?;
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(GATES)?;
        let bytes = table
            .get(gate_id.as_str())?
            .ok_or("active-domain gate is missing")?
            .value()
            .to_vec();
        let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
        gate.leased_write_set.clear();
        let changed = serde_json::to_vec(&gate)?;
        table.insert(gate_id.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("lease/alias domain disagrees with its sealed baseline")
    ));
    Ok(())
}

#[test]
fn migration_rejects_an_active_protected_read_set_weaker_than_its_latest_sealed_observation()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate_for_with_protected_inputs(
        &store,
        "op-active-protected",
        "src/active-protected.ts",
        vec![semantic_input("config/opening.json")?],
    )?;
    append_non_authorizing_close_for_migration(
        &store,
        &gate_id,
        vec![semantic_input("config/latest.json")?],
    )?;
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(GATES)?;
        let bytes = table
            .get(gate_id.as_str())?
            .ok_or("active-protected gate is missing")?
            .value()
            .to_vec();
        let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
        gate.protected_semantic_inputs.clear();
        let changed = serde_json::to_vec(&gate)?;
        table.insert(gate_id.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("protected read set disagrees with its latest sealed observation")
    ));
    Ok(())
}

#[test]
fn migration_rejects_closed_lifecycle_without_an_authorizing_tail()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate_for(&store, "op-false-closed", "src/false-closed.ts")?;
    append_non_authorizing_close_for_migration(&store, &gate_id, Vec::new())?;
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(GATES)?;
        let bytes = table
            .get(gate_id.as_str())?
            .ok_or("false-closed gate is missing")?
            .value()
            .to_vec();
        let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
        gate.lifecycle = GateLifecycle::Closed;
        let changed = serde_json::to_vec(&gate)?;
        table.insert(gate_id.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("lifecycle disagrees with its authorizing revision tail")
    ));
    Ok(())
}

#[test]
fn migration_binds_transition_payloads_to_the_sealed_gate_revision()
-> Result<(), Box<dyn std::error::Error>> {
    for corruption in ["changed-paths", "leased-write-set", "after-snapshot"] {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        let gate_id = open_active_gate_for(
            &store,
            &format!("op-transition-{corruption}"),
            &format!("src/transition-{corruption}.ts"),
        )?;
        close_active_gate_for_migration(&store, &gate_id)?;
        drop(store);

        let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
        let write = database.begin_write()?;
        {
            let mut table = write.open_table(TRANSITIONS)?;
            let key = transition_key(1);
            let bytes = table
                .get(key.as_str())?
                .ok_or("worktree transition is missing")?
                .value()
                .to_vec();
            let mut transition = serde_json::from_slice::<WorktreeTransition>(&bytes)?;
            match corruption {
                "changed-paths" => transition
                    .capsule
                    .changed_paths
                    .push(path("src/injected.ts")?),
                "leased-write-set" => transition.capsule.leased_write_set.clear(),
                "after-snapshot" => {
                    transition.capsule.after_snapshot.analysis_input_id =
                        AnalysisInputId::from_string("analysis_input_injected".to_owned());
                }
                _ => unreachable!(),
            }
            let changed = serde_json::to_vec(&transition)?;
            table.insert(key.as_str(), changed.as_slice())?;
        }
        write.commit()?;
        drop(database);

        let store = open_store(root.path())?;
        assert!(matches!(
            store.migrate_lifecycle_store(),
            Err(StoreError::Integrity(message))
                if message.contains("payload or observation binding disagrees")
        ));
    }
    Ok(())
}

#[test]
fn migration_rejects_a_transition_before_snapshot_outside_its_replayed_chain()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let first_gate = open_active_gate_for(&store, "op-before-chain-a", "src/chain-a.ts")?;
    let second_gate = open_active_gate_for(&store, "op-before-chain-b", "src/chain-b.ts")?;
    close_active_gate_for_migration(&store, &second_gate)?;
    close_active_gate_for_migration(&store, &first_gate)?;
    store.migrate_lifecycle_store()?;
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(TRANSITIONS)?;
        let key = transition_key(2);
        let bytes = table
            .get(key.as_str())?
            .ok_or("second worktree transition is missing")?
            .value()
            .to_vec();
        let mut transition = serde_json::from_slice::<WorktreeTransition>(&bytes)?;
        transition.capsule.before_snapshot.analysis_input_id =
            AnalysisInputId::from_string("analysis_input_injected_before".to_owned());
        let changed = serde_json::to_vec(&transition)?;
        table.insert(key.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("payload or observation binding disagrees")
    ));
    Ok(())
}

#[test]
fn migration_reconstructs_close_observations_with_their_catalog_revision()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate_for(&store, "op-close-catalog", "src/close-catalog.ts")?;
    close_active_gate_for_migration(&store, &gate_id)?;
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(GATES)?;
        let bytes = table
            .get(gate_id.as_str())?
            .ok_or("close catalog gate is missing")?
            .value()
            .to_vec();
        let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
        let revision = gate
            .revisions
            .get_mut(1)
            .ok_or("close catalog gate omitted its close revision")?;
        revision.catalog_revision = revision
            .catalog_revision
            .map(|value| value.saturating_add(1));
        let changed = serde_json::to_vec(&gate)?;
        table.insert(gate_id.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("close observation revision 1 cannot be reconstructed")
    ));
    Ok(())
}

#[test]
fn migration_rejects_complete_observation_payloads_on_an_unsealed_close()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate_for(&store, "op-unsealed-payload", "src/unsealed.ts")?;
    close_active_gate_for_migration(&store, &gate_id)?;
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(GATES)?;
        let bytes = table
            .get(gate_id.as_str())?
            .ok_or("unsealed-payload gate is missing")?
            .value()
            .to_vec();
        let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
        let revision = gate
            .revisions
            .get_mut(1)
            .ok_or("unsealed-payload gate omitted its close revision")?;
        revision.decision = GateDecision::Incomplete;
        revision.observation_binding = Some(ObservationBinding::Unsealed {
            reason: UnsealedObservationReason::AnalysisFailed,
            attempted_domain: Vec::new(),
            last_complete_read_set: Vec::new(),
            conflicting_or_unbounded_inputs: Vec::new(),
        });
        revision.signals = vec![GateSignal::AnalysisFailed {
            detail: "fixture forces an unsealed close".to_owned(),
        }];
        let changed = serde_json::to_vec(&gate)?;
        table.insert(gate_id.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("unsealed revision 1 retained complete-observation payloads")
    ));
    Ok(())
}

#[test]
fn migration_reseals_every_persisted_gate_analysis_snapshot()
-> Result<(), Box<dyn std::error::Error>> {
    for role in ["baseline", "close"] {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        let gate_id = open_active_gate_for(
            &store,
            &format!("op-reseal-{role}"),
            &format!("src/reseal-{role}.ts"),
        )?;
        if role == "close" {
            close_active_gate_for_migration(&store, &gate_id)?;
        }
        drop(store);

        let injected = semantic_input(&format!("config/reseal-{role}.json"))?;
        let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
        let write = database.begin_write()?;
        {
            let mut table = write.open_table(GATES)?;
            let bytes = table
                .get(gate_id.as_str())?
                .ok_or("reseal gate is missing")?
                .value()
                .to_vec();
            let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
            if role == "baseline" {
                gate.baseline
                    .as_mut()
                    .ok_or("reseal gate omitted its baseline")?
                    .snapshot
                    .inputs
                    .push(injected.clone());
            } else {
                gate.revisions
                    .get_mut(1)
                    .and_then(|revision| revision.snapshot.as_mut())
                    .ok_or("reseal gate omitted its close snapshot")?
                    .inputs
                    .push(injected.clone());
            }
            let changed = serde_json::to_vec(&gate)?;
            table.insert(gate_id.as_str(), changed.as_slice())?;
        }
        if role == "close" {
            let mut table = write.open_table(TRANSITIONS)?;
            let key = transition_key(1);
            let bytes = table
                .get(key.as_str())?
                .ok_or("reseal transition is missing")?
                .value()
                .to_vec();
            let mut transition = serde_json::from_slice::<WorktreeTransition>(&bytes)?;
            transition
                .capsule
                .after_snapshot
                .inputs
                .push(injected.clone());
            let changed = serde_json::to_vec(&transition)?;
            table.insert(key.as_str(), changed.as_slice())?;
        }
        write.commit()?;
        drop(database);

        let store = open_store(root.path())?;
        assert!(matches!(
            store.migrate_lifecycle_store(),
            Err(StoreError::Integrity(message))
                if message.contains("analysis input identity cannot be reconstructed")
        ));
    }
    Ok(())
}

#[test]
fn migration_matches_the_complete_operation_result_to_its_revision()
-> Result<(), Box<dyn std::error::Error>> {
    for corruption in [
        "lifecycle",
        "signals",
        "leased-write-set",
        "actual-write-set",
        "deltas",
        "reason",
    ] {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        let operation_id = OperationId::from_string(format!("op-result-{corruption}"));
        open_active_gate_for(
            &store,
            operation_id.as_str(),
            &format!("src/result-{corruption}.ts"),
        )?;
        drop(store);

        let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
        let write = database.begin_write()?;
        {
            let mut table = write.open_table(OPERATIONS)?;
            let bytes = table
                .get(operation_id.as_str())?
                .ok_or("result operation is missing")?
                .value()
                .to_vec();
            let mut operation = serde_json::from_slice::<OperationRecord>(&bytes)?;
            let result = operation
                .result
                .as_mut()
                .ok_or("operation result is missing")?;
            match corruption {
                "lifecycle" => result.lifecycle = GateLifecycle::Closed,
                "signals" => {
                    result
                        .signals
                        .push(GateSignal::FindingWarnings { count: 1 });
                    result.decision = GateDecision::AllowWithWarnings;
                }
                "leased-write-set" => result.leased_write_set.clear(),
                "actual-write-set" => result.actual_write_set = Some(ActualWriteSet::default()),
                "deltas" => result.deltas.push(GateDeltaRecord {
                    key: DeltaKey {
                        owner_capability: "test/migration-result.v1".to_owned(),
                        family: DeltaFactFamily::Opacity,
                        semantic_identity: vec![1],
                    },
                    classification: GateDeltaClassification::Introduced,
                }),
                "reason" => result.reason = Some("forged result reason".to_owned()),
                _ => unreachable!(),
            }
            let changed = serde_json::to_vec(&operation)?;
            table.insert(operation_id.as_str(), changed.as_slice())?;
        }
        write.commit()?;
        drop(database);

        let store = open_store(root.path())?;
        assert!(matches!(
            store.migrate_lifecycle_store(),
            Err(StoreError::Integrity(message))
                if message.contains("result disagrees with its complete gate revision")
        ));
    }
    Ok(())
}

#[test]
fn migration_recomputes_gate_decisions_from_their_signals() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let operation_id = OperationId::from_string("op-signal-decision".to_owned());
    let gate_id = open_active_gate_for(&store, operation_id.as_str(), "src/signal-decision.ts")?;
    drop(store);

    let forged_signals = vec![GateSignal::AnalysisFailed {
        detail: "forged migration signal".to_owned(),
    }];
    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(GATES)?;
        let bytes = table
            .get(gate_id.as_str())?
            .ok_or("signal-decision gate is missing")?
            .value()
            .to_vec();
        let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
        gate.revisions
            .first_mut()
            .ok_or("signal-decision gate omitted its opening revision")?
            .signals = forged_signals.clone();
        let changed = serde_json::to_vec(&gate)?;
        table.insert(gate_id.as_str(), changed.as_slice())?;
    }
    {
        let mut table = write.open_table(OPERATIONS)?;
        let bytes = table
            .get(operation_id.as_str())?
            .ok_or("signal-decision operation is missing")?
            .value()
            .to_vec();
        let mut operation = serde_json::from_slice::<OperationRecord>(&bytes)?;
        operation
            .result
            .as_mut()
            .ok_or("signal-decision result is missing")?
            .signals = forged_signals;
        let changed = serde_json::to_vec(&operation)?;
        table.insert(operation_id.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("decision disagrees with canonical signal policy")
    ));
    Ok(())
}

#[test]
fn migration_rejects_a_regressed_active_gate_catalog_sequence()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let abandoned_gate = open_active_gate_for(
        &store,
        "op-catalog-regression-a",
        "src/catalog-regression-a.ts",
    )?;
    open_active_gate_for(
        &store,
        "op-catalog-regression-b",
        "src/catalog-regression-b.ts",
    )?;
    let abandon_id = OperationId::from_string("op-catalog-regression-abandon".to_owned());
    store.begin_operation(&abandon_id)?.abandon_gate(
        "catalog-regression-abandon-digest",
        &abandoned_gate,
        0,
        "catalog regression fixture",
    )?;
    assert_eq!(store.list_active_gates(None, 100)?.revision, 3);
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(SEQUENCES)?;
        table.insert(ACTIVE_GATE_CATALOG_SEQUENCE_KEY, 2)?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("active-gate catalog sequence regressed")
    ));
    Ok(())
}

#[test]
fn migration_requires_every_durable_revision_to_have_a_committed_result()
-> Result<(), Box<dyn std::error::Error>> {
    for status in [
        GateOperationStatus::Pending,
        GateOperationStatus::Interrupted,
    ] {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        let operation_id = OperationId::from_string(format!("op-revision-{status:?}"));
        open_active_gate_for(
            &store,
            operation_id.as_str(),
            &format!("src/revision-{status:?}.ts"),
        )?;
        drop(store);

        let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
        let write = database.begin_write()?;
        {
            let mut table = write.open_table(OPERATIONS)?;
            let bytes = table
                .get(operation_id.as_str())?
                .ok_or("revision operation is missing")?
                .value()
                .to_vec();
            let mut operation = serde_json::from_slice::<OperationRecord>(&bytes)?;
            operation.status = status;
            operation.result = None;
            let changed = serde_json::to_vec(&operation)?;
            table.insert(operation_id.as_str(), changed.as_slice())?;
        }
        write.commit()?;
        drop(database);

        let store = open_store(root.path())?;
        assert!(matches!(
            store.migrate_lifecycle_store(),
            Err(StoreError::Integrity(message))
                if message.contains("not owned by committed gate operations")
        ));
    }
    Ok(())
}

#[test]
fn migration_reconstructs_every_unsealed_observation_field_from_typed_inputs()
-> Result<(), Box<dyn std::error::Error>> {
    for corruption in [
        "reason",
        "attempted-domain",
        "last-complete-read-set",
        "conflicting-inputs",
    ] {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        let gate_id = open_active_gate_for_with_protected_inputs(
            &store,
            &format!("op-unsealed-{corruption}"),
            &format!("src/unsealed-{corruption}.ts"),
            vec![semantic_input("config/last-complete.json")?],
        )?;
        let operation_id = append_unsealed_close_for_migration(&store, &gate_id)?;
        let gate = store.load_gate(&gate_id)?;
        let revision = gate
            .revisions
            .get(1)
            .ok_or("unsealed close revision is missing")?;
        let inputs = revision
            .unsealed_observation_inputs
            .as_ref()
            .ok_or("unsealed close omitted its typed inputs")?;
        assert_eq!(
            inputs
                .attempted_semantic_inputs
                .iter()
                .map(|binding| binding.path.display.as_str())
                .collect::<Vec<_>>(),
            ["config/unsealed-attempt.json"]
        );
        store.migrate_lifecycle_store()?;
        drop(store);

        let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
        let write = database.begin_write()?;
        let forged_binding = {
            let mut table = write.open_table(GATES)?;
            let bytes = table
                .get(gate_id.as_str())?
                .ok_or("unsealed gate is missing")?
                .value()
                .to_vec();
            let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
            let binding = gate
                .revisions
                .get_mut(1)
                .and_then(|revision| revision.observation_binding.as_mut())
                .ok_or("unsealed gate omitted its binding")?;
            corrupt_unsealed_binding(binding, corruption)?;
            let forged = binding.clone();
            let changed = serde_json::to_vec(&gate)?;
            table.insert(gate_id.as_str(), changed.as_slice())?;
            forged
        };
        {
            let mut table = write.open_table(OPERATIONS)?;
            let bytes = table
                .get(operation_id.as_str())?
                .ok_or("unsealed operation is missing")?
                .value()
                .to_vec();
            let mut operation = serde_json::from_slice::<OperationRecord>(&bytes)?;
            operation
                .result
                .as_mut()
                .ok_or("unsealed operation omitted its result")?
                .observation_binding = Some(forged_binding);
            let changed = serde_json::to_vec(&operation)?;
            table.insert(operation_id.as_str(), changed.as_slice())?;
        }
        write.commit()?;
        drop(database);

        let store = open_store(root.path())?;
        assert!(matches!(
            store.migrate_lifecycle_store(),
            Err(StoreError::Integrity(message))
                if message.contains("unsealed revision 1 cannot be reconstructed")
                    || message.contains("changed its last complete read set")
        ));
    }
    Ok(())
}

fn corrupt_unsealed_binding(
    binding: &mut GateObservationBinding,
    corruption: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let ObservationBinding::Unsealed {
        reason,
        attempted_domain,
        last_complete_read_set,
        conflicting_or_unbounded_inputs,
    } = binding
    else {
        return Err("fixture expected an unsealed observation".into());
    };
    match corruption {
        "reason" => *reason = UnsealedObservationReason::DeclaredPathUnsupported,
        "attempted-domain" => attempted_domain.push(path("src/forged-attempt.ts")?),
        "last-complete-read-set" => last_complete_read_set.clear(),
        "conflicting-inputs" => conflicting_or_unbounded_inputs.clear(),
        _ => unreachable!(),
    }
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
