use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use fs2::FileExt;

use crate::{StoreError, io_error, nonce_hex};

use super::platform::{EntryAccess, EntryKind, HeldEntry};
use super::{
    GlobalNamespaceBinding, LifecycleLockHeader, MANAGED_KINDS, ManagedParentAnchorHeader,
    ManagedStateParentBinding, ManagedStateParentKind, NamespaceBinding, NamespaceGuard,
    NamespaceState, RepositoryMarker, create_or_verify_store, entry_exists, read_canonical_path,
    require_state_volume, validate_marker, write_canonical_entry, write_new_canonical,
};

pub(super) fn bootstrap_namespace(
    state_dir: PathBuf,
    state_directory: HeldEntry,
) -> Result<NamespaceState, StoreError> {
    if fs::read_dir(&state_dir).map_err(io_error)?.next().is_some() {
        return resume_bootstrap(state_dir, state_directory);
    }
    let lock = HeldEntry::create_new(&state_dir.join("lifecycle.lock"), "lifecycle.lock")?;
    FileExt::lock_exclusive(lock.file()).map_err(io_error)?;
    let global = GlobalNamespaceBinding {
        state_directory_identity: state_directory.identity().clone(),
        lifecycle_lock_identity: lock.identity().clone(),
        namespace_nonce: nonce_hex()?,
    };
    write_canonical_entry(
        &lock,
        &LifecycleLockHeader {
            schema_version: "lumin-lifecycle-lock.v1".to_owned(),
            global: global.clone(),
        },
    )?;
    finish_bootstrap(state_dir, state_directory, lock, global)
}

fn resume_bootstrap(
    state_dir: PathBuf,
    state_directory: HeldEntry,
) -> Result<NamespaceState, StoreError> {
    reject_unbound_bootstrap_entries(&state_dir)?;
    let lock_path = state_dir.join("lifecycle.lock");
    let lock = HeldEntry::open(
        &lock_path,
        EntryKind::RegularFile,
        EntryAccess::ReadWrite,
        true,
        "lifecycle.lock",
    )?;
    let header: LifecycleLockHeader = read_canonical_path(&lock_path, "lifecycle.lock")?;
    validate_bootstrap_lock(&header, &state_directory, &lock)?;
    FileExt::lock_exclusive(lock.file()).map_err(io_error)?;

    let marker_path = state_dir.join("repository.json");
    if entry_exists(&marker_path)? {
        FileExt::unlock(lock.file()).map_err(io_error)?;
        let marker: RepositoryMarker = read_canonical_path(&marker_path, "repository marker")?;
        validate_marker(&marker)?;
        let state = NamespaceState {
            state_dir,
            binding: marker.binding,
        };
        state.ensure_store_ready()?;
        return Ok(state);
    }
    if entry_exists(&state_dir.join("lifecycle.store"))? {
        return Err(StoreError::Integrity(
            "pre-marker state cannot contain lifecycle.store".to_owned(),
        ));
    }
    verify_canonical_bootstrap_lock(&lock, &header)?;
    finish_bootstrap(state_dir, state_directory, lock, header.global)
}

fn finish_bootstrap(
    state_dir: PathBuf,
    state_directory: HeldEntry,
    lock: HeldEntry,
    global: GlobalNamespaceBinding,
) -> Result<NamespaceState, StoreError> {
    let mut bindings = Vec::with_capacity(MANAGED_KINDS.len());
    for kind in MANAGED_KINDS {
        let binding = if entry_exists(&state_dir.join(kind.directory_name()))? {
            load_existing_parent(&state_dir, &state_directory, &global, kind)?
        } else {
            create_managed_parent(&state_dir, &state_directory, &global, kind)?
        };
        bindings.push(binding);
    }
    let managed_parents: [ManagedStateParentBinding; 4] = bindings.try_into().map_err(|_| {
        StoreError::Integrity("managed parent initialization was incomplete".to_owned())
    })?;
    let binding = NamespaceBinding {
        global,
        managed_parents,
    };
    write_new_canonical(
        &state_dir.join("repository.json"),
        &RepositoryMarker {
            schema_version: "lumin-repository.v2".to_owned(),
            binding: binding.clone(),
        },
    )?;
    state_directory.sync_directory()?;

    let state = NamespaceState { state_dir, binding };
    let guard = NamespaceGuard::acquire_without_store(state.clone(), lock)?;
    create_or_verify_store(&guard)?;
    guard.validate_complete()?;
    FileExt::unlock(guard.lock.file()).map_err(io_error)?;
    Ok(state)
}

