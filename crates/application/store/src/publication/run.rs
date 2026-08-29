use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use lumin_evidence::RunEvidence;
use lumin_model::{AttemptStatus, RunId, digest_hex};

use super::{AttemptEnvelope, files, latest, liveness, run_id};
use crate::namespace::{
    EntryAccess, EntryKind, HeldEntry, NamespaceGuard, entry_exists, move_entry_noreplace,
    records::ManagedStateParentKind,
};
use crate::{
    PublishedRun, RepositoryStore, RunCatalogRecord, StoreError, insert_catalog_record, io_error,
    read_evidence_store, unix_millis, write_evidence_store,
};

pub(super) fn publish(
    store: &RepositoryStore,
    session: &mut liveness::AttemptSession<'_>,
    evidence: &RunEvidence,
    final_validation: impl FnOnce(
        &std::collections::BTreeSet<lumin_model::PhysicalFileIdentity>,
    ) -> Result<(), StoreError>,
) -> Result<PublishedRun, StoreError> {
    if !session.belongs_to(store) {
        return Err(StoreError::Integrity(
            "attempt session belongs to another repository store".to_owned(),
        ));
    }
    let (envelope, record) = prepare_publication(store, session, evidence, final_validation)?;
    #[cfg(feature = "publication-test-crash")]
    super::barrier::wait_prepared(session.attempt_id())?;

    finalize_publication(store, session, &envelope, &record)
}

fn prepare_publication(
    store: &RepositoryStore,
    session: &liveness::AttemptSession<'_>,
    evidence: &RunEvidence,
    final_validation: impl FnOnce(
        &std::collections::BTreeSet<lumin_model::PhysicalFileIdentity>,
    ) -> Result<(), StoreError>,
) -> Result<(AttemptEnvelope, RunCatalogRecord), StoreError> {
    store.with_shared_lock(|guard| {
        session
            .validate(guard)
            .map_err(|error| publication_error("validate attempt session", error))?;
        let mut envelope = latest::read_attempt(store, guard, session.attempt_id())
            .map_err(|error| publication_error("read running attempt", error))?;
        session.require_running(&envelope)?;
        let reserved_state_identities = guard.reserved_state_identities()?;
        final_validation(&reserved_state_identities)
            .map_err(|error| publication_error("validate run publication inputs", error))?;

        let record = publish_directory(store, guard, &envelope, evidence, session.generation())
            .map_err(|error| publication_error("publish run directory", error))?;
        hit_after_run_rename();

        envelope.state = AttemptStatus::Completed;
        envelope.finished_unix_millis = Some(unix_millis()?);
        envelope.run_id = Some(record.run_id.clone());
        liveness::write_terminal(store, guard, session.generation(), &envelope)
            .map_err(|error| publication_error("publish terminal attempt", error))?;
        hit_after_terminal_attempt();
        Ok((envelope, record))
    })
}

fn finalize_publication(
    store: &RepositoryStore,
    session: &mut liveness::AttemptSession<'_>,
    expected_envelope: &AttemptEnvelope,
    expected_record: &RunCatalogRecord,
) -> Result<PublishedRun, StoreError> {
    #[cfg(feature = "publication-test-crash")]
    let attempt_id = session.attempt_id().clone();

    #[cfg(feature = "publication-test-crash")]
    {
        store.with_exclusive_lock_after_contention(
            || super::barrier::wait_contended(&attempt_id),
            |guard| finalize_under_guard(store, guard, session, expected_envelope, expected_record),
        )
    }
    #[cfg(not(feature = "publication-test-crash"))]
    {
        store.with_exclusive_lock(|guard| {
            finalize_under_guard(store, guard, session, expected_envelope, expected_record)
        })
    }
}

