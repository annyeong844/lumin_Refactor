mod records;
mod recovery;

use std::fs;

use fs2::FileExt;
use lumin_model::{AttemptId, AttemptStatus};

use super::{AttemptEnvelope, attempt_directory, attempt_path, files, latest};
use crate::namespace::{self, NamespaceGuard, entry_exists, records::ManagedStateParentKind};
use crate::{RepositoryStore, StoreError, StoreGeneration, io_error, nonce_hex, unix_millis};

use records::{AttemptLeaseRecord, AttemptLeaseState};

pub struct AttemptSession<'store> {
    store: &'store RepositoryStore,
    lease: AttemptLeaseRecord,
    generation: StoreGeneration,
    lock_file: Option<namespace::HeldEntry>,
}

pub(super) fn begin(store: &RepositoryStore) -> Result<AttemptSession<'_>, StoreError> {
    store.with_exclusive_lock(|guard| {
        latest::ensure(store, guard)?;
        recovery::recover_under_guard(store, guard)?;

        let lease_nonce = nonce_hex()?;
        let lock_name = format!("attempt-liveness-{lease_nonce}.lock");
        hit_before_allocation();
        let allocation =
            records::reserve(guard, lock_name.clone(), lease_nonce, std::process::id())?;
        let lock_file = guard.create_state_file(&lock_name, "attempt process-liveness lock")?;
        lock_file.file().try_lock_exclusive().map_err(io_error)?;
        hit_after_lock_creation();
        let lease = records::activate(guard, &lock_file, &allocation)?;
        hit_after_allocation();

        create_attempt_directory(store, guard, &lease.attempt_id, lease.generation)?;
        let envelope = AttemptEnvelope {
            schema_version: "lumin-attempt.v1".to_owned(),
            attempt_id: lease.attempt_id.clone(),
            sequence: lease.sequence,
            state: AttemptStatus::Running,
            started_unix_millis: unix_millis()?,
            finished_unix_millis: None,
            run_id: None,
            failure: None,
        };
        let directory = guard.open_managed_child_directory(
            ManagedStateParentKind::Attempts,
            lease.attempt_id.as_str(),
            "attempt directory",
        )?;
        files::write_json(
            &attempt_path(store, &lease.attempt_id),
            &directory,
            "attempt envelope",
            &envelope,
        )?;
        hit_after_running();

        latest::publish_attempt(store, guard, &envelope, false)?;
        hit_after_latest_running();
        Ok(AttemptSession {
            store,
            generation: lease.generation,
            lease,
            lock_file: Some(lock_file),
        })
    })
}

pub(super) fn finish_failed(
    store: &RepositoryStore,
    session: &mut AttemptSession<'_>,
    failure: &str,
) -> Result<(), StoreError> {
    if failure.is_empty() {
        return Err(StoreError::Integrity(
            "failed attempt requires a non-empty failure".to_owned(),
        ));
    }
    if !std::ptr::eq(store, session.store) {
        return Err(StoreError::Integrity(
            "attempt session belongs to another repository store".to_owned(),
        ));
    }
    store.with_exclusive_lock(|guard| {
        session.validate(guard)?;
        let mut envelope = latest::read_attempt(store, guard, &session.lease.attempt_id)?;
        require_running(&envelope, &session.lease)?;
        envelope.state = AttemptStatus::Failed;
        envelope.finished_unix_millis = Some(unix_millis()?);
        envelope.failure = Some(failure.to_owned());
        write_terminal(store, guard, session.generation, &envelope)?;
        latest::publish_attempt(store, guard, &envelope, false)?;
        release_session(store, guard, session)
    })
}

pub(super) fn recover(store: &RepositoryStore) -> Result<(), StoreError> {
    store.with_exclusive_lock(|guard| {
        latest::ensure(store, guard)?;
        recovery::recover_under_guard(store, guard)
    })
}

pub(super) fn validate_snapshot(
    rows: &std::collections::BTreeMap<String, Vec<u8>>,
) -> Result<(), StoreError> {
    records::validate_snapshot(rows)
}

pub(super) fn validate_snapshot_locks(
    rows: &std::collections::BTreeMap<String, Vec<u8>>,
    guard: &NamespaceGuard,
) -> Result<(), StoreError> {
    records::validate_snapshot_locks(rows, guard)
}

pub(super) fn has_active_lease(
    guard: &NamespaceGuard,
    attempt_id: &AttemptId,
) -> Result<bool, StoreError> {
    let Some(lease) = records::read(guard, attempt_id)? else {
        return Ok(false);
    };
    if lease.state == AttemptLeaseState::Allocating {
        return Ok(false);
    }
    let file = guard.open_state_file(&lease.lock_name, "attempt process-liveness lock")?;
    records::validate_lock_identity(guard, &file, &lease)?;
    Ok(lease.state == AttemptLeaseState::Active)
}

