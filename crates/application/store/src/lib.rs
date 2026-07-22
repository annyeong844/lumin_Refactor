mod gate;
mod generation;
mod namespace;
mod publication;
mod retention;

pub use gate::{
    ActiveGateLease, OperationSession, PostWriteFinish, PostWriteStart, PreWriteFinish,
    PreWriteStart, SemanticReadReservation,
};
pub use generation::StoreGeneration;
pub use namespace::MigrationIntent;
pub use publication::{AttemptEnvelope, AttemptSession, AttemptState, LatestRunSnapshot};
pub use retention::{RETENTION_PLAN_ITEMS_ORDERING, RetentionPlanRequest};

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use lumin_evidence::RunEvidence;
use lumin_model::{AttemptId, RepositoryBinding, RepositoryId, RunId, digest_hex};
use redb::{
    Database, ReadOnlyDatabase, ReadableDatabase, ReadableTable, TableDefinition, TableError,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub(crate) const SEQUENCES: TableDefinition<&str, u64> = TableDefinition::new("sequences");
pub(crate) const ATTEMPT_LEASES: TableDefinition<&str, &[u8]> =
    TableDefinition::new("attempt-leases");
pub(crate) const RUN_CATALOG: TableDefinition<&str, &[u8]> = TableDefinition::new("run-catalog");
pub(crate) const POINTERS: TableDefinition<&str, &[u8]> = TableDefinition::new("pointers");
const EVIDENCE: TableDefinition<&str, &[u8]> = TableDefinition::new("evidence");
const MAX_RUN_CATALOG_PAGE_SIZE: usize = 100;

#[derive(Clone, Debug)]
pub struct RepositoryStore {
    state_dir: PathBuf,
    namespace: namespace::NamespaceState,
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
    pub repository_id: RepositoryId,
    pub revision: u64,
    pub total: usize,
    pub runs: Vec<RunCatalogRecord>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunCatalogCursor {
    pub repository_id: RepositoryId,
    pub revision: u64,
    pub attempt_id: AttemptId,
    pub run_id: RunId,
    pub sequence: u64,
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
    #[error("run is already owned by retention: {0}")]
    RunRetentionState(String),
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
    #[error("run catalog cursor belongs to another repository")]
    RunCatalogScopeMismatch,
    #[error(
        "run catalog changed before continuation: expected revision {expected}, observed {observed}"
    )]
    RunCatalogRevisionChanged { expected: u64, observed: u64 },
    #[error("run catalog cursor anchor does not exist: {0}")]
    RunCatalogAnchorMissing(String),
    #[error("run catalog page size {requested} is outside 1..={max}")]
    RunCatalogPageSize { requested: usize, max: usize },
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
        let store = Self {
            state_dir,
            namespace,
        };
        store.recover_publication()?;
        Ok(store)
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
            let read = database.begin_read()?;
            if let Some(tombstone) = retention::records::read_validated_tombstone(
                &read,
                lumin_evidence::RetentionItemKind::Run,
                run_id.as_str(),
            )? {
                return if tombstone.envelope.tombstone_identity.is_some() {
                    Ok(lumin_evidence::RecordLookup::Pruned(tombstone.envelope))
                } else {
                    Ok(lumin_evidence::RecordLookup::Pruning(tombstone.envelope))
                };
            }
            drop(read);
            read_live_run(&self.state_dir, &database, run_id)
                .map(lumin_evidence::RecordLookup::Live)
        })
    }

    pub fn list_runs(
        &self,
        cursor: Option<&RunCatalogCursor>,
        limit: usize,
    ) -> Result<RunCatalogSnapshot, StoreError> {
        if !(1..=MAX_RUN_CATALOG_PAGE_SIZE).contains(&limit) {
            return Err(StoreError::RunCatalogPageSize {
                requested: limit,
                max: MAX_RUN_CATALOG_PAGE_SIZE,
            });
        }
        self.with_shared_lock(|guard| {
            let repository_id = guard.repository_id().clone();
            let database = guard.open_database()?;
            let read = database.begin_read()?;
            let revision = read_sequence(&read, "run-catalog")?;
            validate_run_catalog_cursor(&repository_id, revision, cursor)?;
            let (total, runs, truncated) = read_run_catalog_page(&read, cursor, limit)?;
            Ok(RunCatalogSnapshot {
                repository_id,
                revision,
                total,
                runs,
                truncated,
            })
        })
    }

    pub fn latest_run_id(&self) -> Result<Option<RunId>, StoreError> {
        publication::latest_run_id(self)
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

fn insert_catalog_record(
    guard: &namespace::NamespaceGuard,
    database: &namespace::StoreDatabase<'_>,
    record: &RunCatalogRecord,
) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec(record).map_err(serialization_error)?;
    let write = database.begin_write()?;
    let inserted = {
        let mut table = write.open_table(RUN_CATALOG).map_err(backend_error)?;
        let current = table
            .get(record.run_id.as_str())
            .map_err(backend_error)?
            .map(|value| value.value().to_vec());
        match current {
            Some(current) if current == bytes => false,
            Some(_) => {
                return Err(StoreError::Integrity(format!(
                    "run catalog record changed for {}",
                    record.run_id.as_str()
                )));
            }
            None => {
                table
                    .insert(record.run_id.as_str(), bytes.as_slice())
                    .map_err(backend_error)?;
                true
            }
        }
    };
    if inserted {
        retention::records::next_sequence(&write, "run-catalog")?;
    }
    guard.commit(write)
}

