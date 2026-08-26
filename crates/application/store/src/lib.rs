mod cache;
mod gate;
mod generation;
mod namespace;
mod publication;
mod retention;

pub use gate::{
    ActiveGateCatalogCursor, ActiveGateCatalogItem, ActiveGateCatalogSnapshot, ActiveGateLease,
    GateBaselineDraft, ObservationFinalization, OperationSession, PostWriteFinish, PostWriteStart,
    PreWriteFinish, PreWriteStart, SemanticReadReservation,
};
pub use generation::StoreGeneration;
pub use namespace::MigrationIntent;
pub use publication::{AttemptEnvelope, AttemptSession, AttemptState, LatestRunSnapshot};
pub use retention::{RETENTION_PLAN_ITEMS_ORDERING, RetentionPlanRequest};

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lumin_evidence::{RUN_EVIDENCE_SCHEMA_VERSION, RunEvidence};
use lumin_model::{
    AttemptId, PhysicalFileIdentity, RepositoryBinding, RepositoryId, RunId,
    append_length_prefixed, digest_hex,
};
use redb::{
    Database, ReadableDatabase, ReadableTable, StorageBackend, TableDefinition, TableError,
    TableHandle, backends::InMemoryBackend,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;

pub(crate) const SEQUENCES: TableDefinition<&str, u64> = TableDefinition::new("sequences");
pub(crate) const ATTEMPT_LEASES: TableDefinition<&str, &[u8]> =
    TableDefinition::new("attempt-leases");
pub(crate) const RUN_CATALOG: TableDefinition<&str, &[u8]> = TableDefinition::new("run-catalog");
pub(crate) const POINTERS: TableDefinition<&str, &[u8]> = TableDefinition::new("pointers");
pub(crate) const EVIDENCE: TableDefinition<&str, &[u8]> = TableDefinition::new("evidence");
const MAX_RUN_CATALOG_PAGE_SIZE: usize = 100;

pub fn evidence_payload_sha256(evidence: &RunEvidence) -> Result<String, StoreError> {
    let encoded = serde_json::to_vec(evidence).map_err(serialization_error)?;
    let mut framed = Vec::new();
    append_length_prefixed(&mut framed, b"lumin-run-evidence-payload.v1");
    append_length_prefixed(&mut framed, &encoded);
    Ok(digest_hex(&framed))
}

pub(crate) fn decode_closed_json<T>(bytes: &[u8]) -> Result<T, String>
where
    T: DeserializeOwned + Serialize,
{
    let decoded = serde_json::from_slice::<T>(bytes).map_err(|error| error.to_string())?;
    validate_closed_json_projection(bytes, &decoded)?;
    Ok(decoded)
}

fn validate_closed_json_projection<T: Serialize>(bytes: &[u8], decoded: &T) -> Result<(), String> {
    let source =
        serde_json::from_slice::<serde_json::Value>(bytes).map_err(|error| error.to_string())?;
    let projected = serde_json::to_value(decoded).map_err(|error| error.to_string())?;
    if let Some(path) = first_unsupported_json_path(&source, &projected, "") {
        return Err(format!(
            "contains unsupported JSON member or shape at {path}"
        ));
    }
    Ok(())
}

fn first_unsupported_json_path(
    source: &serde_json::Value,
    projected: &serde_json::Value,
    path: &str,
) -> Option<String> {
    match (source, projected) {
        (serde_json::Value::Object(source), serde_json::Value::Object(projected)) => {
            source.iter().find_map(|(key, value)| {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                projected
                    .get(key)
                    .map_or(Some(child_path.clone()), |projected| {
                        first_unsupported_json_path(value, projected, &child_path)
                    })
            })
        }
        (serde_json::Value::Array(source), serde_json::Value::Array(projected)) => {
            if source.len() != projected.len() {
                return Some(if path.is_empty() {
                    "$".to_owned()
                } else {
                    path.to_owned()
                });
            }
            source
                .iter()
                .zip(projected)
                .enumerate()
                .find_map(|(index, (source, projected))| {
                    first_unsupported_json_path(source, projected, &format!("{path}[{index}]"))
                })
        }
        (serde_json::Value::Object(_) | serde_json::Value::Array(_), _) => {
            Some(if path.is_empty() {
                "$".to_owned()
            } else {
                path.to_owned()
            })
        }
        _ => None,
    }
}

#[derive(Clone, Debug)]
pub struct RepositoryStore {
    state_dir: PathBuf,
    namespace: namespace::NamespaceState,
}

#[cfg(any(test, feature = "lifecycle-migration-test-fault"))]
impl RepositoryStore {
    pub fn rewrite_current_store_header_as_prior_for_test(&self) -> Result<(), StoreError> {
        self.namespace
            .rewrite_current_store_header_as_prior_for_test()
    }

    #[cfg(feature = "lifecycle-migration-test-fault")]
    pub fn rewrite_existing_lifecycle_store_header_as_prior_for_test(
        root: &Path,
        binding: &RepositoryBinding,
    ) -> Result<(), StoreError> {
        let namespace = namespace::NamespaceState::open_for_migration(root, binding)?
            .ok_or(StoreError::LifecycleStoreNotInitialized)?;
        namespace.rewrite_current_store_header_as_prior_for_test()
    }

    #[cfg(feature = "lifecycle-migration-test-fault")]
    pub fn corrupt_migration_anchor_for_test(&self) -> Result<(), StoreError> {
        self.namespace.corrupt_migration_anchor_for_test()
    }

    #[cfg(feature = "lifecycle-migration-test-fault")]
    pub fn remove_bound_root_authorization_for_test(
        root: &Path,
        binding: &RepositoryBinding,
    ) -> Result<(), StoreError> {
        let namespace = namespace::NamespaceState::open_for_migration(root, binding)?
            .ok_or(StoreError::LifecycleStoreNotInitialized)?;
        namespace.remove_bound_root_authorization_for_test()
    }
}

#[cfg(any(test, feature = "lifecycle-migration-test-fault"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PriorCacheCleanupDeliveryStatusForTest {
    NotAttempted,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishedRun {
    pub attempt_id: AttemptId,
    pub run_id: RunId,
    pub sequence: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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
    pub page_size: usize,
    pub attempt_id: AttemptId,
    pub run_id: RunId,
    pub sequence: u64,
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("incompatible state schema: {0}")]
    IncompatibleStateSchema(String),
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
    #[error("active gate catalog cursor belongs to another repository or page size")]
    ActiveGateCatalogScopeMismatch,
    #[error(
        "active gate catalog changed before continuation: expected revision {expected}, observed {observed}"
    )]
    ActiveGateCatalogRevisionChanged { expected: u64, observed: u64 },
    #[error("active gate catalog cursor anchor does not exist: {0}")]
    ActiveGateCatalogAnchorMissing(String),
    #[error("active gate catalog page size {requested} is outside 1..={max}")]
    ActiveGateCatalogPageSize { requested: usize, max: usize },
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
    #[error("lifecycle store migration requires 'lumin store migrate'")]
    LifecycleMigrationRequired,
    #[error("lifecycle store is not initialized")]
    LifecycleStoreNotInitialized,
}