fn finalize_under_guard(
    store: &RepositoryStore,
    guard: &NamespaceGuard,
    session: &mut liveness::AttemptSession<'_>,
    expected_envelope: &AttemptEnvelope,
    expected_record: &RunCatalogRecord,
) -> Result<PublishedRun, StoreError> {
    session
        .validate(guard)
        .map_err(|error| publication_error("validate attempt session", error))?;
    ensure_target_available_or_release(store, guard, session, expected_record)?;
    let (envelope, record) = revalidate_publication_candidate(
        store,
        guard,
        session,
        expected_envelope,
        expected_record,
    )?;

    let database = guard.open_database_for_generation(session.generation())?;
    insert_catalog_record(guard, &database, &record)
        .map_err(|error| publication_error("publish run catalog", error))?;
    drop(database);
    latest::publish_attempt(store, guard, &envelope, true)
        .map_err(|error| publication_error("publish latest pointer", error))?;
    liveness::release_session(store, guard, session)
        .map_err(|error| publication_error("release attempt lease", error))?;
    Ok(PublishedRun {
        attempt_id: record.attempt_id,
        run_id: record.run_id,
        sequence: record.sequence,
    })
}

fn ensure_target_available_or_release(
    store: &RepositoryStore,
    guard: &NamespaceGuard,
    session: &mut liveness::AttemptSession<'_>,
    expected_record: &RunCatalogRecord,
) -> Result<(), StoreError> {
    match crate::retention::ensure_publication_target_available(
        guard,
        &expected_record.attempt_id,
        &expected_record.run_id,
    ) {
        Ok(()) => {}
        Err(error @ StoreError::RunRetentionState(_)) => {
            liveness::release_session(store, guard, session)
                .map_err(|cleanup| publication_error("release retained attempt lease", cleanup))?;
            return Err(error);
        }
        Err(error) => return Err(error),
    }
    Ok(())
}

fn revalidate_publication_candidate(
    store: &RepositoryStore,
    guard: &NamespaceGuard,
    session: &liveness::AttemptSession<'_>,
    expected_envelope: &AttemptEnvelope,
    expected_record: &RunCatalogRecord,
) -> Result<(AttemptEnvelope, RunCatalogRecord), StoreError> {
    let envelope = latest::read_attempt(store, guard, session.attempt_id())
        .map_err(|error| publication_error("read terminal attempt", error))?;
    if &envelope != expected_envelope {
        return Err(StoreError::Integrity(format!(
            "terminal attempt changed before catalog publication: {}",
            expected_envelope.attempt_id.as_str()
        )));
    }
    let record = validate_published(guard, &envelope)
        .map_err(|error| publication_error("revalidate published run", error))?;
    if !same_record(&record, expected_record) {
        return Err(StoreError::Integrity(format!(
            "run catalog candidate changed before publication: {}",
            expected_record.run_id.as_str()
        )));
    }
    Ok((envelope, record))
}

fn same_record(left: &RunCatalogRecord, right: &RunCatalogRecord) -> bool {
    left.attempt_id == right.attempt_id
        && left.run_id == right.run_id
        && left.sequence == right.sequence
        && left.evidence_store_sha256 == right.evidence_store_sha256
        && left.evidence_store_size == right.evidence_store_size
}

pub(super) fn recover_completed(
    guard: &NamespaceGuard,
    envelope: &AttemptEnvelope,
) -> Result<(), StoreError> {
    if envelope.state != AttemptStatus::Completed {
        return Err(StoreError::Integrity(format!(
            "non-completed attempt cannot recover a run: {}",
            envelope.attempt_id.as_str()
        )));
    }
    if let Some(run_id) = envelope.run_id.as_ref() {
        crate::retention::ensure_publication_target_available(guard, &envelope.attempt_id, run_id)?;
    }
    let record = validate_published(guard, envelope)?;
    let database = guard.open_database()?;
    insert_catalog_record(guard, &database, &record)
}

