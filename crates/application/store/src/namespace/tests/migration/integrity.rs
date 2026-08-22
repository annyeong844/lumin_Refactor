use std::fs;

use lumin_evidence::{
    ActualWriteSet, GateBaselineObservationInput, GateCloseObservationInput, GateDecision,
    GateLifecycle, GateObservationBinding, GateOperationStatus, GateRecord, GateSignal,
    OperationRecord, PathPrefixIdentity, RepoPathProjection, RetentionMutationResult,
    RetentionOperationRecord, RetentionOperationResult, RetentionPlanScope,
    UnsealedGateObservationInputs, WorktreeTransition, WriteLease, WriteLeaseKind,
    derive_gate_baseline_observation_id, derive_gate_close_observation_id,
    derive_protected_semantic_inputs, derive_unsealed_gate_observation_binding,
    seal_analysis_snapshot,
};
use lumin_model::{
    AnalysisInputId, AttemptId, CapabilityState, DeltaFactFamily, DeltaKey,
    GateDeltaClassification, GateDeltaRecord, ObservationBinding, OperationId,
    PhysicalFileIdentity, RepoPath, ResolutionProfile, RunId, SealedGateObservation,
    UnsealedObservationReason,
};
use redb::{Database, ReadableTable};

use crate::gate::{
    GATES, OPERATIONS, TRANSITIONS, records::ACTIVE_GATE_CATALOG_SEQUENCE_KEY, transition_key,
};
use crate::retention::RETENTION_OPERATIONS;
use crate::{
    GateBaselineDraft, ObservationFinalization, PreWriteFinish, PreWriteStart, RUN_CATALOG,
    RunCatalogRecord, SEQUENCES, StoreError,
};

use super::super::open_store;
use super::{
    append_non_authorizing_close_for_migration, append_unsealed_close_for_migration,
    close_active_gate_for_migration, current_generation, evidence, open_active_gate_for,
    open_active_gate_for_with_protected_inputs, options, path, rejected_test_observation,
    semantic_input,
};

fn reconstructed_baseline_binding(
    gate: &GateRecord,
) -> Result<GateObservationBinding, Box<dyn std::error::Error>> {
    let baseline = gate.baseline.as_ref().ok_or("gate baseline is missing")?;
    let evidence_payload_sha256 = crate::evidence_payload_sha256(&baseline.snapshot.evidence)?;
    Ok(ObservationBinding::Sealed {
        observation: SealedGateObservation::Baseline {
            observation_id: derive_gate_baseline_observation_id(GateBaselineObservationInput {
                catalog_revision: baseline.catalog_revision,
                transition_sequence: baseline.transition_sequence,
                analysis_contract: &baseline.analysis_contract,
                analysis_input_id: &baseline.snapshot.analysis_input_id,
                evidence_payload_sha256: &evidence_payload_sha256,
                declared_write_set: &gate.declared_write_set,
                leased_write_set: &baseline.leased_write_set,
                alias_closures: &baseline.alias_closures,
                protected_semantic_inputs: &baseline.protected_semantic_inputs,
            }),
        },
    })
}

fn reconstructed_close_binding(
    gate: &GateRecord,
    revision_index: usize,
) -> Result<GateObservationBinding, Box<dyn std::error::Error>> {
    let baseline = gate.baseline.as_ref().ok_or("gate baseline is missing")?;
    let revision = gate
        .revisions
        .get(revision_index)
        .ok_or("gate close revision is missing")?;
    let snapshot = revision
        .snapshot
        .as_ref()
        .ok_or("close snapshot is missing")?;
    let actual_write_set = revision
        .actual_write_set
        .as_ref()
        .ok_or("close actual-write set is missing")?;
    let evidence_payload_sha256 = crate::evidence_payload_sha256(&snapshot.evidence)?;
    Ok(ObservationBinding::Sealed {
        observation: SealedGateObservation::Close {
            observation_id: derive_gate_close_observation_id(GateCloseObservationInput {
                gate_id: &gate.gate_id,
                opening_observation_id: &baseline.observation_id,
                opening_analysis_contract: &baseline.analysis_contract,
                prior_revision: revision.revision.saturating_sub(1),
                catalog_revision: revision
                    .catalog_revision
                    .ok_or("close catalog revision is missing")?,
                analysis_input_id: &snapshot.analysis_input_id,
                evidence_payload_sha256: &evidence_payload_sha256,
                leased_write_set: &baseline.leased_write_set,
                protected_semantic_inputs: &revision.protected_semantic_inputs,
                changed_paths: &revision.changed_paths,
                actual_write_set,
                alias_closures: &revision.alias_closures,
                reconciled_transition_sequences: &revision.reconciled_transition_sequences,
            }),
        },
    })
}

fn different_physical_identity(identity: PhysicalFileIdentity) -> PhysicalFileIdentity {
    match identity {
        PhysicalFileIdentity::Unix { device, inode } => PhysicalFileIdentity::Unix {
            device,
            inode: inode.wrapping_add(1),
        },
        PhysicalFileIdentity::Windows {
            volume_serial,
            file_index,
        } => PhysicalFileIdentity::Windows {
            volume_serial,
            file_index: file_index.wrapping_add(1),
        },
    }
}

