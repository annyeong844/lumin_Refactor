use lumin_model::{AttemptId, AttemptStatus, RunId};
use redb::TableError;
use serde::{Deserialize, Serialize};

use super::files;
use super::{AttemptEnvelope, LatestRunSnapshot, attempt_path};
use crate::namespace::{NamespaceGuard, entry_exists, records::ManagedStateParentKind};
use crate::{
    POINTERS, RepositoryStore, StoreError, backend_error, read_catalog_record, read_live_run,
};

const LATEST_SCHEMA: &str = "lumin-latest.v1";
const LATEST_NAME: &str = "latest.json";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct LatestPointer {
    schema_version: String,
    pub(super) latest_attempt: Option<LatestAttemptPointer>,
    pub(super) latest_completed: Option<LatestCompletedPointer>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct LatestAttemptPointer {
    pub(super) attempt_id: AttemptId,
    pub(super) sequence: u64,
    pub(super) status: AttemptStatus,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct LatestCompletedPointer {
    pub(super) run_id: RunId,
    pub(super) sequence: u64,
}

impl Default for LatestPointer {
    fn default() -> Self {
        Self {
            schema_version: LATEST_SCHEMA.to_owned(),
            latest_attempt: None,
            latest_completed: None,
        }
    }
}

pub(super) fn ensure(store: &RepositoryStore, guard: &NamespaceGuard) -> Result<(), StoreError> {
    let latest_path = store.state_dir.join(LATEST_NAME);
    files::validate_and_remove_pending(
        &latest_path.with_extension("json.pending"),
        guard.state_directory_entry(),
        "latest pointer pending file",
        |pending: &LatestPointer| {
            validate_pointer_schema(pending)?;
            validate_document(store, guard, pending)
        },
    )?;
    let latest = if entry_exists(&latest_path)? {
        read_document(store, guard)?
    } else {
        let derived = derive_legacy_document(store, guard)?;
        if derived != LatestPointer::default() {
            files::write_json(
                &latest_path,
                guard.state_directory_entry(),
                "latest pointer",
                &derived,
            )?;
        }
        derived
    };
    validate_document(store, guard, &latest)?;
    sync_index(guard, &latest)
}

pub(super) fn snapshot(store: &RepositoryStore) -> Result<LatestRunSnapshot, StoreError> {
    store.with_shared_lock(|guard| {
        let latest = read_document(store, guard)?;
        validate_document(store, guard, &latest)?;
        let latest_attempt = latest
            .latest_attempt
            .as_ref()
            .map(|pointer| read_attempt(store, guard, &pointer.attempt_id))
            .transpose()?;
        let completed = match latest.latest_completed {
            Some(pointer) => {
                let database = guard.open_database()?;
                Some(read_live_run(&store.state_dir, &database, &pointer.run_id)?)
            }
            None => None,
        };
        Ok(LatestRunSnapshot {
            latest_attempt,
            completed,
        })
    })
}

pub(super) fn completed_run_id(store: &RepositoryStore) -> Result<Option<RunId>, StoreError> {
    store.with_shared_lock(|guard| {
        let latest = read_document(store, guard)?;
        validate_document(store, guard, &latest)?;
        Ok(latest.latest_completed.map(|pointer| pointer.run_id))
    })
}

pub(super) fn publish_attempt(
    store: &RepositoryStore,
    guard: &NamespaceGuard,
    attempt: &AttemptEnvelope,
    terminal_crash_hooks: bool,
) -> Result<(), StoreError> {
    validate_attempt_envelope(attempt)?;
    let current = read_document(store, guard)?;
    validate_document(store, guard, &current)?;
    let candidate = LatestAttemptPointer {
        attempt_id: attempt.attempt_id.clone(),
        sequence: attempt.sequence,
        status: attempt.state,
    };
    let merged = merge(current, candidate, attempt.run_id.as_ref())?;
    if merged.changed {
        let path = store.state_dir.join(LATEST_NAME);
        files::write_json_with_hooks(
            &path,
            guard.state_directory_entry(),
            "latest pointer",
            &merged.pointer,
            || {
                if terminal_crash_hooks {
                    hit_terminal_temp();
                }
            },
            || {
                if terminal_crash_hooks {
                    hit_terminal_replace();
                }
            },
        )?;
    }
    validate_document(store, guard, &merged.pointer)?;
    sync_index(guard, &merged.pointer)
}

pub(super) fn read_document(
    store: &RepositoryStore,
    guard: &NamespaceGuard,
) -> Result<LatestPointer, StoreError> {
    if !entry_exists(&store.state_dir.join(LATEST_NAME))? {
        return Ok(LatestPointer::default());
    }
    read_document_path(
        &store.state_dir.join(LATEST_NAME),
        guard.state_directory_entry(),
        "latest pointer",
    )
}

fn read_document_path(
    path: &std::path::Path,
    parent: &crate::namespace::HeldEntry,
    label: &str,
) -> Result<LatestPointer, StoreError> {
    let latest: LatestPointer = files::read_json(path, parent, label)?;
    validate_pointer_schema(&latest)?;
    Ok(latest)
}

fn validate_pointer_schema(latest: &LatestPointer) -> Result<(), StoreError> {
    if latest.schema_version != LATEST_SCHEMA {
        return Err(StoreError::Integrity(format!(
            "latest pointer schema {} is unsupported; expected {LATEST_SCHEMA}",
            latest.schema_version
        )));
    }
    Ok(())
}

pub(super) fn read_attempt(
    store: &RepositoryStore,
    guard: &NamespaceGuard,
    attempt_id: &AttemptId,
) -> Result<AttemptEnvelope, StoreError> {
    let directory = guard.open_managed_child_directory(
        ManagedStateParentKind::Attempts,
        attempt_id.as_str(),
        "attempt directory",
    )?;
    let envelope = files::read_json(
        &attempt_path(store, attempt_id),
        &directory,
        "attempt envelope",
    )?;
    validate_attempt_envelope(&envelope)?;
    if envelope.attempt_id != *attempt_id {
        return Err(StoreError::Integrity(format!(
            "attempt pointer {} disagrees with its envelope",
            attempt_id.as_str()
        )));
    }
    Ok(envelope)
}

pub(super) fn validate_attempt_envelope(envelope: &AttemptEnvelope) -> Result<(), StoreError> {
    if envelope.schema_version != "lumin-attempt.v1" {
        return Err(StoreError::Integrity(format!(
            "attempt envelope schema {} is unsupported",
            envelope.schema_version
        )));
    }
    match envelope.state {
        AttemptStatus::Running => {
            if envelope.finished_unix_millis.is_some()
                || envelope.run_id.is_some()
                || envelope.failure.is_some()
            {
                return Err(StoreError::Integrity(format!(
                    "running attempt {} contains terminal fields",
                    envelope.attempt_id.as_str()
                )));
            }
        }
        AttemptStatus::Completed => {
            if envelope.finished_unix_millis.is_none()
                || envelope.run_id.is_none()
                || envelope.failure.is_some()
            {
                return Err(StoreError::Integrity(format!(
                    "completed attempt {} has incoherent terminal fields",
                    envelope.attempt_id.as_str()
                )));
            }
        }
        AttemptStatus::Failed | AttemptStatus::Interrupted => {
            if envelope.finished_unix_millis.is_none()
                || envelope.run_id.is_some()
                || envelope.failure.as_deref().is_none_or(str::is_empty)
            {
                return Err(StoreError::Integrity(format!(
                    "unsuccessful attempt {} has incoherent terminal fields",
                    envelope.attempt_id.as_str()
                )));
            }
        }
    }
    Ok(())
}

fn validate_document(
    store: &RepositoryStore,
    guard: &NamespaceGuard,
    latest: &LatestPointer,
) -> Result<(), StoreError> {
    let latest_attempt = latest
        .latest_attempt
        .as_ref()
        .map(|pointer| {
            let envelope = read_attempt(store, guard, &pointer.attempt_id)?;
            let pointer_can_lag_terminal = pointer.status == AttemptStatus::Running
                && phase(envelope.state) > phase(pointer.status);
            if envelope.sequence != pointer.sequence
                || (envelope.state != pointer.status && !pointer_can_lag_terminal)
            {
                return Err(StoreError::Integrity(
                    "latestAttempt disagrees with its attempt envelope".to_owned(),
                ));
            }
            if pointer.status == AttemptStatus::Running
                && !super::liveness::has_active_lease(guard, &pointer.attempt_id)?
            {
                return Err(StoreError::Integrity(
                    "running latestAttempt has no process-liveness lease".to_owned(),
                ));
            }
            Ok(envelope)
        })
        .transpose()?;

    if let Some(pointer) = &latest.latest_completed {
        let database = guard.open_database()?;
        let record = read_catalog_record(&database, &pointer.run_id)?;
        if record.sequence != pointer.sequence {
            return Err(StoreError::Integrity(
                "latestCompleted disagrees with its run catalog record".to_owned(),
            ));
        }
        let attempt = read_attempt(store, guard, &record.attempt_id)?;
        if attempt.state != AttemptStatus::Completed
            || attempt.sequence != record.sequence
            || attempt.run_id.as_ref() != Some(&record.run_id)
        {
            return Err(StoreError::Integrity(
                "latestCompleted run disagrees with its terminal attempt".to_owned(),
            ));
        }
        if latest_attempt
            .as_ref()
            .is_none_or(|latest_attempt| latest_attempt.sequence < pointer.sequence)
        {
            return Err(StoreError::Integrity(
                "latestCompleted is newer than latestAttempt".to_owned(),
            ));
        }
    }
    Ok(())
}

fn derive_legacy_document(
    store: &RepositoryStore,
    guard: &NamespaceGuard,
) -> Result<LatestPointer, StoreError> {
    let database = guard.open_database()?;
    let read = database.begin_read()?;
    let table = match read.open_table(POINTERS) {
        Ok(table) => table,
        Err(TableError::TableDoesNotExist(_)) => return Ok(LatestPointer::default()),
        Err(error) => return Err(backend_error(error)),
    };
    let latest_attempt = table
        .get("latest-attempt")
        .map_err(backend_error)?
        .map(|value| parse_id(value.value(), "legacy latest-attempt"))
        .transpose()?
        .map(|attempt_id| {
            let envelope = read_attempt(store, guard, &attempt_id)?;
            Ok(LatestAttemptPointer {
                attempt_id,
                sequence: envelope.sequence,
                status: envelope.state,
            })
        })
        .transpose()?;
    let latest_completed = table
        .get("latest-completed")
        .map_err(backend_error)?
        .map(|value| parse_run_id(value.value(), "legacy latest-completed"))
        .transpose()?
        .map(|run_id| {
            let record = read_catalog_record(&database, &run_id)?;
            Ok(LatestCompletedPointer {
                run_id,
                sequence: record.sequence,
            })
        })
        .transpose()?;
    Ok(LatestPointer {
        schema_version: LATEST_SCHEMA.to_owned(),
        latest_attempt,
        latest_completed,
    })
}

fn sync_index(guard: &NamespaceGuard, latest: &LatestPointer) -> Result<(), StoreError> {
    let database = guard.open_database()?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(POINTERS).map_err(backend_error)?;
        match &latest.latest_attempt {
            Some(pointer) => {
                table
                    .insert("latest-attempt", pointer.attempt_id.as_str().as_bytes())
                    .map_err(backend_error)?;
            }
            None => {
                table.remove("latest-attempt").map_err(backend_error)?;
            }
        }
        match &latest.latest_completed {
            Some(pointer) => {
                table
                    .insert("latest-completed", pointer.run_id.as_str().as_bytes())
                    .map_err(backend_error)?;
            }
            None => {
                table.remove("latest-completed").map_err(backend_error)?;
            }
        }
    }
    guard.commit(write)
}

struct MergeResult {
    pointer: LatestPointer,
    changed: bool,
}

fn merge(
    mut latest: LatestPointer,
    candidate: LatestAttemptPointer,
    completed_run: Option<&RunId>,
) -> Result<MergeResult, StoreError> {
    let mut changed = merge_attempt(&mut latest, candidate.clone())?;
    if let Some(run_id) = completed_run {
        let candidate_completed = LatestCompletedPointer {
            run_id: run_id.clone(),
            sequence: candidate.sequence,
        };
        changed |= merge_completed(&mut latest, candidate_completed)?;
    }
    Ok(MergeResult {
        pointer: latest,
        changed,
    })
}

fn merge_attempt(
    latest: &mut LatestPointer,
    candidate: LatestAttemptPointer,
) -> Result<bool, StoreError> {
    let Some(current) = &latest.latest_attempt else {
        latest.latest_attempt = Some(candidate);
        return Ok(true);
    };
    if candidate.sequence < current.sequence {
        return Ok(false);
    }
    if candidate.sequence > current.sequence {
        latest.latest_attempt = Some(candidate);
        return Ok(true);
    }
    let candidate_phase = phase(candidate.status);
    let current_phase = phase(current.status);
    if candidate_phase < current_phase {
        return Ok(false);
    }
    if candidate_phase > current_phase {
        latest.latest_attempt = Some(candidate);
        return Ok(true);
    }
    if candidate == *current {
        return Ok(false);
    }
    Err(StoreError::Integrity(format!(
        "attempt sequence {} has two different results",
        candidate.sequence
    )))
}

fn merge_completed(
    latest: &mut LatestPointer,
    candidate: LatestCompletedPointer,
) -> Result<bool, StoreError> {
    let Some(current) = &latest.latest_completed else {
        latest.latest_completed = Some(candidate);
        return Ok(true);
    };
    if candidate.sequence < current.sequence {
        return Ok(false);
    }
    if candidate.sequence > current.sequence {
        latest.latest_completed = Some(candidate);
        return Ok(true);
    }
    if candidate == *current {
        return Ok(false);
    }
    Err(StoreError::Integrity(format!(
        "completed sequence {} names two different runs",
        candidate.sequence
    )))
}

fn phase(status: AttemptStatus) -> u8 {
    match status {
        AttemptStatus::Running => 0,
        AttemptStatus::Completed | AttemptStatus::Failed | AttemptStatus::Interrupted => 1,
    }
}

fn parse_id(bytes: &[u8], label: &str) -> Result<AttemptId, StoreError> {
    let value = std::str::from_utf8(bytes)
        .map_err(|error| StoreError::Integrity(format!("{label} is not UTF-8: {error}")))?;
    Ok(AttemptId::from_string(value.to_owned()))
}

fn parse_run_id(bytes: &[u8], label: &str) -> Result<RunId, StoreError> {
    let value = std::str::from_utf8(bytes)
        .map_err(|error| StoreError::Integrity(format!("{label} is not UTF-8: {error}")))?;
    Ok(RunId::from_string(value.to_owned()))
}

fn hit_terminal_temp() {
    #[cfg(feature = "publication-test-crash")]
    super::crash::hit(super::crash::PublicationCrashPoint::AfterLatestTemp);
}

fn hit_terminal_replace() {
    #[cfg(feature = "publication-test-crash")]
    super::crash::hit(super::crash::PublicationCrashPoint::AfterLatestReplace);
}
