use lumin_evidence::{
    RetentionItemKind, RetentionOperationResult, RetentionOperationStatus, RetentionPlanState,
};
use lumin_model::{OperationId, RetentionPlanId, digest_hex};
use redb::{ReadableTable, TableDefinition, WriteTransaction};

use crate::gate::{GATES, OPERATIONS, TRANSITIONS};
use crate::{RUN_CATALOG, StoreError, backend_error};

use super::super::RUN_PINS;
use super::super::records::{
    StoredRetentionPlan, canonical_tombstone_identity, next_sequence, read_plan,
    read_retention_operation, write_plan, write_pruned_tombstones, write_retention_operation,
};
use super::require_confirmation_owner;

pub(super) fn commit_pruned(
    guard: &crate::namespace::NamespaceGuard,
    plan_id: &RetentionPlanId,
    operation_id: &OperationId,
) -> Result<StoredRetentionPlan, StoreError> {
    let database = guard.open_database()?;
    let write = database.begin_write()?;
    let mut plan = read_plan(&write, plan_id)?;
    require_confirmation_owner(&plan, operation_id)?;
    if plan.record.state == RetentionPlanState::Pruned {
        return Ok(plan);
    }
    if plan.record.recoverable_state
        != Some(lumin_evidence::RetentionRecoverableState::ReadyToCommit)
    {
        return Err(StoreError::RetentionPlanState(plan_id.as_str().to_owned()));
    }
    remove_canonical_records(&write, &plan)?;
    plan.record.state = RetentionPlanState::Pruned;
    plan.record.recoverable_state = None;
    plan.record.tombstone_identity = Some(canonical_tombstone_identity(&plan.record));
    write_plan(&write, &plan)?;
    write_pruned_tombstones(&write, &plan.record)?;
    let mut operation = read_retention_operation(&write, operation_id)?.ok_or_else(|| {
        StoreError::Integrity("retention confirmation operation disappeared".to_owned())
    })?;
    if operation.status != RetentionOperationStatus::Pruning {
        return Err(StoreError::Integrity(
            "retention confirmation operation is not pruning".to_owned(),
        ));
    }
    if !plan.record.physical_reclamation_pending {
        write_committed_result(&write, &mut operation, &plan)?;
    }
    next_sequence(&write, "retention-catalog")?;
    guard.commit(write)?;
    Ok(plan)
}

pub(super) fn commit_reclamation_pending(
    guard: &crate::namespace::NamespaceGuard,
    plan_id: &RetentionPlanId,
    operation_id: &OperationId,
) -> Result<StoredRetentionPlan, StoreError> {
    let database = guard.open_database()?;
    let write = database.begin_write()?;
    let plan = read_plan(&write, plan_id)?;
    require_confirmation_owner(&plan, operation_id)?;
    if plan.record.state != RetentionPlanState::Pruned || !plan.record.physical_reclamation_pending
    {
        return Err(StoreError::RetentionPlanState(plan_id.as_str().to_owned()));
    }
    let mut operation = read_retention_operation(&write, operation_id)?.ok_or_else(|| {
        StoreError::Integrity("retention confirmation operation disappeared".to_owned())
    })?;
    match operation.status {
        RetentionOperationStatus::Pruning => {
            write_committed_result(&write, &mut operation, &plan)?;
        }
        RetentionOperationStatus::Committed => {
            ensure_committed_pending_result(&operation, plan_id)?;
        }
        RetentionOperationStatus::Stale => {
            return Err(StoreError::Integrity(
                "stale retention confirmation owns a pruned plan".to_owned(),
            ));
        }
    }
    guard.commit(write)?;
    Ok(plan)
}

