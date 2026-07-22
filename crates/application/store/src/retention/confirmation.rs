pub(super) mod logical;
pub(super) mod payload;

use lumin_evidence::{
    RetentionMutationResult, RetentionOperationRecord, RetentionOperationResult,
    RetentionOperationStatus, RetentionPlanState, RetentionRecoverableState,
};
use lumin_model::{OperationId, RetentionPlanId};

use crate::namespace::NamespaceGuard;
use crate::{RepositoryStore, StoreError};

use super::planning::{
    build_contents, confirm_operation_kind, confirm_request_digest, reject_gate_operation_collision,
};
use super::records::{
    OPERATION_SCHEMA, RetentionProgress, StoredRetentionPlan,
    ensure_committed_pruned_result_matches_plan, ensure_result_matches, pruned_result,
    pruning_result, read_plan, read_retention_operation, retention_operation_result, write_plan,
    write_pruning_tombstones, write_retention_operation,
};

pub(super) fn confirm(
    store: &RepositoryStore,
    plan_id: &RetentionPlanId,
    operation_id: &OperationId,
) -> Result<RetentionMutationResult, StoreError> {
    confirm_with_reclaimer(store, plan_id, operation_id, payload::reclaim)
}

type ReclaimFn = fn(&NamespaceGuard, &StoredRetentionPlan) -> Result<(), StoreError>;

fn confirm_with_reclaimer(
    store: &RepositoryStore,
    plan_id: &RetentionPlanId,
    operation_id: &OperationId,
    reclaim: ReclaimFn,
) -> Result<RetentionMutationResult, StoreError> {
    store.with_exclusive_lock(|guard| {
        let result = admit_or_resume(guard, plan_id, operation_id)?;
        match &result {
            RetentionMutationResult::Stale { .. }
            | RetentionMutationResult::Pruned {
                physical_reclamation_pending: false,
                ..
            } => Ok(result),
            RetentionMutationResult::Pruned {
                physical_reclamation_pending: true,
                ..
            } => {
                resume_pruning(guard, plan_id, operation_id, reclaim)?;
                Ok(result)
            }
            RetentionMutationResult::Pruning { .. } => {
                resume_pruning(guard, plan_id, operation_id, reclaim)
            }
            RetentionMutationResult::Prepared { .. } => Err(StoreError::Integrity(
                "retention confirmation returned a prepared result".to_owned(),
            )),
        }
    })
}

#[cfg(test)]
pub(super) fn confirm_with_reclaim_io_error(
    store: &RepositoryStore,
    plan_id: &RetentionPlanId,
    operation_id: &OperationId,
) -> Result<RetentionMutationResult, StoreError> {
    fn fail_reclaim(
        _guard: &NamespaceGuard,
        _plan: &StoredRetentionPlan,
    ) -> Result<(), StoreError> {
        Err(StoreError::Io(
            "injected retention reclaim failure".to_owned(),
        ))
    }

    confirm_with_reclaimer(store, plan_id, operation_id, fail_reclaim)
}