fn open_rejected_gate_for(
    store: &crate::RepositoryStore,
    operation: &str,
    source: &str,
) -> Result<lumin_model::GateId, Box<dyn std::error::Error>> {
    let operation_id = OperationId::from_string(operation.to_owned());
    let session = store.begin_operation(&operation_id)?;
    let source = path(source)?;
    let lease = WriteLease {
        path: source.clone(),
        kind: WriteLeaseKind::ExistingFile,
        physical_identity: None,
        nearest_existing_parent: None,
        prefix_identities: Vec::new(),
    };
    let (gate_id, transition_sequence) = match session.reserve_pre_write(
        "migrate-rejected-digest",
        std::slice::from_ref(&source),
        std::slice::from_ref(&lease),
        &options(),
        rejected_test_observation,
    )? {
        PreWriteStart::Analyze {
            gate_id,
            transition_sequence,
        } => (gate_id, transition_sequence),
        PreWriteStart::Committed(_) => return Err("rejected gate fixture committed early".into()),
    };
    let baseline = GateBaselineDraft {
        analysis_contract: "migration-rejected-contract".to_owned(),
        snapshot: seal_analysis_snapshot(Vec::new(), evidence(), Default::default(), Vec::new()),
        protected_semantic_inputs: Vec::new(),
        transition_sequence,
    };
    let unsealed_inputs =
        UnsealedGateObservationInputs::new(vec![lease.clone()], Vec::new(), Vec::new());
    let result = session.finish_pre_write(
        "migrate-rejected-digest",
        &gate_id,
        PreWriteFinish {
            baseline: Some(baseline),
            leased_write_set: vec![lease],
            alias_closures: Vec::new(),
            attempted_semantic_inputs: Vec::new(),
            signals: vec![GateSignal::AnalysisFailed {
                detail: "injected rejected-opening failure".to_owned(),
            }],
        },
        |_, _, signals| ObservationFinalization {
            signals: Vec::new(),
            binding: derive_unsealed_gate_observation_binding(
                std::slice::from_ref(&source),
                &unsealed_inputs,
                signals,
            ),
        },
    )?;
    assert_eq!(result.lifecycle, GateLifecycle::Rejected);
    assert!(result.leased_write_set.is_empty());
    Ok(gate_id)
}

