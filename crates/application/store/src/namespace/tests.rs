use std::fs;

use redb::TableError;

use crate::{RepositoryStore, SEQUENCES, StoreError, backend_error, io_error};

use super::{LifecycleLockHeader, MANAGED_KINDS, RepositoryMarker, read_canonical_path};

mod generation;
mod migration;

fn open_store(root: &std::path::Path) -> Result<RepositoryStore, StoreError> {
    let admission = lumin_inventory::repository_admission(root)
        .map_err(|error| StoreError::Integrity(error.to_string()))?;
    RepositoryStore::open(&admission.canonical_root, &admission.binding)
}

fn require_integrity_failure(
    result: Result<RepositoryStore, StoreError>,
) -> Result<(), Box<dyn std::error::Error>> {
    match result {
        Err(StoreError::Integrity(_)) => Ok(()),
        Err(error) => Err(Box::new(error)),
        Ok(_) => Err(Box::new(std::io::Error::other(
            "state replacement was accepted",
        ))),
    }
}

#[test]
fn rejects_state_namespace_file() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let admission = lumin_inventory::repository_admission(root.path())?;
    fs::write(root.path().join(".lumin"), b"not a directory")?;

    require_integrity_failure(RepositoryStore::open(
        &admission.canonical_root,
        &admission.binding,
    ))
}

#[test]
fn rejects_preexisting_empty_state_directory() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join(".lumin"))?;

    require_integrity_failure(open_store(root.path()))
}

#[test]
fn rejects_a_binding_observed_from_another_repository() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let other = tempfile::tempdir()?;
    let root_admission = lumin_inventory::repository_admission(root.path())?;
    let other_admission = lumin_inventory::repository_admission(other.path())?;

    drop(RepositoryStore::open(
        &root_admission.canonical_root,
        &root_admission.binding,
    )?);
    require_integrity_failure(RepositoryStore::open(
        &root_admission.canonical_root,
        &other_admission.binding,
    ))
}

#[test]
fn marker_and_lock_bind_the_inventory_owned_repository_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let admission = lumin_inventory::repository_admission(root.path())?;
    drop(RepositoryStore::open(
        &admission.canonical_root,
        &admission.binding,
    )?);

    let state = admission.canonical_root.join(".lumin");
    let marker: RepositoryMarker =
        read_canonical_path(&state.join("repository.json"), "repository marker")?;
    let lock: LifecycleLockHeader =
        read_canonical_path(&state.join("lifecycle.lock"), "lifecycle.lock")?;
    assert_eq!(
        marker.binding.global.repository_id,
        *admission.binding.repository_id()
    );
    assert_eq!(
        marker.binding.global.repository_root_canonical,
        admission.binding.root().canonical_bytes()
    );
    assert_eq!(marker.binding.global, lock.global);
    Ok(())
}

#[test]
fn state_directory_replacement_cannot_form_a_second_domain()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let _existing_attempt = store.begin_attempt()?;
    if !try_replace_state_directory(root.path())? {
        assert!(store.latest_run_id()?.is_none());
        return Ok(());
    }
    assert!(matches!(
        store.begin_attempt(),
        Err(StoreError::Integrity(_))
    ));
    require_integrity_failure(open_store(root.path()))?;

    restore_state_directory(root.path())?;
    assert!(open_store(root.path())?.latest_run_id()?.is_none());
    Ok(())
}

#[test]
fn copied_state_directory_cannot_form_a_second_domain() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    drop(open_store(root.path())?);
    let state = root.path().join(".lumin");
    let replacement = root.path().join(".lumin.replacement");
    let displaced = root.path().join(".lumin.displaced");
    copy_directory(&state, &replacement)?;
    fs::rename(&state, &displaced)?;
    fs::rename(&replacement, &state)?;

    require_integrity_failure(open_store(root.path()))?;

    fs::remove_dir_all(&state)?;
    fs::rename(displaced, &state)?;
    drop(open_store(root.path())?);
    Ok(())
}

#[test]
fn lifecycle_lock_replacement_cannot_form_a_second_domain() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let _existing_attempt = store.begin_attempt()?;
    let state = root.path().join(".lumin");
    replace_lock(&state)?;

    assert!(matches!(
        store.begin_attempt(),
        Err(StoreError::Integrity(_))
    ));
    require_integrity_failure(open_store(root.path()))?;

    restore_lock(&state)?;
    assert!(open_store(root.path())?.latest_run_id()?.is_none());
    Ok(())
}

#[test]
fn rejects_copied_real_parent_for_every_managed_kind() -> Result<(), Box<dyn std::error::Error>> {
    for kind in MANAGED_KINDS {
        let root = tempfile::tempdir()?;
        drop(open_store(root.path())?);
        let state_dir = root.path().join(".lumin");
        let parent = state_dir.join(kind.directory_name());
        let original = state_dir.join(format!("{}.original", kind.directory_name()));
        fs::rename(&parent, &original)?;
        fs::create_dir(&parent)?;
        fs::copy(
            original.join("namespace.anchor"),
            parent.join("namespace.anchor"),
        )?;

        require_integrity_failure(open_store(root.path()))?;
    }
    Ok(())
}

