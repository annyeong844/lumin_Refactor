use super::*;

fn rewrite_operation(
    root: &std::path::Path,
    operation_id: &OperationId,
    mutate: impl FnOnce(&mut OperationRecord) -> Result<(), Box<dyn std::error::Error>>,
) -> Result<OperationRecord, Box<dyn std::error::Error>> {
    let database = Database::open(root.join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    let operation = {
        let mut table = write.open_table(OPERATIONS)?;
        let bytes = table
            .get(operation_id.as_str())?
            .ok_or("operation is missing")?
            .value()
            .to_vec();
        let mut operation = serde_json::from_slice::<OperationRecord>(&bytes)?;
        mutate(&mut operation)?;
        let changed = serde_json::to_vec(&operation)?;
        table.insert(operation_id.as_str(), changed.as_slice())?;
        operation
    };
    write.commit()?;
    Ok(operation)
}

fn replace_validation_receipt(
    root: &std::path::Path,
    operation: &OperationRecord,
) -> Result<(), Box<dyn std::error::Error>> {
    let receipt = crate::gate::validation_receipt_for_operation(operation, None)?
        .ok_or("operation omitted its expected validation receipt")?;
    let database = Database::open(root.join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(VALIDATION_RECEIPTS)?;
        let bytes = serde_json::to_vec(&receipt)?;
        table.insert(operation.operation_id.as_str(), bytes.as_slice())?;
    }
    reseal_validation_receipt_set(&write)?;
    write.commit()?;
    Ok(())
}

fn pending_pre_write(
    root: &std::path::Path,
    operation_id: &OperationId,
    source_path: &RepoPath,
) -> Result<(String, lumin_model::GateId), Box<dyn std::error::Error>> {
    fs::create_dir_all(root.join("src"))?;
    let native = root.join(source_path.display_escaped());
    if let Some(parent) = native.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&native, b"export const pending = true;\n")?;
    let source = RepoPathProjection::from(source_path);
    let lease = observed_lease(root, source_path)?;
    let store = open_store(root)?;
    let session = store.begin_operation(operation_id)?;
    let analysis_options = options();
    let request_digest = pre_write_digest(std::slice::from_ref(&source), &analysis_options);
    let gate_id = match session.reserve_pre_write(
        &request_digest,
        std::slice::from_ref(&source),
        std::slice::from_ref(&lease),
        &analysis_options,
        rejected_test_observation,
    )? {
        PreWriteStart::Analyze { gate_id, .. } => gate_id,
        PreWriteStart::Committed(_) => return Err("pending pre-write committed early".into()),
    };
    Ok((request_digest, gate_id))
}

#[test]
fn migration_rejects_pending_inspection_lease_erasure_against_its_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let operation_id = OperationId::from_string("op-receipt-inspection-erasure".to_owned());
    let source = RepoPath::from_portable("src/inspection-erasure.ts")?;
    pending_pre_write(root.path(), &operation_id, &source)?;

    rewrite_operation(root.path(), &operation_id, |operation| {
        operation.leased_write_set.clear();
        let inspection = operation
            .pre_write_declared_path_inspection
            .first_mut()
            .ok_or("pending operation omitted its inspection")?;
        inspection.lease = None;
        inspection.rejection = Some(GateSignal::AnalysisFailed {
            detail: "forged rejection".to_owned(),
        });
        Ok(())
    })?;

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("store-owned validation receipt")
    ));
    Ok(())
}

#[test]
fn migration_requires_zero_target_revision_for_unfinished_pre_write()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let operation_id = OperationId::from_string("op-receipt-prewrite-target".to_owned());
    let source = RepoPath::from_portable("src/nonzero-target.ts")?;
    pending_pre_write(root.path(), &operation_id, &source)?;
    let operation = rewrite_operation(root.path(), &operation_id, |operation| {
        operation.target_revision = 1;
        Ok(())
    })?;
    replace_validation_receipt(root.path(), &operation)?;

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message)) if message.contains("nonzero target revision")
    ));
    Ok(())
}

