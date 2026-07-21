use std::fs;
use std::path::Path;

use crate::namespace::records::ManagedStateParentKind;
use crate::namespace::{EntryAccess, EntryKind, HeldEntry, NamespaceGuard};
use crate::{StoreError, io_error};

use super::super::super::super::records::StoredRetentionPlan;
use super::{entry_exists, open_bound, validate_parent_bindings};

pub(super) fn run(guard: &NamespaceGuard, plan: &StoredRetentionPlan) -> Result<(), StoreError> {
    let progress = plan
        .progress
        .as_ref()
        .ok_or_else(|| StoreError::Integrity("pruned plan has no progress record".to_owned()))?;
    if progress.moves.is_empty() {
        return Ok(());
    }
    validate_parent_bindings(guard, progress)?;
    let bound = open_bound(guard, plan, progress)?;
    validate_tree(&bound.path)?;
    let path = bound.path.clone();
    drop(bound);
    fs::remove_dir_all(&path).map_err(io_error)?;
    guard
        .managed_parent_entry(ManagedStateParentKind::Trash)?
        .sync_directory()?;
    if entry_exists(&path)? {
        return Err(StoreError::Integrity(
            "retention trash directory survived reclamation".to_owned(),
        ));
    }
    guard.validate_bound_entries()
}

fn validate_tree(path: &Path) -> Result<(), StoreError> {
    for entry in fs::read_dir(path).map_err(io_error)? {
        let path = entry.map_err(io_error)?.path();
        let metadata = fs::symlink_metadata(&path).map_err(io_error)?;
        if metadata.file_type().is_symlink() {
            return Err(StoreError::Integrity(format!(
                "retention trash contains a symbolic link: {}",
                path.display()
            )));
        }
        if metadata.is_dir() {
            validate_entry(
                &path,
                EntryKind::Directory,
                false,
                "retention trash directory",
            )?;
            validate_tree(&path)?;
        } else if metadata.is_file() {
            validate_entry(&path, EntryKind::RegularFile, true, "retention trash file")?;
        } else {
            return Err(StoreError::Integrity(format!(
                "retention trash contains an unsupported entry: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn validate_entry(
    path: &Path,
    kind: EntryKind,
    one_link: bool,
    label: &str,
) -> Result<(), StoreError> {
    let held = HeldEntry::open(path, kind, EntryAccess::ReadOnly, one_link, label)?;
    held.validate_path(path, kind, EntryAccess::ReadOnly, one_link, label)
}
