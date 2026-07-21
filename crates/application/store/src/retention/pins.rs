use lumin_evidence::{
    RecordLookup, RetentionItemKind, RetentionOperationKind, RetentionOperationRecord,
    RetentionOperationResult, RetentionOperationStatus, RetentionPlanState, RunPinRecord,
};
use lumin_model::{OperationId, PinId, RunId, append_length_prefixed, digest_hex};
use redb::ReadableTable;

use crate::gate::records::{read_record, write_record};
use crate::{RUN_CATALOG, RepositoryStore, StoreError, backend_error, unix_millis};

use super::RUN_PINS;
use super::planning::reject_gate_operation_collision;
use super::records::{
    OPERATION_SCHEMA, ensure_result_matches, next_sequence, read_retention_operation,
    write_retention_operation,
};

pub(super) fn create(
    store: &RepositoryStore,
    run_id: &RunId,
    operation_id: &OperationId,
    reason: &str,
) -> Result<RunPinRecord, StoreError> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(StoreError::Integrity(
            "run pin reason must not be empty".to_owned(),
        ));
    }
    let request_digest = pin_request_digest(run_id, reason);
    store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        if let Some(operation) = read_retention_operation(&write, operation_id)? {
            ensure_result_matches(&operation, RetentionOperationKind::RunPin, &request_digest)?;
            return pin_created_result(operation);
        }
        reject_gate_operation_collision(&write, operation_id)?;
        ensure_live_run(&write, run_id)?;
        let sequence = next_sequence(&write, "run-pin")?;
        let pin = RunPinRecord {
            schema_version: "lumin-run-pin.v1".to_owned(),
            pin_id: PinId::from_string(format!("pin_{sequence:016x}")),
            run_id: run_id.clone(),
            reason: reason.to_owned(),
            created_unix_millis: unix_millis()?,
            created_operation_id: operation_id.clone(),
            removed_operation_id: None,
        };
        write_record(&write, RUN_PINS, pin.pin_id.as_str(), &pin)?;
        let operation = RetentionOperationRecord {
            schema_version: OPERATION_SCHEMA.to_owned(),
            operation_id: operation_id.clone(),
            kind: RetentionOperationKind::RunPin,
            request_digest,
            status: RetentionOperationStatus::Committed,
            plan_id: None,
            result: RetentionOperationResult::PinCreated { pin: pin.clone() },
        };
        write_retention_operation(&write, &operation)?;
        next_sequence(&write, "retention-catalog")?;
        guard.commit(write)?;
        Ok(pin)
    })
}

pub(super) fn remove(
    store: &RepositoryStore,
    pin_id: &PinId,
    operation_id: &OperationId,
) -> Result<RunPinRecord, StoreError> {
    let request_digest = unpin_request_digest(pin_id);
    store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        if let Some(operation) = read_retention_operation(&write, operation_id)? {
            ensure_result_matches(
                &operation,
                RetentionOperationKind::RunUnpin,
                &request_digest,
            )?;
            let run_id = pin_removed_result(operation, pin_id)?;
            return load_pin(&write, pin_id)?.ok_or_else(|| {
                StoreError::Integrity(format!(
                    "committed unpin references missing pin {} for run {}",
                    pin_id.as_str(),
                    run_id.as_str()
                ))
            });
        }
        reject_gate_operation_collision(&write, operation_id)?;
        let mut pin = load_pin(&write, pin_id)?
            .ok_or_else(|| StoreError::PinNotFound(pin_id.as_str().to_owned()))?;
        if pin.removed_operation_id.is_some() {
            return Err(StoreError::Integrity(format!(
                "run pin is already inactive: {}",
                pin_id.as_str()
            )));
        }
        pin.removed_operation_id = Some(operation_id.clone());
        write_record(&write, RUN_PINS, pin_id.as_str(), &pin)?;
        let operation = RetentionOperationRecord {
            schema_version: OPERATION_SCHEMA.to_owned(),
            operation_id: operation_id.clone(),
            kind: RetentionOperationKind::RunUnpin,
            request_digest,
            status: RetentionOperationStatus::Committed,
            plan_id: None,
            result: RetentionOperationResult::PinRemoved {
                pin_id: pin_id.clone(),
                run_id: pin.run_id.clone(),
            },
        };
        write_retention_operation(&write, &operation)?;
        next_sequence(&write, "retention-catalog")?;
        guard.commit(write)?;
        Ok(pin)
    })
}

