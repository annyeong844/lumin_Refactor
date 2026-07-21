use std::collections::BTreeSet;
use std::path::{Component, Path};

use lumin_evidence::{
    RetentionItemKind, RetentionMutationResult, RetentionOperationKind, RetentionOperationRecord,
    RetentionOperationResult, RetentionOperationStatus, RetentionPlanState,
    RetentionRecoverableState,
};

use crate::StoreError;
use crate::namespace::records::ManagedStateParentKind;

use super::{StoredRetentionPlan, canonical_content_identity};

pub(crate) fn validate_plan(plan: &StoredRetentionPlan) -> Result<(), StoreError> {
    if plan.record.schema_version != super::PLAN_SCHEMA {
        return Err(StoreError::Integrity(
            "retention plan schema is unsupported".to_owned(),
        ));
    }
    require_canonical_collections(plan)?;
    if canonical_content_identity(&plan.record)? != plan.record.content_identity {
        return Err(StoreError::Integrity(
            "retention plan content identity is invalid".to_owned(),
        ));
    }
    match plan.record.state {
        RetentionPlanState::Prepared
            if plan.progress.is_none()
                && plan.record.confirmation_operation_id.is_none()
                && plan.record.recoverable_state.is_none()
                && plan.record.tombstone_identity.is_none()
                && !plan.record.physical_reclamation_pending =>
        {
            Ok(())
        }
        RetentionPlanState::Pruning
            if plan.record.confirmation_operation_id.is_some()
                && plan.record.recoverable_state.is_some()
                && plan.record.tombstone_identity.is_none() =>
        {
            validate_progress(plan, true)
        }
        RetentionPlanState::Pruned
            if plan.record.confirmation_operation_id.is_some()
                && plan.record.recoverable_state.is_none()
                && plan.record.tombstone_identity.is_some() =>
        {
            validate_progress(plan, false)
        }
        _ => Err(StoreError::Integrity(
            "retention plan lifecycle fields are incoherent".to_owned(),
        )),
    }
}

pub(crate) fn validate_retention_operation(
    operation: &RetentionOperationRecord,
) -> Result<(), StoreError> {
    if operation.schema_version != super::OPERATION_SCHEMA {
        return Err(StoreError::Integrity(format!(
            "retention operation {} has unsupported schema",
            operation.operation_id.as_str()
        )));
    }
    let coherent = match (
        operation.kind,
        operation.status,
        operation.plan_id.as_ref(),
        &operation.result,
    ) {
        (
            RetentionOperationKind::RunPin,
            RetentionOperationStatus::Committed,
            None,
            RetentionOperationResult::PinCreated { pin },
        ) => {
            pin.created_operation_id == operation.operation_id && pin.removed_operation_id.is_none()
        }
        (
            RetentionOperationKind::RunUnpin,
            RetentionOperationStatus::Committed,
            None,
            RetentionOperationResult::PinRemoved { .. },
        ) => true,
        (
            RetentionOperationKind::RunPrunePlan | RetentionOperationKind::GatePrunePlan,
            RetentionOperationStatus::Committed,
            Some(plan_id),
            RetentionOperationResult::Retention {
                result:
                    RetentionMutationResult::Prepared {
                        plan_id: result_plan_id,
                        ..
                    },
            },
        ) => plan_id == result_plan_id,
        (
            RetentionOperationKind::RunPruneConfirm | RetentionOperationKind::GatePruneConfirm,
            RetentionOperationStatus::Pruning,
            Some(plan_id),
            RetentionOperationResult::Retention {
                result:
                    RetentionMutationResult::Pruning {
                        plan_id: result_plan_id,
                        ..
                    },
            },
        ) => plan_id == result_plan_id,
        (
            RetentionOperationKind::RunPruneConfirm | RetentionOperationKind::GatePruneConfirm,
            RetentionOperationStatus::Stale,
            Some(plan_id),
            RetentionOperationResult::Retention {
                result:
                    RetentionMutationResult::Stale {
                        plan_id: result_plan_id,
                        changed_inputs,
                    },
            },
        ) => plan_id == result_plan_id && canonical_changed_inputs(changed_inputs),
        (
            RetentionOperationKind::RunPruneConfirm | RetentionOperationKind::GatePruneConfirm,
            RetentionOperationStatus::Committed,
            Some(plan_id),
            RetentionOperationResult::Retention {
                result:
                    RetentionMutationResult::Pruned {
                        plan_id: result_plan_id,
                        ..
                    },
            },
        ) => plan_id == result_plan_id,
        _ => false,
    };
    if !coherent {
        return Err(StoreError::Integrity(format!(
            "retention operation {} has incoherent kind, status, plan, or result",
            operation.operation_id.as_str()
        )));
    }
    Ok(())
}

fn canonical_changed_inputs(changed_inputs: &[String]) -> bool {
    matches!(
        changed_inputs
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .as_slice(),
        ["plan-items"] | ["plan-exclusions"] | ["plan-items", "plan-exclusions"]
    )
}

fn require_canonical_collections(plan: &StoredRetentionPlan) -> Result<(), StoreError> {
    let mut sorted_items = plan.record.items.clone();
    sorted_items.sort();
    if sorted_items != plan.record.items {
        return Err(StoreError::Integrity(
            "retention plan items are not canonically ordered".to_owned(),
        ));
    }
    if plan
        .record
        .items
        .iter()
        .map(|item| (item.kind, item.record_id.as_str()))
        .collect::<BTreeSet<_>>()
        .len()
        != plan.record.items.len()
    {
        return Err(StoreError::Integrity(
            "retention plan contains duplicate item identities".to_owned(),
        ));
    }
    let mut sorted_exclusions = plan.record.exclusions.clone();
    sorted_exclusions.sort();
    if sorted_exclusions != plan.record.exclusions {
        return Err(StoreError::Integrity(
            "retention plan exclusions are not canonically ordered".to_owned(),
        ));
    }
    if plan.record.exclusions.iter().collect::<BTreeSet<_>>().len() != plan.record.exclusions.len()
    {
        return Err(StoreError::Integrity(
            "retention plan contains duplicate exclusions".to_owned(),
        ));
    }
    Ok(())
}

