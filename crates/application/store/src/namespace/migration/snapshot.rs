mod tables;
mod validation;

use std::collections::{BTreeMap, BTreeSet};

use lumin_evidence::{
    CacheCleanupDeliveryCompletion, CacheCleanupDeliveryOutcome, CacheCleanupExecutionLease,
    CacheCleanupOperationRecord, CacheCleanupOperationStatus, CacheCleanupRecoveryReservation,
    CacheCleanupResult,
};
use lumin_model::{CacheEvictionAuthorizationSetId, OperationId, RepositoryId};
use redb::{Database, ReadableDatabase};
use serde::{Deserialize, Serialize};

use crate::{RunCatalogRecord, StoreError, StoreGeneration, backend_error, io_error};

use self::tables::{read_legacy_snapshot, read_snapshot, read_table_inventory, write_snapshot};
use self::validation::validate_legacy_referential_closure;
use super::super::platform::{EntryAccess, EntryKind, HeldEntry, UnpublishedFile};
use super::super::store_header::{
    MigrationProvenanceAnchor, initialize_store_with_anchor, read_store_header_bytes_from_read,
    verify_prior_store_header, verify_store_header_anchor, verify_validation_receipt_set_read,
};
use super::super::{MigrationDatabase, NamespaceGuard, detached_database, require_state_volume};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(super) struct LogicalStoreSnapshot {
    sequences: BTreeMap<String, u64>,
    attempt_leases: BTreeMap<String, Vec<u8>>,
    run_catalog: BTreeMap<String, Vec<u8>>,
    pointers: BTreeMap<String, Vec<u8>>,
    gates: BTreeMap<String, Vec<u8>>,
    operations: BTreeMap<String, Vec<u8>>,
    validation_receipts: BTreeMap<String, Vec<u8>>,
    transitions: BTreeMap<String, Vec<u8>>,
    retention_plans: BTreeMap<String, Vec<u8>>,
    retention_operations: BTreeMap<String, Vec<u8>>,
    retention_tombstones: BTreeMap<String, Vec<u8>>,
    run_pins: BTreeMap<String, Vec<u8>>,
    cache_cleanup_operations: BTreeMap<String, Vec<u8>>,
    cache_eviction_authorizations: BTreeMap<String, Vec<u8>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    analysis_cache_authorizations: BTreeMap<String, Vec<u8>>,
}

pub(super) struct LegacyStore {
    pub(super) entry: HeldEntry,
    pub(super) database: MigrationDatabase,
    pub(super) generation: StoreGeneration,
    pub(super) snapshot: LogicalStoreSnapshot,
}

pub(super) struct CurrentStore {
    pub(super) entry: HeldEntry,
    pub(super) generation: StoreGeneration,
    pub(super) anchor: Option<MigrationProvenanceAnchor>,
    pub(super) snapshot: LogicalStoreSnapshot,
    observation_metadata: CurrentStoreObservationMetadata,
}

struct ValidatedCurrentDatabase {
    snapshot: LogicalStoreSnapshot,
    observation_metadata: CurrentStoreObservationMetadata,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CurrentStoreObservationMetadata {
    store_header: Vec<u8>,
    table_names: BTreeSet<String>,
    multimap_table_names: BTreeSet<String>,
}

#[cfg(any(
    test,
    feature = "logical-store-snapshot-test",
    feature = "namespace-test-crash",
    feature = "retention-test-crash"
))]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CompleteLogicalStoreObservation<'a> {
    store_header: &'a [u8],
    table_names: &'a BTreeSet<String>,
    multimap_table_names: &'a BTreeSet<String>,
    records: &'a LogicalStoreSnapshot,
}

impl CurrentStore {
    pub(super) fn has_same_logical_observation(&self, other: &Self) -> bool {
        self.snapshot == other.snapshot && self.observation_metadata == other.observation_metadata
    }
}

pub(super) fn open_legacy_canonical(guard: &NamespaceGuard) -> Result<LegacyStore, StoreError> {
    open_legacy_writable_at(guard, "lifecycle.store", "prior lifecycle.store")
}

pub(super) fn open_legacy_at(
    guard: &NamespaceGuard,
    name: &str,
    label: &str,
) -> Result<LegacyStore, StoreError> {
    let path = guard.state.state_dir.join(name);
    let entry = HeldEntry::open(
        &path,
        EntryKind::RegularFile,
        EntryAccess::ReadWrite,
        true,
        label,
    )?;
    require_state_volume(&entry, &guard.state_directory, label)?;
    open_legacy_entry(guard, entry, name, label)
}