fn publish_directory(
    store: &RepositoryStore,
    guard: &NamespaceGuard,
    envelope: &AttemptEnvelope,
    evidence: &RunEvidence,
    generation: crate::StoreGeneration,
) -> Result<RunCatalogRecord, StoreError> {
    let run_id = run_id(envelope.sequence);
    let parent = guard.managed_parent_entry(ManagedStateParentKind::Runs)?;
    let staging = staging_path(store, &run_id);
    let published = run_path(store, &run_id);
    if entry_exists(&staging)? || entry_exists(&published)? {
        return Err(StoreError::Integrity(format!(
            "run publication target already exists: {}",
            run_id.as_str()
        )));
    }

    guard
        .mutate_for_generation(generation, || {
            fs::create_dir(&staging).map_err(io_error)?;
            parent.sync_directory()
        })
        .map_err(|error| publication_error("create staging directory", error))?;
    let staging_write_entry = HeldEntry::open(
        &staging,
        EntryKind::Directory,
        EntryAccess::ReadOnly,
        false,
        "run staging directory",
    )?;
    files::require_parent_volume(&staging_write_entry, parent, "run staging directory")?;
    let record = write_staging(&staging, &staging_write_entry, envelope, &run_id, evidence)
        .map_err(|error| publication_error("write staging payload", error))?;
    validate_directory(&staging, &staging_write_entry, &record)
        .map_err(|error| publication_error("validate staging payload", error))?;
    let staging_entry = HeldEntry::open(
        &staging,
        EntryKind::Directory,
        EntryAccess::Move,
        false,
        "run staging directory before publication",
    )?;
    if staging_entry.identity() != staging_write_entry.identity() {
        return Err(StoreError::Integrity(
            "run staging directory changed before publication".to_owned(),
        ));
    }
    files::require_parent_volume(&staging_entry, parent, "run staging directory")?;
    drop(staging_write_entry);

    guard
        .mutate_for_generation(generation, || {
            #[cfg(feature = "namespace-test-crash")]
            crate::namespace::barrier::wait_before_run_rename()?;
            validate_directory(&staging, &staging_entry, &record)
                .map_err(|error| publication_error("revalidate staging payload", error))?;
            guard.validate_bound_entries()?;
            staging_entry.validate_path(
                &staging,
                EntryKind::Directory,
                EntryAccess::Move,
                false,
                "run staging directory before publication",
            )?;
            move_entry_noreplace(
                parent,
                staging.file_name().ok_or_else(|| {
                    StoreError::Integrity("run staging path has no child name".to_owned())
                })?,
                &staging_entry,
                parent,
                OsStr::new(run_id.as_str()),
            )?;
            parent.sync_directory()?;
            staging_entry.validate_path(
                &published,
                EntryKind::Directory,
                EntryAccess::Move,
                false,
                "published run directory",
            )
        })
        .map_err(|error| publication_error("rename staging directory", error))?;
    validate_directory(&published, &staging_entry, &record)
        .map_err(|error| publication_error("validate published run", error))?;
    Ok(record)
}

fn write_staging(
    staging: &Path,
    staging_entry: &HeldEntry,
    envelope: &AttemptEnvelope,
    run_id: &RunId,
    evidence: &RunEvidence,
) -> Result<RunCatalogRecord, StoreError> {
    let evidence_path = staging.join("evidence.store");
    write_evidence_store(&evidence_path, evidence)
        .map_err(|error| publication_error("create evidence store", error))?;
    let evidence_entry = HeldEntry::open(
        &evidence_path,
        EntryKind::RegularFile,
        EntryAccess::ReadWrite,
        true,
        "run evidence store",
    )
    .map_err(|error| publication_error("open evidence store", error))?;
    files::require_parent_volume(&evidence_entry, staging_entry, "run evidence store")
        .map_err(|error| publication_error("bind evidence store", error))?;
    evidence_entry
        .sync()
        .map_err(|error| publication_error("flush evidence store", error))?;
    let evidence_bytes = evidence_entry
        .read_all()
        .map_err(|error| publication_error("hash evidence store", error))?;
    let record = RunCatalogRecord {
        attempt_id: envelope.attempt_id.clone(),
        run_id: run_id.clone(),
        sequence: envelope.sequence,
        evidence_store_sha256: digest_hex(&evidence_bytes),
        evidence_store_size: evidence_bytes.len() as u64,
    };
    files::write_json(
        &staging.join("run.json"),
        staging_entry,
        "run envelope",
        &record,
    )
    .map_err(|error| publication_error("write run envelope", error))?;
    staging_entry
        .sync_directory()
        .map_err(|error| publication_error("flush staging directory", error))?;
    Ok(record)
}

