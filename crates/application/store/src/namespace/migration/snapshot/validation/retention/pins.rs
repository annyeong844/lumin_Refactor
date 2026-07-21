use std::collections::BTreeMap;

use lumin_evidence::{RetentionOperationRecord, RetentionOperationResult, RunPinRecord};

use crate::StoreError;

use super::{LogicalStoreSnapshot, parse_record};

pub(super) fn validate_pins(
    snapshot: &LogicalStoreSnapshot,
    operations: &BTreeMap<&str, RetentionOperationRecord>,
) -> Result<(), StoreError> {
    for (key, bytes) in &snapshot.run_pins {
        let pin = parse_record::<RunPinRecord>("run-pins", key, bytes)?;
        if pin.schema_version != "lumin-run-pin.v1" || pin.pin_id.as_str() != key {
            return Err(StoreError::Integrity(format!(
                "run pin key {key} disagrees with its record"
            )));
        }
        validate_creation(key, &pin, operations)?;
        validate_removal(snapshot, key, &pin, operations)?;
    }
    Ok(())
}

fn validate_creation(
    key: &str,
    pin: &RunPinRecord,
    operations: &BTreeMap<&str, RetentionOperationRecord>,
) -> Result<(), StoreError> {
    let created = operations
        .get(pin.created_operation_id.as_str())
        .is_some_and(|operation| {
            matches!(
                &operation.result,
                RetentionOperationResult::PinCreated { pin: result }
                    if result.pin_id == pin.pin_id
                        && result.run_id == pin.run_id
                        && result.reason == pin.reason
                        && result.created_unix_millis == pin.created_unix_millis
                        && result.created_operation_id == pin.created_operation_id
                        && result.removed_operation_id.is_none()
            )
        });
    if !created {
        return Err(StoreError::Integrity(format!(
            "run pin {key} has no matching creation operation"
        )));
    }
    Ok(())
}

fn validate_removal(
    snapshot: &LogicalStoreSnapshot,
    key: &str,
    pin: &RunPinRecord,
    operations: &BTreeMap<&str, RetentionOperationRecord>,
) -> Result<(), StoreError> {
    let Some(operation_id) = &pin.removed_operation_id else {
        if snapshot.run_catalog.contains_key(pin.run_id.as_str()) {
            return Ok(());
        }
        return Err(StoreError::Integrity(format!(
            "active run pin {key} references a missing run"
        )));
    };
    let removed = operations
        .get(operation_id.as_str())
        .is_some_and(|operation| {
            matches!(
                &operation.result,
                RetentionOperationResult::PinRemoved { pin_id, run_id }
                    if pin_id == &pin.pin_id && run_id == &pin.run_id
            )
        });
    if !removed {
        return Err(StoreError::Integrity(format!(
            "run pin {key} has no matching removal operation"
        )));
    }
    Ok(())
}
