mod gate;
mod generation;
mod namespace;
mod retention;

pub use gate::{
    ActiveGateLease, OperationSession, PostWriteFinish, PostWriteStart, PreWriteFinish,
    PreWriteStart, SemanticReadReservation,
};
pub use generation::StoreGeneration;
pub use namespace::MigrationIntent;
pub use retention::{RETENTION_PLAN_ITEMS_ORDERING, RetentionPlanRequest};

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use lumin_evidence::RunEvidence;
use lumin_model::{AttemptId, RepositoryBinding, RunId, digest_hex};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition, TableError};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use thiserror::Error;

pub(crate) const SEQUENCES: TableDefinition<&str, u64> = TableDefinition::new("sequences");
pub(crate) const RUN_CATALOG: TableDefinition<&str, &[u8]> = TableDefinition::new("run-catalog");
pub(crate) const POINTERS: TableDefinition<&str, &[u8]> = TableDefinition::new("pointers");
const EVIDENCE: TableDefinition<&str, &[u8]> = TableDefinition::new("evidence");

#[derive(Clone, Debug)]
pub struct RepositoryStore {
    state_dir: PathBuf,
    namespace: namespace::NamespaceState,
}

#[derive(Clone, Debug)]
pub struct AttemptHandle {
    attempt_id: AttemptId,
    sequence: u64,
    generation: StoreGeneration,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishedRun {
    pub attempt_id: AttemptId,
    pub run_id: RunId,
    pub sequence: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunCatalogRecord {
    pub attempt_id: AttemptId,
    pub run_id: RunId,
    pub sequence: u64,
    pub evidence_store_sha256: String,
    pub evidence_store_size: u64,
}

#[derive(Clone, Debug)]
pub struct RunCatalogSnapshot {
    pub revision: u64,
    pub runs: Vec<RunCatalogRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttemptEnvelope {
    pub schema_version: String,
    pub attempt_id: AttemptId,
    pub sequence: u64,
    pub state: AttemptState,
    pub started_unix_millis: u128,
    pub finished_unix_millis: Option<u128>,
    pub run_id: Option<RunId>,
    pub failure: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AttemptState {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("state namespace integrity failure: {0}")]
    Integrity(String),
    #[error("state I/O failure: {0}")]
    Io(String),
    #[error("redb failure: {0}")]
    Backend(String),
    #[error("state serialization failure: {0}")]
    Serialization(String),
    #[error("run does not exist: {0}")]
    RunNotFound(String),
    #[error("run pin does not exist: {0}")]
    PinNotFound(String),
    #[error("operation ID was reused with a different request: {0}")]
    OperationConflict(String),
    #[error("operation is already live in another session: {0}")]
    OperationBusy(String),
    #[error("operation does not exist: {0}")]
    OperationNotFound(String),
    #[error("gate does not exist: {0}")]
    GateNotFound(String),
    #[error("gate is not active: {0}")]
    GateNotActive(String),
    #[error("gate revision already has a live close operation: {0}")]
    GateRevisionBusy(String),
    #[error("gate revision changed before lifecycle mutation: {0}")]
    GateRevisionChanged(String),
    #[error("retention plan does not exist: {0}")]
    RetentionPlanNotFound(String),
    #[error("retention plan cannot be confirmed in its current state: {0}")]
    RetentionPlanState(String),
    #[error(
        "lifecycle store generation changed before mutation: expected {expected}, observed {observed}"
    )]
    StoreGenerationChanged {
        expected: StoreGeneration,
        observed: StoreGeneration,
    },
    #[error(
        "lifecycle migration from generation {from_generation} to {to_generation} requires recovery"
    )]
    LifecycleMigrationPending {
        from_generation: StoreGeneration,
        to_generation: StoreGeneration,
    },
    #[error("completed lifecycle migration still has private payloads to clean")]
    LifecycleMigrationCleanupPending,
}

impl RepositoryStore {
    pub fn open(root: &Path, binding: &RepositoryBinding) -> Result<Self, StoreError> {
        let namespace = namespace::NamespaceState::open(root, binding)?;
        let state_dir = namespace.state_dir().to_path_buf();
        Ok(Self {
            state_dir,
            namespace,
        })
    }

