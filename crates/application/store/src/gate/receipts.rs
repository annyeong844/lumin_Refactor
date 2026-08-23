use lumin_evidence::{
    GATE_VALIDATION_RECEIPT_SCHEMA_VERSION, GateOperationKind, GateOperationStatus, GateRecord,
    GateRevision, GateValidationCommitReceipt, GateValidationReceipt, GateValidationReceiptPayload,
    OperationRecord,
};
use redb::{ReadTransaction, WriteTransaction};

use crate::{StoreError, namespace::StoreDatabase};

use super::records::{load_record, load_record_from_read, read_record, write_record};
use super::{GATES, OPERATIONS, VALIDATION_RECEIPTS};

pub(crate) fn validation_receipt_for_operation(
    operation: &OperationRecord,
    gate: Option<&GateRecord>,
) -> Result<Option<GateValidationReceipt>, StoreError> {
    let (payload, commit) = if operation.status == GateOperationStatus::Pending {
        super::validate_reservation_binding_set(operation)?;
        let payload = match operation.kind {
            GateOperationKind::PreWrite => {
                validate_pending_pre_write_inspection(operation)?;
                GateValidationReceiptPayload::PreWriteInspection {
                    declared_path_inspection: operation.pre_write_declared_path_inspection.clone(),
                    leased_write_set: operation.leased_write_set.clone(),
                }
            }
            GateOperationKind::PostWrite => GateValidationReceiptPayload::PostWritePending {
                leased_write_set: operation.leased_write_set.clone(),
            },
            GateOperationKind::GateAbandon => return Ok(None),
        };
        (payload, None)
    } else if operation.status != GateOperationStatus::Committed {
        return Ok(None);
    } else {
        let gate = gate.ok_or_else(|| {
            StoreError::Integrity(format!(
                "committed operation {} lost its gate record",
                operation.operation_id.as_str()
            ))
        })?;
        let revision = committed_revision(operation, gate)?;
        super::integrity::validate_committed_operation_result(operation, gate, revision)?;
        let payload = match operation.kind {
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
            GateOperationKind::GateAbandon => {
                let reason = operation.reason.clone().ok_or_else(|| {
                    StoreError::Integrity(format!(
                        "committed abandon operation {} omitted its reason",
                        operation.operation_id.as_str()
                    ))
                })?;
                GateValidationReceiptPayload::GateAbandon { reason }
            }
        };
        let evidence_payload_sha256 = match operation.kind {
            GateOperationKind::PreWrite => gate
                .baseline
                .as_ref()
                .map(|baseline| crate::evidence_payload_sha256(&baseline.snapshot.evidence))
                .transpose()?,
            GateOperationKind::PostWrite => revision
                .snapshot
                .as_ref()
                .map(|snapshot| crate::evidence_payload_sha256(&snapshot.evidence))
                .transpose()?,
            GateOperationKind::GateAbandon => None,
        };
        (
            payload,
            Some(GateValidationCommitReceipt {
                revision_sha256: gate_revision_sha256(revision)?,
                result_sha256: super::integrity::operation_result_sha256(operation)?,
                operation_sha256: super::integrity::operation_projection_sha256(operation)?,
                gate_projection_sha256: super::integrity::gate_projection_sha256(
                    gate,
                    revision.revision,
                )?,
                committed_unix_millis: revision.committed_unix_millis.ok_or_else(|| {
                    StoreError::Integrity(format!(
                        "committed operation {} has no durable revision timestamp",
                        operation.operation_id.as_str()
                    ))
                })?,
                evidence_payload_sha256,
            }),
        )
    };
    Ok(Some(GateValidationReceipt {
        schema_version: GATE_VALIDATION_RECEIPT_SCHEMA_VERSION.to_owned(),
        operation_id: operation.operation_id.clone(),
        gate_id: operation.gate_id.clone(),
        request_digest: operation.request_digest.clone(),
        target_revision: operation.target_revision,
        pre_write_declared_path_inspection: operation.pre_write_declared_path_inspection.clone(),
        semantic_read_reservations: operation.semantic_read_reservations.clone(),
        semantic_read_reservation_bindings: operation.semantic_read_reservation_bindings.clone(),
        commit,
        payload,
    }))
}

