use std::fs;
use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::namespace::{
    EntryAccess, EntryKind, HeldEntry, entry_exists, publish_file_atomic, replace_file_atomic,
    same_volume,
};
use crate::{StoreError, io_error, serialization_error};

pub(super) fn read_json<T: DeserializeOwned>(
    path: &Path,
    parent: &HeldEntry,
    label: &str,
) -> Result<T, StoreError> {
    let entry = HeldEntry::open(
        path,
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        true,
        label,
    )?;
    require_parent_volume(&entry, parent, label)?;
    serde_json::from_slice(&entry.read_all()?).map_err(serialization_error)
}

pub(super) fn write_json<T: Serialize>(
    path: &Path,
    parent: &HeldEntry,
    label: &str,
    value: &T,
) -> Result<(), StoreError> {
    write_json_with_hooks(path, parent, label, value, || {}, || {})
}

pub(super) fn write_json_with_hooks<T: Serialize>(
    path: &Path,
    parent: &HeldEntry,
    label: &str,
    value: &T,
    after_pending: impl FnOnce(),
    after_replace: impl FnOnce(),
) -> Result<(), StoreError> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(serialization_error)?;
    bytes.push(b'\n');
    let pending = path.with_extension("json.pending");
    remove_pending(&pending, parent, label)?;

    let pending_entry = HeldEntry::create_new(&pending, label)?;
    require_parent_volume(&pending_entry, parent, label)?;
    pending_entry.replace_contents(&bytes)?;
    after_pending();
    drop(pending_entry);

    if entry_exists(path)? {
        let current = HeldEntry::open(
            path,
            EntryKind::RegularFile,
            EntryAccess::ReadWrite,
            true,
            label,
        )?;
        require_parent_volume(&current, parent, label)?;
        drop(current);
        replace_file_atomic(path, &pending)?;
    } else {
        publish_file_atomic(path, &pending)?;
    }
    after_replace();
    parent.sync_directory()?;

    let published = HeldEntry::open(
        path,
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        true,
        label,
    )?;
    require_parent_volume(&published, parent, label)?;
    if published.read_all()? != bytes {
        return Err(StoreError::Integrity(format!(
            "{label} changed during durable publication"
        )));
    }
    Ok(())
}

pub(super) fn validate_and_remove_pending<T: DeserializeOwned>(
    path: &Path,
    parent: &HeldEntry,
    label: &str,
    validate: impl FnOnce(&T) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    if !entry_exists(path)? {
        return Ok(());
    }
    let value = read_json(path, parent, label)?;
    validate(&value)?;
    remove_pending(path, parent, label)
}

pub(super) fn remove_pending(
    path: &Path,
    parent: &HeldEntry,
    label: &str,
) -> Result<(), StoreError> {
    if !entry_exists(path)? {
        return Ok(());
    }
    let entry = HeldEntry::open(
        path,
        EntryKind::RegularFile,
        EntryAccess::ReadWrite,
        true,
        label,
    )?;
    require_parent_volume(&entry, parent, label)?;
    drop(entry);
    fs::remove_file(path).map_err(io_error)?;
    parent.sync_directory()
}

pub(super) fn require_parent_volume(
    entry: &HeldEntry,
    parent: &HeldEntry,
    label: &str,
) -> Result<(), StoreError> {
    if !same_volume(entry.identity(), parent.identity()) {
        return Err(StoreError::Integrity(format!(
            "{label} must remain on its parent volume"
        )));
    }
    Ok(())
}
