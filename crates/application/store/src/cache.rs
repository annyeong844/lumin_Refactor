use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

use lumin_model::PhysicalFileIdentity;

use crate::namespace::records::ManagedStateParentKind;
use crate::namespace::{EntryAccess, EntryKind, HeldEntry, NamespaceGuard, same_volume_and_mount};
use crate::{RepositoryStore, StoreError, io_error};

const CACHE_ANCHOR: &str = "namespace.anchor";

#[derive(Clone, Copy)]
enum PayloadKind {
    Directory,
    RegularFile,
}

struct ValidatedPayload {
    path: PathBuf,
    kind: PayloadKind,
    identity: PhysicalFileIdentity,
}

impl RepositoryStore {
    /// Remove only disposable cache payload descendants. The cache parent and
    /// its immutable namespace anchor remain protected by the namespace guard.
    pub fn clean_cache_payloads(&self) -> Result<(), StoreError> {
        self.with_exclusive_lock(|guard| {
            let payloads = validate_payload_set(guard)?;
            for payload in payloads {
                guard.mutate(|| remove_payload(guard, &payload))?;
            }
            guard.mutate(|| require_anchor_only(guard))
        })
    }
}

fn validate_payload_set(guard: &NamespaceGuard) -> Result<Vec<ValidatedPayload>, StoreError> {
    let parent = guard.managed_parent_entry(ManagedStateParentKind::Cache)?;
    let parent_path = guard.managed_parent_path(ManagedStateParentKind::Cache);
    let mut paths = fs::read_dir(&parent_path)
        .map_err(io_error)?
        .map(|entry| entry.map(|entry| entry.path()).map_err(io_error))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort_by(|left, right| left.as_os_str().cmp(right.as_os_str()));

    let mut payloads = Vec::new();
    for path in paths {
        if path.file_name() == Some(OsStr::new(CACHE_ANCHOR)) {
            continue;
        }
        let (kind, held) = validate_payload_tree(&path, parent)?;
        payloads.push(ValidatedPayload {
            path,
            kind,
            identity: held.identity().clone(),
        });
    }
    parent.validate_path(
        &parent_path,
        EntryKind::Directory,
        EntryAccess::ReadOnly,
        false,
        "cache parent",
    )?;
    Ok(payloads)
}

fn validate_payload_tree(
    path: &Path,
    cache_parent: &HeldEntry,
) -> Result<(PayloadKind, HeldEntry), StoreError> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if metadata.file_type().is_symlink() {
        return Err(StoreError::Integrity(format!(
            "cache payload contains a redirect: {}",
            path.display()
        )));
    }

    if metadata.is_dir() {
        let held = validate_entry(
            path,
            EntryKind::Directory,
            false,
            "cache payload directory",
            cache_parent,
        )?;
        let mut children = fs::read_dir(path)
            .map_err(io_error)?
            .map(|entry| entry.map(|entry| entry.path()).map_err(io_error))
            .collect::<Result<Vec<_>, _>>()?;
        children.sort_by(|left, right| left.as_os_str().cmp(right.as_os_str()));
        for child in children {
            validate_payload_tree(&child, cache_parent)?;
        }
        held.validate_path(
            path,
            EntryKind::Directory,
            EntryAccess::ReadOnly,
            false,
            "cache payload directory",
        )?;
        Ok((PayloadKind::Directory, held))
    } else if metadata.is_file() {
        let held = validate_entry(
            path,
            EntryKind::RegularFile,
            true,
            "cache payload file",
            cache_parent,
        )?;
        Ok((PayloadKind::RegularFile, held))
    } else {
        Err(StoreError::Integrity(format!(
            "cache payload contains an unsupported entry: {}",
            path.display()
        )))
    }
}

fn validate_entry(
    path: &Path,
    kind: EntryKind,
    one_link: bool,
    label: &str,
    cache_parent: &HeldEntry,
) -> Result<HeldEntry, StoreError> {
    let held = HeldEntry::open(path, kind, EntryAccess::ReadOnly, one_link, label)?;
    if !same_volume_and_mount(&held, cache_parent) {
        return Err(StoreError::Integrity(format!(
            "{label} crossed the cache parent volume or mount: {}",
            path.display()
        )));
    }
    held.validate_path(path, kind, EntryAccess::ReadOnly, one_link, label)?;
    Ok(held)
}

