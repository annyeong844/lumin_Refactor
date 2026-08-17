use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use lumin_model::PhysicalFileIdentity;

use crate::StoreError;

use super::{EntryAccess, EntryKind, HeldEntry, NamespaceGuard, same_volume_and_mount};

pub(super) fn collect_identities(
    guard: &NamespaceGuard,
) -> Result<BTreeSet<PhysicalFileIdentity>, StoreError> {
    guard.validate_bound_entries()?;
    let first = collect_snapshot(&guard.state.state_dir)?;
    let second = collect_snapshot(&guard.state.state_dir)?;
    if first != second {
        return Err(StoreError::Integrity(
            "reserved state topology changed while collecting physical identities".to_owned(),
        ));
    }
    guard.validate_bound_entries()?;
    Ok(second)
}

fn collect_snapshot(root: &Path) -> Result<BTreeSet<PhysicalFileIdentity>, StoreError> {
    let mut identities = BTreeSet::new();
    collect_tree(root, None, &mut identities)?;
    Ok(identities)
}

fn collect_tree(
    path: &Path,
    parent: Option<&HeldEntry>,
    identities: &mut BTreeSet<PhysicalFileIdentity>,
) -> Result<(), StoreError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| changed(path, error))?;
    let kind = if metadata.file_type().is_dir() {
        EntryKind::Directory
    } else if metadata.file_type().is_file() {
        EntryKind::RegularFile
    } else {
        return Err(StoreError::Integrity(format!(
            "reserved state object is redirected or unsupported: {}",
            path.display()
        )));
    };
    let held = HeldEntry::open(
        path,
        kind,
        EntryAccess::ReadOnly,
        false,
        "reserved state object",
    )?;
    if parent.is_some_and(|parent| !same_volume_and_mount(parent, &held)) {
        return Err(StoreError::Integrity(format!(
            "reserved state mount crossing detected at {}",
            path.display()
        )));
    }
    identities.insert(held.identity().clone());
    let is_directory = matches!(kind, EntryKind::Directory);
    if !is_directory {
        held.validate_path(
            path,
            kind,
            EntryAccess::ReadOnly,
            false,
            "reserved state object",
        )?;
        return Ok(());
    }

    let entries = fs::read_dir(path).map_err(|error| changed(path, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| changed(path, error))?;
        collect_tree(&entry.path(), Some(&held), identities)?;
    }
    held.validate_path(
        path,
        kind,
        EntryAccess::ReadOnly,
        false,
        "reserved state object",
    )?;
    Ok(())
}

fn changed(path: &Path, error: std::io::Error) -> StoreError {
    StoreError::Integrity(format!(
        "reserved state topology changed while inspecting {}: {error}",
        path.display()
    ))
}