#[test]
fn migration_reopens_pending_semantic_read_reservations() -> Result<(), Box<dyn std::error::Error>>
{
    for state in ["present", "directory", "absent", "not-a-directory"] {
        let root = tempfile::tempdir()?;
        let operation_id = OperationId::from_string(format!("op-semantic-reopen-{state}"));
        let source = RepoPath::from_portable("src/semantic-reservation.ts")?;
        let (request_digest, gate_id) = pending_pre_write(root.path(), &operation_id, &source)?;
        let semantic_path = match state {
            "present" => {
                fs::write(root.path().join("tsconfig.json"), b"{}\n")?;
                RepoPath::from_portable("tsconfig.json")?
            }
            "directory" => {
                fs::create_dir(root.path().join("config-directory"))?;
                RepoPath::from_portable("config-directory")?
            }
            "absent" => RepoPath::from_portable("missing/tsconfig.json")?,
            "not-a-directory" => {
                fs::write(root.path().join("blocked-parent"), b"not a directory\n")?;
                RepoPath::from_portable("blocked-parent/tsconfig.json")?
            }
            _ => unreachable!(),
        };
        let observed = lumin_inventory::observe_config_input_identity(root.path(), &semantic_path)?;
        let binding = lumin_evidence::SemanticReadReservationBinding {
            path: RepoPathProjection::from(&semantic_path),
            physical_identity: observed.physical_identity,
            absence_parent: observed.absence_parent.map(|parent| PathPrefixIdentity {
                path: RepoPathProjection::from(&parent.path),
                physical_identity: parent.physical_identity,
            }),
        };
        let store = open_store(root.path())?;
        let session = store.begin_operation(&operation_id)?;
        let source_projection = RepoPathProjection::from(&source);
        let source_lease = observed_lease(root.path(), &source)?;
        let analysis_options = options();
        assert!(matches!(
            session.reserve_pre_write(
                &request_digest,
                std::slice::from_ref(&source_projection),
                std::slice::from_ref(&source_lease),
                &analysis_options,
                rejected_test_observation,
            )?,
            PreWriteStart::Analyze { .. }
        ));
        assert_eq!(
            session.reserve_pre_write_semantic_inputs(
                &request_digest,
                &gate_id,
                std::slice::from_ref(&binding),
            )?,
            crate::SemanticReadReservation::Reserved
        );
        drop(session);
        drop(store);

        rewrite_operation(root.path(), &operation_id, |operation| {
            let binding = operation
                .semantic_read_reservation_bindings
                .first_mut()
                .ok_or("pending operation omitted its semantic reservation")?;
            if let Some(identity) = binding.physical_identity.take() {
                binding.physical_identity = Some(different_physical_identity(identity));
            } else {
                let parent = binding
                    .absence_parent
                    .as_mut()
                    .ok_or("missing reservation omitted its absence parent")?;
                parent.physical_identity =
                    different_physical_identity(parent.physical_identity.clone());
            }
            Ok(())
        })?;

        let store = open_store(root.path())?;
        let outcome = store.migrate_lifecycle_store();
        assert!(
            matches!(
                &outcome,
                Err(StoreError::Integrity(message))
                    if message.contains("semantic-read reservation changed")
                        || message.contains("physical identity changed")
                        || message.contains("absence-parent identity changed")
            ),
            "unexpected migration outcome: {outcome:?}"
        );
    }
    Ok(())
}

#[test]
fn migration_requires_pending_post_write_leases_to_match_the_active_gate()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate_for(
        &store,
        "op-pending-postwrite-open",
        "src/pending-postwrite.ts",
    )?;
    let operation_id = OperationId::from_string("op-pending-postwrite".to_owned());
    let request_digest = post_write_request_digest(&gate_id);
    let session = store.begin_operation(&operation_id)?;
    assert!(matches!(
        session.begin_post_write(&request_digest, &gate_id)?,
        crate::PostWriteStart::Analyze { .. }
    ));
    drop(session);
    drop(store);

    rewrite_operation(root.path(), &operation_id, |operation| {
        operation.leased_write_set.clear();
        Ok(())
    })?;
    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("lease projection disagrees with its active gate")
    ));
    Ok(())
}

