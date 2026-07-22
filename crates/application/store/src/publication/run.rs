use std::fs;
use std::path::{Path, PathBuf};

use lumin_evidence::RunEvidence;
use lumin_model::{AttemptStatus, RunId, digest_hex};

use super::{AttemptEnvelope, files, latest, liveness, run_id};
use crate::namespace::{
    EntryAccess, EntryKind, HeldEntry, NamespaceGuard, entry_exists, publish_file_atomic,
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
) -> Result<PublishedRun, StoreError> {
    if !session.belongs_to(store) {
        return Err(StoreError::Integrity(
            "attempt session belongs to another repository store".to_owned(),
        ));
    }
    store.with_exclusive_lock(|guard| {
        session
            .validate(guard)
            .map_err(|error| publication_error("validate attempt session", error))?;
        let mut envelope = latest::read_attempt(store, guard, session.attempt_id())
            .map_err(|error| publication_error("read running attempt", error))?;
        session.require_running(&envelope)?;

        let record = publish_directory(
            store,
            guard,
            &envelope,
            evidence,
            session.store_generation(),
        )
        .map_err(|error| publication_error("publish run directory", error))?;
        hit_after_run_rename();

        envelope.state = AttemptStatus::Completed;
        envelope.finished_unix_millis = Some(unix_millis()?);
        envelope.run_id = Some(record.run_id.clone());
        liveness::write_terminal(store, guard, session.store_generation(), &envelope)
            .map_err(|error| publication_error("publish terminal attempt", error))?;
        hit_after_terminal_attempt();

        let database = guard.open_database_for_generation(session.store_generation())?;
        insert_catalog_record(guard, &database, &record)
            .map_err(|error| publication_error("publish run catalog", error))?;
        drop(database);
        latest::publish_attempt(store, guard, &envelope, true)
            .map_err(|error| publication_error("publish latest pointer", error))?;
        liveness::remove_session(store, guard, session)
            .map_err(|error| publication_error("release attempt lease", error))?;
        Ok(PublishedRun {
            attempt_id: record.attempt_id,
            run_id: record.run_id,
            sequence: record.sequence,
        })
    })
}

pub(super) fn recover_completed(
    store: &RepositoryStore,
    guard: &NamespaceGuard,
    envelope: &AttemptEnvelope,
) -> Result<(), StoreError> {
    if envelope.state != AttemptStatus::Completed {
        return Err(StoreError::Integrity(format!(
            "non-completed attempt cannot recover a run: {}",
            envelope.attempt_id.as_str()
        )));
    }
    let record = validate_published(store, guard, envelope)?;
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
    let staging_entry = HeldEntry::open(
        &staging,
        EntryKind::Directory,
        EntryAccess::ReadOnly,
        false,
        "run staging directory",
    )?;
    files::require_parent_volume(&staging_entry, parent, "run staging directory")?;
    let record = write_staging(&staging, &staging_entry, envelope, &run_id, evidence)
        .map_err(|error| publication_error("write staging payload", error))?;
    validate_directory(&staging, &staging_entry, &record)
        .map_err(|error| publication_error("validate staging payload", error))?;
    drop(staging_entry);

    guard
        .mutate_for_generation(generation, || {
            publish_file_atomic(&published, &staging)?;
            parent.sync_directory()
        })
        .map_err(|error| publication_error("rename staging directory", error))?;
    let published_entry = guard
        .open_managed_child_directory(
            ManagedStateParentKind::Runs,
            run_id.as_str(),
            "published run directory",
        )
        .map_err(|error| publication_error("open published run directory", error))?;
    validate_directory(&published, &published_entry, &record)
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

fn validate_published(
    store: &RepositoryStore,
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
    let path = run_path(store, run_id);
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

fn validate_directory(
    directory_path: &Path,
    directory: &HeldEntry,
    expected: &RunCatalogRecord,
) -> Result<(), StoreError> {
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
    let bytes = evidence.read_all()?;
    if digest_hex(&bytes) != expected.evidence_store_sha256
        || bytes.len() as u64 != expected.evidence_store_size
    {
        return Err(StoreError::Integrity(format!(
            "evidence store identity mismatch for {}",
            expected.run_id.as_str()
        )));
    }
    read_evidence_store(&evidence_path)?;
    evidence.validate_path(
        &evidence_path,
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        true,
        "run evidence store",
    )
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