#[test]
fn migration_rejects_leases_on_a_rejected_gate() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_rejected_gate_for(&store, "op-rejected-lease", "src/rejected.ts")?;
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(GATES)?;
        let bytes = table
            .get(gate_id.as_str())?
            .ok_or("rejected gate is missing")?
            .value()
            .to_vec();
        let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
        gate.leased_write_set = gate
            .revisions
            .first()
            .and_then(|revision| revision.unsealed_observation_inputs.as_ref())
            .ok_or("rejected gate omitted its unsealed observation inputs")?
            .attempted_write_leases
            .clone();
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
fn migration_authenticates_the_baseline_transition_boundary_against_catalog_epochs()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_a = open_active_gate_for(&store, "op-boundary-a", "src/a.ts")?;
    let gate_b = open_active_gate_for(&store, "op-boundary-b", "src/b.ts")?;
    close_active_gate_for_migration(&store, &gate_b)?;
    drop(store);

    let opening_operation_id = OperationId::from_string("op-boundary-a".to_owned());
    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    let binding = {
        let mut table = write.open_table(GATES)?;
        let bytes = table
            .get(gate_a.as_str())?
            .ok_or("boundary gate is missing")?
            .value()
            .to_vec();
        let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
        gate.baseline
            .as_mut()
            .ok_or("boundary gate omitted its baseline")?
            .transition_sequence = 1;
        gate.transition_refs.clear();
        let binding = reconstructed_baseline_binding(&gate)?;
        let ObservationBinding::Sealed {
            observation: SealedGateObservation::Baseline { observation_id },
        } = &binding
        else {
            return Err("boundary fixture produced the wrong binding".into());
        };
        gate.baseline
            .as_mut()
            .ok_or("boundary baseline disappeared")?
            .observation_id = observation_id.clone();
        gate.revisions[0].observation_binding = Some(binding.clone());
        let changed = serde_json::to_vec(&gate)?;
        table.insert(gate_a.as_str(), changed.as_slice())?;
        binding
    };
    {
        let mut table = write.open_table(OPERATIONS)?;
        let bytes = table
            .get(opening_operation_id.as_str())?
            .ok_or("boundary opening operation is missing")?
            .value()
            .to_vec();
        let mut operation = serde_json::from_slice::<OperationRecord>(&bytes)?;
        operation.transition_sequence = 1;
        operation
            .result
            .as_mut()
            .ok_or("boundary opening result is missing")?
            .observation_binding = Some(binding);
        let changed = serde_json::to_vec(&operation)?;
        table.insert(opening_operation_id.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("baseline transition boundary disagrees with its catalog epoch")
    ));
    Ok(())
}

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
fn migration_binds_the_opening_revision_payload_to_its_sealed_baseline()
-> Result<(), Box<dyn std::error::Error>> {
    for corruption in ["protected-inputs", "alias-closures"] {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        let gate_id = open_active_gate_for_with_protected_inputs(
            &store,
            &format!("op-opening-payload-{corruption}"),
            &format!("src/opening-payload-{corruption}.ts"),
            vec![semantic_input("config/opening-protected.json")?],
        )?;
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
            let opening = gate
                .revisions
                .first_mut()
                .ok_or("opening-payload gate omitted its opening revision")?;
            match corruption {
                "protected-inputs" => opening.protected_semantic_inputs.clear(),
                "alias-closures" => {
                    opening
                        .alias_closures
                        .push(lumin_evidence::PhysicalAliasClosureRecord {
                            physical_identity: lumin_model::PhysicalFileIdentity::Unix {
                                device: 701,
                                inode: 709,
                            },
                            members: vec![path("src/opening-payload-alias.ts")?],
                        })
                }
                _ => unreachable!(),
            }
            let changed = serde_json::to_vec(&gate)?;
            table.insert(gate_id.as_str(), changed.as_slice())?;
        }
        write.commit()?;
        drop(database);

        let store = open_store(root.path())?;
        assert!(matches!(
            store.migrate_lifecycle_store(),
            Err(StoreError::Integrity(message))
                if message.contains("opening revision payload disagrees with its sealed baseline")
        ));
    }
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
fn migration_reconstructs_the_sealed_lease_domain_from_declared_paths()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let operation_id = OperationId::from_string("op-cleared-sealed-domain".to_owned());
    let gate_id = open_active_gate_for(
        &store,
        operation_id.as_str(),
        "src/cleared-sealed-domain.ts",
    )?;
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    let binding = {
        let mut table = write.open_table(GATES)?;
        let bytes = table
            .get(gate_id.as_str())?
            .ok_or("cleared-domain gate is missing")?
            .value()
            .to_vec();
        let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
        gate.leased_write_set.clear();
        gate.alias_closures.clear();
        let baseline = gate
            .baseline
            .as_mut()
            .ok_or("cleared-domain gate omitted its baseline")?;
        baseline.leased_write_set.clear();
        baseline.alias_closures.clear();
        let binding = reconstructed_baseline_binding(&gate)?;
        let ObservationBinding::Sealed {
            observation: SealedGateObservation::Baseline { observation_id },
        } = &binding
        else {
            return Err("cleared-domain fixture produced the wrong binding".into());
        };
        gate.baseline
            .as_mut()
            .ok_or("cleared-domain baseline disappeared")?
            .observation_id = observation_id.clone();
        gate.revisions[0].observation_binding = Some(binding.clone());
        let changed = serde_json::to_vec(&gate)?;
        table.insert(gate_id.as_str(), changed.as_slice())?;
        binding
    };
    {
        let mut table = write.open_table(OPERATIONS)?;
        let bytes = table
            .get(operation_id.as_str())?
            .ok_or("cleared-domain operation is missing")?
            .value()
            .to_vec();
        let mut operation = serde_json::from_slice::<OperationRecord>(&bytes)?;
        operation.leased_write_set.clear();
        let result = operation
            .result
            .as_mut()
            .ok_or("cleared-domain result is missing")?;
        result.leased_write_set.clear();
        result.observation_binding = Some(binding);
        let changed = serde_json::to_vec(&operation)?;
        table.insert(operation_id.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("does not have exactly one sealed direct lease")
    ));
    Ok(())
}

#[test]
fn migration_derives_baseline_protected_reads_from_the_sealed_snapshot()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let operation_id = OperationId::from_string("op-weakened-baseline-reads".to_owned());
    let gate_id = open_active_gate_for_with_protected_inputs(
        &store,
        operation_id.as_str(),
        "src/weakened-baseline-reads.ts",
        vec![semantic_input("tsconfig.json")?],
    )?;
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    let binding = {
        let mut table = write.open_table(GATES)?;
        let bytes = table
            .get(gate_id.as_str())?
            .ok_or("weakened-read gate is missing")?
            .value()
            .to_vec();
        let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
        gate.protected_semantic_inputs.clear();
        gate.baseline
            .as_mut()
            .ok_or("weakened-read gate omitted its baseline")?
            .protected_semantic_inputs
            .clear();
        gate.revisions[0].protected_semantic_inputs.clear();
        let binding = reconstructed_baseline_binding(&gate)?;
        let ObservationBinding::Sealed {
            observation: SealedGateObservation::Baseline { observation_id },
        } = &binding
        else {
            return Err("weakened-read fixture produced the wrong binding".into());
        };
        gate.baseline
            .as_mut()
            .ok_or("weakened-read baseline disappeared")?
            .observation_id = observation_id.clone();
        gate.revisions[0].observation_binding = Some(binding.clone());
        let changed = serde_json::to_vec(&gate)?;
        table.insert(gate_id.as_str(), changed.as_slice())?;
        binding
    };
    {
        let mut table = write.open_table(OPERATIONS)?;
        let bytes = table
            .get(operation_id.as_str())?
            .ok_or("weakened-read operation is missing")?
            .value()
            .to_vec();
        let mut operation = serde_json::from_slice::<OperationRecord>(&bytes)?;
        operation
            .result
            .as_mut()
            .ok_or("weakened-read result is missing")?
            .observation_binding = Some(binding);
        let changed = serde_json::to_vec(&operation)?;
        table.insert(operation_id.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("baseline protected reads cannot be derived from its sealed snapshot")
    ));
    Ok(())
}