fn create_managed_parent(
    state_dir: &Path,
    state_directory: &HeldEntry,
    global: &GlobalNamespaceBinding,
    kind: ManagedStateParentKind,
) -> Result<ManagedStateParentBinding, StoreError> {
    let name = kind.directory_name();
    let directory_path = state_dir.join(name);
    fs::create_dir(&directory_path).map_err(io_error)?;
    let directory = open_parent_directory(&directory_path, name)?;
    require_state_volume(&directory, state_directory, name)?;
    let anchor = HeldEntry::create_new(
        &directory_path.join("namespace.anchor"),
        &format!("managed state anchor {name}"),
    )?;
    let binding = ManagedStateParentBinding {
        kind,
        directory_physical_identity: directory.identity().clone(),
        anchor_physical_identity: anchor.identity().clone(),
        parent_nonce: nonce_hex()?,
    };
    write_canonical_entry(
        &anchor,
        &ManagedParentAnchorHeader {
            schema_version: "lumin-managed-parent-anchor.v1".to_owned(),
            global: global.clone(),
            binding: binding.clone(),
        },
    )?;
    directory.sync_directory()?;
    Ok(binding)
}

fn load_existing_parent(
    state_dir: &Path,
    state_directory: &HeldEntry,
    global: &GlobalNamespaceBinding,
    kind: ManagedStateParentKind,
) -> Result<ManagedStateParentBinding, StoreError> {
    let name = kind.directory_name();
    let path = state_dir.join(name);
    require_anchor_only(&path, name)?;
    let directory = open_parent_directory(&path, name)?;
    require_state_volume(&directory, state_directory, name)?;
    let anchor_path = path.join("namespace.anchor");
    let anchor = HeldEntry::open(
        &anchor_path,
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        true,
        &format!("managed state anchor {name}"),
    )?;
    let header: ManagedParentAnchorHeader =
        read_canonical_path(&anchor_path, &format!("managed state anchor {name}"))?;
    if header.schema_version != "lumin-managed-parent-anchor.v1"
        || &header.global != global
        || header.binding.kind != kind
        || header.binding.directory_physical_identity != *directory.identity()
        || header.binding.anchor_physical_identity != *anchor.identity()
        || !valid_nonce(&header.binding.parent_nonce)
    {
        return Err(StoreError::Integrity(format!(
            "managed state parent {name} is not a matching bootstrap remnant"
        )));
    }
    Ok(header.binding)
}

fn validate_bootstrap_lock(
    header: &LifecycleLockHeader,
    state_directory: &HeldEntry,
    lock: &HeldEntry,
) -> Result<(), StoreError> {
    if header.schema_version != "lumin-lifecycle-lock.v1"
        || header.global.state_directory_identity != *state_directory.identity()
        || header.global.lifecycle_lock_identity != *lock.identity()
        || !valid_nonce(&header.global.namespace_nonce)
    {
        return Err(StoreError::Integrity(
            "lifecycle.lock is not a matching bootstrap remnant".to_owned(),
        ));
    }
    Ok(())
}

fn verify_canonical_bootstrap_lock(
    lock: &HeldEntry,
    header: &LifecycleLockHeader,
) -> Result<(), StoreError> {
    let current: LifecycleLockHeader =
        serde_json::from_slice(&lock.read_all()?).map_err(|error| {
            StoreError::Integrity(format!("lifecycle.lock header is malformed: {error}"))
        })?;
    if &current != header {
        return Err(StoreError::Integrity(
            "lifecycle.lock changed before bootstrap resumed".to_owned(),
        ));
    }
    Ok(())
}

fn reject_unbound_bootstrap_entries(state_dir: &Path) -> Result<(), StoreError> {
    for entry in fs::read_dir(state_dir).map_err(io_error)? {
        let name = entry.map_err(io_error)?.file_name();
        let allowed = name == OsStr::new("lifecycle.lock")
            || name == OsStr::new("repository.json")
            || name == OsStr::new("lifecycle.store")
            || MANAGED_KINDS
                .iter()
                .any(|kind| name == OsStr::new(kind.directory_name()));
        if !allowed {
            return Err(StoreError::Integrity(
                "unbound pre-marker state contains a foreign entry".to_owned(),
            ));
        }
    }
    Ok(())
}

fn require_anchor_only(path: &Path, name: &str) -> Result<(), StoreError> {
    let mut entries = fs::read_dir(path).map_err(io_error)?;
    let anchor = entries
        .next()
        .transpose()
        .map_err(io_error)?
        .ok_or_else(|| {
            StoreError::Integrity(format!("managed state parent {name} omitted its anchor"))
        })?;
    if anchor.file_name() != OsStr::new("namespace.anchor") || entries.next().is_some() {
        return Err(StoreError::Integrity(format!(
            "managed state parent {name} contains foreign pre-marker state"
        )));
    }
    Ok(())
}

fn open_parent_directory(path: &Path, name: &str) -> Result<HeldEntry, StoreError> {
    HeldEntry::open(
        path,
        EntryKind::Directory,
        EntryAccess::ReadOnly,
        false,
        &format!("managed state parent {name}"),
    )
}

fn valid_nonce(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