fn read_sequence(read: &redb::ReadTransaction, key: &str) -> Result<u64, StoreError> {
    let table = read.open_table(SEQUENCES).map_err(backend_error)?;
    table
        .get(key)
        .map_err(backend_error)
        .map(|value| value.map_or(0, |value| value.value()))
}

fn validate_run_catalog_cursor(
    repository_id: &RepositoryId,
    revision: u64,
    cursor: Option<&RunCatalogCursor>,
) -> Result<(), StoreError> {
    let Some(cursor) = cursor else {
        return Ok(());
    };
    if &cursor.repository_id != repository_id {
        return Err(StoreError::RunCatalogScopeMismatch);
    }
    if cursor.revision != revision {
        return Err(StoreError::RunCatalogRevisionChanged {
            expected: cursor.revision,
            observed: revision,
        });
    }
    Ok(())
}

fn read_run_catalog_page(
    read: &redb::ReadTransaction,
    cursor: Option<&RunCatalogCursor>,
    limit: usize,
) -> Result<(usize, Vec<RunCatalogRecord>, bool), StoreError> {
    let tombstones = match read.open_table(retention::RETENTION_TOMBSTONES) {
        Ok(table) => Some(table),
        Err(TableError::TableDoesNotExist(_)) => None,
        Err(error) => return Err(backend_error(error)),
    };
    let table = read.open_table(RUN_CATALOG).map_err(backend_error)?;
    let mut runs = Vec::with_capacity(limit.saturating_add(1));
    let mut total = 0usize;
    let mut anchor_found = cursor.is_none();
    for row in table.iter().map_err(backend_error)?.rev() {
        let (key, value) = row.map_err(backend_error)?;
        let key = key.value();
        let tombstone_key =
            retention::records::tombstone_key(lumin_evidence::RetentionItemKind::Run, key);
        if let Some(tombstones) = &tombstones
            && let Some(bytes) = tombstones
                .get(tombstone_key.as_str())
                .map_err(backend_error)?
                .map(|value| value.value().to_vec())
        {
            let tombstone: retention::records::StoredTombstone =
                serde_json::from_slice(&bytes).map_err(serialization_error)?;
            retention::records::validate_tombstone_owner(read, &tombstone_key, &tombstone)?;
            continue;
        }
        let record: RunCatalogRecord =
            serde_json::from_slice(value.value()).map_err(serialization_error)?;
        if record.run_id.as_str() != key {
            return Err(StoreError::Integrity(format!(
                "run catalog key {key} disagrees with its record"
            )));
        }
        total = total
            .checked_add(1)
            .ok_or_else(|| StoreError::Integrity("run catalog total overflow".to_owned()))?;
        if !anchor_found {
            anchor_found = cursor.is_some_and(|cursor| {
                cursor.attempt_id == record.attempt_id
                    && cursor.run_id == record.run_id
                    && cursor.sequence == record.sequence
            });
            continue;
        }
        if runs.len() <= limit {
            runs.push(record);
        }
    }
    if !anchor_found {
        return Err(StoreError::RunCatalogAnchorMissing(
            cursor
                .map(|cursor| cursor.run_id.as_str().to_owned())
                .unwrap_or_default(),
        ));
    }
    let truncated = runs.len() > limit;
    runs.truncate(limit);
    Ok((total, runs, truncated))
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
    // Writable redb opens may update container metadata and invalidate the published byte hash.
    let database = ReadOnlyDatabase::open(path).map_err(backend_error)?;
    let read = database.begin_read().map_err(backend_error)?;
    let table = read.open_table(EVIDENCE).map_err(backend_error)?;
    let value = table
        .get("run")
        .map_err(backend_error)?
        .ok_or_else(|| StoreError::Integrity("run evidence row is missing".to_owned()))?;
    serde_json::from_slice(value.value()).map_err(serialization_error)
}

#[cfg(test)]
fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, StoreError> {
    let bytes = fs::read(path).map_err(io_error)?;
    serde_json::from_slice(&bytes).map_err(serialization_error)
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