pub(super) fn validate_published(
    guard: &NamespaceGuard,
    envelope: &AttemptEnvelope,
) -> Result<RunCatalogRecord, StoreError> {
    let run_id = envelope.run_id.as_ref().ok_or_else(|| {
        StoreError::Integrity(format!(
            "completed attempt omitted its run ID: {}",
            envelope.attempt_id.as_str()
        ))
    })?;
    let directory = guard.open_managed_child_directory(
        ManagedStateParentKind::Runs,
        run_id.as_str(),
        "published run directory",
    )?;
    let path = guard
        .managed_parent_path(ManagedStateParentKind::Runs)
        .join(run_id.as_str());
    let record: RunCatalogRecord =
        files::read_json(&path.join("run.json"), &directory, "run envelope")?;
    if record.attempt_id != envelope.attempt_id
        || record.run_id != *run_id
        || record.sequence != envelope.sequence
    {
        return Err(StoreError::Integrity(format!(
            "published run disagrees with completed attempt: {}",
            envelope.attempt_id.as_str()
        )));
    }
    validate_directory(&path, &directory, &record)?;
    Ok(record)
}

pub(crate) fn read_validated_directory(
    directory_path: &Path,
    directory: &HeldEntry,
) -> Result<RunCatalogRecord, StoreError> {
    let record: RunCatalogRecord =
        files::read_json(&directory_path.join("run.json"), directory, "run envelope")?;
    validate_directory(directory_path, directory, &record)?;
    Ok(record)
}

pub(crate) fn validate_directory(
    directory_path: &Path,
    directory: &HeldEntry,
    expected: &RunCatalogRecord,
) -> Result<(), StoreError> {
    validate_directory_with_hooks_and_evidence(
        directory_path,
        directory,
        expected,
        || Ok(()),
        || Ok(()),
        || Ok(()),
        |_| Ok(()),
    )
}

pub(crate) fn validate_directory_for_migration(
    directory_path: &Path,
    directory: &HeldEntry,
    expected: &RunCatalogRecord,
) -> Result<(), StoreError> {
    validate_directory_with_hooks_and_evidence(
        directory_path,
        directory,
        expected,
        || Ok(()),
        || Ok(()),
        || Ok(()),
        |evidence| {
            lumin_evidence::validate_migration_run_evidence(evidence).map_err(|error| {
                StoreError::Integrity(format!(
                    "legacy run evidence cannot be authenticated for migration: {error}"
                ))
            })
        },
    )
}

#[cfg(test)]
pub(crate) fn validate_directory_with_evidence_read_hook(
    directory_path: &Path,
    directory: &HeldEntry,
    expected: &RunCatalogRecord,
    after_evidence_read: impl FnOnce() -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    validate_directory_with_hooks_and_evidence(
        directory_path,
        directory,
        expected,
        after_evidence_read,
        || Ok(()),
        || Ok(()),
        |_| Ok(()),
    )
}

#[cfg(test)]
pub(crate) fn validate_directory_with_inventory_hooks(
    directory_path: &Path,
    directory: &HeldEntry,
    expected: &RunCatalogRecord,
    before_final_inventory: impl FnOnce() -> Result<(), StoreError>,
    after_final_inventory: impl FnOnce() -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    validate_directory_with_hooks_and_evidence(
        directory_path,
        directory,
        expected,
        || Ok(()),
        before_final_inventory,
        after_final_inventory,
        |_| Ok(()),
    )
}

