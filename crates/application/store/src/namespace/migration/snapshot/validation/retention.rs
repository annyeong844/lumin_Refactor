mod pins;
mod tombstones;

use std::collections::BTreeMap;

use lumin_evidence::{
    OperationRecord, RetentionDomain, RetentionMutationResult, RetentionOperationKind,
    RetentionOperationRecord, RetentionOperationResult, RetentionOperationStatus,
    RetentionPlanState, RetentionRecoverableState,
};

use crate::StoreError;
use crate::retention::records::{
    StoredRetentionPlan, ensure_committed_pruned_result_matches_plan, validate_plan,
    validate_retention_operation,
};

use super::{LogicalStoreSnapshot, parse_record};

pub(super) fn validate_retention(
    snapshot: &LogicalStoreSnapshot,
    gate_operations: &BTreeMap<&str, OperationRecord>,
) -> Result<(), StoreError> {
    let plans = read_plans(snapshot)?;
    let operations = read_operations(snapshot, gate_operations, &plans)?;
    validate_plan_operations(&plans, &operations)?;
    pins::validate_pins(snapshot, &operations)?;
    tombstones::validate_tombstones(snapshot, &plans)
}

fn read_plans(
    snapshot: &LogicalStoreSnapshot,
) -> Result<BTreeMap<&str, StoredRetentionPlan>, StoreError> {
    let mut plans = BTreeMap::new();
    for (key, bytes) in &snapshot.retention_plans {
        let plan = parse_record::<StoredRetentionPlan>("retention-plans", key, bytes)?;
        validate_plan(&plan)?;
        if plan.record.plan_id.as_str() != key {
            return Err(StoreError::Integrity(format!(
                "retention plan key {key} disagrees with its record"
            )));
        }
        plans.insert(key.as_str(), plan);
    }
    Ok(plans)
}

fn read_operations<'a>(
    snapshot: &'a LogicalStoreSnapshot,
    gate_operations: &BTreeMap<&str, OperationRecord>,
    plans: &BTreeMap<&str, StoredRetentionPlan>,
) -> Result<BTreeMap<&'a str, RetentionOperationRecord>, StoreError> {
    let mut operations = BTreeMap::new();
    for (key, bytes) in &snapshot.retention_operations {
        if gate_operations.contains_key(key.as_str()) {
            return Err(StoreError::Integrity(format!(
                "operation ID {key} is owned by both gate and retention tables"
            )));
        }
        let operation =
            parse_record::<RetentionOperationRecord>("retention-operations", key, bytes)?;
        if operation.operation_id.as_str() != key {
            return Err(StoreError::Integrity(format!(
                "retention operation key {key} disagrees with its record"
            )));
        }
        validate_operation_plan_binding(&operation, plans)?;
        operations.insert(key.as_str(), operation);
    }
    Ok(operations)
}

fn validate_operation_plan_binding(
    operation: &RetentionOperationRecord,
    plans: &BTreeMap<&str, StoredRetentionPlan>,
) -> Result<(), StoreError> {
    validate_retention_operation(operation)?;
    let Some(plan_id) = &operation.plan_id else {
        return Ok(());
    };
    let plan = plans.get(plan_id.as_str()).ok_or_else(|| {
        StoreError::Integrity(format!(
            "retention operation {} references a missing plan",
            operation.operation_id.as_str()
        ))
    })?;
    let expected_kind = match &operation.result {
        RetentionOperationResult::Retention {
            result:
                RetentionMutationResult::Prepared {
                    content_identity, ..
                },
        } => {
            if content_identity != &plan.record.content_identity {
                return Err(StoreError::Integrity(format!(
                    "retention operation {} changed its plan content identity",
                    operation.operation_id.as_str()
                )));
            }
            plan_kind(plan.record.scope.domain())
        }
        RetentionOperationResult::Retention { .. } => confirm_kind(plan.record.scope.domain()),
        RetentionOperationResult::PinCreated { .. }
        | RetentionOperationResult::PinRemoved { .. } => {
            return Err(StoreError::Integrity(format!(
                "retention operation {} binds a pin result to a plan",
                operation.operation_id.as_str()
            )));
        }
    };
    if operation.kind != expected_kind {
        return Err(StoreError::Integrity(format!(
            "retention operation {} disagrees with its plan scope",
            operation.operation_id.as_str()
        )));
    }
    Ok(())
}