fn committed_revision<'a>(
    operation: &OperationRecord,
    gate: &'a GateRecord,
) -> Result<&'a GateRevision, StoreError> {
    let result = operation.result.as_ref().ok_or_else(|| {
        StoreError::Integrity(format!(
            "committed operation {} omitted its result",
            operation.operation_id.as_str()
        ))
    })?;
    if gate.gate_id != operation.gate_id || result.gate_id != operation.gate_id {
        return Err(StoreError::Integrity(format!(
            "committed operation {} disagrees with its gate owner",
            operation.operation_id.as_str()
        )));
    }
    let mut matches = gate.revisions.iter().filter(|revision| {
        revision.operation_id == operation.operation_id && revision.revision == result.revision
    });
    let revision = matches.next().ok_or_else(|| {
        StoreError::Integrity(format!(
            "committed operation {} lost its gate revision",
            operation.operation_id.as_str()
        ))
    })?;
    if matches.next().is_some() {
        return Err(StoreError::Integrity(format!(
            "committed operation {} owns multiple gate revisions",
            operation.operation_id.as_str()
        )));
    }
    Ok(revision)
}

fn gate_revision_sha256(revision: &GateRevision) -> Result<String, StoreError> {
    let bytes = serde_json::to_vec(revision).map_err(crate::serialization_error)?;
    let mut framed = Vec::new();
    lumin_model::append_length_prefixed(&mut framed, b"lumin-gate-validation-revision.v1");
    lumin_model::append_length_prefixed(&mut framed, &bytes);
    Ok(crate::digest_hex(&framed))
}

pub(super) fn persist_validation_receipt(
    write: &WriteTransaction,
    operation: &OperationRecord,
    gate: Option<&GateRecord>,
) -> Result<(), StoreError> {
    let expected = validation_receipt_for_operation(operation, gate)?;
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
    let same_owner = existing.schema_version == expected.schema_version
        && existing.operation_id == expected.operation_id
        && existing.gate_id == expected.gate_id
        && existing.request_digest == expected.request_digest
        && existing.target_revision == expected.target_revision
        && existing.pre_write_declared_path_inspection
            == expected.pre_write_declared_path_inspection;
    same_owner
        && (pending_receipt_can_advance(existing, expected)
            || pending_receipt_can_commit(existing, expected))
}

fn pending_receipt_can_advance(
    existing: &GateValidationReceipt,
    expected: &GateValidationReceipt,
) -> bool {
    existing.commit.is_none()
        && expected.commit.is_none()
        && existing.payload == expected.payload
        && pending_receipt_is_self_consistent(existing)
        && pending_receipt_is_self_consistent(expected)
        && canonical_subset(
            &existing.semantic_read_reservations,
            &expected.semantic_read_reservations,
        )
        && canonical_subset(
            &existing.semantic_read_reservation_bindings,
            &expected.semantic_read_reservation_bindings,
        )
}

fn pending_receipt_can_commit(
    existing: &GateValidationReceipt,
    expected: &GateValidationReceipt,
) -> bool {
    existing.commit.is_none()
        && expected.commit.is_some()
        && expected.semantic_read_reservations.is_empty()
        && expected.semantic_read_reservation_bindings.is_empty()
        && pending_receipt_is_self_consistent(existing)
        && matches!(
            (&existing.payload, &expected.payload),
            (
                GateValidationReceiptPayload::PreWriteInspection { .. },
                GateValidationReceiptPayload::PreWriteAdmission { .. }
                    | GateValidationReceiptPayload::PreWriteFinal { .. }
            ) | (
                GateValidationReceiptPayload::PostWritePending { .. },
                GateValidationReceiptPayload::PostWriteFinal { .. }
            )
        )
}

fn canonical_subset<T: Ord>(existing: &[T], expected: &[T]) -> bool {
    canonical_strict_order(existing)
        && canonical_strict_order(expected)
        && existing
            .iter()
            .all(|item| expected.binary_search(item).is_ok())
}