pub(super) fn open_legacy_entry(
    guard: &NamespaceGuard,
    entry: HeldEntry,
    name: &str,
    label: &str,
) -> Result<LegacyStore, StoreError> {
    let path = guard.state.state_dir.join(name);
    let database = detached_database(guard, &entry)?;
    let generation = verify_prior_store_header(&database, &guard.state.binding)?;
    let read = database.begin_read().map_err(backend_error)?;
    let snapshot = read_legacy_snapshot(&read)?;
    drop(read);
    entry.validate_path(
        &path,
        EntryKind::RegularFile,
        EntryAccess::ReadWrite,
        true,
        label,
    )?;
    Ok(LegacyStore {
        entry,
        database,
        generation,
        snapshot,
    })
}

fn open_legacy_writable_at(
    guard: &NamespaceGuard,
    name: &str,
    label: &str,
) -> Result<LegacyStore, StoreError> {
    let path = guard.state.state_dir.join(name);
    let entry = HeldEntry::open(
        &path,
        EntryKind::RegularFile,
        EntryAccess::ReadWrite,
        true,
        label,
    )?;
    require_state_volume(&entry, &guard.state_directory, label)?;
    let database = Database::builder()
        .create_file(entry.file().try_clone().map_err(io_error)?)
        .map_err(backend_error)?;
    let generation = verify_prior_store_header(&database, &guard.state.binding)?;
    let read = database.begin_read().map_err(backend_error)?;
    let snapshot = read_legacy_snapshot(&read)?;
    drop(read);
    entry.validate_path(
        &path,
        EntryKind::RegularFile,
        EntryAccess::ReadWrite,
        true,
        label,
    )?;
    Ok(LegacyStore {
        entry,
        database: MigrationDatabase::Direct(database),
        generation,
        snapshot,
    })
}

pub(super) fn open_current_canonical(guard: &NamespaceGuard) -> Result<CurrentStore, StoreError> {
    let path = guard.state.state_dir.join("lifecycle.store");
    let entry = HeldEntry::open(
        &path,
        EntryKind::RegularFile,
        EntryAccess::ReadWrite,
        true,
        "lifecycle.store",
    )?;
    require_state_volume(&entry, &guard.state_directory, "lifecycle.store")?;
    let database = detached_database(guard, &entry)?;
    let (generation, anchor) = verify_store_header_anchor(&database, &guard.state.binding)?;
    let validated = validate_current_database(guard, &database, generation, anchor.as_ref(), None)?;
    drop(database);
    entry.validate_path(
        &path,
        EntryKind::RegularFile,
        EntryAccess::ReadWrite,
        true,
        "lifecycle.store",
    )?;
    Ok(CurrentStore {
        entry,
        generation,
        anchor,
        snapshot: validated.snapshot,
        observation_metadata: validated.observation_metadata,
    })
}

pub(super) fn create_unpublished_target(
    guard: &NamespaceGuard,
    unpublished: &UnpublishedFile,
    generation: StoreGeneration,
    snapshot: &LogicalStoreSnapshot,
    anchor: &MigrationProvenanceAnchor,
) -> Result<(), StoreError> {
    initialize_store_with_anchor(
        unpublished.entry(),
        &guard.state.binding,
        generation,
        Some(anchor),
    )?;
    let database = Database::builder()
        .create_file(unpublished.entry().file().try_clone().map_err(io_error)?)
        .map_err(backend_error)?;
    write_snapshot(&database, snapshot)?;
    validate_current_database(guard, &database, generation, Some(anchor), Some(snapshot))?;
    drop(database);
    unpublished.entry().sync()
}

pub(super) fn read_current_entry(
    guard: &NamespaceGuard,
    entry: &HeldEntry,
    generation: StoreGeneration,
    expected_anchor: Option<&MigrationProvenanceAnchor>,
) -> Result<LogicalStoreSnapshot, StoreError> {
    let database = detached_database(guard, entry)?;
    let validated = validate_current_database(guard, &database, generation, expected_anchor, None)?;
    drop(database);
    Ok(validated.snapshot)
}