pub(super) fn lookup(
    store: &RepositoryStore,
    pin_id: &PinId,
) -> Result<RecordLookup<RunPinRecord>, StoreError> {
    store.with_shared_lock(|guard| {
        let database = guard.open_database()?;
        let read = database.begin_read()?;
        if let Some(tombstone) = super::records::read_validated_tombstone(
            &read,
            RetentionItemKind::PinOrReference,
            pin_id.as_str(),
        )? {
            return if tombstone.envelope.tombstone_identity.is_some() {
                Ok(RecordLookup::Pruned(tombstone.envelope))
            } else {
                Ok(RecordLookup::Pruning(tombstone.envelope))
            };
        }
        drop(read);
        if let Some(pin) = crate::gate::records::load_record(&database, RUN_PINS, pin_id.as_str())?
        {
            return Ok(RecordLookup::Live(pin));
        }
        Err(StoreError::PinNotFound(pin_id.as_str().to_owned()))
    })
}

fn ensure_live_run(write: &redb::WriteTransaction, run_id: &RunId) -> Result<(), StoreError> {
    let table = write.open_table(RUN_CATALOG).map_err(backend_error)?;
    if table.get(run_id.as_str()).map_err(backend_error)?.is_none() {
        return Err(StoreError::RunNotFound(run_id.as_str().to_owned()));
    }
    drop(table);
    if let Some((_tombstone, owner)) = super::records::read_validated_tombstone_with_owner(
        write,
        RetentionItemKind::Run,
        run_id.as_str(),
    )? {
        if owner.record.state != RetentionPlanState::Pruning {
            return Err(StoreError::Integrity(format!(
                "live run {} has a non-active retention owner",
                run_id.as_str()
            )));
        }
        return Err(StoreError::RunRetentionState(run_id.as_str().to_owned()));
    }
    Ok(())
}

fn load_pin(
    write: &redb::WriteTransaction,
    pin_id: &PinId,
) -> Result<Option<RunPinRecord>, StoreError> {
    read_record(write, RUN_PINS, pin_id.as_str())
}

fn pin_created_result(operation: RetentionOperationRecord) -> Result<RunPinRecord, StoreError> {
    match operation.result {
        RetentionOperationResult::PinCreated { pin } => Ok(pin),
        _ => Err(StoreError::OperationConflict(
            operation.operation_id.as_str().to_owned(),
        )),
    }
}

fn pin_removed_result(
    operation: RetentionOperationRecord,
    expected_pin: &PinId,
) -> Result<RunId, StoreError> {
    match operation.result {
        RetentionOperationResult::PinRemoved { pin_id, run_id } if &pin_id == expected_pin => {
            Ok(run_id)
        }
        _ => Err(StoreError::OperationConflict(
            operation.operation_id.as_str().to_owned(),
        )),
    }
}

fn pin_request_digest(run_id: &RunId, reason: &str) -> String {
    let mut bytes = Vec::new();
    append_length_prefixed(&mut bytes, b"lumin-run-pin-request.v1");
    append_length_prefixed(&mut bytes, run_id.as_str().as_bytes());
    append_length_prefixed(&mut bytes, reason.as_bytes());
    digest_hex(&bytes)
}

fn unpin_request_digest(pin_id: &PinId) -> String {
    let mut bytes = Vec::new();
    append_length_prefixed(&mut bytes, b"lumin-run-unpin-request.v1");
    append_length_prefixed(&mut bytes, pin_id.as_str().as_bytes());
    digest_hex(&bytes)
}