fn validate_progress(plan: &StoredRetentionPlan, pruning: bool) -> Result<(), StoreError> {
    let progress = plan.progress.as_ref().ok_or_else(|| {
        StoreError::Integrity("non-prepared retention plan has no progress".to_owned())
    })?;
    if progress.trash_parent_binding.kind != ManagedStateParentKind::Trash {
        return Err(StoreError::Integrity(
            "retention progress has the wrong trash parent kind".to_owned(),
        ));
    }
    let expected_source_kinds = progress
        .moves
        .iter()
        .map(|movement| movement.source_parent)
        .collect::<BTreeSet<_>>();
    let actual_source_kinds = progress
        .source_parent_bindings
        .iter()
        .map(|binding| binding.kind)
        .collect::<BTreeSet<_>>();
    if expected_source_kinds != actual_source_kinds
        || actual_source_kinds.len() != progress.source_parent_bindings.len()
        || !progress
            .source_parent_bindings
            .windows(2)
            .all(|pair| pair[0].kind < pair[1].kind)
    {
        return Err(StoreError::Integrity(
            "retention progress source parent bindings are not the exact ordered set".to_owned(),
        ));
    }
    if progress.moves.is_empty() != progress.trash_directory.is_none() {
        return Err(StoreError::Integrity(
            "retention progress trash binding does not match its payload moves".to_owned(),
        ));
    }
    if plan.record.physical_reclamation_pending && progress.moves.is_empty() {
        return Err(StoreError::Integrity(
            "payload-free retention plan claims pending reclamation".to_owned(),
        ));
    }
    if pruning {
        let expected_pending = !progress.moves.is_empty();
        if plan.record.physical_reclamation_pending != expected_pending {
            return Err(StoreError::Integrity(
                "pruning reclamation state disagrees with its payload moves".to_owned(),
            ));
        }
        match (plan.record.recoverable_state, progress.moves.is_empty()) {
            (Some(RetentionRecoverableState::ReadyToCommit), _)
            | (Some(RetentionRecoverableState::MovingPayloads), false) => {}
            _ => {
                return Err(StoreError::Integrity(
                    "pruning recoverable state disagrees with its payload moves".to_owned(),
                ));
            }
        }
    }
    validate_moves(plan)
}

fn validate_moves(plan: &StoredRetentionPlan) -> Result<(), StoreError> {
    let progress = plan.progress.as_ref().ok_or_else(|| {
        StoreError::Integrity("non-prepared retention plan has no progress".to_owned())
    })?;
    let mut move_keys = BTreeSet::new();
    let mut trash_children = BTreeSet::new();
    for movement in &progress.moves {
        if !move_keys.insert((movement.kind, movement.record_id.as_str()))
            || !trash_children.insert(movement.trash_child.as_str())
        {
            return Err(StoreError::Integrity(
                "retention progress contains duplicate payload moves".to_owned(),
            ));
        }
        require_single_component(&movement.source_child, "retention source child")?;
        require_single_component(&movement.trash_child, "retention trash child")?;
        let item = plan
            .record
            .items
            .iter()
            .find(|item| item.kind == movement.kind && item.record_id == movement.record_id)
            .ok_or_else(|| {
                StoreError::Integrity(format!(
                    "retention payload move has no plan item: {}",
                    movement.record_id
                ))
            })?;
        require_source_mapping(
            movement.kind,
            &movement.record_id,
            movement.source_parent,
            &movement.source_child,
        )?;
        let expected_trash_child = format!(
            "{:02}-{:016x}-{}",
            movement.kind.rank(),
            item.owning_sequence,
            &lumin_model::digest_hex(movement.record_id.as_bytes())[..16]
        );
        if movement.trash_child != expected_trash_child {
            return Err(StoreError::Integrity(format!(
                "retention payload move has a noncanonical trash name: {}",
                movement.record_id
            )));
        }
    }
    Ok(())
}

fn require_source_mapping(
    kind: RetentionItemKind,
    record_id: &str,
    parent: ManagedStateParentKind,
    child: &str,
) -> Result<(), StoreError> {
    let expected = match kind {
        RetentionItemKind::Attempt => (ManagedStateParentKind::Attempts, record_id),
        RetentionItemKind::Run => (ManagedStateParentKind::Runs, record_id),
        RetentionItemKind::OrphanPayload => {
            let (prefix, expected_child) = record_id.split_once('/').ok_or_else(|| {
                StoreError::Integrity("orphan payload identity has no parent".to_owned())
            })?;
            let expected_parent = match prefix {
                "attempts" => ManagedStateParentKind::Attempts,
                "runs" => ManagedStateParentKind::Runs,
                _ => {
                    return Err(StoreError::Integrity(format!(
                        "unsupported orphan payload parent: {prefix}"
                    )));
                }
            };
            (expected_parent, expected_child)
        }
        _ => {
            return Err(StoreError::Integrity(format!(
                "unsupported physical retention item: {record_id}"
            )));
        }
    };
    if (parent, child) != expected {
        return Err(StoreError::Integrity(format!(
            "retention payload source mapping changed: {record_id}"
        )));
    }
    Ok(())
}

fn require_single_component(value: &str, label: &str) -> Result<(), StoreError> {
    let mut components = Path::new(value).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(StoreError::Integrity(format!(
            "{label} must be one normal path component"
        )));
    }
    Ok(())
}
