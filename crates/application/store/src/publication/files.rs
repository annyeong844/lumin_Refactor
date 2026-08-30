use std::fs;
use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::namespace::{
    EntryAccess, EntryKind, HeldEntry, entry_exists, move_entry_noreplace, replace_entry_atomic,
    same_volume_and_mount,
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
    write_json_with_hooks(path, parent, label, value, || {}, || Ok(()), || {})
}

pub(super) fn write_json_with_hooks<T: Serialize>(
    path: &Path,
    parent: &HeldEntry,
    label: &str,
    value: &T,
    after_pending: impl FnOnce(),
    before_replace: impl FnOnce() -> Result<(), StoreError>,
    after_replace: impl FnOnce(),
) -> Result<(), StoreError> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(serialization_error)?;
    bytes.push(b'\n');
    let pending = path.with_extension("json.pending");
    remove_pending(&pending, parent, label)?;

    let pending_entry = HeldEntry::create_new_movable(&pending, label)?;
    require_parent_volume(&pending_entry, parent, label)?;
    pending_entry.replace_contents(&bytes)?;
    after_pending();

    let replace_existing = entry_exists(path)?;
    let current = replace_existing
        .then(|| HeldEntry::open(path, EntryKind::RegularFile, EntryAccess::Move, true, label))
        .transpose()?;
    if let Some(current) = current.as_ref() {
        require_parent_volume(current, parent, label)?;
    }
    before_replace()?;
    let pending_name = pending.file_name().ok_or_else(|| {
        StoreError::Integrity(format!("{label} pending path has no final component"))
    })?;
    let published_name = path
        .file_name()
        .ok_or_else(|| StoreError::Integrity(format!("{label} path has no final component")))?;
    if replace_existing {
        replace_entry_atomic(parent, pending_name, &pending_entry, published_name)?;
    } else {
        move_entry_noreplace(parent, pending_name, &pending_entry, parent, published_name)?;
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
    if published.identity() != pending_entry.identity() || published.read_all()? != bytes {
        return Err(StoreError::Integrity(format!(
            "{label} changed during durable publication"
        )));
    }
    published.validate_path(
        path,
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        true,
        label,
    )?;
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
    if !same_volume_and_mount(entry, parent) {
        return Err(StoreError::Integrity(format!(
            "{label} must remain on its parent volume and mount"
        )));
    }
    Ok(())
}

#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
mod tests {
    use super::*;

    #[test]
    fn substituted_pending_name_never_authenticates_the_published_path()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let parent = HeldEntry::open(
            root.path(),
            EntryKind::Directory,
            EntryAccess::ReadOnly,
            false,
            "test publication parent",
        )?;
        let published = root.path().join("latest.json");
        let pending = root.path().join("latest.json.pending");
        let detached = root.path().join("held-pending.detached");
        fs::write(&published, b"{\"old\":true}\n")?;
        let value = serde_json::json!({ "expected": true });
        let substitute = b"{\"substitute\":true}\n";

        let result = write_json_with_hooks(
            &published,
            &parent,
            "test latest pointer",
            &value,
            || {},
            || {
                fs::rename(&pending, &detached).map_err(io_error)?;
                fs::write(&pending, substitute).map_err(io_error)?;
                Ok(())
            },
            || {},
        );

        assert!(matches!(
            result,
            Err(StoreError::Integrity(message))
                if message == "test latest pointer changed during durable publication"
        ));
        assert_eq!(fs::read(&published)?, substitute);
        let mut expected = serde_json::to_vec_pretty(&value)?;
        expected.push(b'\n');
        assert_eq!(fs::read(detached)?, expected);
        Ok(())
    }
}