    pub fn begin_attempt(&self) -> Result<AttemptHandle, StoreError> {
        self.with_exclusive_lock(|guard| {
            let database = guard.open_database()?;
            let generation = database.generation();
            let sequence = next_attempt_sequence(guard, &database)?;
            let attempt = AttemptHandle {
                attempt_id: AttemptId::from_string(format!("attempt_{sequence:016x}")),
                sequence,
                generation,
            };
            let envelope = AttemptEnvelope {
                schema_version: "lumin-attempt.v1".to_owned(),
                attempt_id: attempt.attempt_id.clone(),
                sequence,
                state: AttemptState::Running,
                started_unix_millis: unix_millis()?,
                finished_unix_millis: None,
                run_id: None,
                failure: None,
            };
            let directory = self
                .state_dir
                .join("attempts")
                .join(attempt.attempt_id.as_str());
            drop(database);
            guard.mutate_for_generation(generation, || {
                fs::create_dir(&directory).map_err(io_error)?;
                write_json(&directory.join("attempt.json"), &envelope)
            })?;
            let database = guard.open_database_for_generation(generation)?;
            set_pointer(
                guard,
                &database,
                "latest-attempt",
                attempt.attempt_id.as_str().as_bytes(),
            )?;
            Ok(attempt)
        })
    }

    pub fn fail_attempt(&self, attempt: &AttemptHandle, failure: &str) -> Result<(), StoreError> {
        self.with_exclusive_lock(|guard| {
            let database = guard.open_database_for_generation(attempt.generation)?;
            drop(database);
            let path = self
                .state_dir
                .join("attempts")
                .join(attempt.attempt_id.as_str())
                .join("attempt.json");
            let mut envelope = read_json::<AttemptEnvelope>(&path)?;
            envelope.state = AttemptState::Failed;
            envelope.finished_unix_millis = Some(unix_millis()?);
            envelope.failure = Some(failure.to_owned());
            guard.mutate_for_generation(attempt.generation, || write_json(&path, &envelope))
        })
    }

    pub fn publish_run(
        &self,
        attempt: &AttemptHandle,
        evidence: &RunEvidence,
    ) -> Result<PublishedRun, StoreError> {
        self.with_exclusive_lock(|guard| {
            let database = guard.open_database_for_generation(attempt.generation)?;
            drop(database);
            let run_id = RunId::from_string(format!("run_{:016x}", attempt.sequence));
            let staging = self
                .state_dir
                .join("runs")
                .join(format!(".{}.staging", run_id.as_str()));
            let run_dir = self.state_dir.join("runs").join(run_id.as_str());
            if staging.exists() || run_dir.exists() {
                return Err(StoreError::Integrity(format!(
                    "run publication target already exists: {}",
                    run_id.as_str()
                )));
            }
            let record = guard.mutate_for_generation(attempt.generation, || {
                fs::create_dir(&staging).map_err(io_error)?;
                let evidence_path = staging.join("evidence.store");
                write_evidence_store(&evidence_path, evidence)?;
                let evidence_bytes = fs::read(&evidence_path).map_err(io_error)?;
                let record = RunCatalogRecord {
                    attempt_id: attempt.attempt_id.clone(),
                    run_id: run_id.clone(),
                    sequence: attempt.sequence,
                    evidence_store_sha256: digest_hex(&evidence_bytes),
                    evidence_store_size: evidence_bytes.len() as u64,
                };
                write_json(&staging.join("run.json"), &record)?;
                Ok(record)
            })?;
            guard.mutate_for_generation(attempt.generation, || {
                fs::rename(&staging, &run_dir).map_err(io_error)
            })?;

            let database = guard.open_database_for_generation(attempt.generation)?;
            insert_catalog_record(guard, &database, &record)?;
            set_pointer(
                guard,
                &database,
                "latest-completed",
                run_id.as_str().as_bytes(),
            )?;
            drop(database);

            let attempt_path = self
                .state_dir
                .join("attempts")
                .join(attempt.attempt_id.as_str())
                .join("attempt.json");
            let mut envelope = read_json::<AttemptEnvelope>(&attempt_path)?;
            envelope.state = AttemptState::Completed;
            envelope.finished_unix_millis = Some(unix_millis()?);
            envelope.run_id = Some(run_id.clone());
            guard.mutate_for_generation(attempt.generation, || {
                write_json(&attempt_path, &envelope)
            })?;

            Ok(PublishedRun {
                attempt_id: attempt.attempt_id.clone(),
                run_id,
                sequence: attempt.sequence,
            })
        })
    }

    pub fn load_run(&self, run_id: &RunId) -> Result<(RunCatalogRecord, RunEvidence), StoreError> {
        self.with_shared_lock(|guard| {
            let database = guard.open_database()?;
            read_live_run(&self.state_dir, &database, run_id)
        })
    }

