mod gate;

pub use gate::{
    ActiveGateLease, PostWriteFinish, PostWriteStart, PreWriteFinish, PreWriteStart,
    SemanticReadReservation,
};

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use lumin_evidence::RunEvidence;
use lumin_model::{AttemptId, RunId, digest_hex};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use thiserror::Error;

const SEQUENCES: TableDefinition<&str, u64> = TableDefinition::new("sequences");
const RUN_CATALOG: TableDefinition<&str, &[u8]> = TableDefinition::new("run-catalog");
const POINTERS: TableDefinition<&str, &[u8]> = TableDefinition::new("pointers");
const EVIDENCE: TableDefinition<&str, &[u8]> = TableDefinition::new("evidence");

#[derive(Clone, Debug)]
pub struct RepositoryStore {
    state_dir: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttemptHandle {
    pub attempt_id: AttemptId,
    pub sequence: u64,
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
    #[error("operation ID was reused with a different request: {0}")]
    OperationConflict(String),
    #[error("operation does not exist: {0}")]
    OperationNotFound(String),
    #[error("gate does not exist: {0}")]
    GateNotFound(String),
    #[error("gate is not active: {0}")]
    GateNotActive(String),
    #[error("gate revision already has a live close operation: {0}")]
    GateRevisionBusy(String),
}

impl RepositoryStore {
    pub fn open(root: &Path) -> Result<Self, StoreError> {
        let state_dir = root.join(".lumin");
        initialize_namespace(&state_dir)?;
        Ok(Self { state_dir })
    }

