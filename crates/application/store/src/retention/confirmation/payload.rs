mod migration;
mod trash;

use std::fs;
use std::path::Path;

use lumin_evidence::{RetentionItemKind, RetentionPlanItem, RetentionPlanRecord};
use lumin_model::digest_hex;

use crate::namespace::records::ManagedStateParentKind;
use crate::namespace::{EntryAccess, EntryKind, HeldEntry, NamespaceGuard};
use crate::{StoreError, io_error};

use super::super::records::{
    RetentionPayloadMove, RetentionProgress, StoredRetentionPlan, TrashDirectoryBinding,
};

pub(super) fn prepare_progress(
    guard: &NamespaceGuard,
    plan: &RetentionPlanRecord,
) -> Result<RetentionProgress, StoreError> {
    let mut moves = Vec::new();
    for item in &plan.items {
        let Some((parent, source_child)) = payload_source(item.kind, &item.record_id)? else {
            continue;
        };
        let held = guard.open_managed_child_directory(parent, &source_child, "retention source")?;
        moves.push(RetentionPayloadMove {
            kind: item.kind,
            record_id: item.record_id.clone(),
            source_parent: parent,
            source_child,
            trash_child: format!(
                "{:02}-{:016x}-{}",
                item.kind.rank(),
                item.owning_sequence,
                &digest_hex(item.record_id.as_bytes())[..16]
            ),
            physical_identity: held.identity().clone(),
        });
    }
    moves.sort_by(|left, right| {
        left.kind
            .rank()
            .cmp(&right.kind.rank())
            .then_with(|| left.record_id.cmp(&right.record_id))
    });
    for pair in moves.windows(2) {
        if pair[0].trash_child == pair[1].trash_child {
            return Err(StoreError::Integrity(
                "retention payload move names are not unique".to_owned(),
            ));
        }
    }
    let mut source_parent_bindings = moves
        .iter()
        .map(|item| guard.managed_parent_binding(item.source_parent).cloned())
        .collect::<Result<Vec<_>, _>>()?;
    source_parent_bindings.sort_by_key(|binding| binding.kind);
    source_parent_bindings.dedup_by_key(|binding| binding.kind);
    Ok(RetentionProgress {
        source_parent_bindings,
        trash_parent_binding: guard
            .managed_parent_binding(ManagedStateParentKind::Trash)?
            .clone(),
        trash_directory: None,
        moves,
    })
}

pub(super) fn bind_trash_directory(
    guard: &NamespaceGuard,
    plan: &StoredRetentionPlan,
) -> Result<TrashDirectoryBinding, StoreError> {
    trash::bind_directory(guard, plan)
}

pub(super) fn move_payloads(
    guard: &NamespaceGuard,
    plan: &StoredRetentionPlan,
) -> Result<(), StoreError> {
    let progress = plan
        .progress
        .as_ref()
        .ok_or_else(|| StoreError::Integrity("pruning plan has no progress record".to_owned()))?;
    trash::validate_parent_bindings(guard, progress)?;
    let held_trash = trash::open_bound(guard, plan, progress)?;
    for movement in &progress.moves {
        trash::validate_parent_bindings(guard, progress)?;
        held_trash.validate(plan, progress)?;
        let source = guard.managed_child_path(movement.source_parent, &movement.source_child)?;
        let destination = held_trash.path.join(&movement.trash_child);
        let source_exists = trash::entry_exists(&source)?;
        let destination_exists = trash::entry_exists(&destination)?;
        match (source_exists, destination_exists) {
            (true, false) => {
                let held = guard.open_managed_child_directory(
                    movement.source_parent,
                    &movement.source_child,
                    "retention source payload",
                )?;
                trash::require_identity(
                    &held,
                    &movement.physical_identity,
                    "retention source payload",
                )?;
                validate_payload_content(&source, movement, &plan.record.items)?;
                fs::rename(&source, &destination).map_err(io_error)?;
                guard
                    .managed_parent_entry(movement.source_parent)?
                    .sync_directory()?;
                held_trash.directory.sync_directory()?;
                held.validate_path(
                    &destination,
                    EntryKind::Directory,
                    EntryAccess::ReadOnly,
                    false,
                    "retention moved payload",
                )?;
                if trash::entry_exists(&source)? {
                    return Err(StoreError::Integrity(
                        "retention payload remained at its source after move".to_owned(),
                    ));
                }
            }
            (false, true) => {
                let held = HeldEntry::open(
                    &destination,
                    EntryKind::Directory,
                    EntryAccess::ReadOnly,
                    false,
                    "retention moved payload",
                )?;
                trash::require_identity(
                    &held,
                    &movement.physical_identity,
                    "retention moved payload",
                )?;
                validate_payload_content(&destination, movement, &plan.record.items)?;
            }
            (true, true) => {
                return Err(StoreError::Integrity(format!(
                    "retention payload exists at both source and trash: {}",
                    movement.record_id
                )));
            }
            (false, false) => {
                return Err(StoreError::Integrity(format!(
                    "retention payload is missing from source and trash: {}",
                    movement.record_id
                )));
            }
        }
        trash::validate_parent_bindings(guard, progress)?;
        held_trash.validate(plan, progress)?;
    }
    Ok(())
}

