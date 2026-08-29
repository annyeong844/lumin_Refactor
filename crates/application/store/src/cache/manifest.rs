use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Component, Path, PathBuf};

use lumin_evidence::{
    CacheEvictionAuthorization, CacheEvictionComponentKey, CacheEvictionEntryKind,
    CacheEvictionManifest, CacheEvictionManifestRow, CacheEvictionPathKey,
};
use lumin_model::{
    OperationId, append_length_prefixed, decode_native_path_component, digest_hex,
    encode_native_path_component,
};

use crate::namespace::records::ManagedStateParentKind;
use crate::namespace::{
    EntryAccess, EntryKind, HeldEntry, NamespaceGuard, move_entry_noreplace, same_volume_and_mount,
};
use crate::{StoreError, io_error};

const NAMESPACE_ANCHOR: &str = "namespace.anchor";

pub(super) struct PreparedCachePayload {
    pub(super) source_component: CacheEvictionComponentKey,
    pub(super) manifest: CacheEvictionManifest,
}

pub(super) fn prepare_active_payloads(
    guard: &NamespaceGuard,
    operation_id: Option<&OperationId>,
) -> Result<Vec<PreparedCachePayload>, StoreError> {
    let cache = guard.managed_parent_entry(ManagedStateParentKind::Cache)?;
    let cache_path = guard.managed_parent_path(ManagedStateParentKind::Cache);
    let mut children = direct_children(&cache_path)?;
    children.retain(|(_, name)| name != OsStr::new(NAMESPACE_ANCHOR));
    let mut payloads = Vec::with_capacity(children.len());
    for (index, (path, name)) in children.into_iter().enumerate() {
        let source = component_projection(&name)?;
        let barrier = operation_id
            .map(|operation_id| {
                u64::try_from(index)
                    .map(|ordinal| (operation_id, ordinal))
                    .map_err(|_| {
                        StoreError::Integrity("cache cleanup plan ordinal overflow".to_owned())
                    })
            })
            .transpose()?;
        let manifest = flush_and_manifest_tree(&path, cache, barrier)?;
        payloads.push(PreparedCachePayload {
            source_component: source,
            manifest,
        });
    }
    cache.validate_path(
        &cache_path,
        EntryKind::Directory,
        EntryAccess::ReadOnly,
        false,
        "cache parent",
    )?;
    Ok(payloads)
}