#[test]
fn migration_reconstructs_new_file_parent_and_prefix_bindings()
-> Result<(), Box<dyn std::error::Error>> {
    for corruption in ["missing-prefix-chain", "changed-prefix-identity"] {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        let operation_id = OperationId::from_string(format!("op-new-file-prefix-{corruption}"));
        let gate_id = open_active_gate_for(
            &store,
            operation_id.as_str(),
            &format!("generated/{corruption}/main.ts"),
        )?;
        drop(store);

        let root_path = RepoPath::empty();
        let root_projection = RepoPathProjection::from(&root_path);
        let wrong_root_identity = different_physical_identity(
            lumin_inventory::directory_physical_identity(root.path(), &root_path)?,
        );

        let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
        let write = database.begin_write()?;
        let (binding, lease) = {
            let mut table = write.open_table(GATES)?;
            let bytes = table
                .get(gate_id.as_str())?
                .ok_or("new-file-prefix gate is missing")?
                .value()
                .to_vec();
            let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
            let declared = gate
                .declared_write_set
                .first()
                .ok_or("new-file-prefix gate omitted its declared path")?
                .clone();
            let (nearest_existing_parent, prefix_identities) = match corruption {
                "missing-prefix-chain" => (None, Vec::new()),
                "changed-prefix-identity" => (
                    Some(root_projection.clone()),
                    vec![PathPrefixIdentity {
                        path: root_projection.clone(),
                        physical_identity: wrong_root_identity.clone(),
                    }],
                ),
                _ => unreachable!(),
            };
            let lease = WriteLease {
                path: declared,
                kind: WriteLeaseKind::NewFile,
                physical_identity: None,
                nearest_existing_parent,
                prefix_identities,
            };
            gate.leased_write_set = vec![lease.clone()];
            gate.baseline
                .as_mut()
                .ok_or("new-file-prefix gate omitted its baseline")?
                .leased_write_set = vec![lease];
            let binding = reconstructed_baseline_binding(&gate)?;
            let ObservationBinding::Sealed {
                observation: SealedGateObservation::Baseline { observation_id },
            } = &binding
            else {
                return Err("new-file-prefix fixture produced the wrong binding".into());
            };
            gate.baseline
                .as_mut()
                .ok_or("new-file-prefix baseline disappeared")?
                .observation_id = observation_id.clone();
            gate.revisions[0].observation_binding = Some(binding.clone());
            let lease = gate.leased_write_set[0].clone();
            let changed = serde_json::to_vec(&gate)?;
            table.insert(gate_id.as_str(), changed.as_slice())?;
            (binding, lease)
        };
        {
            let mut table = write.open_table(OPERATIONS)?;
            let bytes = table
                .get(operation_id.as_str())?
                .ok_or("new-file-prefix operation is missing")?
                .value()
                .to_vec();
            let mut operation = serde_json::from_slice::<OperationRecord>(&bytes)?;
            operation.leased_write_set = vec![lease.clone()];
            let result = operation
                .result
                .as_mut()
                .ok_or("new-file-prefix result is missing")?;
            result.leased_write_set = vec![lease];
            result.observation_binding = Some(binding);
            let changed = serde_json::to_vec(&operation)?;
            table.insert(operation_id.as_str(), changed.as_slice())?;
        }
        write.commit()?;
        drop(database);

        let expected = match corruption {
            "missing-prefix-chain" => "omitted its nearest existing parent",
            "changed-prefix-identity" => "prefix identity changed",
            _ => unreachable!(),
        };
        let store = open_store(root.path())?;
        assert!(matches!(
            store.migrate_lifecycle_store(),
            Err(StoreError::Integrity(message)) if message.contains(expected)
        ));
    }
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
        vec![semantic_input("config/opening.json")?],
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
fn migration_rejects_an_authorizing_administrative_abandon()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate_for(&store, "op-abandon-target", "src/abandon-target.ts")?;
    let operation_id = OperationId::from_string("op-abandon-forged-allow".to_owned());
    store.begin_operation(&operation_id)?.abandon_gate(
        "abandon-forged-allow-digest",
        &gate_id,
        0,
        "administrative fixture",
    )?;
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(GATES)?;
        let bytes = table
            .get(gate_id.as_str())?
            .ok_or("abandoned gate is missing")?
            .value()
            .to_vec();
        let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
        gate.revisions
            .last_mut()
            .ok_or("abandoned gate omitted its terminal revision")?
            .decision = GateDecision::Allow;
        let changed = serde_json::to_vec(&gate)?;
        table.insert(gate_id.as_str(), changed.as_slice())?;
    }
    {
        let mut table = write.open_table(OPERATIONS)?;
        let bytes = table
            .get(operation_id.as_str())?
            .ok_or("abandon operation is missing")?
            .value()
            .to_vec();
        let mut operation = serde_json::from_slice::<OperationRecord>(&bytes)?;
        operation
            .result
            .as_mut()
            .ok_or("abandon operation omitted its result")?
            .decision = GateDecision::Allow;
        let changed = serde_json::to_vec(&operation)?;
        table.insert(operation_id.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("administrative abandon operation")
                && message.contains("must deny")
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
fn migration_rejects_an_active_gate_with_an_incomplete_transition_reference_set()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let active_gate =
        open_active_gate_for(&store, "op-active-transition-ref", "src/active-ref.ts")?;
    let closing_gate =
        open_active_gate_for(&store, "op-closing-transition-ref", "src/closing-ref.ts")?;
    close_active_gate_for_migration(&store, &closing_gate)?;
    assert_eq!(store.load_gate(&active_gate)?.transition_refs, [1]);
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(GATES)?;
        let bytes = table
            .get(active_gate.as_str())?
            .ok_or("active transition-reference gate is missing")?
            .value()
            .to_vec();
        let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
        gate.transition_refs.clear();
        let changed = serde_json::to_vec(&gate)?;
        table.insert(active_gate.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("transition reference set disagrees with the complete catalog")
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
fn migration_authenticates_every_complete_gate_evidence_payload()
-> Result<(), Box<dyn std::error::Error>> {
    for role in ["baseline", "close"] {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        let gate_id = open_active_gate_for(
            &store,
            &format!("op-evidence-payload-{role}"),
            &format!("src/evidence-payload-{role}.ts"),
        )?;
        if role == "close" {
            close_active_gate_for_migration(&store, &gate_id)?;
        }
        drop(store);

        let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
        let write = database.begin_write()?;
        {
            let mut table = write.open_table(GATES)?;
            let bytes = table
                .get(gate_id.as_str())?
                .ok_or("evidence-payload gate is missing")?
                .value()
                .to_vec();
            let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
            let evidence = if role == "baseline" {
                &mut gate
                    .baseline
                    .as_mut()
                    .ok_or("evidence-payload gate omitted its baseline")?
                    .snapshot
                    .evidence
            } else {
                &mut gate
                    .revisions
                    .get_mut(1)
                    .and_then(|revision| revision.snapshot.as_mut())
                    .ok_or("evidence-payload gate omitted its close snapshot")?
                    .evidence
            };
            evidence.metrics.logical_source_count =
                evidence.metrics.logical_source_count.saturating_add(1);
            let changed = serde_json::to_vec(&gate)?;
            table.insert(gate_id.as_str(), changed.as_slice())?;
        }
        write.commit()?;
        drop(database);

        let store = open_store(root.path())?;
        assert!(matches!(
            store.migrate_lifecycle_store(),
            Err(StoreError::Integrity(message))
                if message.contains("observation")
                    && message.contains("cannot be reconstructed")
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
fn migration_rejects_a_gate_sequence_below_retained_allocations()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    open_active_gate_for(
        &store,
        "op-gate-sequence-regression",
        "src/gate-sequence.ts",
    )?;
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(SEQUENCES)?;
        table.insert("gate", 0)?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("gate sequence regressed below retained gate allocation")
    ));
    Ok(())
}

#[test]
fn migration_rejects_unfinished_pre_write_gate_id_collisions()
-> Result<(), Box<dyn std::error::Error>> {
    for collision in ["retained-gate", "unfinished-opening"] {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        let retained_gate_id = if collision == "retained-gate" {
            Some(open_active_gate_for(
                &store,
                "op-retained-gate-id",
                "src/retained-gate-id.ts",
            )?)
        } else {
            None
        };
        let mut unfinished_gate_ids = Vec::new();
        let count = if collision == "retained-gate" { 1 } else { 2 };
        for ordinal in 0..count {
            let operation_id =
                OperationId::from_string(format!("op-unfinished-gate-id-{collision}-{ordinal}"));
            let session = store.begin_operation(&operation_id)?;
            let source = path(&format!("src/unfinished-{collision}-{ordinal}.ts"))?;
            let source_lease = WriteLease {
                path: source.clone(),
                kind: WriteLeaseKind::ExistingFile,
                physical_identity: None,
                nearest_existing_parent: None,
                prefix_identities: Vec::new(),
            };
            let PreWriteStart::Analyze { gate_id, .. } = session.reserve_pre_write(
                &format!("unfinished-gate-id-{collision}-{ordinal}"),
                std::slice::from_ref(&source),
                std::slice::from_ref(&source_lease),
                &options(),
                rejected_test_observation,
            )?
            else {
                return Err("unfinished pre-write fixture committed early".into());
            };
            unfinished_gate_ids.push((operation_id, gate_id));
        }
        drop(store);

        let reused_gate_id = retained_gate_id.unwrap_or_else(|| unfinished_gate_ids[0].1.clone());
        let operation_id = &unfinished_gate_ids
            .last()
            .ok_or("unfinished pre-write fixture omitted its operation")?
            .0;
        let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
        let write = database.begin_write()?;
        {
            let mut table = write.open_table(OPERATIONS)?;
            let bytes = table
                .get(operation_id.as_str())?
                .ok_or("unfinished pre-write operation is missing")?
                .value()
                .to_vec();
            let mut operation = serde_json::from_slice::<OperationRecord>(&bytes)?;
            operation.gate_id = reused_gate_id;
            let changed = serde_json::to_vec(&operation)?;
            table.insert(operation_id.as_str(), changed.as_slice())?;
        }
        write.commit()?;
        drop(database);

        let store = open_store(root.path())?;
        assert!(matches!(
            store.migrate_lifecycle_store(),
            Err(StoreError::Integrity(message))
                if message.contains("reuses an allocated gate ID")
        ));
    }
    Ok(())
}

#[test]
fn migration_rejects_a_transition_sequence_below_retained_history()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate_for(
        &store,
        "op-transition-sequence-regression",
        "src/transition-sequence-regression.ts",
    )?;
    close_active_gate_for_migration(&store, &gate_id)?;
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(SEQUENCES)?;
        table.insert("transition", 0)?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("transition sequence regressed below durable transition history")
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

#[test]
fn migration_requires_a_terminal_transition_for_every_closed_gate()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate_for(&store, "op-terminal-transition", "src/terminal.ts")?;
    close_active_gate_for_migration(&store, &gate_id)?;
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(TRANSITIONS)?;
        let removed = table.remove(transition_key(1).as_str())?;
        assert!(removed.is_some());
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("requires exactly one terminal worktree transition")
    ));
    Ok(())
}

#[test]
fn migration_rejects_evidence_payloads_on_an_administrative_abandon()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate_for(&store, "op-abandon-payload-open", "src/abandon.ts")?;
    let abandon_id = OperationId::from_string("op-abandon-payload-close".to_owned());
    store.begin_operation(&abandon_id)?.abandon_gate(
        "abandon-payload-digest",
        &gate_id,
        0,
        "administrative fixture",
    )?;
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(GATES)?;
        let bytes = table
            .get(gate_id.as_str())?
            .ok_or("abandon-payload gate is missing")?
            .value()
            .to_vec();
        let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
        let injected = gate
            .baseline
            .as_ref()
            .ok_or("abandon-payload gate omitted its baseline")?
            .snapshot
            .clone();
        gate.revisions
            .last_mut()
            .ok_or("abandon-payload gate omitted its tail")?
            .snapshot = Some(injected);
        let changed = serde_json::to_vec(&gate)?;
        table.insert(gate_id.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("administrative abandon revision")
                && message.contains("retained evidence payloads")
    ));
    Ok(())
}