#[test]
fn migration_authenticates_every_unfinished_abandon_request()
-> Result<(), Box<dyn std::error::Error>> {
    for status in [
        GateOperationStatus::Pending,
        GateOperationStatus::Interrupted,
    ] {
        for corruption in ["missing-reason", "forged-digest"] {
            let root = tempfile::tempdir()?;
            let store = open_store(root.path())?;
            let opening_id = OperationId::from_string(format!(
                "op-abandon-request-open-{status:?}-{corruption}"
            ));
            let gate_id = open_active_gate_for(
                &store,
                opening_id.as_str(),
                &format!("src/abandon-request-{status:?}-{corruption}.ts"),
            )?;
            let gate = store.load_gate(&gate_id)?;
            let mut operation = store.load_operation(&opening_id)?;
            let operation_id =
                OperationId::from_string(format!("op-abandon-request-{status:?}-{corruption}"));
            operation.operation_id = operation_id.clone();
            operation.kind = lumin_evidence::GateOperationKind::GateAbandon;
            operation.status = status;
            operation.gate_id = gate_id;
            operation.target_revision = gate.current_revision;
            operation.reason = (corruption == "forged-digest").then(|| "reason".to_owned());
            operation.request_digest = "forged".to_owned();
            operation.declared_write_set.clear();
            operation.leased_write_set.clear();
            operation.semantic_read_reservations.clear();
            operation.semantic_read_reservation_bindings.clear();
            operation.operation_liveness = None;
            operation.pre_write_declared_path_inspection.clear();
            operation.pre_write_admission_evidence = None;
            operation.pre_write_final_validation = None;
            operation.post_write_final_validation = None;
            operation.analysis_options = None;
            operation.result = None;
            drop(store);

            let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
            let write = database.begin_write()?;
            {
                let mut table = write.open_table(OPERATIONS)?;
                let bytes = serde_json::to_vec(&operation)?;
                table.insert(operation_id.as_str(), bytes.as_slice())?;
            }
            write.commit()?;
            drop(database);

            let store = open_store(root.path())?;
            let outcome = store.migrate_lifecycle_store();
            let expected = if corruption == "missing-reason" {
                "omitted its reason"
            } else {
                "disagrees with its authenticated request"
            };
            assert!(
                matches!(&outcome, Err(StoreError::Integrity(message)) if message.contains(expected)),
                "unexpected migration outcome: {outcome:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn migration_authenticates_historical_pending_conflict_witnesses()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(
        root.path().join("src/pending-owner.ts"),
        b"export const owner = true;\n",
    )?;
    let source_path = RepoPath::from_portable("src/pending-owner.ts")?;
    let source = RepoPathProjection::from(&source_path);
    let lease = observed_lease(root.path(), &source_path)?;
    let analysis_options = options();
    let request_digest = pre_write_digest(std::slice::from_ref(&source), &analysis_options);
    let store = open_store(root.path())?;
    let owner_id = OperationId::from_string("op-admission-pending-owner".to_owned());
    let owner = store.begin_operation(&owner_id)?;
    assert!(matches!(
        owner.reserve_pre_write(
            &request_digest,
            std::slice::from_ref(&source),
            std::slice::from_ref(&lease),
            &analysis_options,
            rejected_test_observation,
        )?,
        PreWriteStart::Analyze { .. }
    ));
    let rejected_id = OperationId::from_string("op-admission-pending-rejected".to_owned());
    let rejected = store.begin_operation(&rejected_id)?;
    let unsealed_inputs =
        UnsealedGateObservationInputs::new(vec![lease.clone()], Vec::new(), Vec::new());
    let source_for_binding = source.clone();
    assert!(matches!(
        rejected.reserve_pre_write(
            &request_digest,
            std::slice::from_ref(&source),
            std::slice::from_ref(&lease),
            &analysis_options,
            |signals| derive_unsealed_gate_observation_binding(
                std::slice::from_ref(&source_for_binding),
                &unsealed_inputs,
                signals,
            ),
        )?,
        PreWriteStart::Committed(_)
    ));
    drop(rejected);
    drop(owner);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(OPERATIONS)?;
        let owner_bytes = table
            .get(owner_id.as_str())?
            .ok_or("pending conflict owner is missing")?
            .value()
            .to_vec();
        let mut owner = serde_json::from_slice::<OperationRecord>(&owner_bytes)?;
        owner.status = GateOperationStatus::Interrupted;
        owner.interruption_count = 1;
        owner.leased_write_set.clear();
        owner.semantic_read_reservations.clear();
        owner.semantic_read_reservation_bindings.clear();
        owner.operation_liveness = None;
        let owner_changed = serde_json::to_vec(&owner)?;
        table.insert(owner_id.as_str(), owner_changed.as_slice())?;

        let rejected_bytes = table
            .get(rejected_id.as_str())?
            .ok_or("admission rejection is missing")?
            .value()
            .to_vec();
        let mut rejected = serde_json::from_slice::<OperationRecord>(&rejected_bytes)?;
        let evidence = rejected
            .pre_write_admission_evidence
            .as_mut()
            .ok_or("admission rejection omitted its evidence")?;
        let owner_lease = evidence
            .conflict_owners
            .iter_mut()
            .find_map(|owner| match owner {
                lumin_evidence::PreWriteAdmissionConflictOwner::PendingOperation {
                    leased_write_set,
                    ..
                } => leased_write_set.first_mut(),
                _ => None,
            })
            .ok_or("admission rejection omitted its pending owner lease")?;
        let identity = owner_lease
            .physical_identity
            .take()
            .ok_or("admission witness lease omitted its physical identity")?;
        owner_lease.physical_identity = Some(different_physical_identity(identity));
        let derived = lumin_evidence::derive_pre_write_admission_signals(evidence);
        let result = rejected
            .result
            .as_ref()
            .ok_or("admission rejection omitted its result")?;
        assert_eq!(derived, result.signals);
        let rejected_changed = serde_json::to_vec(&rejected)?;
        table.insert(rejected_id.as_str(), rejected_changed.as_slice())?;
    }
    {
        let mut table = write.open_table(VALIDATION_RECEIPTS)?;
        table.remove(owner_id.as_str())?;
    }
    write.commit()?;
    drop(database);

    let outcome = store.migrate_lifecycle_store();
    assert!(
        matches!(
            &outcome,
            Err(StoreError::Integrity(message))
                if message.contains("store-owned validation receipt")
        ),
        "unexpected migration outcome: {outcome:?}"
    );
    Ok(())
}

#[test]
fn migration_requires_the_store_owned_close_validation_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate_for(&store, "op-close-receipt-open", "src/close-receipt.ts")?;
    close_active_gate_for_migration(&store, &gate_id)?;
    let gate = store.load_gate(&gate_id)?;
    let close_id = gate
        .revisions
        .get(1)
        .ok_or("closed gate omitted its close revision")?
        .operation_id
        .clone();

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(VALIDATION_RECEIPTS)?;
        table.remove(close_id.as_str())?;
    }
    write.commit()?;
    drop(database);

    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("store-owned validation receipt")
    ));
    Ok(())
}

