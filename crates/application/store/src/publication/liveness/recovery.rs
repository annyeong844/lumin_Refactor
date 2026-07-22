use std::fs;

use fs2::FileExt;
use lumin_model::AttemptStatus;

use super::super::{AttemptEnvelope, files, latest};
use super::records::{self, AttemptLeaseRecord, AttemptLeaseState};
use crate::namespace::{
    HeldEntry, NamespaceGuard, entry_exists, lock_contended, records::ManagedStateParentKind,
};
use crate::{RepositoryStore, StoreError, io_error, unix_millis};

pub(super) fn recover_under_guard(
    store: &RepositoryStore,
    guard: &NamespaceGuard,
) -> Result<(), StoreError> {
    let leases = records::read_all(guard)?;
    for lease in leases {
        if lease.state == AttemptLeaseState::Releasing {
            let lock = acquire_releasing_lock(store, guard, &lease)?;
            recover_one(store, guard, &lease, false)?;
            finish_releasing(store, guard, &lease, lock)?;
            continue;
        }
        let lock = guard.open_state_file(&lease.lock_name, "attempt process-liveness lock")?;
        records::validate_lock_identity(guard, &lock, &lease)?;
        match lock.file().try_lock_exclusive() {
            Ok(()) => {}
            Err(error) if lock_contended(&error) => continue,
            Err(error) => return Err(io_error(error)),
        }
        records::validate_lock(guard, &lock, &lease)?;
        recover_one(store, guard, &lease, true)?;
        let releasing = records::mark_releasing(guard, &lease)?;
        finish_releasing(store, guard, &releasing, Some(lock))?;
    }
    Ok(())
}

fn recover_one(
    store: &RepositoryStore,
    guard: &NamespaceGuard,
    lease: &AttemptLeaseRecord,
    interrupt_running: bool,
) -> Result<(), StoreError> {
    let path = super::super::attempt_path(store, &lease.attempt_id);
    if !entry_exists(&path)? {
        remove_gap_directory(store, guard, lease)?;
        return Ok(());
    }

    let directory = guard.open_managed_child_directory(
        ManagedStateParentKind::Attempts,
        lease.attempt_id.as_str(),
        "attempt directory",
    )?;
    remove_pending_envelope(&path.with_extension("json.pending"), &directory, lease)?;
    let mut envelope = latest::read_attempt(store, guard, &lease.attempt_id)?;
    if envelope.sequence != lease.sequence {
        return Err(StoreError::Integrity(format!(
            "attempt lease sequence disagrees with its envelope: {}",
            lease.attempt_id.as_str()
        )));
    }
    match envelope.state {
        AttemptStatus::Running => {
            if !interrupt_running {
                return Err(StoreError::Integrity(format!(
                    "releasing attempt still has a running envelope: {}",
                    lease.attempt_id.as_str()
                )));
            }
            envelope.state = AttemptStatus::Interrupted;
            envelope.finished_unix_millis = Some(unix_millis()?);
            envelope.failure =
                Some("attempt owner process exited before terminal publication".to_owned());
            let generation = {
                let database = guard.open_database()?;
                database.generation()
            };
            super::write_terminal(store, guard, generation, &envelope)?;
        }
        AttemptStatus::Completed => {
            super::super::run::recover_completed(store, guard, &envelope)?;
        }
        AttemptStatus::Failed | AttemptStatus::Interrupted => {}
    }
    latest::publish_attempt(store, guard, &envelope, false)
}

fn remove_gap_directory(
    store: &RepositoryStore,
    guard: &NamespaceGuard,
    lease: &AttemptLeaseRecord,
) -> Result<(), StoreError> {
    let directory_path = super::super::attempt_directory(store, &lease.attempt_id);
    if !entry_exists(&directory_path)? {
        return Ok(());
    }
    let directory = guard.open_managed_child_directory(
        ManagedStateParentKind::Attempts,
        lease.attempt_id.as_str(),
        "unstarted attempt directory",
    )?;
    remove_pending_envelope(
        &directory_path.join("attempt.json.pending"),
        &directory,
        lease,
    )?;
    if fs::read_dir(&directory_path)
        .map_err(io_error)?
        .next()
        .transpose()
        .map_err(io_error)?
        .is_some()
    {
        return Err(StoreError::Integrity(format!(
            "unstarted attempt directory is not empty: {}",
            lease.attempt_id.as_str()
        )));
    }
    drop(directory);
    fs::remove_dir(&directory_path).map_err(io_error)?;
    guard
        .managed_parent_entry(ManagedStateParentKind::Attempts)?
        .sync_directory()
}

pub(super) fn finish_releasing(
    store: &RepositoryStore,
    guard: &NamespaceGuard,
    lease: &AttemptLeaseRecord,
    lock: Option<HeldEntry>,
) -> Result<(), StoreError> {
    if lease.state != AttemptLeaseState::Releasing {
        return Err(StoreError::Integrity(format!(
            "attempt lease is not releasing: {}",
            lease.attempt_id.as_str()
        )));
    }
    let lock_path = store.state_dir.join(&lease.lock_name);
    match lock {
        Some(lock) => {
            records::validate_lock(guard, &lock, lease)?;
            lock.validate_path(
                &lock_path,
                crate::namespace::EntryKind::RegularFile,
                crate::namespace::EntryAccess::ReadWrite,
                true,
                "attempt process-liveness lock",
            )?;
            drop(lock);
            fs::remove_file(&lock_path).map_err(io_error)?;
            guard.state_directory_entry().sync_directory()?;
        }
        None if entry_exists(&lock_path)? => {
            return Err(StoreError::Integrity(format!(
                "releasing attempt omitted its liveness lock handle: {}",
                lease.attempt_id.as_str()
            )));
        }
        None => {}
    }
    records::remove(guard, lease)
}

fn acquire_releasing_lock(
    store: &RepositoryStore,
    guard: &NamespaceGuard,
    lease: &AttemptLeaseRecord,
) -> Result<Option<HeldEntry>, StoreError> {
    let path = store.state_dir.join(&lease.lock_name);
    if !entry_exists(&path)? {
        return Ok(None);
    }
    let lock = guard.open_state_file(&lease.lock_name, "attempt process-liveness lock")?;
    records::validate_lock_identity(guard, &lock, lease)?;
    lock.file().try_lock_exclusive().map_err(io_error)?;
    records::validate_lock(guard, &lock, lease)?;
    Ok(Some(lock))
}

fn remove_pending_envelope(
    path: &std::path::Path,
    directory: &HeldEntry,
    lease: &AttemptLeaseRecord,
) -> Result<(), StoreError> {
    files::validate_and_remove_pending(
        path,
        directory,
        "attempt envelope pending file",
        |envelope: &AttemptEnvelope| {
            latest::validate_attempt_envelope(envelope)?;
            if envelope.attempt_id != lease.attempt_id || envelope.sequence != lease.sequence {
                return Err(StoreError::Integrity(format!(
                    "pending attempt envelope disagrees with its lease: {}",
                    lease.attempt_id.as_str()
                )));
            }
            Ok(())
        },
    )
}