pub(super) fn reconcile_authorized_move(
    guard: &NamespaceGuard,
    authorization: &CacheEvictionAuthorization,
) -> Result<(), StoreError> {
    let source_name = native_component(&authorization.source_component)?;
    let destination_name = native_component(&authorization.destination_component)?;
    let cache_parent = guard.managed_parent_entry(ManagedStateParentKind::Cache)?;
    let quarantine_parent = guard.cache_eviction_parent_entry();
    let source = guard
        .managed_parent_path(ManagedStateParentKind::Cache)
        .join(&source_name);
    let destination = guard.cache_eviction_parent_path().join(&destination_name);
    let source_exists = entry_exists(&source)?;
    let destination_exists = entry_exists(&destination)?;
    match (source_exists, destination_exists) {
        (true, false) => {
            require_manifest(&source, cache_parent, &authorization.expected_manifest)?;
            let kind = root_entry_kind(&authorization.expected_manifest)?;
            let held = HeldEntry::open(
                &source,
                kind,
                EntryAccess::Move,
                matches!(kind, EntryKind::RegularFile),
                "authorized cache payload",
            )?;
            require_root_identity(&held, &authorization.expected_manifest)?;
            #[cfg(feature = "cache-cleanup-test-fault")]
            super::barrier::wait_before_move(&authorization.operation_id, authorization.ordinal)?;
            #[cfg(feature = "namespace-test-crash")]
            crate::namespace::barrier::wait_before_cache_move()?;
            guard.validate_bound_entries()?;
            require_manifest(&source, cache_parent, &authorization.expected_manifest)?;
            move_entry_noreplace(
                cache_parent,
                &source_name,
                &held,
                quarantine_parent,
                &destination_name,
            )?;
            if entry_exists(&source)? {
                return Err(StoreError::Integrity(format!(
                    "authorized cache source name was replaced during movement: {}",
                    source.display()
                )));
            }
            #[cfg(feature = "cache-cleanup-test-fault")]
            super::barrier::wait_after_move(&authorization.operation_id, authorization.ordinal)?;
            #[cfg(feature = "cache-cleanup-test-fault")]
            super::crash::hit(super::crash::CacheCleanupCrashPoint::AfterRenameVisible(
                authorization.ordinal,
            ));
            let moved = flush_and_manifest_tree(&destination, quarantine_parent, None)?;
            if moved != authorization.expected_manifest {
                return Err(StoreError::Integrity(format!(
                    "moved cache payload disagrees with its authorization: {}",
                    destination.display()
                )));
            }
        }
        (false, true) => {
            let moved = flush_and_manifest_tree(&destination, quarantine_parent, None)?;
            if moved != authorization.expected_manifest {
                return Err(StoreError::Integrity(format!(
                    "recovered cache payload disagrees with its authorization: {}",
                    destination.display()
                )));
            }
        }
        (true, true) => {
            return Err(StoreError::Integrity(format!(
                "authorized cache payload exists at both source and destination: {}",
                authorization.operation_id.as_str()
            )));
        }
        (false, false) => {
            return Err(StoreError::Integrity(format!(
                "authorized cache payload exists at neither source nor destination: {}",
                authorization.operation_id.as_str()
            )));
        }
    }
    Ok(())
}

pub(super) fn validate_authorized_location(
    guard: &NamespaceGuard,
    authorization: &CacheEvictionAuthorization,
) -> Result<bool, StoreError> {
    let source_name = native_component(&authorization.source_component)?;
    let destination_name = native_component(&authorization.destination_component)?;
    let source = guard
        .managed_parent_path(ManagedStateParentKind::Cache)
        .join(source_name);
    let destination = guard.cache_eviction_parent_path().join(destination_name);
    let source_exists = entry_exists(&source)?;
    let destination_exists = entry_exists(&destination)?;
    match (source_exists, destination_exists) {
        (true, false) => {
            require_manifest(
                &source,
                guard.managed_parent_entry(ManagedStateParentKind::Cache)?,
                &authorization.expected_manifest,
            )?;
            Ok(false)
        }
        (false, true) => {
            require_manifest(
                &destination,
                guard.cache_eviction_parent_entry(),
                &authorization.expected_manifest,
            )?;
            Ok(true)
        }
        (true, true) => Err(StoreError::Integrity(format!(
            "authorized cache payload exists at both source and destination: {}",
            authorization.operation_id.as_str()
        ))),
        (false, false) => Err(StoreError::Integrity(format!(
            "authorized cache payload exists at neither source nor destination: {}",
            authorization.operation_id.as_str()
        ))),
    }
}

pub(super) fn validate_quarantine_child(
    guard: &NamespaceGuard,
    name: &str,
    authorization: &CacheEvictionAuthorization,
) -> Result<(), StoreError> {
    let path = guard.cache_eviction_parent_path().join(name);
    require_manifest(
        &path,
        guard.cache_eviction_parent_entry(),
        &authorization.expected_manifest,
    )
}

pub(super) fn quarantine_child_names(guard: &NamespaceGuard) -> Result<Vec<String>, StoreError> {
    let mut names = Vec::new();
    for (_, name) in direct_children(&guard.cache_eviction_parent_path())? {
        if name == OsStr::new(NAMESPACE_ANCHOR) {
            continue;
        }
        let name = name.into_string().map_err(|_| {
            StoreError::Integrity("cache quarantine child name is not UTF-8".to_owned())
        })?;
        names.push(name);
    }
    names.sort();
    Ok(names)
}