fn validate_current_database(
    guard: &NamespaceGuard,
    database: &Database,
    generation: StoreGeneration,
    expected_anchor: Option<&MigrationProvenanceAnchor>,
    expected_snapshot: Option<&LogicalStoreSnapshot>,
) -> Result<ValidatedCurrentDatabase, StoreError> {
    let (observed_generation, observed_anchor) =
        verify_store_header_anchor(database, &guard.state.binding)?;
    if observed_generation != generation {
        return Err(StoreError::StoreGenerationChanged {
            expected: generation,
            observed: observed_generation,
        });
    }
    if observed_anchor.as_ref() != expected_anchor {
        return Err(StoreError::Integrity(
            "migration target provenance anchor changed".to_owned(),
        ));
    }
    let read = database.begin_read().map_err(backend_error)?;
    let store_header = read_store_header_bytes_from_read(&read)?;
    let (table_names, multimap_table_names) = read_table_inventory(&read)?;
    verify_validation_receipt_set_read(&read, &guard.state.binding, generation)?;
    let snapshot = read_snapshot(&read)?;
    if expected_snapshot.is_some_and(|expected| expected != &snapshot) {
        return Err(StoreError::Integrity(
            "migration target logical snapshot changed".to_owned(),
        ));
    }
    Ok(ValidatedCurrentDatabase {
        snapshot,
        observation_metadata: CurrentStoreObservationMetadata {
            store_header,
            table_names,
            multimap_table_names,
        },
    })
}

#[cfg(any(
    test,
    feature = "logical-store-snapshot-test",
    feature = "namespace-test-crash",
    feature = "retention-test-crash"
))]
impl CurrentStore {
    pub(super) fn complete_logical_observation(&self) -> Result<Vec<u8>, StoreError> {
        serde_json::to_vec(&CompleteLogicalStoreObservation {
            store_header: &self.observation_metadata.store_header,
            table_names: &self.observation_metadata.table_names,
            multimap_table_names: &self.observation_metadata.multimap_table_names,
            records: &self.snapshot,
        })
        .map_err(crate::serialization_error)
    }
}

impl LogicalStoreSnapshot {
    pub(super) fn validate_external_references(
        &self,
        guard: &NamespaceGuard,
    ) -> Result<(), StoreError> {
        validation::validate_external_references(self, guard)
    }

    pub(super) fn validate_external_references_for_ordinary_admission(
        &self,
        guard: &NamespaceGuard,
    ) -> Result<(), StoreError> {
        validation::validate_external_references_for_ordinary_admission(self, guard)
    }

    pub(super) fn validate_legacy_external_references(
        &self,
        guard: &NamespaceGuard,
    ) -> Result<(), StoreError> {
        validation::validate_legacy_external_references(self, guard)
    }

    pub(super) fn transformed_from_v12(
        mut self,
        guard: &NamespaceGuard,
    ) -> Result<Self, StoreError> {
        crate::publication::reconcile_migration_attempt_allocations(
            &mut self.attempt_leases,
            guard,
        )?;
        crate::publication::validate_migration_attempt_leases(&self.attempt_leases)?;
        let run_catalog = &self.run_catalog;
        let attempt_leases = &self.attempt_leases;
        let mut read_run = |run_id: &lumin_model::RunId| {
            let bytes = run_catalog.get(run_id.as_str()).ok_or_else(|| {
                StoreError::Integrity(format!(
                    "latest-completed pointer references missing run {}",
                    run_id.as_str()
                ))
            })?;
            crate::decode_closed_json::<RunCatalogRecord>(bytes).map_err(|error| {
                StoreError::Integrity(format!(
                    "run catalog record {} is malformed: {error}",
                    run_id.as_str()
                ))
            })
        };
        let mut has_active_lease = |attempt_id: &lumin_model::AttemptId| {
            crate::publication::migration_has_active_lease(attempt_leases, attempt_id)
        };
        crate::publication::reconcile_migration_pointer_index(
            &guard.state.state_dir,
            guard,
            &mut self.pointers,
            &mut read_run,
            &mut has_active_lease,
        )?;
        for (key, bytes) in &mut self.cache_cleanup_operations {
            let legacy = crate::decode_closed_json::<LegacyCacheCleanupOperationRecord>(bytes)
                .map_err(|error| {
                    StoreError::IncompatibleStateSchema(format!(
                        "private v1 cache cleanup operation {key} is malformed: {error}"
                    ))
                })?;
            let current = transform_legacy_cleanup_operation(key, legacy)?;
            *bytes = serde_json::to_vec(&current).map_err(crate::serialization_error)?;
        }
        validate_legacy_referential_closure(&self)?;
        Ok(self)
    }

    pub(super) fn logical_sha256(&self) -> Result<String, StoreError> {
        let bytes = serde_json::to_vec(self).map_err(crate::serialization_error)?;
        let mut framed = Vec::new();
        lumin_model::append_length_prefixed(&mut framed, b"lumin-lifecycle-store-user-logical.v1");
        lumin_model::append_length_prefixed(&mut framed, &bytes);
        Ok(crate::digest_hex(&framed))
    }