#[test]
fn migration_requires_the_complete_transition_chain_on_every_sealed_close()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let active_gate = open_active_gate_for(
        &store,
        "op-incomplete-close-chain-open",
        "src/incomplete-chain.ts",
    )?;
    append_non_authorizing_close_for_migration(&store, &active_gate, Vec::new())?;
    let closing_gate = open_active_gate_for(
        &store,
        "op-incomplete-close-chain-peer",
        "src/incomplete-chain-peer.ts",
    )?;
    close_active_gate_for_migration(&store, &closing_gate)?;
    drop(store);

    let operation_id = OperationId::from_string("op-migrate-incomplete-close".to_owned());
    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(OPERATIONS)?;
        let bytes = table
            .get(operation_id.as_str())?
            .ok_or("non-authorizing close operation is missing")?
            .value()
            .to_vec();
        let mut operation = serde_json::from_slice::<OperationRecord>(&bytes)?;
        operation.transition_sequence = 1;
        let changed = serde_json::to_vec(&operation)?;
        table.insert(operation_id.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("omitted or reordered its transition chain")
    ));
    Ok(())
}

#[test]
fn migration_binds_a_sealed_close_to_the_opening_invocation()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate_for(&store, "op-close-invocation", "src/invocation.ts")?;
    append_non_authorizing_close_for_migration(&store, &gate_id, Vec::new())?;
    drop(store);

    let operation_id = OperationId::from_string("op-migrate-incomplete-close".to_owned());
    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    let binding = {
        let mut table = write.open_table(GATES)?;
        let bytes = table
            .get(gate_id.as_str())?
            .ok_or("close-invocation gate is missing")?
            .value()
            .to_vec();
        let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
        let revision = gate
            .revisions
            .get_mut(1)
            .ok_or("close-invocation revision is missing")?;
        let snapshot = revision
            .snapshot
            .as_ref()
            .ok_or("close-invocation snapshot is missing")?;
        let mut invocation = snapshot.scan_invocation.clone();
        invocation.resolution_profile = Some(ResolutionProfile::Node16);
        revision.snapshot = Some(seal_analysis_snapshot(
            snapshot.inputs.clone(),
            snapshot.evidence.clone(),
            invocation,
            snapshot.entry_selections.clone(),
        ));
        let binding = reconstructed_close_binding(&gate, 1)?;
        gate.revisions[1].observation_binding = Some(binding.clone());
        let changed = serde_json::to_vec(&gate)?;
        table.insert(gate_id.as_str(), changed.as_slice())?;
        binding
    };
    {
        let mut table = write.open_table(OPERATIONS)?;
        let bytes = table
            .get(operation_id.as_str())?
            .ok_or("close-invocation operation is missing")?
            .value()
            .to_vec();
        let mut operation = serde_json::from_slice::<OperationRecord>(&bytes)?;
        operation
            .result
            .as_mut()
            .ok_or("close-invocation result is missing")?
            .observation_binding = Some(binding);
        let changed = serde_json::to_vec(&operation)?;
        table.insert(operation_id.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("changed its opening analysis invocation")
    ));
    Ok(())
}