pub(super) fn admit_or_resume(
    guard: &NamespaceGuard,
    plan_id: &RetentionPlanId,
    operation_id: &OperationId,
) -> Result<RetentionMutationResult, StoreError> {
    let database = guard.open_database()?;
    let write = database.begin_write()?;
    let mut plan = read_plan(&write, plan_id)?;
    let kind = confirm_operation_kind(&plan.record.scope);
    let request_digest = confirm_request_digest(plan_id);
    if let Some(operation) = read_retention_operation(&write, operation_id)? {
        ensure_result_matches(&operation, kind, &request_digest)?;
        ensure_operation_plan(&operation, plan_id)?;
        if operation.status == RetentionOperationStatus::Committed {
            ensure_committed_pruned_result_matches_plan(&plan, &operation)?;
        }
        return retention_operation_result(operation);
    }
    reject_gate_operation_collision(&write, operation_id)?;
    if plan.record.state != RetentionPlanState::Prepared {
        return Err(StoreError::RetentionPlanState(plan_id.as_str().to_owned()));
    }

    let current = build_contents(guard, &write, &plan.record.scope)?;
    if current.items != plan.record.items || current.exclusions != plan.record.exclusions {
        let result = RetentionMutationResult::Stale {
            plan_id: plan_id.clone(),
            changed_inputs: changed_inputs(&plan, &current.items, &current.exclusions),
        };
        let operation = RetentionOperationRecord {
            schema_version: OPERATION_SCHEMA.to_owned(),
            operation_id: operation_id.clone(),
            kind,
            request_digest,
            status: RetentionOperationStatus::Stale,
            plan_id: Some(plan_id.clone()),
            result: RetentionOperationResult::Retention {
                result: result.clone(),
            },
        };
        write_retention_operation(&write, &operation)?;
        guard.commit(write)?;
        return Ok(result);
    }

    let mut progress = payload::prepare_progress(guard, &plan.record)?;
    let recoverable_state = if progress.moves.is_empty() {
        RetentionRecoverableState::ReadyToCommit
    } else {
        let mut binding_plan = plan.clone();
        binding_plan.progress = Some(progress.clone());
        progress.trash_directory = Some(payload::bind_trash_directory(guard, &binding_plan)?);
        RetentionRecoverableState::MovingPayloads
    };
    plan.record.state = RetentionPlanState::Pruning;
    plan.record.confirmation_operation_id = Some(operation_id.clone());
    plan.record.recoverable_state = Some(recoverable_state);
    plan.record.physical_reclamation_pending = !progress.moves.is_empty();
    plan.progress = Some(progress);
    let result = pruning_result(&plan.record)?;
    let operation = RetentionOperationRecord {
        schema_version: OPERATION_SCHEMA.to_owned(),
        operation_id: operation_id.clone(),
        kind,
        request_digest,
        status: RetentionOperationStatus::Pruning,
        plan_id: Some(plan_id.clone()),
        result: RetentionOperationResult::Retention {
            result: result.clone(),
        },
    };
    write_plan(&write, &plan)?;
    write_pruning_tombstones(&write, &plan.record)?;
    write_retention_operation(&write, &operation)?;
    if plan
        .record
        .items
        .iter()
        .any(|item| item.kind == lumin_evidence::RetentionItemKind::Run)
    {
        super::records::next_sequence(&write, "run-catalog")?;
    }
    #[cfg(feature = "retention-test-crash")]
    super::crash::hit(super::crash::RetentionCrashPoint::BeforePruningCommit);
    guard.commit(write)?;
    #[cfg(feature = "retention-test-crash")]
    super::crash::hit(super::crash::RetentionCrashPoint::AfterPruningCommit);
    Ok(result)
}