impl RepositoryStore {
    pub fn open(root: &Path, binding: &RepositoryBinding) -> Result<Self, StoreError> {
        let namespace = namespace::NamespaceState::open(root, binding)?;
        Self::from_namespace(namespace)
    }

    /// Open only a marker-bound namespace without creating or resuming state.
    pub fn open_if_bound(
        root: &Path,
        binding: &RepositoryBinding,
    ) -> Result<Option<Self>, StoreError> {
        namespace::NamespaceState::open_if_bound(root, binding)?
            .map(Self::from_namespace)
            .transpose()
    }

    fn from_namespace(namespace: namespace::NamespaceState) -> Result<Self, StoreError> {
        let state_dir = namespace.state_dir().to_path_buf();
        let store = Self {
            state_dir,
            namespace,
        };
        store.recover_publication()?;
        Ok(store)
    }

    /// Resolve one actual shared evidence candidate against store ownership.
    /// Suspicious shared or mount-crossing candidates re-index retained state
    /// under the cross-process lifecycle lock. Ordinary one-link evidence never
    /// enters this path.
    pub fn owns_reserved_state_identity(
        &self,
        identity: &PhysicalFileIdentity,
    ) -> Result<bool, StoreError> {
        self.namespace
            .reserved_state_identities()
            .map(|identities| identities.contains(identity))
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
            validate_run_catalog_cursor(&repository_id, revision, cursor, limit)?;
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

    pub fn migrate_existing_lifecycle_store(
        root: &Path,
        binding: &RepositoryBinding,
    ) -> Result<StoreGeneration, StoreError> {
        let namespace = namespace::NamespaceState::open_for_migration(root, binding)?
            .ok_or(StoreError::LifecycleStoreNotInitialized)?;
        namespace.migrate_lifecycle_store()
    }

    #[cfg(feature = "lifecycle-migration-test-fault")]
    pub fn corrupt_migrating_cleanup_operation_for_test(
        root: &Path,
        binding: &RepositoryBinding,
        operation_id: &lumin_model::OperationId,
    ) -> Result<(), StoreError> {
        let namespace = namespace::NamespaceState::open_for_migration(root, binding)?
            .ok_or(StoreError::LifecycleStoreNotInitialized)?;
        namespace.corrupt_migrating_cleanup_operation_for_test(operation_id)
    }

    fn with_exclusive_lock<T>(
        &self,
        operation: impl FnOnce(&namespace::NamespaceGuard) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.namespace.with_exclusive_lock(operation)
    }

    #[cfg(feature = "publication-test-crash")]
    fn with_exclusive_lock_after_contention<T>(
        &self,
        on_contention: impl FnOnce() -> Result<(), StoreError>,
        operation: impl FnOnce(&namespace::NamespaceGuard) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.namespace
            .with_exclusive_lock_after_contention(on_contention, operation)
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
    limit: usize,
) -> Result<(), StoreError> {
    let Some(cursor) = cursor else {
        return Ok(());
    };
    if &cursor.repository_id != repository_id || cursor.page_size != limit {
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
    // Collect all visible (non-tombstoned) records
    let mut visible = Vec::new();
    for row in table.iter().map_err(backend_error)? {
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
        visible.push(record);
    }
    // Sort explicitly: sequence DESC then run_id ASC
    visible.sort_by(|a, b| {
        b.sequence
            .cmp(&a.sequence)
            .then_with(|| a.run_id.as_str().cmp(b.run_id.as_str()))
    });
    let total = visible.len();
    // Find cursor anchor in sorted visible vec
    let start = if let Some(cursor) = cursor {
        let anchor_index = visible
            .iter()
            .position(|record| {
                record.attempt_id == cursor.attempt_id
                    && record.run_id == cursor.run_id
                    && record.sequence == cursor.sequence
            })
            .ok_or_else(|| {
                StoreError::RunCatalogAnchorMissing(cursor.run_id.as_str().to_owned())
            })?;
        let resume_offset = anchor_index + 1;
        // Validate canonical nonterminal boundary
        if !resume_offset.is_multiple_of(limit) || resume_offset >= total {
            return Err(StoreError::RunCatalogAnchorMissing(
                cursor.run_id.as_str().to_owned(),
            ));
        }
        resume_offset
    } else {
        0
    };
    let end = start.saturating_add(limit).min(total);
    let runs = visible[start..end].to_vec();
    let truncated = end < total;
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
    Ok((record, read_evidence_store(&bytes)?))
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

fn read_evidence_store(bytes: &[u8]) -> Result<RunEvidence, StoreError> {
    let backend = InMemoryBackend::new();
    let length = u64::try_from(bytes.len())
        .map_err(|_| StoreError::Integrity("evidence store byte count overflow".to_owned()))?;
    backend.set_len(length).map_err(io_error)?;
    backend.write(0, bytes).map_err(io_error)?;
    // Decode the exact bytes already bound to the run envelope. A writable
    // redb open may update its private backend metadata, so the isolated
    // in-memory copy also keeps the published container immutable.
    let mut builder = Database::builder();
    builder.set_repair_callback(|session| session.abort());
    let database = builder
        .create_with_backend(backend)
        .map_err(backend_error)?;
    let read = database.begin_read().map_err(backend_error)?;
    let observed_tables = read
        .list_tables()
        .map_err(backend_error)?
        .map(|table| table.name().to_owned())
        .collect::<BTreeSet<_>>();
    let expected_tables = BTreeSet::from(["evidence".to_owned()]);
    if observed_tables != expected_tables {
        return Err(StoreError::Integrity(format!(
            "evidence store contains unsupported table inventory: {}",
            observed_tables.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    if read
        .list_multimap_tables()
        .map_err(backend_error)?
        .next()
        .is_some()
    {
        return Err(StoreError::Integrity(
            "evidence store contains unsupported multimap tables".to_owned(),
        ));
    }
    let table = read.open_table(EVIDENCE).map_err(backend_error)?;
    let mut rows = table.iter().map_err(backend_error)?;
    let (key, value) = rows
        .next()
        .transpose()
        .map_err(backend_error)?
        .ok_or_else(|| StoreError::Integrity("run evidence row is missing".to_owned()))?;
    if key.value() != "run" || rows.next().transpose().map_err(backend_error)?.is_some() {
        return Err(StoreError::Integrity(
            "evidence store must contain exactly the run evidence row".to_owned(),
        ));
    }
    let bytes = value.value().to_vec();
    let evidence: RunEvidence = serde_json::from_slice(&bytes).map_err(serialization_error)?;
    validate_closed_json_projection(&bytes, &evidence)
        .map_err(|error| StoreError::Integrity(format!("run evidence row {error}")))?;
    if evidence.schema_version != RUN_EVIDENCE_SCHEMA_VERSION {
        return Err(StoreError::IncompatibleStateSchema(format!(
            "run evidence uses unsupported schema {}; expected {RUN_EVIDENCE_SCHEMA_VERSION}",
            evidence.schema_version
        )));
    }
    lumin_evidence::validate_run_evidence_identities(&evidence).map_err(|error| {
        StoreError::Integrity(format!("run evidence identity validation failed: {error}"))
    })?;
    Ok(evidence)
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

pub(crate) fn unix_millis() -> Result<u64, StoreError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| StoreError::Io(error.to_string()))?;
    unix_millis_from_duration(duration)
}

pub(crate) fn unix_millis_from_duration(duration: Duration) -> Result<u64, StoreError> {
    duration.as_millis().try_into().map_err(|_| {
        StoreError::Io("Unix millisecond timestamp exceeds the supported u64 range".to_owned())
    })
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