#[test]
fn migration_matches_changed_paths_to_the_sealed_actual_write_set()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate_for(&store, "op-close-paths", "src/paths.ts")?;
    append_non_authorizing_close_for_migration(&store, &gate_id, Vec::new())?;
    drop(store);

    let operation_id = OperationId::from_string("op-migrate-incomplete-close".to_owned());
    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    let binding = {
        let mut table = write.open_table(GATES)?;
        let bytes = table
            .get(gate_id.as_str())?
            .ok_or("close-path gate is missing")?
            .value()
            .to_vec();
        let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
        gate.revisions
            .get_mut(1)
            .ok_or("close-path revision is missing")?
            .changed_paths
            .push(path("src/forged-write.ts")?);
        let binding = reconstructed_close_binding(&gate, 1)?;
        gate.revisions[1].observation_binding = Some(binding.clone());
        let changed = serde_json::to_vec(&gate)?;
        table.insert(gate_id.as_str(), changed.as_slice())?;
        binding
    };
    {
        let mut table = write.open_table(OPERATIONS)?;
        let bytes = table
            .get(operation_id.as_str())?
            .ok_or("close-path operation is missing")?
            .value()
            .to_vec();
        let mut operation = serde_json::from_slice::<OperationRecord>(&bytes)?;
        operation
            .result
            .as_mut()
            .ok_or("close-path result is missing")?
            .observation_binding = Some(binding);
        let changed = serde_json::to_vec(&operation)?;
        table.insert(operation_id.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("changed paths disagree with its actual-write set")
    ));
    Ok(())
}