fn plan_kind(domain: RetentionDomain) -> RetentionOperationKind {
    match domain {
        RetentionDomain::Runs => RetentionOperationKind::RunPrunePlan,
        RetentionDomain::Gates => RetentionOperationKind::GatePrunePlan,
    }
}

fn confirm_kind(domain: RetentionDomain) -> RetentionOperationKind {
    match domain {
        RetentionDomain::Runs => RetentionOperationKind::RunPruneConfirm,
        RetentionDomain::Gates => RetentionOperationKind::GatePruneConfirm,
    }
}

fn validate_plan_operations(
    plans: &BTreeMap<&str, StoredRetentionPlan>,
    operations: &BTreeMap<&str, RetentionOperationRecord>,
) -> Result<(), StoreError> {
    for operation in operations.values() {
        if matches!(
            operation.result,
            RetentionOperationResult::Retention {
                result: RetentionMutationResult::Pruning { .. }
                    | RetentionMutationResult::Pruned { .. }
            }
        ) {
            let plan_id = operation.plan_id.as_ref().ok_or_else(|| {
                StoreError::Integrity("retention confirmation has no plan ID".to_owned())
            })?;
            let plan = plans.get(plan_id.as_str()).ok_or_else(|| {
                StoreError::Integrity("retention confirmation plan disappeared".to_owned())
            })?;
            if plan.record.confirmation_operation_id.as_ref() != Some(&operation.operation_id) {
                return Err(StoreError::Integrity(format!(
                    "retention operation {} is not its plan's confirmation owner",
                    operation.operation_id.as_str()
                )));
            }
        }
    }
    for plan in plans.values() {
        let creation_count = operations
            .values()
            .filter(|operation| {
                matches!(
                    &operation.result,
                    RetentionOperationResult::Retention {
                        result: RetentionMutationResult::Prepared { plan_id, .. }
                    } if plan_id == &plan.record.plan_id
                )
            })
            .count();
        if creation_count != 1 {
            return Err(StoreError::Integrity(format!(
                "retention plan {} has {creation_count} creation operations",
                plan.record.plan_id.as_str()
            )));
        }
        validate_confirmation(plan, operations)?;
    }
    Ok(())
}

fn validate_confirmation(
    plan: &StoredRetentionPlan,
    operations: &BTreeMap<&str, RetentionOperationRecord>,
) -> Result<(), StoreError> {
    if plan.record.state == RetentionPlanState::Prepared {
        return Ok(());
    }
    let operation_id = plan
        .record
        .confirmation_operation_id
        .as_ref()
        .ok_or_else(|| {
            StoreError::Integrity("non-prepared retention plan has no confirmation".to_owned())
        })?;
    let operation = operations.get(operation_id.as_str()).ok_or_else(|| {
        StoreError::Integrity(format!(
            "retention plan {} references a missing confirmation operation",
            plan.record.plan_id.as_str()
        ))
    })?;
    if operation.plan_id.as_ref() != Some(&plan.record.plan_id) {
        return Err(StoreError::Integrity(format!(
            "retention plan {} confirmation references another plan",
            plan.record.plan_id.as_str()
        )));
    }
    let state_matches = match (&plan.record.state, operation.status, &operation.result) {
        (
            RetentionPlanState::Pruning,
            RetentionOperationStatus::Pruning,
            RetentionOperationResult::Retention {
                result:
                    RetentionMutationResult::Pruning {
                        recoverable_state, ..
                    },
            },
        ) => Some(*recoverable_state) == plan.record.recoverable_state,
        (
            RetentionPlanState::Pruned,
            RetentionOperationStatus::Pruning,
            RetentionOperationResult::Retention {
                result:
                    RetentionMutationResult::Pruning {
                        recoverable_state: RetentionRecoverableState::ReadyToCommit,
                        ..
                    },
            },
        ) => plan.record.physical_reclamation_pending,
        (
            RetentionPlanState::Pruned,
            RetentionOperationStatus::Committed,
            RetentionOperationResult::Retention {
                result: RetentionMutationResult::Pruned { .. },
            },
        ) => ensure_committed_pruned_result_matches_plan(plan, operation).is_ok(),
        _ => false,
    };
    if !state_matches {
        return Err(StoreError::Integrity(format!(
            "retention plan {} disagrees with its confirmation result",
            plan.record.plan_id.as_str()
        )));
    }
    Ok(())
}