fn resume_pruning(
    guard: &NamespaceGuard,
    plan_id: &RetentionPlanId,
    operation_id: &OperationId,
    reclaim: ReclaimFn,
) -> Result<RetentionMutationResult, StoreError> {
    let plan = advance_to_pruned_without_reclaim(guard, plan_id, operation_id)?;
    if plan.record.state != RetentionPlanState::Pruned {
        return pruning_result(&plan.record);
    }
    let committed_result = pruned_result(&plan.record)?;
    if plan.record.physical_reclamation_pending {
        match reclaim(guard, &plan) {
            Ok(()) => {
                logical::mark_reclaimed(guard, plan_id, operation_id)?;
            }
            Err(StoreError::Io(_)) => {
                logical::commit_reclamation_pending(guard, plan_id, operation_id)?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(committed_result)
}

fn advance_to_pruned_without_reclaim(
    guard: &NamespaceGuard,
    plan_id: &RetentionPlanId,
    operation_id: &OperationId,
) -> Result<StoredRetentionPlan, StoreError> {
    let mut plan = load_plan_for_resume(guard, plan_id, operation_id)?;
    if plan.record.state == RetentionPlanState::Pruned {
        return Ok(plan);
    }
    if plan.record.recoverable_state == Some(RetentionRecoverableState::MovingPayloads) {
        payload::move_payloads(guard, &plan)?;
        plan = update_progress(guard, plan_id, operation_id, |plan, _| {
            plan.record.recoverable_state = Some(RetentionRecoverableState::ReadyToCommit);
            Ok(())
        })?;
        #[cfg(feature = "retention-test-crash")]
        super::crash::hit(super::crash::RetentionCrashPoint::AfterMovesCommitted);
    }
    if plan.record.recoverable_state == Some(RetentionRecoverableState::ReadyToCommit) {
        plan = logical::commit_pruned(guard, plan_id, operation_id)?;
        #[cfg(feature = "retention-test-crash")]
        super::crash::hit(super::crash::RetentionCrashPoint::AfterPrunedCommit);
    }
    Ok(plan)
}

#[cfg(test)]
pub(super) fn commit_pruned_without_reclaim(
    guard: &NamespaceGuard,
    plan_id: &RetentionPlanId,
    operation_id: &OperationId,
) -> Result<StoredRetentionPlan, StoreError> {
    advance_to_pruned_without_reclaim(guard, plan_id, operation_id)
}

#[cfg(test)]
pub(super) fn reclaim_without_mark(
    guard: &NamespaceGuard,
    plan_id: &RetentionPlanId,
    operation_id: &OperationId,
) -> Result<(), StoreError> {
    let plan = load_plan_for_resume(guard, plan_id, operation_id)?;
    if plan.record.state != RetentionPlanState::Pruned || !plan.record.physical_reclamation_pending
    {
        return Err(StoreError::Integrity(
            "retention plan is not waiting for physical reclamation".to_owned(),
        ));
    }
    payload::reclaim(guard, &plan)
}

pub(super) fn update_progress(
    guard: &NamespaceGuard,
    plan_id: &RetentionPlanId,
    operation_id: &OperationId,
    update: impl FnOnce(&mut StoredRetentionPlan, &mut RetentionProgress) -> Result<(), StoreError>,
) -> Result<StoredRetentionPlan, StoreError> {
    let database = guard.open_database()?;
    let write = database.begin_write()?;
    let mut plan = read_plan(&write, plan_id)?;
    require_confirmation_owner(&plan, operation_id)?;
    let mut progress = plan.progress.take().ok_or_else(|| {
        StoreError::Integrity("pruning retention plan has no progress record".to_owned())
    })?;
    update(&mut plan, &mut progress)?;
    plan.progress = Some(progress);
    write_plan(&write, &plan)?;
    write_pruning_tombstones(&write, &plan.record)?;
    let mut operation = read_retention_operation(&write, operation_id)?.ok_or_else(|| {
        StoreError::Integrity("retention confirmation operation disappeared".to_owned())
    })?;
    operation.result = RetentionOperationResult::Retention {
        result: pruning_result(&plan.record)?,
    };
    write_retention_operation(&write, &operation)?;
    guard.commit(write)?;
    Ok(plan)
}

pub(super) fn load_plan_for_resume(
    guard: &NamespaceGuard,
    plan_id: &RetentionPlanId,
    operation_id: &OperationId,
) -> Result<StoredRetentionPlan, StoreError> {
    let database = guard.open_database()?;
    let write = database.begin_write()?;
    let plan = read_plan(&write, plan_id)?;
    require_confirmation_owner(&plan, operation_id)?;
    Ok(plan)
}

pub(super) fn require_confirmation_owner(
    plan: &StoredRetentionPlan,
    operation_id: &OperationId,
) -> Result<(), StoreError> {
    if plan.record.confirmation_operation_id.as_ref() != Some(operation_id) {
        return Err(StoreError::RetentionPlanState(
            plan.record.plan_id.as_str().to_owned(),
        ));
    }
    Ok(())
}

fn ensure_operation_plan(
    operation: &RetentionOperationRecord,
    plan_id: &RetentionPlanId,
) -> Result<(), StoreError> {
    if operation.plan_id.as_ref() != Some(plan_id) {
        return Err(StoreError::OperationConflict(
            operation.operation_id.as_str().to_owned(),
        ));
    }
    Ok(())
}

fn changed_inputs(
    plan: &StoredRetentionPlan,
    current_items: &[lumin_evidence::RetentionPlanItem],
    current_exclusions: &[lumin_evidence::RetentionPlanExclusion],
) -> Vec<String> {
    let mut changed = Vec::new();
    if plan.record.items != current_items {
        changed.push("plan-items".to_owned());
    }
    if plan.record.exclusions != current_exclusions {
        changed.push("plan-exclusions".to_owned());
    }
    changed
}