#[test]
fn migration_derives_the_actual_write_set_from_sealed_snapshots()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate_for(&store, "op-derived-paths", "src/derived-paths.ts")?;
    close_active_gate_for_migration(&store, &gate_id)?;
    drop(store);

    let injected = semantic_input("config/omitted-change.json")?;
    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    let (operation_id, binding, forged_snapshot) = {
        let mut table = write.open_table(GATES)?;
        let bytes = table
            .get(gate_id.as_str())?
            .ok_or("derived-path gate is missing")?
            .value()
            .to_vec();
        let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
        let revision = gate
            .revisions
            .get_mut(1)
            .ok_or("derived-path gate omitted its close revision")?;
        let snapshot = revision
            .snapshot
            .take()
            .ok_or("derived-path gate omitted its close snapshot")?;
        let mut inputs = snapshot.inputs;
        inputs.push(injected);
        let forged_snapshot = seal_analysis_snapshot(
            inputs,
            snapshot.evidence,
            snapshot.scan_invocation,
            snapshot.entry_selections,
        );
        revision.protected_semantic_inputs = derive_protected_semantic_inputs(
            &forged_snapshot,
            &gate
                .baseline
                .as_ref()
                .ok_or("derived-path gate omitted its baseline")?
                .leased_write_set,
        );
        revision.snapshot = Some(forged_snapshot.clone());
        let operation_id = revision.operation_id.clone();
        let binding = reconstructed_close_binding(&gate, 1)?;
        gate.revisions[1].observation_binding = Some(binding.clone());
        let changed = serde_json::to_vec(&gate)?;
        table.insert(gate_id.as_str(), changed.as_slice())?;
        (operation_id, binding, forged_snapshot)
    };
    {
        let mut table = write.open_table(OPERATIONS)?;
        let bytes = table
            .get(operation_id.as_str())?
            .ok_or("derived-path close operation is missing")?
            .value()
            .to_vec();
        let mut operation = serde_json::from_slice::<OperationRecord>(&bytes)?;
        operation
            .result
            .as_mut()
            .ok_or("derived-path close result is missing")?
            .observation_binding = Some(binding.clone());
        let changed = serde_json::to_vec(&operation)?;
        table.insert(operation_id.as_str(), changed.as_slice())?;
    }
    {
        let mut table = write.open_table(TRANSITIONS)?;
        let key = transition_key(1);
        let bytes = table
            .get(key.as_str())?
            .ok_or("derived-path transition is missing")?
            .value()
            .to_vec();
        let mut transition = serde_json::from_slice::<WorktreeTransition>(&bytes)?;
        transition.capsule.after_snapshot = forged_snapshot;
        let ObservationBinding::Sealed {
            observation: SealedGateObservation::Close { observation_id },
        } = binding
        else {
            return Err("derived-path fixture produced an unsealed close".into());
        };
        transition.capsule.close_observation_id = observation_id;
        let changed = serde_json::to_vec(&transition)?;
        table.insert(key.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("actual-write set cannot be derived from its sealed snapshots")
    ));
    Ok(())
}

