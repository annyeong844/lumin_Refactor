use lumin_evidence::{
    GATE_VALIDATION_RECEIPT_SCHEMA_VERSION, GateOperationKind, GateOperationStatus, GateRecord,
    GateValidationReceipt, GateValidationReceiptPayload, OperationRecord,
};
use redb::{ReadTransaction, WriteTransaction};

use crate::{StoreError, namespace::StoreDatabase};

use super::records::{load_record, load_record_from_read, read_record, write_record};
use super::{OPERATIONS, VALIDATION_RECEIPTS};

pub(crate) fn validation_receipt_for_operation(
    operation: &OperationRecord,
) -> Result<Option<GateValidationReceipt>, StoreError> {
    let payload = if operation.status == GateOperationStatus::Pending
        && operation.kind == GateOperationKind::PreWrite
    {
        validate_pending_pre_write_inspection(operation)?;
        GateValidationReceiptPayload::PreWriteInspection {
            declared_path_inspection: operation.pre_write_declared_path_inspection.clone(),
            leased_write_set: operation.leased_write_set.clone(),
        }
    } else if operation.status != GateOperationStatus::Committed {
        return Ok(None);
    } else {
        match operation.kind {
            GateOperationKind::PreWrite => {
                match (
                    operation.pre_write_admission_evidence.as_ref(),
                    operation.pre_write_final_validation.as_ref(),
                ) {
                    (Some(evidence), None) => GateValidationReceiptPayload::PreWriteAdmission {
                        evidence: evidence.clone(),
                    },
                    (None, Some(validation)) => GateValidationReceiptPayload::PreWriteFinal {
                        validation: validation.clone(),
                    },
                    _ => {
                        return Err(StoreError::Integrity(format!(
                            "committed pre-write operation {} has no unique validation receipt payload",
                            operation.operation_id.as_str()
                        )));
                    }
                }
            }
            GateOperationKind::PostWrite => {
                let Some(validation) = operation.post_write_final_validation.as_ref() else {
                    return Err(StoreError::Integrity(format!(
                        "committed post-write operation {} omitted its final validation receipt payload",
                        operation.operation_id.as_str()
                    )));
                };
                GateValidationReceiptPayload::PostWriteFinal {
                    validation: validation.clone(),
                }
            }
            GateOperationKind::GateAbandon => return Ok(None),
        }
    };
    Ok(Some(GateValidationReceipt {
        schema_version: GATE_VALIDATION_RECEIPT_SCHEMA_VERSION.to_owned(),
        operation_id: operation.operation_id.clone(),
        gate_id: operation.gate_id.clone(),
        request_digest: operation.request_digest.clone(),
        target_revision: operation.target_revision,
        pre_write_declared_path_inspection: operation.pre_write_declared_path_inspection.clone(),
        payload,
    }))
}

pub(super) fn persist_validation_receipt(
    write: &WriteTransaction,
    operation: &OperationRecord,
) -> Result<(), StoreError> {
    let expected = validation_receipt_for_operation(operation)?;
    let existing = read_record::<GateValidationReceipt>(
        write,
        VALIDATION_RECEIPTS,
        operation.operation_id.as_str(),
    )?;
    match (expected, existing) {
        (Some(expected), None) => write_record(
            write,
            VALIDATION_RECEIPTS,
            operation.operation_id.as_str(),
            &expected,
        ),
        (Some(expected), Some(existing)) if expected == existing => Ok(()),
        (Some(expected), Some(existing)) if receipt_can_advance(&existing, &expected) => {
            write_record(
                write,
                VALIDATION_RECEIPTS,
                operation.operation_id.as_str(),
                &expected,
            )
        }
        (None, None) => Ok(()),
        _ => Err(StoreError::Integrity(format!(
            "operation {} disagrees with its immutable validation receipt",
            operation.operation_id.as_str()
        ))),
    }
}

fn receipt_can_advance(existing: &GateValidationReceipt, expected: &GateValidationReceipt) -> bool {
    existing.schema_version == expected.schema_version
        && existing.operation_id == expected.operation_id
        && existing.gate_id == expected.gate_id
        && existing.request_digest == expected.request_digest
        && existing.target_revision == expected.target_revision
        && existing.pre_write_declared_path_inspection
            == expected.pre_write_declared_path_inspection
        && inspection_receipt_is_self_consistent(existing)
        && matches!(
            expected.payload,
            GateValidationReceiptPayload::PreWriteAdmission { .. }
                | GateValidationReceiptPayload::PreWriteFinal { .. }
        )
}

