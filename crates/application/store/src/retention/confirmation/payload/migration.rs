use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lumin_evidence::{RetentionItemKind, RetentionPlanState};

use crate::StoreError;
use crate::namespace::records::ManagedStateParentKind;
use crate::namespace::{EntryAccess, EntryKind, HeldEntry, NamespaceGuard};

use super::super::super::records::StoredRetentionPlan;
use super::{trash, validate_payload_content};

pub(super) fn validate(
    guard: &NamespaceGuard,
    plan: &StoredRetentionPlan,
) -> Result<BTreeMap<String, PathBuf>, StoreError> {
    let progress = plan.progress.as_ref().ok_or_else(|| {
        StoreError::Integrity("non-prepared retention plan has no progress".to_owned())
    })?;
    trash::validate_parent_bindings(guard, progress)?;
    let trash_path =
        guard.managed_child_path(ManagedStateParentKind::Trash, plan.record.plan_id.as_str())?;
    let bound_trash = bound_trash_if_present(guard, plan, &trash_path)?;
    let mut moved_runs = BTreeMap::new();
    for movement in &progress.moves {
        let source = guard.managed_child_path(movement.source_parent, &movement.source_child)?;
        let destination = trash_path.join(&movement.trash_child);
        let authoritative = authoritative_payload(
            plan,
            &source,
            &destination,
            trash::entry_exists(&source)?,
            trash::entry_exists(&destination)?,
            &movement.record_id,
        )?;
        if let Some(path) = authoritative {
            let held = HeldEntry::open(
                path,
                EntryKind::Directory,
                EntryAccess::ReadOnly,
                false,
                "retention migration payload",
            )?;
            trash::require_identity(
                &held,
                &movement.physical_identity,
                "retention migration payload",
            )?;
            validate_payload_content(path, movement, &plan.record.items)?;
            held.validate_path(
                path,
                EntryKind::Directory,
                EntryAccess::ReadOnly,
                false,
                "retention migration payload",
            )?;
            if movement.kind == RetentionItemKind::Run {
                moved_runs.insert(movement.record_id.clone(), path.to_path_buf());
            }
        }
        if let Some(bound) = &bound_trash {
            bound.validate(plan, progress)?;
        }
    }
    Ok(moved_runs)
}

fn bound_trash_if_present(
    guard: &NamespaceGuard,
    plan: &StoredRetentionPlan,
    trash_path: &Path,
) -> Result<Option<trash::BoundTrash>, StoreError> {
    let progress = plan.progress.as_ref().ok_or_else(|| {
        StoreError::Integrity("non-prepared retention plan has no progress".to_owned())
    })?;
    if progress.moves.is_empty() {
        if trash::entry_exists(trash_path)? {
            return Err(StoreError::Integrity(
                "payload-free retention plan has an unexpected trash directory".to_owned(),
            ));
        }
        return Ok(None);
    }
    match (plan.record.state, plan.record.physical_reclamation_pending) {
        (RetentionPlanState::Pruned, false) => {
            if trash::entry_exists(trash_path)? {
                return Err(StoreError::Integrity(
                    "reclaimed retention plan still has a trash directory".to_owned(),
                ));
            }
            Ok(None)
        }
        (RetentionPlanState::Pruned, true) => match trash::reclaim_state(guard, plan, progress)? {
            trash::TrashReclaimState::Bound(bound) => Ok(Some(bound)),
            trash::TrashReclaimState::Absent | trash::TrashReclaimState::AnchorRemoved { .. } => {
                Ok(None)
            }
        },
        _ => trash::open_bound(guard, plan, progress).map(Some),
    }
}

fn authoritative_payload<'a>(
    plan: &StoredRetentionPlan,
    source: &'a Path,
    destination: &'a Path,
    source_exists: bool,
    destination_exists: bool,
    record_id: &str,
) -> Result<Option<&'a Path>, StoreError> {
    match plan.record.state {
        RetentionPlanState::Pruning if source_exists ^ destination_exists => {
            Ok(Some(if source_exists { source } else { destination }))
        }
        RetentionPlanState::Pruned
            if plan.record.physical_reclamation_pending && !source_exists && destination_exists =>
        {
            Ok(Some(destination))
        }
        RetentionPlanState::Pruned
            if plan.record.physical_reclamation_pending
                && !source_exists
                && !destination_exists =>
        {
            Ok(None)
        }
        RetentionPlanState::Pruned
            if !plan.record.physical_reclamation_pending
                && !source_exists
                && !destination_exists =>
        {
            Ok(None)
        }
        _ => Err(StoreError::Integrity(format!(
            "retention payload {record_id} has incoherent source/trash state"
        ))),
    }
}