    pub fn begin_attempt(&self) -> Result<AttemptHandle, StoreError> {
        self.with_exclusive_lock(|| {
            let database = open_lifecycle_database(&self.state_dir)?;
            let sequence = next_attempt_sequence(&database)?;
            let attempt = AttemptHandle {
                attempt_id: AttemptId::from_string(format!("attempt_{sequence:016x}")),
                sequence,
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
            fs::create_dir(&directory).map_err(io_error)?;
            write_json(&directory.join("attempt.json"), &envelope)?;
            set_pointer(
                &database,
                "latest-attempt",
                attempt.attempt_id.as_str().as_bytes(),
            )?;
            Ok(attempt)
        })
    }

    pub fn fail_attempt(&self, attempt: &AttemptHandle, failure: &str) -> Result<(), StoreError> {
        self.with_exclusive_lock(|| {
            let path = self
                .state_dir
                .join("attempts")
                .join(attempt.attempt_id.as_str())
                .join("attempt.json");
            let mut envelope = read_json::<AttemptEnvelope>(&path)?;
            envelope.state = AttemptState::Failed;
            envelope.finished_unix_millis = Some(unix_millis()?);
            envelope.failure = Some(failure.to_owned());
            write_json(&path, &envelope)
        })
    }

    pub fn publish_run(
        &self,
        attempt: &AttemptHandle,
        evidence: &RunEvidence,
    ) -> Result<PublishedRun, StoreError> {
        self.with_exclusive_lock(|| {
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
            fs::rename(&staging, &run_dir).map_err(io_error)?;

            let database = open_lifecycle_database(&self.state_dir)?;
            insert_catalog_record(&database, &record)?;
            set_pointer(&database, "latest-completed", run_id.as_str().as_bytes())?;

            let attempt_path = self
                .state_dir
                .join("attempts")
                .join(attempt.attempt_id.as_str())
                .join("attempt.json");
            let mut envelope = read_json::<AttemptEnvelope>(&attempt_path)?;
            envelope.state = AttemptState::Completed;
            envelope.finished_unix_millis = Some(unix_millis()?);
            envelope.run_id = Some(run_id.clone());
            write_json(&attempt_path, &envelope)?;

            Ok(PublishedRun {
                attempt_id: attempt.attempt_id.clone(),
                run_id,
                sequence: attempt.sequence,
            })
        })
    }

    pub fn load_run(&self, run_id: &RunId) -> Result<(RunCatalogRecord, RunEvidence), StoreError> {
        self.with_shared_lock(|| {
            let database = open_lifecycle_database(&self.state_dir)?;
            let record = read_catalog_record(&database, run_id)?;
            let path = self
                .state_dir
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
            let evidence = read_evidence_store(&path)?;
            Ok((record, evidence))
        })
    }

    pub fn latest_run_id(&self) -> Result<Option<RunId>, StoreError> {
        self.with_shared_lock(|| {
            let database = open_lifecycle_database(&self.state_dir)?;
            let read = database.begin_read().map_err(backend_error)?;
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

    fn with_exclusive_lock<T>(
        &self,
        operation: impl FnOnce() -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let lock = open_lock(&self.state_dir)?;
        FileExt::lock_exclusive(&lock).map_err(io_error)?;
        let result = operation();
        FileExt::unlock(&lock).map_err(io_error)?;
        result
    }

    fn with_shared_lock<T>(
        &self,
        operation: impl FnOnce() -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let lock = open_lock(&self.state_dir)?;
        FileExt::lock_shared(&lock).map_err(io_error)?;
        let result = operation();
        FileExt::unlock(&lock).map_err(io_error)?;
        result
    }
}

fn initialize_namespace(state_dir: &Path) -> Result<(), StoreError> {
    ensure_real_directory(state_dir, ".lumin")?;

    for name in ["attempts", "runs", "trash", "cache"] {
        let parent = state_dir.join(name);
        ensure_real_directory(&parent, &format!("managed state parent {name}"))?;
        let anchor = parent.join("namespace.anchor");
        ensure_real_file(&anchor, &format!("managed state anchor {name}"), || {
            nonce_hex().map(|nonce| nonce.into_bytes())
        })?;
    }

    let lock = state_dir.join("lifecycle.lock");
    ensure_real_file(&lock, "lifecycle.lock", || {
        nonce_hex().map(|nonce| nonce.into_bytes())
    })?;

    let marker = state_dir.join("repository.json");
    ensure_real_file(&marker, "repository marker", || {
        let value = serde_json::json!({
            "schemaVersion": "lumin-repository.v1",
            "namespaceNonce": nonce_hex()?,
        });
        let mut bytes = serde_json::to_vec_pretty(&value).map_err(serialization_error)?;
        bytes.push(b'\n');
        Ok(bytes)
    })?;
    let _database = open_lifecycle_database(state_dir)?;
    Ok(())
}

fn ensure_real_directory(path: &Path, label: &str) -> Result<(), StoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => Ok(()),
        Ok(_) => Err(StoreError::Integrity(format!(
            "{label} must be a real directory"
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(io_error)
        }
        Err(error) => Err(io_error(error)),
    }
}

fn ensure_real_file(
    path: &Path,
    label: &str,
    initial_bytes: impl FnOnce() -> Result<Vec<u8>, StoreError>,
) -> Result<(), StoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_file() => Ok(()),
        Ok(_) => Err(StoreError::Integrity(format!(
            "{label} must be a real file"
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            write_new_file(path, &initial_bytes()?)
        }
        Err(error) => Err(io_error(error)),
    }
}

fn open_lock(state_dir: &Path) -> Result<File, StoreError> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(state_dir.join("lifecycle.lock"))
        .map_err(io_error)
}

fn open_lifecycle_database(state_dir: &Path) -> Result<Database, StoreError> {
    let path = state_dir.join("lifecycle.store");
    match fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_file() => {
            Database::open(path).map_err(backend_error)
        }
        Ok(_) => Err(StoreError::Integrity(
            "lifecycle.store must be a real file".to_owned(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Database::create(path).map_err(backend_error)
        }
        Err(error) => Err(io_error(error)),
    }
}

fn next_attempt_sequence(database: &Database) -> Result<u64, StoreError> {
    let write = database.begin_write().map_err(backend_error)?;
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
    write.commit().map_err(backend_error)?;
    Ok(next)
}

fn insert_catalog_record(database: &Database, record: &RunCatalogRecord) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec(record).map_err(serialization_error)?;
    let write = database.begin_write().map_err(backend_error)?;
    {
        let mut table = write.open_table(RUN_CATALOG).map_err(backend_error)?;
        table
            .insert(record.run_id.as_str(), bytes.as_slice())
            .map_err(backend_error)?;
    }
    write.commit().map_err(backend_error)
}

fn read_catalog_record(
    database: &Database,
    run_id: &RunId,
) -> Result<RunCatalogRecord, StoreError> {
    let read = database.begin_read().map_err(backend_error)?;
    let table = read.open_table(RUN_CATALOG).map_err(backend_error)?;
    let value = table
        .get(run_id.as_str())
        .map_err(backend_error)?
        .ok_or_else(|| StoreError::RunNotFound(run_id.as_str().to_owned()))?;
    serde_json::from_slice(value.value()).map_err(serialization_error)
}

fn set_pointer(database: &Database, key: &str, value: &[u8]) -> Result<(), StoreError> {
    let write = database.begin_write().map_err(backend_error)?;
    {
        let mut table = write.open_table(POINTERS).map_err(backend_error)?;
        table.insert(key, value).map_err(backend_error)?;
    }
    write.commit().map_err(backend_error)
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

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io_error)?;
    file.write_all(bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)
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

fn nonce_hex() -> Result<String, StoreError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| StoreError::Io(error.to_string()))?;
    Ok(digest_hex(&bytes)[..32].to_owned())
}

fn unix_millis() -> Result<u128, StoreError> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn rejects_managed_parent_replacement() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        RepositoryStore::open(root.path())?;
        let runs = root.path().join(".lumin/runs");
        fs::remove_file(runs.join("namespace.anchor"))?;
        fs::remove_dir(&runs)?;
        fs::write(&runs, b"not a directory")?;

        require_integrity_failure(RepositoryStore::open(root.path()))
    }

    #[test]
    fn rejects_managed_parent_anchor_replacement() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        RepositoryStore::open(root.path())?;
        let anchor = root.path().join(".lumin/runs/namespace.anchor");
        fs::remove_file(&anchor)?;
        fs::create_dir(&anchor)?;

        require_integrity_failure(RepositoryStore::open(root.path()))
    }

    #[test]
    fn rejects_repository_marker_replacement() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        RepositoryStore::open(root.path())?;
        let marker = root.path().join(".lumin/repository.json");
        fs::remove_file(&marker)?;
        fs::create_dir(&marker)?;

        require_integrity_failure(RepositoryStore::open(root.path()))
    }
}