pub(super) fn mark_reclaimed(
    guard: &crate::namespace::NamespaceGuard,
    plan_id: &RetentionPlanId,
    operation_id: &OperationId,
) -> Result<StoredRetentionPlan, StoreError> {
    let database = guard.open_database()?;
    let write = database.begin_write()?;
    let mut plan = read_plan(&write, plan_id)?;
    require_confirmation_owner(&plan, operation_id)?;
    if plan.record.state != RetentionPlanState::Pruned {
        return Err(StoreError::RetentionPlanState(plan_id.as_str().to_owned()));
    }
    plan.record.physical_reclamation_pending = false;
    write_plan(&write, &plan)?;
    write_pruned_tombstones(&write, &plan.record)?;
    let mut operation = read_retention_operation(&write, operation_id)?.ok_or_else(|| {
        StoreError::Integrity("retention confirmation operation disappeared".to_owned())
    })?;
    match operation.status {
        RetentionOperationStatus::Pruning => {
            write_committed_result(&write, &mut operation, &plan)?;
        }
        RetentionOperationStatus::Committed => {
            ensure_committed_pending_result(&operation, plan_id)?;
        }
        RetentionOperationStatus::Stale => {
            return Err(StoreError::Integrity(
                "stale retention confirmation owns a pruned plan".to_owned(),
            ));
        }
    }
    guard.commit(write)?;
    Ok(plan)
}

fn write_committed_result(
    write: &WriteTransaction,
    operation: &mut lumin_evidence::RetentionOperationRecord,
    plan: &StoredRetentionPlan,
) -> Result<(), StoreError> {
    operation.status = RetentionOperationStatus::Committed;
    operation.result = RetentionOperationResult::Retention {
        result: super::super::records::pruned_result(&plan.record)?,
    };
    write_retention_operation(write, operation)
}

fn ensure_committed_pending_result(
    operation: &lumin_evidence::RetentionOperationRecord,
    plan_id: &RetentionPlanId,
) -> Result<(), StoreError> {
    match &operation.result {
        RetentionOperationResult::Retention {
            result:
                lumin_evidence::RetentionMutationResult::Pruned {
                    plan_id: result_plan_id,
                    physical_reclamation_pending: true,
                    ..
                },
        } if result_plan_id == plan_id => Ok(()),
        _ => Err(StoreError::Integrity(
            "committed retention result cannot resume physical reclamation".to_owned(),
        )),
    }
}

fn remove_canonical_records(
    write: &WriteTransaction,
    plan: &StoredRetentionPlan,
) -> Result<(), StoreError> {
    for item in &plan.record.items {
        match item.kind {
            RetentionItemKind::Run => {
                remove_checked(write, RUN_CATALOG, &item.record_id, &item.identity_sha256)?;
            }
            RetentionItemKind::Gate => {
                remove_checked(write, GATES, &item.record_id, &item.identity_sha256)?;
            }
            RetentionItemKind::Operation => {
                remove_checked(write, OPERATIONS, &item.record_id, &item.identity_sha256)?;
            }
            RetentionItemKind::Transition => {
                remove_checked(write, TRANSITIONS, &item.record_id, &item.identity_sha256)?;
            }
            RetentionItemKind::PinOrReference => {
                remove_checked(write, RUN_PINS, &item.record_id, &item.identity_sha256)?;
            }
            RetentionItemKind::Attempt
            | RetentionItemKind::GateRevision
            | RetentionItemKind::Finding
            | RetentionItemKind::Evidence
            | RetentionItemKind::OrphanPayload
            | RetentionItemKind::Tombstone => {}
        }
    }
    Ok(())
}

fn remove_checked(
    write: &WriteTransaction,
    definition: TableDefinition<'static, &str, &[u8]>,
    key: &str,
    expected_sha256: &str,
) -> Result<(), StoreError> {
    let mut table = write.open_table(definition).map_err(backend_error)?;
    let bytes = table
        .get(key)
        .map_err(backend_error)?
        .map(|value| value.value().to_vec())
        .ok_or_else(|| {
            StoreError::Integrity(format!("retention target disappeared before commit: {key}"))
        })?;
    if digest_hex(&bytes) != expected_sha256 {
        return Err(StoreError::Integrity(format!(
            "retention target changed before commit: {key}"
        )));
    }
    table.remove(key).map_err(backend_error)?;
    Ok(())
}