#[test]
fn committed_operation_queries_require_the_store_owned_validation_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let operation_id = OperationId::from_string("op-query-receipt-open".to_owned());
    let store = open_store(root.path())?;
    let gate_id = open_active_gate_for(&store, operation_id.as_str(), "src/query-receipt.ts")?;
    let operation = store.load_operation(&operation_id)?;
    let request_digest = operation.request_digest.clone();
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(VALIDATION_RECEIPTS)?;
        table.remove(operation_id.as_str())?;
    }
    reseal_validation_receipt_set(&write)?;
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.load_gate(&gate_id),
        Err(StoreError::Integrity(message))
            if message.contains("store-owned validation receipt")
    ));
    assert!(matches!(
        store.lookup_gate(&gate_id),
        Err(StoreError::Integrity(message))
            if message.contains("store-owned validation receipt")
    ));
    assert!(matches!(
        store.load_operation(&operation_id),
        Err(StoreError::Integrity(message))
            if message.contains("store-owned validation receipt")
    ));
    assert!(matches!(
        store.replay_pre_write_result(&operation_id, &request_digest),
        Err(StoreError::Integrity(message))
            if message.contains("store-owned validation receipt")
    ));
    Ok(())
}

#[test]
fn migration_rejects_exhausted_gate_allocator_sequences() -> Result<(), Box<dyn std::error::Error>>
{
    for (key, expected) in [
        ("gate", "gate sequence is exhausted"),
        ("transition", "transition sequence is exhausted"),
        (
            ACTIVE_GATE_CATALOG_SEQUENCE_KEY,
            "active-gate catalog sequence is exhausted",
        ),
    ] {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        drop(store);

        let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
        let write = database.begin_write()?;
        {
            let mut table = write.open_table(SEQUENCES)?;
            table.insert(key, u64::MAX)?;
        }
        write.commit()?;
        drop(database);

        let store = open_store(root.path())?;
        assert!(matches!(
            store.migrate_lifecycle_store(),
            Err(StoreError::Integrity(message)) if message.contains(expected)
        ));
    }
    Ok(())
}