pub(super) fn require_active_cache_anchor_only(guard: &NamespaceGuard) -> Result<(), StoreError> {
    let names = direct_children(&guard.managed_parent_path(ManagedStateParentKind::Cache))?
        .into_iter()
        .map(|(_, name)| name)
        .collect::<Vec<_>>();
    if names != [OsString::from(NAMESPACE_ANCHOR)] {
        return Err(StoreError::Integrity(
            "active cache contains payloads after cleanup".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn flush_cleanup_parents(guard: &NamespaceGuard) -> Result<(), StoreError> {
    guard
        .managed_parent_entry(ManagedStateParentKind::Cache)?
        .sync_directory()?;
    guard.cache_eviction_parent_entry().sync_directory()?;
    guard
        .managed_parent_entry(ManagedStateParentKind::Trash)?
        .sync_directory()
}

pub(super) fn manifest_digest(manifest: &CacheEvictionManifest) -> String {
    let mut framed = Vec::new();
    append_length_prefixed(&mut framed, b"cache-eviction-manifest.v1");
    for row in &manifest.rows {
        let mut path_frame = Vec::new();
        for component in &row.relative_path.components {
            append_length_prefixed(&mut path_frame, &component.canonical);
        }
        append_length_prefixed(&mut framed, &path_frame);
        framed.push(match row.kind {
            CacheEvictionEntryKind::Directory => 1,
            CacheEvictionEntryKind::RegularFile => 2,
        });
        append_length_prefixed(&mut framed, &row.physical_identity.canonical_bytes());
        framed.extend_from_slice(&row.link_count.to_be_bytes());
        match row.byte_length {
            Some(length) => {
                framed.push(1);
                framed.extend_from_slice(&length.to_be_bytes());
            }
            None => framed.push(0),
        }
        match &row.payload_sha256 {
            Some(digest) => {
                framed.push(1);
                append_length_prefixed(&mut framed, digest.as_bytes());
            }
            None => framed.push(0),
        }
    }
    digest_hex(&framed)
}

fn flush_and_manifest_tree(
    path: &Path,
    parent: &HeldEntry,
    initial_flush_barrier: Option<(&OperationId, u64)>,
) -> Result<CacheEvictionManifest, StoreError> {
    let flushed = manifest_tree(path, parent, true)?;
    #[cfg(feature = "cache-cleanup-test-fault")]
    if let Some((operation_id, ordinal)) = initial_flush_barrier {
        super::barrier::wait_after_initial_flush(operation_id, ordinal)?;
    }
    #[cfg(not(feature = "cache-cleanup-test-fault"))]
    let _ = initial_flush_barrier;
    let observed = manifest_tree(path, parent, false)?;
    if observed != flushed {
        return Err(StoreError::Integrity(format!(
            "cache payload changed while becoming durable: {}",
            path.display()
        )));
    }
    Ok(observed)
}

fn require_manifest(
    path: &Path,
    parent: &HeldEntry,
    expected: &CacheEvictionManifest,
) -> Result<(), StoreError> {
    let observed = manifest_tree(path, parent, false)?;
    if &observed != expected {
        return Err(StoreError::Integrity(format!(
            "cache payload manifest changed: {}",
            path.display()
        )));
    }
    Ok(())
}

fn manifest_tree(
    path: &Path,
    parent: &HeldEntry,
    flush: bool,
) -> Result<CacheEvictionManifest, StoreError> {
    manifest_tree_with_observer(path, parent, flush, &mut |_, _| {})
}

fn manifest_tree_with_observer(
    path: &Path,
    parent: &HeldEntry,
    flush: bool,
    observer: &mut impl FnMut(&Path, EntryKind),
) -> Result<CacheEvictionManifest, StoreError> {
    let root = path.to_path_buf();
    let mut rows = Vec::new();
    manifest_entry(&root, &root, parent, flush, &mut rows, observer)?;
    rows.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(CacheEvictionManifest { rows })
}

fn manifest_entry(
    root: &Path,
    path: &Path,
    parent: &HeldEntry,
    flush: bool,
    rows: &mut Vec<CacheEvictionManifestRow>,
    observer: &mut impl FnMut(&Path, EntryKind),
) -> Result<(), StoreError> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if metadata.file_type().is_symlink() {
        return Err(StoreError::Integrity(format!(
            "cache payload contains a redirect: {}",
            path.display()
        )));
    }
    let relative = path
        .strip_prefix(root)
        .map_err(|_| StoreError::Integrity("cache manifest path escaped its root".to_owned()))?;
    let relative = path_key(relative)?;

    if metadata.is_dir() {
        let held = HeldEntry::open(
            path,
            EntryKind::Directory,
            EntryAccess::ReadOnly,
            false,
            "cache payload directory",
        )?;
        require_payload_mount(&held, parent, path)?;
        rows.push(CacheEvictionManifestRow {
            relative_path: relative,
            kind: CacheEvictionEntryKind::Directory,
            physical_identity: held.identity().clone(),
            link_count: held.links(),
            byte_length: None,
            payload_sha256: None,
        });
        for (child, _) in direct_children(path)? {
            manifest_entry(root, &child, parent, flush, rows, observer)?;
        }
        if flush {
            held.sync_directory()?;
            observer(path, EntryKind::Directory);
        }
        held.validate_path(
            path,
            EntryKind::Directory,
            EntryAccess::ReadOnly,
            false,
            "cache payload directory",
        )?;
    } else if metadata.is_file() {
        let held = HeldEntry::open(
            path,
            EntryKind::RegularFile,
            EntryAccess::Move,
            true,
            "cache payload file",
        )?;
        require_payload_mount(&held, parent, path)?;
        if flush {
            held.sync()?;
            observer(path, EntryKind::RegularFile);
        }
        let bytes = held.read_all()?;
        let byte_length = u64::try_from(bytes.len())
            .map_err(|_| StoreError::Integrity("cache payload length exceeds u64".to_owned()))?;
        rows.push(CacheEvictionManifestRow {
            relative_path: relative,
            kind: CacheEvictionEntryKind::RegularFile,
            physical_identity: held.identity().clone(),
            link_count: held.links(),
            byte_length: Some(byte_length),
            payload_sha256: Some(digest_hex(&bytes)),
        });
        held.validate_path(
            path,
            EntryKind::RegularFile,
            EntryAccess::Move,
            true,
            "cache payload file",
        )?;
    } else {
        return Err(StoreError::Integrity(format!(
            "cache payload contains an unsupported entry: {}",
            path.display()
        )));
    }
    Ok(())
}

fn direct_children(path: &Path) -> Result<Vec<(PathBuf, OsString)>, StoreError> {
    let mut children = fs::read_dir(path)
        .map_err(io_error)?
        .map(|entry| {
            let entry = entry.map_err(io_error)?;
            Ok((entry.path(), entry.file_name()))
        })
        .collect::<Result<Vec<_>, StoreError>>()?;
    children.sort_by(|(_, left), (_, right)| {
        component_key(left)
            .unwrap_or_default()
            .cmp(&component_key(right).unwrap_or_default())
    });
    for (_, name) in &children {
        component_key(name)?;
    }
    Ok(children)
}

fn component_key(name: &OsStr) -> Result<Vec<u8>, StoreError> {
    encode_native_path_component(name)
        .map_err(|error| StoreError::Integrity(format!("cache component is invalid: {error}")))
}

pub(super) fn component_projection(name: &OsStr) -> Result<CacheEvictionComponentKey, StoreError> {
    Ok(CacheEvictionComponentKey {
        canonical: component_key(name)?,
    })
}

fn native_component(projection: &CacheEvictionComponentKey) -> Result<OsString, StoreError> {
    decode_native_path_component(&projection.canonical).map_err(|error| {
        StoreError::Integrity(format!("cache component projection is invalid: {error}"))
    })
}

fn path_key(path: &Path) -> Result<CacheEvictionPathKey, StoreError> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => components.push(component_projection(name)?),
            Component::CurDir if components.is_empty() => {}
            Component::CurDir
            | Component::ParentDir
            | Component::Prefix(_)
            | Component::RootDir => {
                return Err(StoreError::Integrity(
                    "cache manifest path is not a normalized relative path".to_owned(),
                ));
            }
        }
    }
    Ok(CacheEvictionPathKey { components })
}