    pub(super) fn anchored_logical_sha256(
        &self,
        anchor: &MigrationProvenanceAnchor,
    ) -> Result<String, StoreError> {
        let anchor = serde_json::to_vec(anchor).map_err(crate::serialization_error)?;
        let logical = self.logical_sha256()?;
        let mut framed = Vec::new();
        lumin_model::append_length_prefixed(
            &mut framed,
            b"lumin-lifecycle-store-migrated-target-logical.v1",
        );
        lumin_model::append_length_prefixed(&mut framed, &anchor);
        lumin_model::append_length_prefixed(&mut framed, logical.as_bytes());
        Ok(crate::digest_hex(&framed))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum LegacyCacheCleanupDeliveryStatus {
    NotAttempted,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyCacheCleanupOperationRecord {
    schema_version: String,
    repository_id: RepositoryId,
    operation_id: OperationId,
    request_digest: String,
    status: CacheCleanupOperationStatus,
    interruption_count: u64,
    invocation_id: String,
    initial_authorization_set_id: CacheEvictionAuthorizationSetId,
    initial_authorization_count: u64,
    plan_initialized: bool,
    authorization_keys: Vec<String>,
    validated_count: u64,
    execution_lease: Option<CacheCleanupExecutionLease>,
    recovery_reservation: Option<CacheCleanupRecoveryReservation>,
    result: Option<CacheCleanupResult>,
    last_delivery_status: LegacyCacheCleanupDeliveryStatus,
}

fn validate_legacy_cleanup_operation(
    key: &str,
    operation: &LegacyCacheCleanupOperationRecord,
) -> Result<(), StoreError> {
    let unique = operation
        .authorization_keys
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        == operation.authorization_keys.len();
    let plan_valid = operation.plan_initialized
        || (operation.authorization_keys.is_empty() && operation.validated_count == 0);
    let state_valid = match operation.status {
        CacheCleanupOperationStatus::Pending => {
            operation.execution_lease.is_some()
                && operation.recovery_reservation.is_none()
                && operation.result.is_none()
        }
        CacheCleanupOperationStatus::Interrupted => {
            operation.execution_lease.is_none()
                && operation.recovery_reservation.is_some()
                && operation.result.is_none()
        }
        CacheCleanupOperationStatus::Committed => {
            operation.execution_lease.is_none()
                && operation.recovery_reservation.is_none()
                && operation.result.as_ref().is_some_and(|result| {
                    result.operation_id == operation.operation_id
                        && result.request_digest == operation.request_digest
                })
                && operation.plan_initialized
                && operation.validated_count == operation.authorization_keys.len() as u64
        }
    };
    let interruption_count_valid = match operation.status {
        CacheCleanupOperationStatus::Pending => operation.interruption_count < u64::MAX,
        CacheCleanupOperationStatus::Interrupted => {
            (1..u64::MAX).contains(&operation.interruption_count)
        }
        CacheCleanupOperationStatus::Committed => true,
    };
    if operation.schema_version != "lumin-cache-cleanup-operation.v1"
        || operation.operation_id.as_str() != key
        || operation.request_digest.is_empty()
        || operation.invocation_id.len() != 32
        || !operation
            .invocation_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || !unique
        || operation.validated_count > operation.authorization_keys.len() as u64
        || !plan_valid
        || !state_valid
        || !interruption_count_valid
    {
        return Err(StoreError::IncompatibleStateSchema(format!(
            "private v1 cache cleanup operation {key} is incoherent"
        )));
    }
    Ok(())
}

fn transform_legacy_cleanup_operation(
    key: &str,
    legacy: LegacyCacheCleanupOperationRecord,
) -> Result<CacheCleanupOperationRecord, StoreError> {
    validate_legacy_cleanup_operation(key, &legacy)?;
    let (
        greatest_allocated_delivery_sequence,
        greatest_completed_delivery_sequence,
        delivery_completions,
    ) = match (legacy.status, legacy.last_delivery_status) {
        (
            CacheCleanupOperationStatus::Pending | CacheCleanupOperationStatus::Interrupted,
            LegacyCacheCleanupDeliveryStatus::NotAttempted,
        ) => (0, None, Vec::new()),
        (
            CacheCleanupOperationStatus::Committed,
            LegacyCacheCleanupDeliveryStatus::NotAttempted,
        ) => (1, None, Vec::new()),
        (CacheCleanupOperationStatus::Committed, LegacyCacheCleanupDeliveryStatus::Succeeded) => (
            2,
            Some(1),
            vec![CacheCleanupDeliveryCompletion {
                sequence: 1,
                outcome: CacheCleanupDeliveryOutcome::Succeeded,
            }],
        ),
        (CacheCleanupOperationStatus::Committed, LegacyCacheCleanupDeliveryStatus::Failed) => (
            2,
            Some(1),
            vec![CacheCleanupDeliveryCompletion {
                sequence: 1,
                outcome: CacheCleanupDeliveryOutcome::Failed,
            }],
        ),
        _ => {
            return Err(StoreError::IncompatibleStateSchema(format!(
                "private v1 cache cleanup operation {key} has an impossible delivery state"
            )));
        }
    };
    let current = CacheCleanupOperationRecord {
        schema_version: "lumin-cache-cleanup-operation.v2".to_owned(),
        repository_id: legacy.repository_id,
        operation_id: legacy.operation_id,
        request_digest: legacy.request_digest,
        status: legacy.status,
        interruption_count: legacy.interruption_count,
        invocation_id: legacy.invocation_id,
        initial_authorization_set_id: legacy.initial_authorization_set_id,
        initial_authorization_count: legacy.initial_authorization_count,
        plan_initialized: legacy.plan_initialized,
        authorization_keys: legacy.authorization_keys,
        validated_count: legacy.validated_count,
        execution_lease: legacy.execution_lease,
        recovery_reservation: legacy.recovery_reservation,
        result: legacy.result,
        greatest_allocated_delivery_sequence,
        greatest_completed_delivery_sequence,
        delivery_completions,
    };
    crate::cache::validate_operation_shape(&current).map_err(|error| {
        StoreError::IncompatibleStateSchema(format!(
            "private v1 cache cleanup operation {key} cannot map to v2: {error}"
        ))
    })?;
    Ok(current)
}

#[cfg(test)]
mod tests {
    use lumin_evidence::{
        CacheCleanupDeliveryStatus, CacheCleanupExecutionLease, CacheCleanupRecoveryReservation,
        OperationLivenessLease,
    };
    use lumin_model::{CacheEvictionAuthorizationSetId, OperationId, RepositoryId};

    use super::*;

    #[derive(Debug, Deserialize, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct ClosedRecordFixture {
        nested: ClosedNestedFixture,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        optional: Option<String>,
    }

    #[derive(Debug, Deserialize, Serialize)]
    struct ClosedNestedFixture {
        value: u64,
    }

    #[test]
    fn closed_json_rejects_nested_unknown_members_but_allows_absent_defaults() {
        let accepted =
            crate::decode_closed_json::<ClosedRecordFixture>(br#"{"nested":{"value":1}}"#);
        assert!(accepted.is_ok());

        let rejected = crate::decode_closed_json::<ClosedRecordFixture>(
            br#"{"nested":{"value":1,"opaqueControl":true}}"#,
        );
        assert!(matches!(
            rejected,
            Err(message) if message.contains("nested.opaqueControl")
        ));
    }

    #[test]
    fn legacy_cleanup_delivery_states_map_fail_closed() -> Result<(), StoreError> {
        let pending = transform_legacy_cleanup_operation(
            "cache-pending",
            legacy_operation(
                "cache-pending",
                CacheCleanupOperationStatus::Pending,
                LegacyCacheCleanupDeliveryStatus::NotAttempted,
            ),
        )?;
        assert_delivery(
            &pending,
            0,
            None,
            &[],
            CacheCleanupDeliveryStatus::NotAttempted,
        );

        let interrupted = transform_legacy_cleanup_operation(
            "cache-interrupted",
            legacy_operation(
                "cache-interrupted",
                CacheCleanupOperationStatus::Interrupted,
                LegacyCacheCleanupDeliveryStatus::NotAttempted,
            ),
        )?;
        assert_delivery(
            &interrupted,
            0,
            None,
            &[],
            CacheCleanupDeliveryStatus::NotAttempted,
        );

        let not_attempted = transform_legacy_cleanup_operation(
            "cache-committed-unfinished",
            legacy_operation(
                "cache-committed-unfinished",
                CacheCleanupOperationStatus::Committed,
                LegacyCacheCleanupDeliveryStatus::NotAttempted,
            ),
        )?;
        assert_delivery(
            &not_attempted,
            1,
            None,
            &[],
            CacheCleanupDeliveryStatus::Unknown,
        );

        for (legacy_status, outcome) in [
            (
                LegacyCacheCleanupDeliveryStatus::Succeeded,
                CacheCleanupDeliveryOutcome::Succeeded,
            ),
            (
                LegacyCacheCleanupDeliveryStatus::Failed,
                CacheCleanupDeliveryOutcome::Failed,
            ),
        ] {
            let operation = transform_legacy_cleanup_operation(
                "cache-committed-complete",
                legacy_operation(
                    "cache-committed-complete",
                    CacheCleanupOperationStatus::Committed,
                    legacy_status,
                ),
            )?;
            assert_delivery(
                &operation,
                2,
                Some(1),
                &[CacheCleanupDeliveryCompletion {
                    sequence: 1,
                    outcome,
                }],
                CacheCleanupDeliveryStatus::Unknown,
            );
        }

        let impossible = transform_legacy_cleanup_operation(
            "cache-pending",
            legacy_operation(
                "cache-pending",
                CacheCleanupOperationStatus::Pending,
                LegacyCacheCleanupDeliveryStatus::Succeeded,
            ),
        );
        assert!(matches!(
            impossible,
            Err(StoreError::IncompatibleStateSchema(message))
                if message.contains("impossible delivery state")
        ));

        let mut impossible_interruption = legacy_operation(
            "cache-interrupted-zero-count",
            CacheCleanupOperationStatus::Interrupted,
            LegacyCacheCleanupDeliveryStatus::NotAttempted,
        );
        impossible_interruption.interruption_count = 0;
        assert!(matches!(
            transform_legacy_cleanup_operation(
                "cache-interrupted-zero-count",
                impossible_interruption,
            ),
            Err(StoreError::IncompatibleStateSchema(message))
                if message.contains("is incoherent")
        ));
        Ok(())
    }

    fn legacy_operation(
        operation_id: &str,
        status: CacheCleanupOperationStatus,
        last_delivery_status: LegacyCacheCleanupDeliveryStatus,
    ) -> LegacyCacheCleanupOperationRecord {
        let operation_id = OperationId::from_string(operation_id.to_owned());
        let request_digest = "request-digest".to_owned();
        let (execution_lease, recovery_reservation, result, plan_initialized) = match status {
            CacheCleanupOperationStatus::Pending => (
                Some(CacheCleanupExecutionLease {
                    execution_attempt_id: "execution-attempt".to_owned(),
                    liveness: OperationLivenessLease {
                        lease_nonce: "lease-nonce".to_owned(),
                        owner_process_id: 1,
                        lock_physical_identity: None,
                    },
                }),
                None,
                None,
                false,
            ),
            CacheCleanupOperationStatus::Interrupted => (
                None,
                Some(CacheCleanupRecoveryReservation {
                    interrupted_execution_attempt_id: "execution-attempt".to_owned(),
                }),
                None,
                false,
            ),
            CacheCleanupOperationStatus::Committed => (
                None,
                None,
                Some(CacheCleanupResult {
                    operation_id: operation_id.clone(),
                    request_digest: request_digest.clone(),
                }),
                true,
            ),
        };
        LegacyCacheCleanupOperationRecord {
            schema_version: "lumin-cache-cleanup-operation.v1".to_owned(),
            repository_id: RepositoryId::from_string("repository-test".to_owned()),
            operation_id,
            request_digest,
            status,
            interruption_count: if status == CacheCleanupOperationStatus::Interrupted {
                1
            } else {
                0
            },
            invocation_id: "0".repeat(32),
            initial_authorization_set_id: CacheEvictionAuthorizationSetId::from_string(
                "cache-evictions-test".to_owned(),
            ),
            initial_authorization_count: 0,
            plan_initialized,
            authorization_keys: Vec::new(),
            validated_count: 0,
            execution_lease,
            recovery_reservation,
            result,
            last_delivery_status,
        }
    }

    fn assert_delivery(
        operation: &CacheCleanupOperationRecord,
        greatest_allocated: u64,
        greatest_completed: Option<u64>,
        completions: &[CacheCleanupDeliveryCompletion],
        status: CacheCleanupDeliveryStatus,
    ) {
        assert_eq!(
            operation.greatest_allocated_delivery_sequence,
            greatest_allocated
        );
        assert_eq!(
            operation.greatest_completed_delivery_sequence,
            greatest_completed
        );
        assert_eq!(operation.delivery_completions, completions);
        assert_eq!(operation.last_delivery_status(), status);
    }
}
