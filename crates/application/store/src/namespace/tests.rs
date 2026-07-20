use std::fs;

use redb::{ReadableDatabase, TableError};

use crate::{RepositoryStore, SEQUENCES, StoreError, backend_error, io_error};

use super::MANAGED_KINDS;

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
    fs::write(root.path().join(".lumin"), b"not a directory")?;

    require_integrity_failure(RepositoryStore::open(root.path()))
}

#[test]
fn rejects_copied_real_parent_for_every_managed_kind() -> Result<(), Box<dyn std::error::Error>> {
    for kind in MANAGED_KINDS {
        let root = tempfile::tempdir()?;
        drop(RepositoryStore::open(root.path())?);
        let state_dir = root.path().join(".lumin");
        let parent = state_dir.join(kind.directory_name());
        let original = state_dir.join(format!("{}.original", kind.directory_name()));
        fs::rename(&parent, &original)?;
        fs::create_dir(&parent)?;
        fs::copy(
            original.join("namespace.anchor"),
            parent.join("namespace.anchor"),
        )?;

        require_integrity_failure(RepositoryStore::open(root.path()))?;
    }
    Ok(())
}

#[test]
fn rejects_byte_identical_anchor_replacement() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    drop(RepositoryStore::open(root.path())?);
    let anchor = root.path().join(".lumin/runs/namespace.anchor");
    let bytes = fs::read(&anchor)?;
    fs::remove_file(&anchor)?;
    fs::write(&anchor, bytes)?;

    require_integrity_failure(RepositoryStore::open(root.path()))
}

#[test]
fn rejects_anchor_with_an_extra_hard_link() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    drop(RepositoryStore::open(root.path())?);
    let anchor = root.path().join(".lumin/trash/namespace.anchor");
    fs::hard_link(&anchor, root.path().join(".lumin/trash/anchor.extra"))?;

    require_integrity_failure(RepositoryStore::open(root.path()))
}

#[test]
fn rejects_repository_marker_mutation() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    drop(RepositoryStore::open(root.path())?);
    let marker = root.path().join(".lumin/repository.json");
    fs::write(&marker, b"not the immutable marker")?;

    require_integrity_failure(RepositoryStore::open(root.path()))
}

#[test]
fn resumes_exact_nonce_bound_pre_marker_parents() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    drop(RepositoryStore::open(root.path())?);
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

    drop(RepositoryStore::open(root.path())?);
    assert!(state.join("repository.json").is_file());
    assert!(state.join("lifecycle.store").is_file());
    assert!(state.join("trash/namespace.anchor").is_file());
    assert!(state.join("cache/namespace.anchor").is_file());
    Ok(())
}

#[test]
fn pre_marker_recovery_rejects_a_copied_parent() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    drop(RepositoryStore::open(root.path())?);
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

    require_integrity_failure(RepositoryStore::open(root.path()))
}

#[test]
fn parent_swap_cannot_cross_a_guarded_store_commit() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = RepositoryStore::open(root.path())?;
    let state = root.path().join(".lumin");
    let protected = store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write().map_err(backend_error)?;
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
        let commit = guard.commit(&database, write);
        restore_parent(&state, "runs")?;
        Ok(matches!(commit, Err(StoreError::Integrity(_))))
    })?;
    assert!(protected);

    let committed = store.with_shared_lock(|guard| {
        let database = guard.open_database()?;
        let read = database.begin_read().map_err(backend_error)?;
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
fn parent_swap_cannot_cross_a_guarded_physical_mutation() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    let store = RepositoryStore::open(root.path())?;
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