fn require_payload_mount(
    entry: &HeldEntry,
    parent: &HeldEntry,
    path: &Path,
) -> Result<(), StoreError> {
    if !same_volume_and_mount(entry, parent) {
        return Err(StoreError::Integrity(format!(
            "cache payload crossed its parent volume or mount: {}",
            path.display()
        )));
    }
    Ok(())
}

fn root_entry_kind(manifest: &CacheEvictionManifest) -> Result<EntryKind, StoreError> {
    let root = manifest
        .rows
        .iter()
        .find(|row| row.relative_path.components.is_empty())
        .ok_or_else(|| StoreError::Integrity("cache manifest omitted its root".to_owned()))?;
    Ok(match root.kind {
        CacheEvictionEntryKind::Directory => EntryKind::Directory,
        CacheEvictionEntryKind::RegularFile => EntryKind::RegularFile,
    })
}

fn require_root_identity(
    held: &HeldEntry,
    manifest: &CacheEvictionManifest,
) -> Result<(), StoreError> {
    let root = manifest
        .rows
        .iter()
        .find(|row| row.relative_path.components.is_empty())
        .ok_or_else(|| StoreError::Integrity("cache manifest omitted its root".to_owned()))?;
    if held.identity() != &root.physical_identity {
        return Err(StoreError::Integrity(
            "cache payload root identity changed before movement".to_owned(),
        ));
    }
    Ok(())
}

