#[macro_use]
mod audit_profile;
mod cache;
mod gate;
mod generation;
mod namespace;
mod publication;
mod retention;

#[cfg(all(
    feature = "audit-store-test-profile",
    any(
        feature = "cache-cleanup-test-fault",
        feature = "collection-ordering-test-perturb",
        feature = "lifecycle-migration-test-fault",
        feature = "namespace-test-crash",
        feature = "publication-test-crash",
        feature = "retention-test-crash"
    )
))]
compile_error!(
    "audit-execution-test-profile cannot be combined with fault/crash/perturbation features (enabled by audit-store-test-profile)"
);

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
const EVIDENCE_CHUNK_BYTES: usize = 63 * 1024;
const EVIDENCE_CHUNK_PREFIX: &str = "run-chunk.v1/";
const MAX_RUN_CATALOG_PAGE_SIZE: usize = 100;

pub struct PreparedRunEvidence {
    row: Vec<u8>,
}

#[cfg(feature = "collection-ordering-test-perturb")]
const COLLECTION_ORDERING_PERTURB_ENV: &str = "LUMIN_TEST_COLLECTION_ORDERING_PERTURB";
#[cfg(feature = "collection-ordering-test-perturb")]
const COLLECTION_ORDERING_TRACE_ENV: &str = "LUMIN_TEST_COLLECTION_ORDERING_TRACE";

#[cfg(feature = "collection-ordering-test-perturb")]
pub(crate) fn perturb_collection_order<T>(items: &mut [T], collection: &str) {
    if items.len() < 2
        || !std::env::var(COLLECTION_ORDERING_PERTURB_ENV).is_ok_and(|value| value == "reverse")
    {
        return;
    }
    items.reverse();
    if let Some(trace_root) = std::env::var_os(COLLECTION_ORDERING_TRACE_ENV) {
        // The public fixture requires the exact marker set, so a failed write
        // still fails the test without adding a panic path to store code.
        let _ = std::fs::write(
            std::path::PathBuf::from(trace_root).join(collection),
            b"reversed\n",
        );
    }
}

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
    let canonical = serde_json::to_vec(decoded).map_err(|error| error.to_string())?;
    if bytes == canonical {
        return Ok(());
    }
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

#[cfg(any(feature = "namespace-test-crash", feature = "retention-test-crash"))]
pub fn state_entry_physical_identity_for_test(
    path: &Path,
) -> Result<PhysicalFileIdentity, StoreError> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    let kind = if metadata.file_type().is_dir() {
        namespace::EntryKind::Directory
    } else if metadata.file_type().is_file() {
        namespace::EntryKind::RegularFile
    } else {
        return Err(StoreError::Integrity(
            "test state entry is not a regular file or directory".to_owned(),
        ));
    };
    namespace::HeldEntry::open(
        path,
        kind,
        namespace::EntryAccess::ReadOnly,
        false,
        "test state entry",
    )
    .map(|entry| entry.identity().clone())
}

#[cfg(any(
    test,
    feature = "lifecycle-migration-test-fault",
    feature = "logical-store-snapshot-test",
    feature = "namespace-test-crash",
    feature = "retention-test-crash"
))]
impl RepositoryStore {
    #[cfg(any(test, feature = "lifecycle-migration-test-fault"))]
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

    #[cfg(feature = "namespace-test-crash")]
    pub fn remove_cache_eviction_binding_for_test(&self) -> Result<(), StoreError> {
        self.namespace.remove_cache_eviction_binding_for_test()
    }

