use std::collections::BTreeMap;

use lumin_evidence::{
    RetentionItemKind, RetentionOperationRecord, RetentionOperationResult, RetentionPlanState,
    RunPinRecord,
};

use crate::StoreError;
use crate::retention::records::StoredRetentionPlan;

use super::{LogicalStoreSnapshot, parse_record};

pub(super) fn validate_pins(
    snapshot: &LogicalStoreSnapshot,
    operations: &BTreeMap<&str, RetentionOperationRecord>,
    plans: &BTreeMap<&str, StoredRetentionPlan>,
) -> Result<(), StoreError> {
    for operation in operations.values() {
        if let RetentionOperationResult::PinCreated { pin } = &operation.result {
            validate_reason(pin)?;
        }
    }
    let mut pins = BTreeMap::new();
    for (key, bytes) in &snapshot.run_pins {
        let pin = parse_record::<RunPinRecord>("run-pins", key, bytes)?;
        if pin.schema_version != "lumin-run-pin.v1" || pin.pin_id.as_str() != key {
            return Err(StoreError::Integrity(format!(
                "run pin key {key} disagrees with its record"
            )));
        }
        validate_reason(&pin)?;
        validate_creation(key, &pin, operations)?;
        validate_removal(snapshot, key, &pin, operations)?;
        pins.insert(key.as_str(), pin);
    }
    let pruned_pins = reconstruct_pruned_pins(&pins, operations, plans)?;
    validate_operation_rows(&pins, &pruned_pins, operations)
}

fn validate_reason(pin: &RunPinRecord) -> Result<(), StoreError> {
    if pin.reason.is_empty() || pin.reason.trim() != pin.reason {
        return Err(StoreError::Integrity(format!(
            "run pin {} has a noncanonical reason",
            pin.pin_id.as_str()
        )));
    }
    Ok(())
}

fn validate_operation_rows(
    pins: &BTreeMap<&str, RunPinRecord>,
    pruned_pins: &BTreeMap<String, RunPinRecord>,
    operations: &BTreeMap<&str, RetentionOperationRecord>,
) -> Result<(), StoreError> {
    for operation in operations.values() {
        let matches = match &operation.result {
            RetentionOperationResult::PinCreated { pin } => pins
                .get(pin.pin_id.as_str())
                .or_else(|| pruned_pins.get(pin.pin_id.as_str()))
                .is_some_and(|stored| {
                    pin.created_operation_id == operation.operation_id
                        && pin.removed_operation_id.is_none()
                        && stored.schema_version == pin.schema_version
                        && stored.pin_id == pin.pin_id
                        && stored.run_id == pin.run_id
                        && stored.reason == pin.reason
                        && stored.created_unix_millis == pin.created_unix_millis
                        && stored.created_operation_id == operation.operation_id
                }),
            RetentionOperationResult::PinRemoved { pin_id, run_id } => pins
                .get(pin_id.as_str())
                .or_else(|| pruned_pins.get(pin_id.as_str()))
                .is_some_and(|stored| {
                    stored.run_id == *run_id
                        && stored.removed_operation_id.as_ref() == Some(&operation.operation_id)
                }),
            RetentionOperationResult::Retention { .. } => true,
        };
        if !matches {
            return Err(StoreError::Integrity(format!(
                "retention operation {} has no matching durable pin row",
                operation.operation_id.as_str()
            )));
        }
    }
    Ok(())
}

fn reconstruct_pruned_pins(
    live_pins: &BTreeMap<&str, RunPinRecord>,
    operations: &BTreeMap<&str, RetentionOperationRecord>,
    plans: &BTreeMap<&str, StoredRetentionPlan>,
) -> Result<BTreeMap<String, RunPinRecord>, StoreError> {
    let mut pruned = BTreeMap::new();
    for operation in operations.values() {
        let RetentionOperationResult::PinCreated { pin } = &operation.result else {
            continue;
        };
        if live_pins.contains_key(pin.pin_id.as_str()) {
            continue;
        }
        let mut removals = operations.values().filter(|candidate| {
            matches!(
                &candidate.result,
                RetentionOperationResult::PinRemoved { pin_id, run_id }
                    if pin_id == &pin.pin_id && run_id == &pin.run_id
            )
        });
        let Some(removal) = removals.next() else {
            continue;
        };
        if removals.next().is_some() {
            return Err(StoreError::Integrity(format!(
                "pruned run pin {} has multiple removal operations",
                pin.pin_id.as_str()
            )));
        }
        let mut reconstructed = pin.clone();
        reconstructed.removed_operation_id = Some(removal.operation_id.clone());
        if !pruned_plan_owns_pin(&reconstructed, plans)? {
            continue;
        }
        if pruned
            .insert(pin.pin_id.as_str().to_owned(), reconstructed)
            .is_some()
        {
            return Err(StoreError::Integrity(format!(
                "pruned run pin {} has multiple creation operations",
                pin.pin_id.as_str()
            )));
        }
    }
    Ok(pruned)
}

fn pruned_plan_owns_pin(
    pin: &RunPinRecord,
    plans: &BTreeMap<&str, StoredRetentionPlan>,
) -> Result<bool, StoreError> {
    let bytes = serde_json::to_vec(pin).map_err(crate::serialization_error)?;
    let byte_count = u64::try_from(bytes.len())
        .map_err(|_| StoreError::Integrity("run pin byte count overflow".to_owned()))?;
    let identity = crate::digest_hex(&bytes);
    Ok(plans.values().any(|plan| {
        plan.record.state == RetentionPlanState::Pruned
            && plan.record.items.iter().any(|item| {
                item.kind == RetentionItemKind::PinOrReference
                    && item.record_id == pin.pin_id.as_str()
                    && item.identity_sha256 == identity
                    && item.byte_count == byte_count
            })
    }))
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