fn remove_payload(guard: &NamespaceGuard, expected: &ValidatedPayload) -> Result<(), StoreError> {
    let parent = guard.managed_parent_entry(ManagedStateParentKind::Cache)?;
    let (kind, current) = validate_payload_tree(&expected.path, parent)?;
    let kind_matches = matches!(
        (expected.kind, kind),
        (PayloadKind::Directory, PayloadKind::Directory)
            | (PayloadKind::RegularFile, PayloadKind::RegularFile)
    );
    if !kind_matches || current.identity() != &expected.identity {
        return Err(StoreError::Integrity(format!(
            "cache payload changed before cleanup: {}",
            expected.path.display()
        )));
    }
    drop(current);

    match expected.kind {
        PayloadKind::Directory => fs::remove_dir_all(&expected.path).map_err(io_error)?,
        PayloadKind::RegularFile => fs::remove_file(&expected.path).map_err(io_error)?,
    }
    parent.sync_directory()?;
    match fs::symlink_metadata(&expected.path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(error)),
        Ok(_) => Err(StoreError::Integrity(format!(
            "cache payload survived cleanup: {}",
            expected.path.display()
        ))),
    }
}

fn require_anchor_only(guard: &NamespaceGuard) -> Result<(), StoreError> {
    let parent = guard.managed_parent_entry(ManagedStateParentKind::Cache)?;
    let parent_path = guard.managed_parent_path(ManagedStateParentKind::Cache);
    parent.sync_directory()?;
    let mut remaining = fs::read_dir(&parent_path)
        .map_err(io_error)?
        .map(|entry| entry.map(|entry| entry.file_name()).map_err(io_error))
        .collect::<Result<Vec<_>, _>>()?;
    remaining.sort();
    if remaining != [OsString::from(CACHE_ANCHOR)] {
        return Err(StoreError::Integrity(
            "cache payload descendants survived cleanup".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_store(root: &Path) -> Result<RepositoryStore, Box<dyn std::error::Error>> {
        let admission = lumin_inventory::repository_admission(root)?;
        Ok(RepositoryStore::open(
            &admission.canonical_root,
            &admission.binding,
        )?)
    }

    #[test]
    fn cleanup_preserves_cache_binding_and_is_idempotent() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        let cache = root.path().join(".lumin/cache");
        let anchor = cache.join(CACHE_ANCHOR);
        let parent_identity = lumin_inventory::physical_file_identity(&cache)?;
        let anchor_identity = lumin_inventory::physical_file_identity(&anchor)?;
        let anchor_bytes = fs::read(&anchor)?;

        fs::create_dir_all(cache.join("nested/deep"))?;
        fs::write(cache.join("nested/deep/payload.bin"), b"nested")?;
        fs::write(cache.join("direct.bin"), b"direct")?;

        store.clean_cache_payloads()?;
        store.clean_cache_payloads()?;

        assert_eq!(
            fs::read_dir(&cache)?
                .map(|entry| entry.map(|entry| entry.file_name()))
                .collect::<Result<Vec<_>, _>>()?,
            [OsString::from(CACHE_ANCHOR)]
        );
        assert_eq!(
            lumin_inventory::physical_file_identity(&cache)?,
            parent_identity
        );
        assert_eq!(
            lumin_inventory::physical_file_identity(&anchor)?,
            anchor_identity
        );
        assert_eq!(fs::read(&anchor)?, anchor_bytes);
        drop(store);
        drop(open_store(root.path())?);
        Ok(())
    }

    #[test]
    fn invalid_payload_prevents_partial_cleanup() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        let cache = root.path().join(".lumin/cache");
        let ordinary = root.path().join("ordinary.bin");
        fs::write(cache.join("a-valid.bin"), b"valid")?;
        fs::write(&ordinary, b"shared")?;
        fs::hard_link(&ordinary, cache.join("z-shared.bin"))?;

        let error = match store.clean_cache_payloads() {
            Err(error) => error,
            Ok(()) => {
                return Err(std::io::Error::other("shared payload was accepted").into());
            }
        };
        assert!(matches!(error, StoreError::Integrity(_)));
        assert!(cache.join("a-valid.bin").is_file());
        assert!(cache.join("z-shared.bin").is_file());
        assert!(cache.join(CACHE_ANCHOR).is_file());
        Ok(())
    }
}