    #[cfg(any(
        test,
        feature = "logical-store-snapshot-test",
        feature = "namespace-test-crash",
        feature = "retention-test-crash"
    ))]
    pub fn current_logical_snapshot_for_test(
        root: &Path,
        binding: &RepositoryBinding,
    ) -> Result<Vec<u8>, StoreError> {
        let namespace = namespace::NamespaceState::open_for_observation(root, binding)?
            .ok_or(StoreError::LifecycleStoreNotInitialized)?;
        namespace.with_observation_lock(namespace::current_logical_snapshot_for_test)
    }

    #[cfg(any(
        test,
        feature = "logical-store-snapshot-test",
        feature = "namespace-test-crash",
        feature = "retention-test-crash"
    ))]
    pub fn complete_logical_observation_for_test(
        root: &Path,
        binding: &RepositoryBinding,
    ) -> Result<Vec<u8>, StoreError> {
        let namespace = namespace::NamespaceState::open_for_observation(root, binding)?
            .ok_or(StoreError::LifecycleStoreNotInitialized)?;
        namespace.with_observation_lock(namespace::complete_logical_observation_for_test)
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
        Self::open_core(
            root,
            binding,
            #[cfg(feature = "audit-store-test-profile")]
            None,
        )
    }

    #[cfg(feature = "audit-store-test-profile")]
    pub fn open_observed(
        root: &Path,
        binding: &RepositoryBinding,
    ) -> (
        Result<Self, StoreError>,
        lumin_model::audit_store_diagnostic::AuditStoreTimings,
    ) {
        use lumin_model::audit_store_diagnostic::AuditStorePhase;
        let mut recorder = audit_profile::StoreProfiler::new(AuditStorePhase::StoreOpen);
        recorder.begin(AuditStorePhase::StoreOpen);
        let result = Self::open_core(root, binding, Some(&mut recorder));
        recorder.end(AuditStorePhase::StoreOpen);
        (result, recorder.finish())
    }

    fn open_core(
        root: &Path,
        binding: &RepositoryBinding,
        #[cfg(feature = "audit-store-test-profile")] mut profile: Option<
            &mut audit_profile::StoreProfiler,
        >,
    ) -> Result<Self, StoreError> {
        store_phase_begin!(profile, NamespaceOpen);
        let namespace = namespace::NamespaceState::open(
            root,
            binding,
            #[cfg(feature = "audit-store-test-profile")]
            profile.as_deref_mut(),
        )?;
        store_phase_end!(profile, NamespaceOpen);
        Self::from_namespace_core(
            namespace,
            #[cfg(feature = "audit-store-test-profile")]
            profile,
        )
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
        Self::from_namespace_core(
            namespace,
            #[cfg(feature = "audit-store-test-profile")]
            None,
        )
    }

    fn from_namespace_core(
        namespace: namespace::NamespaceState,
        #[cfg(feature = "audit-store-test-profile")] mut profile: Option<
            &mut audit_profile::StoreProfiler,
        >,
    ) -> Result<Self, StoreError> {
        let state_dir = namespace.state_dir().to_path_buf();
        let store = Self {
            state_dir,
            namespace,
        };
        store_phase_begin!(profile, OpenRecovery);
        store.recover_publication(
            #[cfg(feature = "audit-store-test-profile")]
            profile.as_deref_mut(),
        )?;
        store_phase_end!(profile, OpenRecovery);
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

    fn with_admission_exclusive_lock<T>(
        &self,
        operation: impl FnOnce(&namespace::NamespaceGuard) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.namespace.with_admission_exclusive_lock(operation)
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
    #[cfg(feature = "collection-ordering-test-perturb")]
    perturb_collection_order(&mut visible, "runs");
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

pub fn prepare_run_evidence(evidence: &RunEvidence) -> Result<PreparedRunEvidence, StoreError> {
    if evidence.schema_version != RUN_EVIDENCE_SCHEMA_VERSION {
        return Err(StoreError::IncompatibleStateSchema(format!(
            "run evidence uses unsupported schema {}; expected {RUN_EVIDENCE_SCHEMA_VERSION}",
            evidence.schema_version
        )));
    }
    lumin_evidence::validate_run_evidence_identities(evidence).map_err(|error| {
        StoreError::Integrity(format!("run evidence identity validation failed: {error}"))
    })?;
    let row = serde_json::to_vec(evidence).map_err(serialization_error)?;
    Ok(PreparedRunEvidence { row })
}

fn write_evidence_store(
    path: &Path,
    evidence: &PreparedRunEvidence,
    #[cfg(feature = "audit-store-test-profile")] mut profile: Option<
        &mut audit_profile::StoreProfiler,
    >,
) -> Result<(), StoreError> {
    store_phase_begin!(profile, EvidenceCreate);
    let database = Database::create(path).map_err(backend_error)?;
    store_phase_end!(profile, EvidenceCreate);
    store_phase_begin!(profile, EvidenceBeginWrite);
    let write = database.begin_write().map_err(backend_error)?;
    store_phase_end!(profile, EvidenceBeginWrite);
    store_phase_begin!(profile, EvidenceRows);
    {
        let mut table = write.open_table(EVIDENCE).map_err(backend_error)?;
        // redb reserves a power-of-two page extent for one large variable-width value. A 63 KiB
        // payload plus its key/value framing stays below the 64 KiB extent boundary, keeping the
        // immutable container proportional to its logical payload while preserving one
        // deterministic, closed physical representation.
        for (index, chunk) in evidence.row.chunks(EVIDENCE_CHUNK_BYTES).enumerate() {
            let key = format!("{EVIDENCE_CHUNK_PREFIX}{index:016x}");
            table.insert(key.as_str(), chunk).map_err(backend_error)?;
        }
    }
    store_phase_end!(profile, EvidenceRows);
    store_phase_begin!(profile, EvidenceCommit);
    write.commit().map_err(backend_error)?;
    store_phase_end!(profile, EvidenceCommit);
    store_phase_begin!(profile, EvidenceClose);
    drop(database);
    store_phase_end!(profile, EvidenceClose);
    Ok(())
}

impl PreparedRunEvidence {
    pub(crate) fn row(&self) -> &[u8] {
        &self.row
    }
}

fn read_evidence_store(bytes: &[u8]) -> Result<RunEvidence, StoreError> {
    let bytes = read_evidence_store_row(bytes)?;
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

pub(crate) fn read_evidence_store_row(bytes: &[u8]) -> Result<Vec<u8>, StoreError> {
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
    let (first_key, first_value) = rows
        .next()
        .transpose()
        .map_err(backend_error)?
        .ok_or_else(|| StoreError::Integrity("run evidence row is missing".to_owned()))?;
    if first_key.value() == "run" {
        if rows.next().transpose().map_err(backend_error)?.is_some() {
            return Err(StoreError::Integrity(
                "legacy evidence store contains additional rows".to_owned(),
            ));
        }
        return Ok(first_value.value().to_vec());
    }

    let mut result = Vec::new();
    let mut previous_chunk_len = None;
    for (index, row) in std::iter::once(Ok((first_key, first_value)))
        .chain(rows)
        .enumerate()
    {
        if previous_chunk_len.is_some_and(|length| length != EVIDENCE_CHUNK_BYTES) {
            return Err(StoreError::Integrity(
                "evidence store contains a noncanonical chunk inventory".to_owned(),
            ));
        }
        let (key, value) = row.map_err(backend_error)?;
        let expected = format!("{EVIDENCE_CHUNK_PREFIX}{index:016x}");
        if key.value() != expected
            || value.value().is_empty()
            || value.value().len() > EVIDENCE_CHUNK_BYTES
        {
            return Err(StoreError::Integrity(
                "evidence store contains a noncanonical chunk inventory".to_owned(),
            ));
        }
        result
            .try_reserve(value.value().len())
            .map_err(|_| StoreError::Integrity("run evidence row is too large".to_owned()))?;
        result.extend_from_slice(value.value());
        previous_chunk_len = Some(value.value().len());
    }
    Ok(result)
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

#[cfg(test)]
mod evidence_store_tests {
    use super::*;

    #[test]
    fn chunked_evidence_store_round_trips_the_exact_row() -> Result<(), Box<dyn std::error::Error>>
    {
        let fixture = tempfile::tempdir()?;
        let path = fixture.path().join("evidence.store");
        let row = (0..(EVIDENCE_CHUNK_BYTES * 2 + 17))
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        write_evidence_store(
            &path,
            &PreparedRunEvidence { row: row.clone() },
            #[cfg(feature = "audit-store-test-profile")]
            None,
        )?;

        let store_bytes = fs::read(&path)?;
        assert_eq!(read_evidence_store_row(&store_bytes)?, row);

        let database = Database::open(&path)?;
        let read = database.begin_read()?;
        let table = read.open_table(EVIDENCE)?;
        let rows = table
            .iter()?
            .map(|row| {
                let (key, value) = row?;
                Ok((key.value().to_owned(), value.value().len()))
            })
            .collect::<Result<Vec<_>, redb::StorageError>>()?;
        assert_eq!(
            rows,
            vec![
                (
                    format!("{EVIDENCE_CHUNK_PREFIX}0000000000000000"),
                    EVIDENCE_CHUNK_BYTES
                ),
                (
                    format!("{EVIDENCE_CHUNK_PREFIX}0000000000000001"),
                    EVIDENCE_CHUNK_BYTES
                ),
                (format!("{EVIDENCE_CHUNK_PREFIX}0000000000000002"), 17),
            ]
        );
        Ok(())
    }

    #[test]
    fn legacy_single_row_evidence_store_remains_readable() -> Result<(), Box<dyn std::error::Error>>
    {
        let fixture = tempfile::tempdir()?;
        let path = fixture.path().join("evidence.store");
        write_test_evidence_store(&path, &[("run", b"legacy-row")])?;

        assert_eq!(read_evidence_store_row(&fs::read(path)?)?, b"legacy-row");
        Ok(())
    }

    #[test]
    fn chunked_evidence_store_rejects_gaps_and_short_nonfinal_chunks()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = tempfile::tempdir()?;
        let gap = fixture.path().join("gap.store");
        write_test_evidence_store(
            &gap,
            &[(&format!("{EVIDENCE_CHUNK_PREFIX}0000000000000001"), b"gap")],
        )?;
        assert!(matches!(
            read_evidence_store_row(&fs::read(gap)?),
            Err(StoreError::Integrity(detail)) if detail.contains("noncanonical chunk inventory")
        ));

        let short = fixture.path().join("short.store");
        let first = format!("{EVIDENCE_CHUNK_PREFIX}0000000000000000");
        let second = format!("{EVIDENCE_CHUNK_PREFIX}0000000000000001");
        write_test_evidence_store(
            &short,
            &[(first.as_str(), b"short"), (second.as_str(), b"next")],
        )?;
        assert!(matches!(
            read_evidence_store_row(&fs::read(short)?),
            Err(StoreError::Integrity(detail)) if detail.contains("noncanonical chunk inventory")
        ));
        Ok(())
    }

    fn write_test_evidence_store(
        path: &Path,
        rows: &[(&str, &[u8])],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let database = Database::create(path)?;
        let write = database.begin_write()?;
        {
            let mut table = write.open_table(EVIDENCE)?;
            for (key, value) in rows {
                table.insert(*key, *value)?;
            }
        }
        write.commit()?;
        Ok(())
    }
}