pub(super) fn reclaim(
    guard: &NamespaceGuard,
    plan: &StoredRetentionPlan,
) -> Result<(), StoreError> {
    trash::reclaim(guard, plan)
}

pub(in crate::retention) fn validate_migration_state(
    guard: &NamespaceGuard,
    plan: &StoredRetentionPlan,
) -> Result<std::collections::BTreeMap<String, std::path::PathBuf>, StoreError> {
    migration::validate(guard, plan)
}

fn validate_payload_content(
    path: &Path,
    movement: &RetentionPayloadMove,
    items: &[RetentionPlanItem],
) -> Result<(), StoreError> {
    let item = items
        .iter()
        .find(|item| item.kind == movement.kind && item.record_id == movement.record_id)
        .ok_or_else(|| {
            StoreError::Integrity(format!(
                "retention move has no plan item: {}",
                movement.record_id
            ))
        })?;
    match movement.kind {
        RetentionItemKind::Attempt => {
            let bytes = fs::read(path.join("attempt.json")).map_err(io_error)?;
            require_payload_hash(&bytes, item, "attempt envelope")
        }
        RetentionItemKind::Run => {
            let evidence = items
                .iter()
                .find(|candidate| {
                    candidate.kind == RetentionItemKind::Evidence
                        && candidate.record_id == format!("run:{}/evidence", movement.record_id)
                })
                .ok_or_else(|| {
                    StoreError::Integrity(format!(
                        "run {} has no evidence plan item",
                        movement.record_id
                    ))
                })?;
            let bytes = fs::read(path.join("evidence.store")).map_err(io_error)?;
            require_payload_hash(&bytes, evidence, "run evidence store")
        }
        RetentionItemKind::OrphanPayload => {
            let (identity, byte_count) = super::super::planning::orphan_payload_identity(path)?;
            if identity != item.identity_sha256 || byte_count != item.byte_count {
                return Err(StoreError::Integrity(format!(
                    "orphan payload changed before retention move: {}",
                    movement.record_id
                )));
            }
            Ok(())
        }
        _ => Err(StoreError::Integrity(format!(
            "unsupported physical retention item: {}",
            movement.record_id
        ))),
    }
}

fn require_payload_hash(
    bytes: &[u8],
    item: &RetentionPlanItem,
    label: &str,
) -> Result<(), StoreError> {
    if digest_hex(bytes) != item.identity_sha256 || bytes.len() as u64 != item.byte_count {
        return Err(StoreError::Integrity(format!(
            "{label} changed before retention move"
        )));
    }
    Ok(())
}

fn payload_source(
    kind: RetentionItemKind,
    record_id: &str,
) -> Result<Option<(ManagedStateParentKind, String)>, StoreError> {
    match kind {
        RetentionItemKind::Attempt => Ok(Some((
            ManagedStateParentKind::Attempts,
            record_id.to_owned(),
        ))),
        RetentionItemKind::Run => Ok(Some((ManagedStateParentKind::Runs, record_id.to_owned()))),
        RetentionItemKind::OrphanPayload => {
            let (parent, child) = record_id.split_once('/').ok_or_else(|| {
                StoreError::Integrity("orphan payload identity has no parent".to_owned())
            })?;
            let parent = match parent {
                "attempts" => ManagedStateParentKind::Attempts,
                "runs" => ManagedStateParentKind::Runs,
                _ => {
                    return Err(StoreError::Integrity(format!(
                        "unsupported orphan payload parent: {parent}"
                    )));
                }
            };
            Ok(Some((parent, child.to_owned())))
        }
        _ => Ok(None),
    }
}