impl AttemptSession<'_> {
    pub(crate) fn attempt_id(&self) -> &AttemptId {
        &self.lease.attempt_id
    }

    pub(crate) fn generation(&self) -> StoreGeneration {
        self.generation
    }

    pub(super) fn belongs_to(&self, store: &RepositoryStore) -> bool {
        std::ptr::eq(store, self.store)
    }

    pub(super) fn require_running(&self, envelope: &AttemptEnvelope) -> Result<(), StoreError> {
        require_running(envelope, &self.lease)
    }

    pub(super) fn validate(&self, guard: &NamespaceGuard) -> Result<(), StoreError> {
        if self.lease.state != AttemptLeaseState::Active {
            return Err(StoreError::Integrity(format!(
                "attempt session is no longer active: {}",
                self.lease.attempt_id.as_str()
            )));
        }
        let database = guard.open_database_for_generation(self.generation)?;
        drop(database);
        let persisted = records::read(guard, &self.lease.attempt_id)?.ok_or_else(|| {
            StoreError::Integrity(format!(
                "attempt process-liveness lease is missing: {}",
                self.lease.attempt_id.as_str()
            ))
        })?;
        if persisted != self.lease {
            return Err(StoreError::Integrity(format!(
                "attempt process-liveness lease changed: {}",
                self.lease.attempt_id.as_str()
            )));
        }
        let lock_file = self.lock_file.as_ref().ok_or_else(|| {
            StoreError::Integrity(format!(
                "attempt session released its liveness lock: {}",
                self.lease.attempt_id.as_str()
            ))
        })?;
        records::validate_lock(guard, lock_file, &self.lease)
    }
}

pub(super) fn release_session(
    store: &RepositoryStore,
    guard: &NamespaceGuard,
    session: &mut AttemptSession<'_>,
) -> Result<(), StoreError> {
    session.validate(guard)?;
    session.lease = records::mark_releasing(guard, &session.lease)?;
    recovery::finish_releasing(store, guard, &session.lease, session.lock_file.take())
}

fn require_running(
    envelope: &AttemptEnvelope,
    lease: &AttemptLeaseRecord,
) -> Result<(), StoreError> {
    if envelope.attempt_id != lease.attempt_id
        || envelope.sequence != lease.sequence
        || envelope.state != AttemptStatus::Running
    {
        return Err(StoreError::Integrity(format!(
            "attempt session does not own a running envelope: {}",
            lease.attempt_id.as_str()
        )));
    }
    Ok(())
}

pub(super) fn write_terminal(
    store: &RepositoryStore,
    guard: &NamespaceGuard,
    generation: StoreGeneration,
    envelope: &AttemptEnvelope,
) -> Result<(), StoreError> {
    latest::validate_attempt_envelope(envelope)?;
    let directory = guard.open_managed_child_directory(
        ManagedStateParentKind::Attempts,
        envelope.attempt_id.as_str(),
        "attempt directory",
    )?;
    guard.mutate_for_generation(generation, || {
        files::write_json(
            &attempt_path(store, &envelope.attempt_id),
            &directory,
            "attempt envelope",
            envelope,
        )
    })
}

fn create_attempt_directory(
    store: &RepositoryStore,
    guard: &NamespaceGuard,
    attempt_id: &AttemptId,
    generation: StoreGeneration,
) -> Result<(), StoreError> {
    let path = attempt_directory(store, attempt_id);
    if entry_exists(&path)? {
        return Err(StoreError::Integrity(format!(
            "attempt directory already exists: {}",
            attempt_id.as_str()
        )));
    }
    let parent = guard.managed_parent_entry(ManagedStateParentKind::Attempts)?;
    guard.mutate_for_generation(generation, || {
        fs::create_dir(&path).map_err(io_error)?;
        parent.sync_directory()
    })
}

fn hit_before_allocation() {
    #[cfg(feature = "publication-test-crash")]
    super::crash::hit(super::crash::PublicationCrashPoint::BeforeAttemptCatalogAllocation);
}

fn hit_after_allocation() {
    #[cfg(feature = "publication-test-crash")]
    super::crash::hit(super::crash::PublicationCrashPoint::AfterCatalogAllocation);
}

fn hit_after_lock_creation() {
    #[cfg(feature = "publication-test-crash")]
    super::crash::hit(super::crash::PublicationCrashPoint::AfterAttemptLockCreation);
}

fn hit_after_running() {
    #[cfg(feature = "publication-test-crash")]
    super::crash::hit(super::crash::PublicationCrashPoint::AfterRunningEnvelope);
}

fn hit_after_latest_running() {
    #[cfg(feature = "publication-test-crash")]
    super::crash::hit(super::crash::PublicationCrashPoint::AfterLatestRunning);
}