#[test]
fn rejects_byte_identical_anchor_replacement() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    drop(open_store(root.path())?);
    let anchor = root.path().join(".lumin/runs/namespace.anchor");
    let bytes = fs::read(&anchor)?;
    let replacement = root.path().join(".lumin/runs/replacement.anchor");
    fs::write(&replacement, bytes)?;
    fs::remove_file(&anchor)?;
    fs::rename(replacement, &anchor)?;

    require_integrity_failure(open_store(root.path()))
}

#[test]
fn rejects_anchor_with_an_extra_hard_link() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    drop(open_store(root.path())?);
    let anchor = root.path().join(".lumin/trash/namespace.anchor");
    fs::hard_link(&anchor, root.path().join(".lumin/trash/anchor.extra"))?;

    require_integrity_failure(open_store(root.path()))
}

#[test]
fn rejects_repository_marker_mutation() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    drop(open_store(root.path())?);
    let marker = root.path().join(".lumin/repository.json");
    fs::write(&marker, b"not the immutable marker")?;

    require_integrity_failure(open_store(root.path()))
}

#[test]
fn resumes_exact_nonce_bound_pre_marker_parents() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    drop(open_store(root.path())?);
    let state = root.path().join(".lumin");
    fs::rename(
        state.join("repository.json"),
        root.path().join("marker.saved"),
    )?;
    fs::rename(
        state.join("lifecycle.store"),
        root.path().join("store.saved"),
    )?;
    fs::rename(state.join("trash"), root.path().join("trash.saved"))?;
    fs::rename(state.join("cache"), root.path().join("cache.saved"))?;

    drop(open_store(root.path())?);
    assert!(state.join("repository.json").is_file());
    assert!(state.join("lifecycle.store").is_file());
    assert!(state.join("trash/namespace.anchor").is_file());
    assert!(state.join("cache/namespace.anchor").is_file());
    Ok(())
}

#[test]
fn pre_marker_recovery_rejects_a_copied_parent() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    drop(open_store(root.path())?);
    let state = root.path().join(".lumin");
    fs::rename(
        state.join("repository.json"),
        root.path().join("marker.saved"),
    )?;
    fs::rename(
        state.join("lifecycle.store"),
        root.path().join("store.saved"),
    )?;
    let runs = state.join("runs");
    fs::rename(&runs, root.path().join("runs.saved"))?;
    fs::create_dir(&runs)?;
    fs::copy(
        root.path().join("runs.saved/namespace.anchor"),
        runs.join("namespace.anchor"),
    )?;

    require_integrity_failure(open_store(root.path()))
}

#[test]
fn parent_swap_cannot_cross_a_guarded_store_commit() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let state = root.path().join(".lumin");
    let protected = store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        {
            let mut table = write.open_table(SEQUENCES).map_err(backend_error)?;
            table
                .insert("parent-swap-probe", 1)
                .map_err(backend_error)?;
        }
        let swapped = try_swap_parent(&state, "runs")?;
        if !swapped {
            return Ok(true);
        }
        let commit = guard.commit(write);
        restore_parent(&state, "runs")?;
        Ok(matches!(commit, Err(StoreError::Integrity(_))))
    })?;
    assert!(protected);

    let committed = store.with_shared_lock(|guard| {
        let database = guard.open_database()?;
        let read = database.begin_read()?;
        let table = match read.open_table(SEQUENCES) {
            Ok(table) => table,
            Err(TableError::TableDoesNotExist(_)) => return Ok(false),
            Err(error) => return Err(backend_error(error)),
        };
        table
            .get("parent-swap-probe")
            .map_err(backend_error)
            .map(|value| value.is_some())
    })?;
    assert!(!committed);
    Ok(())
}

#[test]
fn lock_swap_cannot_cross_a_guarded_store_commit() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let state = root.path().join(".lumin");
    let lock_bytes = fs::read(state.join("lifecycle.lock"))?;
    let protected = store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        {
            let mut table = write.open_table(SEQUENCES).map_err(backend_error)?;
            table.insert("lock-swap-probe", 1).map_err(backend_error)?;
        }
        let swapped = try_replace_lock_with_bytes(&state, &lock_bytes)?;
        if !swapped {
            return Ok(true);
        }
        let commit = guard.commit(write);
        restore_lock(&state)?;
        Ok(matches!(commit, Err(StoreError::Integrity(_))))
    })?;
    assert!(protected);

    let committed = store.with_shared_lock(|guard| {
        let database = guard.open_database()?;
        let read = database.begin_read()?;
        let table = match read.open_table(SEQUENCES) {
            Ok(table) => table,
            Err(TableError::TableDoesNotExist(_)) => return Ok(false),
            Err(error) => return Err(backend_error(error)),
        };
        table
            .get("lock-swap-probe")
            .map_err(backend_error)
            .map(|value| value.is_some())
    })?;
    assert!(!committed);
    Ok(())
}

