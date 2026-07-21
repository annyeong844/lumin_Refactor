use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::namespace::records::ManagedStateParentKind;
use crate::namespace::{EntryAccess, EntryKind, HeldEntry, NamespaceGuard, same_volume};
use crate::{StoreError, io_error};

pub(super) use crate::namespace::entry_exists;

use super::super::super::records::{RetentionProgress, StoredRetentionPlan, TrashDirectoryBinding};

const TRASH_ANCHOR: &str = ".plan-anchor";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrashAnchor {
    schema_version: String,
    repository_id: lumin_model::RepositoryId,
    plan_id: lumin_model::RetentionPlanId,
    trash_nonce: String,
}

pub(super) struct BoundTrash {
    pub(super) path: PathBuf,
    pub(super) directory: HeldEntry,
    anchor: HeldEntry,
}

impl BoundTrash {
    pub(super) fn validate(
        &self,
        plan: &StoredRetentionPlan,
        progress: &RetentionProgress,
    ) -> Result<(), StoreError> {
        let expected = progress.trash_directory.as_ref().ok_or_else(|| {
            StoreError::Integrity("retention trash directory is not durably bound".to_owned())
        })?;
        require_identity(
            &self.directory,
            &expected.directory_physical_identity,
            "retention trash plan directory",
        )?;
        self.directory.validate_path(
            &self.path,
            EntryKind::Directory,
            EntryAccess::ReadOnly,
            false,
            "retention trash plan directory",
        )?;
        require_identity(
            &self.anchor,
            &expected.anchor_physical_identity,
            "retention trash anchor",
        )?;
        self.anchor.validate_path(
            &self.path.join(TRASH_ANCHOR),
            EntryKind::RegularFile,
            EntryAccess::ReadOnly,
            true,
            "retention trash anchor",
        )?;
        if self.anchor.read_all()? != anchor_bytes(plan)? {
            return Err(StoreError::Integrity(
                "retention trash anchor changed after binding".to_owned(),
            ));
        }
        Ok(())
    }
}

pub(super) fn bind_directory(
    guard: &NamespaceGuard,
    plan: &StoredRetentionPlan,
) -> Result<TrashDirectoryBinding, StoreError> {
    let progress = plan
        .progress
        .as_ref()
        .ok_or_else(|| StoreError::Integrity("pruning plan has no progress record".to_owned()))?;
    validate_parent_bindings(guard, progress)?;
    let path = trash_directory_path(guard, plan)?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(StoreError::Integrity(
                "retention trash plan path is not a real directory".to_owned(),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&path).map_err(io_error)?;
            guard
                .managed_parent_entry(ManagedStateParentKind::Trash)?
                .sync_directory()?;
        }
        Err(error) => return Err(io_error(error)),
    }
    let directory = guard.open_managed_child_directory(
        ManagedStateParentKind::Trash,
        plan.record.plan_id.as_str(),
        "retention trash plan directory",
    )?;
    let anchor_path = path.join(TRASH_ANCHOR);
    let expected_bytes = anchor_bytes(plan)?;
    let anchor = match fs::symlink_metadata(&anchor_path) {
        Ok(_) => HeldEntry::open(
            &anchor_path,
            EntryKind::RegularFile,
            EntryAccess::ReadOnly,
            true,
            "retention trash anchor",
        )?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if fs::read_dir(&path).map_err(io_error)?.next().is_some() {
                return Err(StoreError::Integrity(
                    "unbound retention trash directory is not empty".to_owned(),
                ));
            }
            let anchor = HeldEntry::create_new(&anchor_path, "retention trash anchor")?;
            anchor.replace_contents(&expected_bytes)?;
            directory.sync_directory()?;
            anchor
        }
        Err(error) => return Err(io_error(error)),
    };
    if !same_volume(anchor.identity(), directory.identity()) {
        return Err(StoreError::Integrity(
            "retention trash anchor left its plan directory volume".to_owned(),
        ));
    }
    if anchor.read_all()? != expected_bytes {
        return Err(StoreError::Integrity(
            "retention trash anchor disagrees with its plan".to_owned(),
        ));
    }
    directory.validate_path(
        &path,
        EntryKind::Directory,
        EntryAccess::ReadOnly,
        false,
        "retention trash plan directory",
    )?;
    anchor.validate_path(
        &anchor_path,
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        true,
        "retention trash anchor",
    )?;
    Ok(TrashDirectoryBinding {
        directory_physical_identity: directory.identity().clone(),
        anchor_physical_identity: anchor.identity().clone(),
    })
}

pub(super) fn open_bound(
    guard: &NamespaceGuard,
    plan: &StoredRetentionPlan,
    progress: &RetentionProgress,
) -> Result<BoundTrash, StoreError> {
    let path = trash_directory_path(guard, plan)?;
    let directory = guard.open_managed_child_directory(
        ManagedStateParentKind::Trash,
        plan.record.plan_id.as_str(),
        "retention trash plan directory",
    )?;
    let anchor = HeldEntry::open(
        &path.join(TRASH_ANCHOR),
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        true,
        "retention trash anchor",
    )?;
    if !same_volume(anchor.identity(), directory.identity()) {
        return Err(StoreError::Integrity(
            "retention trash anchor left its plan directory volume".to_owned(),
        ));
    }
    let bound = BoundTrash {
        path,
        directory,
        anchor,
    };
    bound.validate(plan, progress)?;
    Ok(bound)
}

pub(super) fn reclaim(
    guard: &NamespaceGuard,
    plan: &StoredRetentionPlan,
) -> Result<(), StoreError> {
    reclaim::run(guard, plan)
}

pub(super) fn validate_parent_bindings(
    guard: &NamespaceGuard,
    progress: &RetentionProgress,
) -> Result<(), StoreError> {
    for expected in &progress.source_parent_bindings {
        if guard.managed_parent_binding(expected.kind)? != expected {
            return Err(StoreError::Integrity(format!(
                "managed source parent binding changed for {}",
                expected.kind.directory_name()
            )));
        }
    }
    if guard.managed_parent_binding(ManagedStateParentKind::Trash)?
        != &progress.trash_parent_binding
    {
        return Err(StoreError::Integrity(
            "managed trash parent binding changed".to_owned(),
        ));
    }
    guard.validate_bound_entries()
}

pub(super) fn require_identity(
    entry: &HeldEntry,
    expected: &lumin_model::PhysicalFileIdentity,
    label: &str,
) -> Result<(), StoreError> {
    if entry.identity() != expected {
        return Err(StoreError::Integrity(format!(
            "{label} physical identity changed"
        )));
    }
    Ok(())
}

fn anchor_bytes(plan: &StoredRetentionPlan) -> Result<Vec<u8>, StoreError> {
    serde_json::to_vec(&TrashAnchor {
        schema_version: "lumin-retention-trash-anchor.v1".to_owned(),
        repository_id: plan.record.repository_id.clone(),
        plan_id: plan.record.plan_id.clone(),
        trash_nonce: plan.trash_nonce.clone(),
    })
    .map_err(crate::serialization_error)
}

fn trash_directory_path(
    guard: &NamespaceGuard,
    plan: &StoredRetentionPlan,
) -> Result<PathBuf, StoreError> {
    guard.managed_child_path(ManagedStateParentKind::Trash, plan.record.plan_id.as_str())
}

mod reclaim;