fn entry_exists(path: &Path) -> Result<bool, StoreError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RepositoryStore;

    #[test]
    fn payload_flush_order_is_deterministic_and_bottom_up() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = tempfile::tempdir()?;
        let admission = lumin_inventory::repository_admission(root.path())?;
        let store = RepositoryStore::open(&admission.canonical_root, &admission.binding)?;
        let tree = root.path().join(".lumin/cache/tree");
        fs::create_dir_all(tree.join("a-dir"))?;
        fs::write(tree.join("a-dir/nested.bin"), b"nested")?;
        fs::write(tree.join("z.bin"), b"direct")?;

        store.with_exclusive_lock(|guard| {
            let parent = guard.managed_parent_entry(ManagedStateParentKind::Cache)?;
            let mut order = Vec::new();
            manifest_tree_with_observer(&tree, parent, true, &mut |path, kind| {
                order.push((
                    path.to_path_buf(),
                    match kind {
                        EntryKind::Directory => "directory",
                        EntryKind::RegularFile => "file",
                    },
                ));
            })?;
            let order = order
                .into_iter()
                .map(|(path, kind)| {
                    path.strip_prefix(&tree)
                        .map(|relative| (relative.to_path_buf(), kind))
                        .map_err(|error| {
                            StoreError::Integrity(format!(
                                "flush path left manifested root: {error}"
                            ))
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            assert_eq!(
                order,
                [
                    (PathBuf::from("a-dir").join("nested.bin"), "file"),
                    (PathBuf::from("a-dir"), "directory"),
                    (PathBuf::from("z.bin"), "file"),
                    (PathBuf::new(), "directory"),
                ]
            );
            Ok(())
        })?;
        Ok(())
    }
}