fn canonical_strict_order<T: Ord>(items: &[T]) -> bool {
    items.windows(2).all(|pair| pair[0] < pair[1])
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

fn pending_receipt_is_self_consistent(receipt: &GateValidationReceipt) -> bool {
    let payload_is_consistent = match &receipt.payload {
        GateValidationReceiptPayload::PreWriteInspection {
            declared_path_inspection,
            leased_write_set,
        } => {
            let inspected_leases = declared_path_inspection
                .iter()
                .filter_map(|inspection| inspection.lease.clone())
                .collect::<Vec<_>>();
            declared_path_inspection == &receipt.pre_write_declared_path_inspection
                && &inspected_leases == leased_write_set
        }
        GateValidationReceiptPayload::PostWritePending { .. } => {
            receipt.pre_write_declared_path_inspection.is_empty()
        }
        _ => false,
    };
    let mut bound_paths = receipt
        .semantic_read_reservation_bindings
        .iter()
        .map(|binding| binding.path.clone())
        .collect::<Vec<_>>();
    bound_paths.sort();
    bound_paths.dedup();
    payload_is_consistent
        && canonical_strict_order(&receipt.semantic_read_reservations)
        && canonical_strict_order(&receipt.semantic_read_reservation_bindings)
        && bound_paths == receipt.semantic_read_reservations
}

pub(super) fn validate_stored_validation_receipt(
    write: &WriteTransaction,
    operation: &OperationRecord,
) -> Result<(), StoreError> {
    let gate = gate_for_stored_operation(write, operation)?;
    let expected = validation_receipt_for_operation(operation, gate.as_ref())?;
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
    let gate = gate_for_loaded_operation(database, operation)?;
    let expected = validation_receipt_for_operation(operation, gate.as_ref())?;
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
    super::integrity::validate_gate_record_shape(gate)?;
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
        let expected = validation_receipt_for_operation(&operation, Some(gate))?;
        let existing = load_record_from_read::<GateValidationReceipt>(
            read,
            VALIDATION_RECEIPTS,
            operation.operation_id.as_str(),
        )?;
        validate_validation_receipt_pair(&operation, expected, existing)?;
    }
    Ok(())
}

pub(super) fn validate_stored_gate_validation_receipts(
    write: &WriteTransaction,
    gate: &GateRecord,
) -> Result<(), StoreError> {
    super::integrity::validate_gate_record_shape(gate)?;
    for revision in &gate.revisions {
        let operation =
            read_record::<OperationRecord>(write, OPERATIONS, revision.operation_id.as_str())?
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
        let expected = validation_receipt_for_operation(&operation, Some(gate))?;
        let existing = read_record::<GateValidationReceipt>(
            write,
            VALIDATION_RECEIPTS,
            operation.operation_id.as_str(),
        )?;
        validate_validation_receipt_pair(&operation, expected, existing)?;
    }
    Ok(())
}

fn gate_for_stored_operation(
    write: &WriteTransaction,
    operation: &OperationRecord,
) -> Result<Option<GateRecord>, StoreError> {
    if operation.status != GateOperationStatus::Committed {
        return Ok(None);
    }
    read_record::<GateRecord>(write, GATES, operation.gate_id.as_str())?
        .ok_or_else(|| {
            StoreError::Integrity(format!(
                "committed operation {} lost its gate record",
                operation.operation_id.as_str()
            ))
        })
        .map(Some)
}

fn gate_for_loaded_operation(
    database: &StoreDatabase<'_>,
    operation: &OperationRecord,
) -> Result<Option<GateRecord>, StoreError> {
    if operation.status != GateOperationStatus::Committed {
        return Ok(None);
    }
    load_record::<GateRecord>(database, GATES, operation.gate_id.as_str())?
        .ok_or_else(|| {
            StoreError::Integrity(format!(
                "committed operation {} lost its gate record",
                operation.operation_id.as_str()
            ))
        })
        .map(Some)
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