#[test]
fn migration_recomputes_opening_signals_from_the_sealed_snapshot()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let operation_id = OperationId::from_string("op-opening-policy".to_owned());
    let gate_id = open_active_gate_for(&store, operation_id.as_str(), "src/opening-policy.ts")?;
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    let binding = {
        let mut table = write.open_table(GATES)?;
        let bytes = table
            .get(gate_id.as_str())?
            .ok_or("opening-policy gate is missing")?
            .value()
            .to_vec();
        let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
        let baseline = gate
            .baseline
            .as_mut()
            .ok_or("opening-policy gate omitted its baseline")?;
        let mut evidence = baseline.snapshot.evidence.clone();
        evidence
            .capabilities
            .first_mut()
            .ok_or("opening-policy evidence omitted its capability")?
            .state = CapabilityState::Failed;
        baseline.snapshot = seal_analysis_snapshot(
            baseline.snapshot.inputs.clone(),
            evidence,
            baseline.snapshot.scan_invocation.clone(),
            baseline.snapshot.entry_selections.clone(),
        );
        let binding = reconstructed_baseline_binding(&gate)?;
        let ObservationBinding::Sealed {
            observation: SealedGateObservation::Baseline { observation_id },
        } = &binding
        else {
            return Err("opening-policy fixture produced the wrong binding".into());
        };
        gate.baseline
            .as_mut()
            .ok_or("opening-policy baseline disappeared")?
            .observation_id = observation_id.clone();
        gate.revisions[0].observation_binding = Some(binding.clone());
        let changed = serde_json::to_vec(&gate)?;
        table.insert(gate_id.as_str(), changed.as_slice())?;
        binding
    };
    {
        let mut table = write.open_table(OPERATIONS)?;
        let bytes = table
            .get(operation_id.as_str())?
            .ok_or("opening-policy operation is missing")?
            .value()
            .to_vec();
        let mut operation = serde_json::from_slice::<OperationRecord>(&bytes)?;
        operation
            .result
            .as_mut()
            .ok_or("opening-policy result is missing")?
            .observation_binding = Some(binding);
        let changed = serde_json::to_vec(&operation)?;
        table.insert(operation_id.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("opening signals disagree with its sealed analysis snapshot")
    ));
    Ok(())
}

#[test]
fn migration_rejects_signals_that_a_sealed_close_cannot_emit()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate_for(&store, "op-close-signal-kind", "src/signal-kind.ts")?;
    append_non_authorizing_close_for_migration(&store, &gate_id, Vec::new())?;
    drop(store);

    let operation_id = OperationId::from_string("op-migrate-incomplete-close".to_owned());
    let injected = GateSignal::FindingWarnings { count: 1 };
    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(GATES)?;
        let bytes = table
            .get(gate_id.as_str())?
            .ok_or("close-signal-kind gate is missing")?
            .value()
            .to_vec();
        let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
        gate.revisions
            .get_mut(1)
            .ok_or("close-signal-kind revision is missing")?
            .signals
            .push(injected.clone());
        let changed = serde_json::to_vec(&gate)?;
        table.insert(gate_id.as_str(), changed.as_slice())?;
    }
    {
        let mut table = write.open_table(OPERATIONS)?;
        let bytes = table
            .get(operation_id.as_str())?
            .ok_or("close-signal-kind operation is missing")?
            .value()
            .to_vec();
        let mut operation = serde_json::from_slice::<OperationRecord>(&bytes)?;
        operation
            .result
            .as_mut()
            .ok_or("close-signal-kind result is missing")?
            .signals
            .push(injected);
        let changed = serde_json::to_vec(&operation)?;
        table.insert(operation_id.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("close revision 1 signals disagree")
    ));
    Ok(())
}

#[test]
fn migration_rejects_an_authorizing_close_before_the_durable_tail()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate_for(&store, "op-early-authorize-open", "src/early.ts")?;
    append_non_authorizing_close_for_migration(&store, &gate_id, Vec::new())?;
    let abandon_id = OperationId::from_string("op-early-authorize-abandon".to_owned());
    store.begin_operation(&abandon_id)?.abandon_gate(
        "early-authorize-abandon-digest",
        &gate_id,
        1,
        "administrative fixture",
    )?;
    drop(store);

    let close_id = OperationId::from_string("op-migrate-incomplete-close".to_owned());
    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(GATES)?;
        let bytes = table
            .get(gate_id.as_str())?
            .ok_or("early-authorize gate is missing")?
            .value()
            .to_vec();
        let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
        let revision = gate
            .revisions
            .get_mut(1)
            .ok_or("early-authorize close revision is missing")?;
        revision.decision = GateDecision::Allow;
        revision.signals.clear();
        let changed = serde_json::to_vec(&gate)?;
        table.insert(gate_id.as_str(), changed.as_slice())?;
    }
    {
        let mut table = write.open_table(OPERATIONS)?;
        let bytes = table
            .get(close_id.as_str())?
            .ok_or("early-authorize close operation is missing")?
            .value()
            .to_vec();
        let mut operation = serde_json::from_slice::<OperationRecord>(&bytes)?;
        let result = operation
            .result
            .as_mut()
            .ok_or("early-authorize close result is missing")?;
        result.decision = GateDecision::Allow;
        result.signals.clear();
        result.lifecycle = GateLifecycle::Closed;
        let changed = serde_json::to_vec(&operation)?;
        table.insert(close_id.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("authorizing close revision 1 is not the durable tail")
    ));
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