    pub fn lookup_run(
        &self,
        run_id: &RunId,
    ) -> Result<lumin_evidence::RecordLookup<(RunCatalogRecord, RunEvidence)>, StoreError> {
        self.with_shared_lock(|guard| {
            let database = guard.open_database()?;
            let tombstone_key = retention::records::tombstone_key(
                lumin_evidence::RetentionItemKind::Run,
                run_id.as_str(),
            );
            if let Some(tombstone) = gate::records::load_record::<
                retention::records::StoredTombstone,
            >(
                &database, retention::RETENTION_TOMBSTONES, &tombstone_key
            )? {
                return if tombstone.envelope.tombstone_identity.is_some() {
                    Ok(lumin_evidence::RecordLookup::Pruned(tombstone.envelope))
                } else {
                    Ok(lumin_evidence::RecordLookup::Pruning(tombstone.envelope))
                };
            }
            read_live_run(&self.state_dir, &database, run_id)
                .map(lumin_evidence::RecordLookup::Live)
        })
    }

    pub fn list_runs(&self) -> Result<RunCatalogSnapshot, StoreError> {
        self.with_shared_lock(|guard| {
            let database = guard.open_database()?;
            let read = database.begin_read()?;
            let revision = read_sequence(&read, "run-catalog")?;
            let tombstones = match read.open_table(retention::RETENTION_TOMBSTONES) {
                Ok(table) => Some(table),
                Err(TableError::TableDoesNotExist(_)) => None,
                Err(error) => return Err(backend_error(error)),
            };
            let table = read.open_table(RUN_CATALOG).map_err(backend_error)?;
            let mut runs = Vec::new();
            for row in table.iter().map_err(backend_error)? {
                let (key, value) = row.map_err(backend_error)?;
                let key = key.value();
                let tombstone_key =
                    retention::records::tombstone_key(lumin_evidence::RetentionItemKind::Run, key);
                if let Some(tombstones) = &tombstones
                    && tombstones
                        .get(tombstone_key.as_str())
                        .map_err(backend_error)?
                        .is_some()
                {
                    continue;
                }
                let record: RunCatalogRecord =
                    serde_json::from_slice(value.value()).map_err(serialization_error)?;
                if record.run_id.as_str() != key {
                    return Err(StoreError::Integrity(format!(
                        "run catalog key {key} disagrees with its record"
                    )));
                }
                runs.push(record);
            }
            runs.sort_by(|left, right| {
                right
                    .sequence
                    .cmp(&left.sequence)
                    .then_with(|| left.run_id.cmp(&right.run_id))
            });
            Ok(RunCatalogSnapshot { revision, runs })
        })
    }

    pub fn latest_run_id(&self) -> Result<Option<RunId>, StoreError> {
        self.with_shared_lock(|guard| {
            let database = guard.open_database()?;
            let read = database.begin_read()?;
            let table = read.open_table(POINTERS).map_err(backend_error)?;
            let value = table.get("latest-completed").map_err(backend_error)?;
            let Some(value) = value else {
                return Ok(None);
            };
            let text = std::str::from_utf8(value.value())
                .map_err(|error| StoreError::Integrity(error.to_string()))?;
            Ok(Some(RunId::from_string(text.to_owned())))
        })
    }

    pub fn migrate_lifecycle_store(&self) -> Result<StoreGeneration, StoreError> {
        self.namespace.migrate_lifecycle_store()
    }

    fn with_exclusive_lock<T>(
        &self,
        operation: impl FnOnce(&namespace::NamespaceGuard) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.namespace.with_exclusive_lock(operation)
    }

    fn with_shared_lock<T>(
        &self,
        operation: impl FnOnce(&namespace::NamespaceGuard) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.namespace.with_shared_lock(operation)
    }
}

fn next_attempt_sequence(
    guard: &namespace::NamespaceGuard,
    database: &namespace::StoreDatabase<'_>,
) -> Result<u64, StoreError> {
    let write = database.begin_write()?;
    let next = {
        let mut table = write.open_table(SEQUENCES).map_err(backend_error)?;
        let current = table
            .get("attempt")
            .map_err(backend_error)?
            .map_or(0, |value| value.value());
        let next = current
            .checked_add(1)
            .ok_or_else(|| StoreError::Integrity("attempt sequence overflow".to_owned()))?;
        table.insert("attempt", next).map_err(backend_error)?;
        next
    };
    guard.commit(write)?;
    Ok(next)
}