#[test]
fn migration_rejects_an_active_gate_with_an_exhausted_revision_sequence()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate_for(
        &store,
        "op-exhausted-revision-open",
        "src/exhausted-revision.ts",
    )?;
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(GATES)?;
        let bytes = table
            .get(gate_id.as_str())?
            .ok_or("active gate is missing")?
            .value()
            .to_vec();
        let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
        gate.current_revision = u64::MAX;
        let changed = serde_json::to_vec(&gate)?;
        table.insert(gate_id.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("exhausted its revision sequence")
    ));
    Ok(())
}

#[test]
fn migration_rejects_unfinished_operations_with_exhausted_interruption_counts()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let operation_id = OperationId::from_string("op-exhausted-interruptions".to_owned());
    let source = RepoPath::from_portable("src/exhausted-interruptions.ts")?;
    pending_pre_write(root.path(), &operation_id, &source)?;
    rewrite_operation(root.path(), &operation_id, |operation| {
        operation.interruption_count = u64::MAX;
        Ok(())
    })?;

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("exhausted its interruption count")
    ));
    Ok(())
}

#[test]
fn committed_abandon_requires_a_header_bound_validation_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id =
        open_active_gate_for(&store, "op-abandon-receipt-open", "src/abandon-receipt.ts")?;
    let gate = store.load_gate(&gate_id)?;
    let operation_id = OperationId::from_string("op-abandon-receipt".to_owned());
    let reason = "administrative cleanup";
    let request_digest = gate_abandon_request_digest(&gate_id, gate.current_revision, reason);
    store.begin_operation(&operation_id)?.abandon_gate(
        &request_digest,
        &gate_id,
        gate.current_revision,
        reason,
    )?;
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(VALIDATION_RECEIPTS)?;
        let bytes = table
            .get(operation_id.as_str())?
            .ok_or("committed abandon omitted its validation receipt")?
            .value()
            .to_vec();
        let receipt = serde_json::from_slice::<lumin_evidence::GateValidationReceipt>(&bytes)?;
        assert!(receipt.commit.is_some());
        assert!(matches!(
            receipt.payload,
            lumin_evidence::GateValidationReceiptPayload::GateAbandon { .. }
        ));
        table.remove(operation_id.as_str())?;
    }
    reseal_validation_receipt_set(&write)?;
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("store-owned validation receipt")
    ));
    Ok(())
}

#[test]
fn migration_authenticates_terminal_revision_timestamps_with_the_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate_for(
        &store,
        "op-timestamp-receipt-open",
        "src/timestamp-receipt.ts",
    )?;
    close_active_gate_for_migration(&store, &gate_id)?;
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(GATES)?;
        let bytes = table
            .get(gate_id.as_str())?
            .ok_or("closed gate is missing")?
            .value()
            .to_vec();
        let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
        gate.revisions
            .last_mut()
            .ok_or("closed gate omitted its terminal revision")?
            .committed_unix_millis = Some(0);
        let changed = serde_json::to_vec(&gate)?;
        table.insert(gate_id.as_str(), changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    let store = open_store(root.path())?;
    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("store-owned validation receipt")
    ));
    Ok(())
}

#[test]
fn migration_authenticates_sealed_evidence_payloads_with_the_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let operation_id = OperationId::from_string("op-payload-receipt-open".to_owned());
    let store = open_store(root.path())?;
    let gate_id = open_active_gate_for(&store, operation_id.as_str(), "src/payload-receipt.ts")?;
    drop(store);

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    let binding = {
        let mut table = write.open_table(GATES)?;
        let bytes = table
            .get(gate_id.as_str())?
            .ok_or("active gate is missing")?
            .value()
            .to_vec();
        let mut gate = serde_json::from_slice::<GateRecord>(&bytes)?;
        gate.baseline
            .as_mut()
            .ok_or("active gate omitted its baseline")?
            .snapshot
            .evidence
            .metrics
            .logical_source_count += 1;
        let binding = reconstructed_baseline_binding(&gate)?;
        let ObservationBinding::Sealed {
            observation: SealedGateObservation::Baseline { observation_id },
        } = &binding
        else {
            return Err("payload fixture produced the wrong observation kind".into());
        };
        gate.baseline
            .as_mut()
            .ok_or("active gate omitted its baseline")?
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
            .ok_or("opening operation is missing")?
            .value()
            .to_vec();
        let mut operation = serde_json::from_slice::<OperationRecord>(&bytes)?;
        operation
            .result
            .as_mut()
            .ok_or("opening operation omitted its result")?
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
            if message.contains("store-owned validation receipt")
    ));
    Ok(())
}
