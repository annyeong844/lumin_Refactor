use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::namespace::records::ManagedStateParentKind;
use crate::namespace::{EntryAccess, EntryKind, HeldEntry, NamespaceGuard};
use crate::{StoreError, io_error};

#[cfg(feature = "retention-test-crash")]
use super::super::super::super::crash::{CrashPointSequence, RetentionCrashPoint, hit};
use super::super::super::super::records::StoredRetentionPlan;
use super::{
    TRASH_ANCHOR, TrashReclaimState, entry_exists, reclaim_state, validate_parent_bindings,
};

pub(super) fn run(guard: &NamespaceGuard, plan: &StoredRetentionPlan) -> Result<(), StoreError> {
    let progress = plan
        .progress
        .as_ref()
        .ok_or_else(|| StoreError::Integrity("pruned plan has no progress record".to_owned()))?;
    if progress.moves.is_empty() {
        return Ok(());
    }
    validate_parent_bindings(guard, progress)?;
    match reclaim_state(guard, plan, progress)? {
        TrashReclaimState::Absent => {}
        TrashReclaimState::AnchorRemoved { path, directory } => {
            drop(directory);
            fs::remove_dir(&path).map_err(io_error)?;
            #[cfg(feature = "retention-test-crash")]
            hit(RetentionCrashPoint::AfterReclaimDirectoryRemoved);
        }
        TrashReclaimState::Bound(bound) => reclaim_bound(bound, progress)?,
    }
    finish_reclamation(guard)?;
    #[cfg(feature = "retention-test-crash")]
    hit(RetentionCrashPoint::AfterReclaimParentFlushed);
    Ok(())
}

fn reclaim_bound(
    bound: super::BoundTrash,
    progress: &super::super::super::super::records::RetentionProgress,
) -> Result<(), StoreError> {
    validate_tree(&bound.path)?;
    let expected = progress
        .moves
        .iter()
        .map(|movement| movement.trash_child.as_str())
        .collect::<BTreeSet<_>>();
    for entry in fs::read_dir(&bound.path).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| {
            StoreError::Integrity("retention trash contains a non-UTF-8 child name".to_owned())
        })?;
        if name != TRASH_ANCHOR && !expected.contains(name) {
            return Err(StoreError::Integrity(format!(
                "retention trash contains an unplanned child: {}",
                entry.path().display()
            )));
        }
    }
    #[cfg(feature = "retention-test-crash")]
    let mut crash_sequence = CrashPointSequence::default();
    for child in expected {
        let path = bound.path.join(child);
        if entry_exists(&path)? {
            fs::remove_dir_all(&path).map_err(io_error)?;
        }
        #[cfg(feature = "retention-test-crash")]
        crash_sequence.hit_indexed(RetentionCrashPoint::ReclaimChildRemoved);
    }
    bound.directory.sync_directory()?;
    #[cfg(feature = "retention-test-crash")]
    hit(RetentionCrashPoint::AfterReclaimPayloadsFlushed);
    let remaining = fs::read_dir(&bound.path)
        .map_err(io_error)?
        .map(|entry| entry.map(|entry| entry.file_name()).map_err(io_error))
        .collect::<Result<Vec<_>, _>>()?;
    if remaining.as_slice() != [std::ffi::OsString::from(TRASH_ANCHOR)] {
        return Err(StoreError::Integrity(
            "retention trash payload children survived reclamation".to_owned(),
        ));
    }
    let path = bound.path;
    let directory = bound.directory;
    let anchor = bound.anchor;
    drop(anchor);
    fs::remove_file(path.join(TRASH_ANCHOR)).map_err(io_error)?;
    directory.sync_directory()?;
    #[cfg(feature = "retention-test-crash")]
    hit(RetentionCrashPoint::AfterReclaimAnchorRemoved);
    drop(directory);
    fs::remove_dir(&path).map_err(io_error)?;
    #[cfg(feature = "retention-test-crash")]
    hit(RetentionCrashPoint::AfterReclaimDirectoryRemoved);
    Ok(())
}

fn finish_reclamation(guard: &NamespaceGuard) -> Result<(), StoreError> {
    guard
        .managed_parent_entry(ManagedStateParentKind::Trash)?
        .sync_directory()?;
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