fn validate_directory_with_hooks_and_evidence(
    directory_path: &Path,
    directory: &HeldEntry,
    expected: &RunCatalogRecord,
    after_evidence_read: impl FnOnce() -> Result<(), StoreError>,
    before_final_inventory: impl FnOnce() -> Result<(), StoreError>,
    after_final_inventory: impl FnOnce() -> Result<(), StoreError>,
    validate_evidence: impl FnOnce(&RunEvidence) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    validate_directory_inventory(directory, expected)?;
    let observed: RunCatalogRecord =
        files::read_json(&directory_path.join("run.json"), directory, "run envelope")?;
    if observed.attempt_id != expected.attempt_id
        || observed.run_id != expected.run_id
        || observed.sequence != expected.sequence
        || observed.evidence_store_sha256 != expected.evidence_store_sha256
        || observed.evidence_store_size != expected.evidence_store_size
    {
        return Err(StoreError::Integrity(format!(
            "run envelope changed during publication: {}",
            expected.run_id.as_str()
        )));
    }
    let evidence_path = directory_path.join("evidence.store");
    let evidence = HeldEntry::open(
        &evidence_path,
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        true,
        "run evidence store",
    )?;
    files::require_parent_volume(&evidence, directory, "run evidence store")?;
    let bytes = validate_evidence_store_identity(&evidence, expected)?;
    after_evidence_read()?;
    let run_evidence = read_evidence_store(&bytes)?;
    validate_evidence(&run_evidence)?;
    evidence.validate_path(
        &evidence_path,
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        true,
        "run evidence store",
    )?;
    directory.validate_path(
        directory_path,
        EntryKind::Directory,
        EntryAccess::ReadOnly,
        false,
        "run directory",
    )?;
    before_final_inventory()?;
    validate_directory_inventory(directory, expected)?;
    after_final_inventory()?;
    evidence.validate_path(
        &evidence_path,
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        true,
        "run evidence store",
    )?;
    validate_evidence_store_identity(&evidence, expected)?;
    directory.validate_path(
        directory_path,
        EntryKind::Directory,
        EntryAccess::ReadOnly,
        false,
        "run directory",
    )?;
    validate_directory_inventory(directory, expected)
}

fn validate_evidence_store_identity(
    evidence: &HeldEntry,
    expected: &RunCatalogRecord,
) -> Result<Vec<u8>, StoreError> {
    let bytes = evidence.read_all()?;
    if digest_hex(&bytes) != expected.evidence_store_sha256
        || bytes.len() as u64 != expected.evidence_store_size
    {
        return Err(StoreError::Integrity(format!(
            "evidence store identity mismatch for {}",
            expected.run_id.as_str()
        )));
    }
    Ok(bytes)
}

fn validate_directory_inventory(
    directory: &HeldEntry,
    expected: &RunCatalogRecord,
) -> Result<(), StoreError> {
    let names = directory.directory_names("run directory")?;
    if names
        != [
            std::ffi::OsString::from("evidence.store"),
            std::ffi::OsString::from("run.json"),
        ]
    {
        return Err(StoreError::Integrity(format!(
            "run directory contains an unknown entry: {}",
            expected.run_id.as_str()
        )));
    }
    Ok(())
}

fn run_path(store: &RepositoryStore, run_id: &RunId) -> PathBuf {
    store.state_dir.join("runs").join(run_id.as_str())
}

fn staging_path(store: &RepositoryStore, run_id: &RunId) -> PathBuf {
    store
        .state_dir
        .join("runs")
        .join(format!(".{}.staging", run_id.as_str()))
}

fn hit_after_run_rename() {
    #[cfg(feature = "publication-test-crash")]
    super::crash::hit(super::crash::PublicationCrashPoint::AfterRunRename);
}

fn hit_after_terminal_attempt() {
    #[cfg(feature = "publication-test-crash")]
    super::crash::hit(super::crash::PublicationCrashPoint::AfterTerminalAttempt);
}

fn publication_error(stage: &str, error: StoreError) -> StoreError {
    match error {
        StoreError::Io(message) => StoreError::Io(format!("{stage}: {message}")),
        StoreError::Backend(message) => StoreError::Backend(format!("{stage}: {message}")),
        error => error,
    }
}