#[test]
fn state_directory_swap_cannot_cross_a_guarded_store_commit()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let protected = store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        {
            let mut table = write.open_table(SEQUENCES).map_err(backend_error)?;
            table.insert("state-swap-probe", 1).map_err(backend_error)?;
        }
        let swapped = try_replace_state_directory(root.path())?;
        if !swapped {
            return Ok(true);
        }
        let commit = guard.commit(write);
        restore_state_directory(root.path())?;
        Ok(matches!(commit, Err(StoreError::Integrity(_))))
    })?;
    assert!(protected);

    let committed = store.with_shared_lock(|guard| {
        let database = guard.open_database()?;
        let read = database.begin_read()?;
        let table = match read.open_table(SEQUENCES) {
            Ok(table) => table,
            Err(TableError::TableDoesNotExist(_)) => return Ok(false),
            Err(error) => return Err(backend_error(error)),
        };
        table
            .get("state-swap-probe")
            .map_err(backend_error)
            .map(|value| value.is_some())
    })?;
    assert!(!committed);
    Ok(())
}

#[test]
fn parent_swap_cannot_cross_a_guarded_physical_mutation() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let state = root.path().join(".lumin");
    let protected = store.with_exclusive_lock(|guard| {
        let mut swapped = false;
        let mutation = guard.mutate(|| {
            swapped = try_swap_parent(&state, "runs")?;
            if swapped {
                fs::write(state.join("runs/foreign-payload"), b"must not publish")
                    .map_err(io_error)?;
            }
            Ok(())
        });
        if !swapped {
            mutation?;
            return Ok(true);
        }
        restore_parent(&state, "runs")?;
        Ok(matches!(mutation, Err(StoreError::Integrity(_))))
    })?;
    assert!(protected);
    assert!(!state.join("runs/foreign-payload").exists());
    Ok(())
}

fn try_swap_parent(state: &std::path::Path, name: &str) -> Result<bool, StoreError> {
    let parent = state.join(name);
    let original = state.join(format!("{name}.displaced"));
    match fs::rename(&parent, &original) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return Ok(false),
        Err(error) => return Err(io_error(error)),
    }
    fs::create_dir(&parent).map_err(io_error)?;
    fs::copy(
        original.join("namespace.anchor"),
        parent.join("namespace.anchor"),
    )
    .map_err(io_error)?;
    Ok(true)
}

fn restore_parent(state: &std::path::Path, name: &str) -> Result<(), StoreError> {
    fs::remove_dir_all(state.join(name)).map_err(io_error)?;
    fs::rename(state.join(format!("{name}.displaced")), state.join(name)).map_err(io_error)
}

fn replace_lock(state: &std::path::Path) -> Result<(), StoreError> {
    if !try_replace_lock(state)? {
        return Err(StoreError::Integrity(
            "platform prevented lifecycle.lock replacement".to_owned(),
        ));
    }
    Ok(())
}

fn try_replace_lock(state: &std::path::Path) -> Result<bool, StoreError> {
    let lock = state.join("lifecycle.lock");
    let bytes = fs::read(&lock).map_err(io_error)?;
    try_replace_lock_with_bytes(state, &bytes)
}

fn try_replace_lock_with_bytes(state: &std::path::Path, bytes: &[u8]) -> Result<bool, StoreError> {
    let lock = state.join("lifecycle.lock");
    let displaced = state.join("lifecycle.lock.displaced");
    match fs::rename(&lock, &displaced) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return Ok(false),
        Err(error) => return Err(io_error(error)),
    }
    fs::write(&lock, bytes).map_err(io_error)?;
    Ok(true)
}

fn restore_lock(state: &std::path::Path) -> Result<(), StoreError> {
    fs::remove_file(state.join("lifecycle.lock")).map_err(io_error)?;
    fs::rename(
        state.join("lifecycle.lock.displaced"),
        state.join("lifecycle.lock"),
    )
    .map_err(io_error)
}

fn try_replace_state_directory(root: &std::path::Path) -> Result<bool, StoreError> {
    let state = root.join(".lumin");
    let displaced = root.join(".lumin.displaced");
    match fs::rename(&state, &displaced) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return Ok(false),
        Err(error) => return Err(io_error(error)),
    }
    fs::create_dir(&state).map_err(io_error)?;
    Ok(true)
}

fn restore_state_directory(root: &std::path::Path) -> Result<(), StoreError> {
    fs::remove_dir(root.join(".lumin")).map_err(io_error)?;
    fs::rename(root.join(".lumin.displaced"), root.join(".lumin")).map_err(io_error)
}

fn copy_directory(source: &std::path::Path, target: &std::path::Path) -> std::io::Result<()> {
    fs::create_dir(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let destination = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory(&entry.path(), &destination)?;
        } else {
            fs::copy(entry.path(), destination)?;
        }
    }
    Ok(())
}