fn validate_pending_pre_write_inspection(operation: &OperationRecord) -> Result<(), StoreError> {
    let inspected_paths = operation
        .pre_write_declared_path_inspection
        .iter()
        .map(|inspection| inspection.path.clone())
        .collect::<Vec<_>>();
    let inspected_leases = operation
        .pre_write_declared_path_inspection
        .iter()
        .filter_map(|inspection| inspection.lease.clone())
        .collect::<Vec<_>>();
    let inspection_is_total =
        operation
            .pre_write_declared_path_inspection
            .iter()
            .all(|inspection| {
                inspection.lease.is_some() != inspection.rejection.is_some()
                    && inspection
                        .lease
                        .as_ref()
                        .is_none_or(|lease| lease.path == inspection.path)
            });
    if !inspection_is_total
        || inspected_paths != operation.declared_write_set
        || inspected_leases != operation.leased_write_set
    {
        return Err(StoreError::Integrity(format!(
            "pending pre-write operation {} has an incoherent declared-path inspection",
            operation.operation_id.as_str()
        )));
    }
    Ok(())
}

fn inspection_receipt_is_self_consistent(receipt: &GateValidationReceipt) -> bool {
    let GateValidationReceiptPayload::PreWriteInspection {
        declared_path_inspection,
        leased_write_set,
    } = &receipt.payload
    else {
        return false;
    };
    let inspected_leases = declared_path_inspection
        .iter()
        .filter_map(|inspection| inspection.lease.clone())
        .collect::<Vec<_>>();
    declared_path_inspection == &receipt.pre_write_declared_path_inspection
        && &inspected_leases == leased_write_set
}

pub(super) fn validate_stored_validation_receipt(
    write: &WriteTransaction,
    operation: &OperationRecord,
) -> Result<(), StoreError> {
    let expected = validation_receipt_for_operation(operation)?;
    let existing = read_record::<GateValidationReceipt>(
        write,
        VALIDATION_RECEIPTS,
        operation.operation_id.as_str(),
    )?;
    validate_validation_receipt_pair(operation, expected, existing)
}

pub(super) fn validate_loaded_validation_receipt(
    database: &StoreDatabase<'_>,
    operation: &OperationRecord,
) -> Result<(), StoreError> {
    let expected = validation_receipt_for_operation(operation)?;
    let existing = load_record::<GateValidationReceipt>(
        database,
        VALIDATION_RECEIPTS,
        operation.operation_id.as_str(),
    )?;
    validate_validation_receipt_pair(operation, expected, existing)
}

pub(super) fn validate_gate_validation_receipts(
    read: &ReadTransaction,
    gate: &GateRecord,
) -> Result<(), StoreError> {
    for revision in &gate.revisions {
        let operation = load_record_from_read::<OperationRecord>(
            read,
            OPERATIONS,
            revision.operation_id.as_str(),
        )?
        .ok_or_else(|| {
            StoreError::Integrity(format!(
                "gate {} revision {} lost its operation {}",
                gate.gate_id.as_str(),
                revision.revision,
                revision.operation_id.as_str()
            ))
        })?;
        if operation.gate_id != gate.gate_id {
            return Err(StoreError::Integrity(format!(
                "gate {} revision {} is owned by another operation gate",
                gate.gate_id.as_str(),
                revision.revision
            )));
        }
        let expected = validation_receipt_for_operation(&operation)?;
        let existing = load_record_from_read::<GateValidationReceipt>(
            read,
            VALIDATION_RECEIPTS,
            operation.operation_id.as_str(),
        )?;
        validate_validation_receipt_pair(&operation, expected, existing)?;
    }
    Ok(())
}

fn validate_validation_receipt_pair(
    operation: &OperationRecord,
    expected: Option<GateValidationReceipt>,
    existing: Option<GateValidationReceipt>,
) -> Result<(), StoreError> {
    match (expected, existing) {
        (Some(expected), Some(existing)) if expected == existing => Ok(()),
        (None, None) => Ok(()),
        _ => Err(StoreError::Integrity(format!(
            "operation {} disagrees with its store-owned validation receipt",
            operation.operation_id.as_str()
        ))),
    }
}

pub(super) fn remove_validation_receipt(
    write: &WriteTransaction,
    operation: &OperationRecord,
) -> Result<(), StoreError> {
    let mut table = write
        .open_table(VALIDATION_RECEIPTS)
        .map_err(crate::backend_error)?;
    table
        .remove(operation.operation_id.as_str())
        .map_err(crate::backend_error)?;
    Ok(())
}

pub(crate) fn operation_retention_identity(
    operation_bytes: &[u8],
    receipt_bytes: Option<&[u8]>,
) -> (String, u64) {
    let Some(receipt_bytes) = receipt_bytes else {
        return (
            crate::digest_hex(operation_bytes),
            operation_bytes.len() as u64,
        );
    };
    let mut framed = Vec::new();
    for field in [
        b"lumin-gate-operation-retention.v1".as_slice(),
        operation_bytes,
        receipt_bytes,
    ] {
        framed.extend_from_slice(&(field.len() as u64).to_be_bytes());
        framed.extend_from_slice(field);
    }
    (
        crate::digest_hex(&framed),
        operation_bytes.len().saturating_add(receipt_bytes.len()) as u64,
    )
}