fn insert_catalog_record(
    guard: &namespace::NamespaceGuard,
    database: &namespace::StoreDatabase<'_>,
    record: &RunCatalogRecord,
) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec(record).map_err(serialization_error)?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(RUN_CATALOG).map_err(backend_error)?;
        table
            .insert(record.run_id.as_str(), bytes.as_slice())
            .map_err(backend_error)?;
    }
    retention::records::next_sequence(&write, "run-catalog")?;
    guard.commit(write)
}

fn read_sequence(read: &redb::ReadTransaction, key: &str) -> Result<u64, StoreError> {
    let table = read.open_table(SEQUENCES).map_err(backend_error)?;
    table
        .get(key)
        .map_err(backend_error)
        .map(|value| value.map_or(0, |value| value.value()))
}

fn read_catalog_record(
    database: &namespace::StoreDatabase<'_>,
    run_id: &RunId,
) -> Result<RunCatalogRecord, StoreError> {
    let read = database.begin_read()?;
    let table = read.open_table(RUN_CATALOG).map_err(backend_error)?;
    let value = table
        .get(run_id.as_str())
        .map_err(backend_error)?
        .ok_or_else(|| StoreError::RunNotFound(run_id.as_str().to_owned()))?;
    serde_json::from_slice(value.value()).map_err(serialization_error)
}

fn read_live_run(
    state_dir: &Path,
    database: &namespace::StoreDatabase<'_>,
    run_id: &RunId,
) -> Result<(RunCatalogRecord, RunEvidence), StoreError> {
    let record = read_catalog_record(database, run_id)?;
    let path = state_dir
        .join("runs")
        .join(run_id.as_str())
        .join("evidence.store");
    let bytes = fs::read(&path).map_err(io_error)?;
    if digest_hex(&bytes) != record.evidence_store_sha256
        || bytes.len() as u64 != record.evidence_store_size
    {
        return Err(StoreError::Integrity(format!(
            "evidence store identity mismatch for {}",
            run_id.as_str()
        )));
    }
    Ok((record, read_evidence_store(&path)?))
}

fn set_pointer(
    guard: &namespace::NamespaceGuard,
    database: &namespace::StoreDatabase<'_>,
    key: &str,
    value: &[u8],
) -> Result<(), StoreError> {
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(POINTERS).map_err(backend_error)?;
        table.insert(key, value).map_err(backend_error)?;
    }
    guard.commit(write)
}

fn write_evidence_store(path: &Path, evidence: &RunEvidence) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec(evidence).map_err(serialization_error)?;
    let database = Database::create(path).map_err(backend_error)?;
    let write = database.begin_write().map_err(backend_error)?;
    {
        let mut table = write.open_table(EVIDENCE).map_err(backend_error)?;
        table
            .insert("run", bytes.as_slice())
            .map_err(backend_error)?;
    }
    write.commit().map_err(backend_error)?;
    drop(database);
    Ok(())
}

fn read_evidence_store(path: &Path) -> Result<RunEvidence, StoreError> {
    let database = Database::open(path).map_err(backend_error)?;
    let read = database.begin_read().map_err(backend_error)?;
    let table = read.open_table(EVIDENCE).map_err(backend_error)?;
    let value = table
        .get("run")
        .map_err(backend_error)?
        .ok_or_else(|| StoreError::Integrity("run evidence row is missing".to_owned()))?;
    serde_json::from_slice(value.value()).map_err(serialization_error)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), StoreError> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(serialization_error)?;
    bytes.push(b'\n');
    write_replace(path, &bytes)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, StoreError> {
    let bytes = fs::read(path).map_err(io_error)?;
    serde_json::from_slice(&bytes).map_err(serialization_error)
}

fn write_replace(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| StoreError::Io("state file has no parent".to_owned()))?;
    let mut temp = NamedTempFile::new_in(parent).map_err(io_error)?;
    temp.write_all(bytes).map_err(io_error)?;
    temp.as_file().sync_all().map_err(io_error)?;
    temp.persist(path)
        .map(|_| ())
        .map_err(|error| io_error(error.error))
}

pub(crate) fn nonce_hex() -> Result<String, StoreError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| StoreError::Io(error.to_string()))?;
    Ok(digest_hex(&bytes)[..32].to_owned())
}

pub(crate) fn unix_millis() -> Result<u128, StoreError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .map_err(|error| StoreError::Io(error.to_string()))
}

fn io_error(error: std::io::Error) -> StoreError {
    StoreError::Io(error.to_string())
}

fn backend_error(error: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(error.to_string())
}

fn serialization_error(error: serde_json::Error) -> StoreError {
    StoreError::Serialization(error.to_string())
}
