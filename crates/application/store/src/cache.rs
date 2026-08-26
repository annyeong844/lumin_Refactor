mod manifest;

#[cfg(feature = "cache-cleanup-test-fault")]
mod barrier;
#[cfg(feature = "cache-cleanup-test-fault")]
mod crash;

#[cfg(all(feature = "cache-cleanup-test-fault", not(debug_assertions)))]
compile_error!("cache-cleanup-test-fault is restricted to debug test builds");

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
#[cfg(feature = "cache-cleanup-test-fault")]
use std::path::{Component, Path};

use lumin_evidence::{
    CacheCleanupDeliveryCompletion, CacheCleanupDeliveryOutcome, CacheCleanupExecutionLease,
    CacheCleanupOperationRecord, CacheCleanupOperationStatus, CacheCleanupRecoveryReservation,
    CacheCleanupResult, CacheEvictionAuthorization, CacheEvictionAuthorizationState,
};
use lumin_model::{
    CacheEvictionAuthorizationSetId, OperationId, RepositoryId, append_length_prefixed,
    portable_path_component,
};
use redb::{TableDefinition, WriteTransaction};

use crate::gate::records::{load_record, read_record, read_records, write_record};
use crate::namespace::NamespaceGuard;
#[cfg(feature = "cache-cleanup-test-fault")]
use crate::namespace::records::ManagedStateParentKind;
#[cfg(feature = "cache-cleanup-test-fault")]
use crate::namespace::{EntryKind, HeldEntry, same_volume_and_mount};
use crate::{OperationSession, RepositoryStore, StoreError, nonce_hex};

use self::manifest::{
    PreparedCachePayload, component_projection, flush_cleanup_parents, manifest_digest,
    prepare_active_payloads, quarantine_child_names, reconcile_authorized_move,
    require_active_cache_anchor_only, validate_authorized_location, validate_quarantine_child,
};

pub(crate) const CACHE_CLEANUP_OPERATIONS: TableDefinition<&str, &[u8]> =
    TableDefinition::new("cache-cleanup-operations");
pub(crate) const CACHE_EVICTION_AUTHORIZATIONS: TableDefinition<&str, &[u8]> =
    TableDefinition::new("cache-eviction-authorizations");

const CACHE_CLEANUP_OPERATION_SCHEMA: &str = "lumin-cache-cleanup-operation.v2";
const CACHE_EVICTION_AUTHORIZATION_SCHEMA: &str = "lumin-cache-eviction-authorization.v1";

impl RepositoryStore {
    #[cfg(any(test, feature = "lifecycle-migration-test-fault"))]
    pub fn rewrite_cache_cleanup_operation_as_prior_for_test(
        &self,
        operation_id: &OperationId,
        last_delivery_status: crate::PriorCacheCleanupDeliveryStatusForTest,
    ) -> Result<(), StoreError> {
        self.with_exclusive_lock(|guard| {
            let database = guard.open_database()?;
            let write = database.begin_write()?;
            let operation = read_record::<CacheCleanupOperationRecord>(
                &write,
                CACHE_CLEANUP_OPERATIONS,
                operation_id.as_str(),
            )?
            .ok_or_else(|| StoreError::OperationNotFound(operation_id.as_str().to_owned()))?;
            validate_operation_shape(&operation)?;
            let legacy = serde_json::json!({
                "schemaVersion": "lumin-cache-cleanup-operation.v1",
                "repositoryId": operation.repository_id,
                "operationId": operation.operation_id,
                "requestDigest": operation.request_digest,
                "status": operation.status,
                "interruptionCount": operation.interruption_count,
                "invocationId": operation.invocation_id,
                "initialAuthorizationSetId": operation.initial_authorization_set_id,
                "initialAuthorizationCount": operation.initial_authorization_count,
                "planInitialized": operation.plan_initialized,
                "authorizationKeys": operation.authorization_keys,
                "validatedCount": operation.validated_count,
                "executionLease": operation.execution_lease,
                "recoveryReservation": operation.recovery_reservation,
                "result": operation.result,
                "lastDeliveryStatus": last_delivery_status,
            });
            write_record(
                &write,
                CACHE_CLEANUP_OPERATIONS,
                operation_id.as_str(),
                &legacy,
            )?;
            guard.commit(write)
        })
    }

    #[cfg(feature = "cache-cleanup-test-fault")]
    pub fn write_active_cache_payload_for_test(
        &self,
        name: &str,
        payload: &[u8],
    ) -> Result<(), StoreError> {
        if name == "namespace.anchor"
            || !matches!(
                Path::new(name).components().collect::<Vec<_>>().as_slice(),
                [Component::Normal(_)]
            )
        {
            return Err(StoreError::Integrity(
                "test cache writer requires one ordinary child name".to_owned(),
            ));
        }
        self.with_exclusive_lock(|guard| {
            let database = guard.open_database()?;
            let write = database.begin_write()?;
            reject_active_cache_mutation_reservation(&write, None)?;
            let parent = guard.managed_parent_entry(ManagedStateParentKind::Cache)?;
            let path = guard
                .managed_parent_path(ManagedStateParentKind::Cache)
                .join(name);
            let entry = HeldEntry::create_new(&path, "test active-cache payload")?;
            if !same_volume_and_mount(&entry, parent) {
                return Err(StoreError::Integrity(
                    "test active-cache payload crossed its bound volume or mount".to_owned(),
                ));
            }
            entry.replace_contents(payload)?;
            entry.validate_path(
                &path,
                EntryKind::RegularFile,
                crate::namespace::EntryAccess::ReadWrite,
                true,
                "test active-cache payload",
            )?;
            parent.sync_directory()?;
            guard.commit(write)
        })
    }

    pub fn clean_cache_payloads(
        &self,
        operation_id: &OperationId,
        request_digest: &str,
    ) -> Result<CacheCleanupResult, StoreError> {
        if let Some(operation) = self.load_cache_cleanup_operation_optional(operation_id)? {
            validate_operation_identity(
                &operation,
                operation_id,
                request_digest,
                &self.repository_id()?,
            )?;
            if let Some(result) = operation.result {
                return Ok(result);
            }
        }
        self.reject_cleanup_operation_collision(operation_id)?;
        self.reject_foreign_cleanup_operation(operation_id)?;

        let session = self.begin_operation(operation_id)?;
        if let Some(result) = self.interrupt_stale_cleanup(&session, request_digest)? {
            return Ok(result);
        }
        self.attach_or_create_cleanup(&session, request_digest)?;
        self.authorize_cleanup_plan(&session, request_digest)?;
        self.advance_cleanup_plan(&session, request_digest)
    }

    pub fn load_cache_cleanup_operation(
        &self,
        operation_id: &OperationId,
    ) -> Result<CacheCleanupOperationRecord, StoreError> {
        self.load_cache_cleanup_operation_optional(operation_id)?
            .ok_or_else(|| StoreError::OperationNotFound(operation_id.as_str().to_owned()))
    }

    pub fn allocate_cache_cleanup_delivery(
        &self,
        operation_id: &OperationId,
        request_digest: &str,
    ) -> Result<u64, StoreError> {
        self.with_exclusive_lock(|guard| {
            let database = guard.open_database()?;
            let write = database.begin_write()?;
            let mut operation = read_record::<CacheCleanupOperationRecord>(
                &write,
                CACHE_CLEANUP_OPERATIONS,
                operation_id.as_str(),
            )?
            .ok_or_else(|| StoreError::OperationNotFound(operation_id.as_str().to_owned()))?;
            validate_operation_identity(
                &operation,
                operation_id,
                request_digest,
                guard.repository_id(),
            )?;
            validate_operation_shape(&operation)?;
            if operation.status != CacheCleanupOperationStatus::Committed
                || operation.result.is_none()
            {
                return Err(StoreError::Integrity(format!(
                    "cache cleanup delivery cannot precede commit: {}",
                    operation_id.as_str()
                )));
            }
            let sequence = operation
                .greatest_allocated_delivery_sequence
                .checked_add(1)
                .ok_or_else(|| {
                    StoreError::Integrity(format!(
                        "cache cleanup delivery sequence overflow: {}",
                        operation_id.as_str()
                    ))
                })?;
            if sequence == u64::MAX {
                return Err(StoreError::Integrity(format!(
                    "cache cleanup delivery sequence exhausted: {}",
                    operation_id.as_str()
                )));
            }
            operation.greatest_allocated_delivery_sequence = sequence;
            write_record(
                &write,
                CACHE_CLEANUP_OPERATIONS,
                operation_id.as_str(),
                &operation,
            )?;
            guard.commit(write)?;
            Ok(sequence)
        })
    }

    pub fn record_cache_cleanup_delivery(
        &self,
        operation_id: &OperationId,
        request_digest: &str,
        sequence: u64,
        outcome: CacheCleanupDeliveryOutcome,
    ) -> Result<(), StoreError> {
        self.with_exclusive_lock(|guard| {
            let database = guard.open_database()?;
            let write = database.begin_write()?;
            let mut operation = read_record::<CacheCleanupOperationRecord>(
                &write,
                CACHE_CLEANUP_OPERATIONS,
                operation_id.as_str(),
            )?
            .ok_or_else(|| StoreError::OperationNotFound(operation_id.as_str().to_owned()))?;
            validate_operation_identity(
                &operation,
                operation_id,
                request_digest,
                guard.repository_id(),
            )?;
            validate_operation_shape(&operation)?;
            if operation.status != CacheCleanupOperationStatus::Committed
                || operation.result.is_none()
                || sequence == 0
                || sequence > operation.greatest_allocated_delivery_sequence
            {
                return Err(StoreError::Integrity(format!(
                    "cache cleanup delivery completion has no allocated attempt: {} sequence {sequence}",
                    operation_id.as_str()
                )));
            }
            match operation
                .delivery_completions
                .binary_search_by_key(&sequence, |completion| completion.sequence)
            {
                Ok(index) if operation.delivery_completions[index].outcome == outcome => {
                    return Ok(());
                }
                Ok(_) => {
                    return Err(StoreError::Integrity(format!(
                        "cache cleanup delivery sequence changed outcome: {} sequence {sequence}",
                        operation_id.as_str()
                    )));
                }
                Err(index) => operation.delivery_completions.insert(
                    index,
                    CacheCleanupDeliveryCompletion { sequence, outcome },
                ),
            }
            operation.greatest_completed_delivery_sequence = operation
                .delivery_completions
                .last()
                .map(|completion| completion.sequence);
            validate_operation_shape(&operation)?;
            write_record(
                &write,
                CACHE_CLEANUP_OPERATIONS,
                operation_id.as_str(),
                &operation,
            )?;
            guard.commit(write)
        })
    }

    fn repository_id(&self) -> Result<RepositoryId, StoreError> {
        self.with_shared_lock(|guard| Ok(guard.repository_id().clone()))
    }

    fn load_cache_cleanup_operation_optional(
        &self,
        operation_id: &OperationId,
    ) -> Result<Option<CacheCleanupOperationRecord>, StoreError> {
        self.with_shared_lock(|guard| {
            let database = guard.open_database()?;
            let operation =
                load_record(&database, CACHE_CLEANUP_OPERATIONS, operation_id.as_str())?;
            if let Some(operation) = operation.as_ref() {
                validate_operation_shape(operation)?;
            }
            Ok(operation)
        })
    }

    fn reject_cleanup_operation_collision(
        &self,
        operation_id: &OperationId,
    ) -> Result<(), StoreError> {
        self.with_shared_lock(|guard| {
            let database = guard.open_database()?;
            if load_record::<lumin_evidence::OperationRecord>(
                &database,
                crate::gate::OPERATIONS,
                operation_id.as_str(),
            )?
            .is_some()
                || load_record::<lumin_evidence::RetentionOperationRecord>(
                    &database,
                    crate::retention::RETENTION_OPERATIONS,
                    operation_id.as_str(),
                )?
                .is_some()
            {
                return Err(StoreError::OperationConflict(
                    operation_id.as_str().to_owned(),
                ));
            }
            Ok(())
        })
    }

    fn reject_foreign_cleanup_operation(
        &self,
        operation_id: &OperationId,
    ) -> Result<(), StoreError> {
        self.with_exclusive_lock(|guard| {
            let database = guard.open_database()?;
            let write = database.begin_write()?;
            for operation in
                read_records::<CacheCleanupOperationRecord>(&write, CACHE_CLEANUP_OPERATIONS)?
            {
                validate_operation_shape(&operation)?;
                reject_active_cache_mutation_reservation_for_operation(
                    &operation,
                    Some(operation_id),
                )?;
            }
            Ok(())
        })
    }

    fn interrupt_stale_cleanup(
        &self,
        session: &OperationSession<'_>,
        request_digest: &str,
    ) -> Result<Option<CacheCleanupResult>, StoreError> {
        session.validate_live_lock()?;
        let outcome = self.with_exclusive_lock(|guard| {
            let database = session.open_database(guard)?;
            let write = database.begin_write()?;
            reject_non_cleanup_collision(&write, session.operation_id())?;
            let Some(mut operation) = read_record::<CacheCleanupOperationRecord>(
                &write,
                CACHE_CLEANUP_OPERATIONS,
                session.operation_id().as_str(),
            )?
            else {
                return Ok(InterruptOutcome::None);
            };
            validate_operation_identity(
                &operation,
                session.operation_id(),
                request_digest,
                guard.repository_id(),
            )?;
            validate_operation_shape(&operation)?;
            if let Some(result) = operation.result.clone() {
                return Ok(InterruptOutcome::Committed(result));
            }
            if operation.status == CacheCleanupOperationStatus::Pending {
                let expected_lock = operation
                    .execution_lease
                    .as_ref()
                    .and_then(|lease| lease.liveness.lock_physical_identity.as_ref())
                    .ok_or_else(|| {
                        StoreError::Integrity(format!(
                            "pending cache cleanup omitted its liveness identity: {}",
                            operation.operation_id.as_str()
                        ))
                    })?;
                if session.liveness().lock_physical_identity.as_ref() != Some(expected_lock) {
                    return Err(StoreError::Integrity(format!(
                        "cache cleanup liveness lock identity changed: {}",
                        operation.operation_id.as_str()
                    )));
                }
                let interrupted_attempt = operation
                    .execution_lease
                    .take()
                    .ok_or_else(|| {
                        StoreError::Integrity(
                            "pending cache cleanup omitted its execution lease".to_owned(),
                        )
                    })?
                    .execution_attempt_id;
                operation.status = CacheCleanupOperationStatus::Interrupted;
                operation.interruption_count =
                    next_cleanup_interruption_count(operation.interruption_count)?;
                operation.recovery_reservation = Some(CacheCleanupRecoveryReservation {
                    interrupted_execution_attempt_id: interrupted_attempt,
                });
                validate_operation_shape(&operation)?;
                write_record(
                    &write,
                    CACHE_CLEANUP_OPERATIONS,
                    operation.operation_id.as_str(),
                    &operation,
                )?;
                guard.commit(write)?;
                return Ok(InterruptOutcome::Interrupted);
            }
            Ok(InterruptOutcome::None)
        })?;
        match outcome {
            InterruptOutcome::None => Ok(None),
            InterruptOutcome::Committed(result) => Ok(Some(result)),
            InterruptOutcome::Interrupted => {
                #[cfg(feature = "cache-cleanup-test-fault")]
                barrier::wait_interrupted(session.operation_id())?;
                Ok(None)
            }
        }
    }

    fn attach_or_create_cleanup(
        &self,
        session: &OperationSession<'_>,
        request_digest: &str,
    ) -> Result<(), StoreError> {
        session.validate_live_lock()?;
        let attached = self.with_exclusive_lock(|guard| {
            let database = session.open_database(guard)?;
            let write = database.begin_write()?;
            reject_non_cleanup_collision(&write, session.operation_id())?;
            reject_active_cache_mutation_reservation(&write, Some(session.operation_id()))?;
            let (operation, attached) = if let Some(mut operation) =
                read_record::<CacheCleanupOperationRecord>(
                    &write,
                    CACHE_CLEANUP_OPERATIONS,
                    session.operation_id().as_str(),
                )? {
                validate_operation_identity(
                    &operation,
                    session.operation_id(),
                    request_digest,
                    guard.repository_id(),
                )?;
                validate_operation_shape(&operation)?;
                if operation.status == CacheCleanupOperationStatus::Committed {
                    return Ok(false);
                }
                if operation.status == CacheCleanupOperationStatus::Pending {
                    if operation
                        .execution_lease
                        .as_ref()
                        .is_some_and(|lease| lease.liveness == *session.liveness())
                    {
                        return Ok(false);
                    }
                    return Err(StoreError::Integrity(format!(
                        "cache cleanup pending lease was not interrupted before reattachment: {}",
                        operation.operation_id.as_str()
                    )));
                }
                operation.status = CacheCleanupOperationStatus::Pending;
                operation.execution_lease = Some(new_execution_lease(session)?);
                operation.recovery_reservation = None;
                (operation, true)
            } else {
                let existing = validate_authenticated_quarantine(guard, &write)?;
                let initial_rows = existing
                    .iter()
                    .map(authorization_set_frame)
                    .collect::<Vec<_>>();
                (
                    CacheCleanupOperationRecord {
                        schema_version: CACHE_CLEANUP_OPERATION_SCHEMA.to_owned(),
                        repository_id: guard.repository_id().clone(),
                        operation_id: session.operation_id().clone(),
                        request_digest: request_digest.to_owned(),
                        status: CacheCleanupOperationStatus::Pending,
                        interruption_count: 0,
                        invocation_id: nonce_hex()?,
                        initial_authorization_set_id:
                            CacheEvictionAuthorizationSetId::for_canonical_rows(&initial_rows),
                        initial_authorization_count: u64::try_from(existing.len()).map_err(
                            |_| {
                                StoreError::Integrity(
                                    "cache quarantine authorization count overflow".to_owned(),
                                )
                            },
                        )?,
                        plan_initialized: false,
                        authorization_keys: Vec::new(),
                        validated_count: 0,
                        execution_lease: Some(new_execution_lease(session)?),
                        recovery_reservation: None,
                        result: None,
                        greatest_allocated_delivery_sequence: 0,
                        greatest_completed_delivery_sequence: None,
                        delivery_completions: Vec::new(),
                    },
                    true,
                )
            };
            validate_operation_shape(&operation)?;
            write_record(
                &write,
                CACHE_CLEANUP_OPERATIONS,
                operation.operation_id.as_str(),
                &operation,
            )?;
            guard.commit(write)?;
            Ok(attached)
        })?;
        #[cfg(feature = "cache-cleanup-test-fault")]
        if attached {
            barrier::wait_pending(session.operation_id())?;
        }
        let _ = attached;
        Ok(())
    }

    fn authorize_cleanup_plan(
        &self,
        session: &OperationSession<'_>,
        request_digest: &str,
    ) -> Result<(), StoreError> {
        session.validate_live_lock()?;
        let initialized = self.with_exclusive_lock(|guard| {
            let database = session.open_database(guard)?;
            let write = database.begin_write()?;
            let mut operation =
                load_pending_cleanup(&write, session, request_digest, guard.repository_id())?;
            if operation.plan_initialized || operation.result.is_some() {
                return Ok(false);
            }
            let existing = validate_authenticated_quarantine(guard, &write)?;
            let initial_rows = existing
                .iter()
                .map(authorization_set_frame)
                .collect::<Vec<_>>();
            if CacheEvictionAuthorizationSetId::for_canonical_rows(&initial_rows)
                != operation.initial_authorization_set_id
                || existing.len() as u64 != operation.initial_authorization_count
            {
                return Err(StoreError::Integrity(
                    "cache quarantine changed before cleanup plan authorization".to_owned(),
                ));
            }

            let payloads = prepare_active_payloads(guard, Some(&operation.operation_id))?;
            let mut keys = Vec::with_capacity(payloads.len());
            for (index, payload) in payloads.into_iter().enumerate() {
                let ordinal = u64::try_from(index).map_err(|_| {
                    StoreError::Integrity("cache cleanup plan ordinal overflow".to_owned())
                })?;
                let authorization =
                    authorization_for_payload(guard.repository_id(), &operation, ordinal, payload)?;
                let key = destination_name(&authorization)?;
                if read_record::<CacheEvictionAuthorization>(
                    &write,
                    CACHE_EVICTION_AUTHORIZATIONS,
                    &key,
                )?
                .is_some()
                {
                    return Err(StoreError::Integrity(format!(
                        "cache eviction authorization destination already exists: {key}"
                    )));
                }
                write_record(&write, CACHE_EVICTION_AUTHORIZATIONS, &key, &authorization)?;
                keys.push(key);
            }
            operation.authorization_keys = keys;
            operation.plan_initialized = true;
            validate_operation_shape(&operation)?;
            write_record(
                &write,
                CACHE_CLEANUP_OPERATIONS,
                operation.operation_id.as_str(),
                &operation,
            )?;
            guard.commit(write)?;
            Ok(true)
        })?;
        #[cfg(feature = "cache-cleanup-test-fault")]
        if initialized {
            crash::hit(crash::CacheCleanupCrashPoint::AfterAuthorization);
        }
        let _ = initialized;
        Ok(())
    }

    fn advance_cleanup_plan(
        &self,
        session: &OperationSession<'_>,
        request_digest: &str,
    ) -> Result<CacheCleanupResult, StoreError> {
        loop {
            session.validate_live_lock()?;
            let outcome = self.with_exclusive_lock(|guard| {
                let database = session.open_database(guard)?;
                let write = database.begin_write()?;
                let mut operation =
                    load_pending_cleanup(&write, session, request_digest, guard.repository_id())?;
                if let Some(result) = operation.result.clone() {
                    return Ok(AdvanceOutcome::Committed(result));
                }
                if !operation.plan_initialized {
                    return Err(StoreError::Integrity(
                        "cache cleanup plan was not authorized before movement".to_owned(),
                    ));
                }
                let index = usize::try_from(operation.validated_count).map_err(|_| {
                    StoreError::Integrity("cache cleanup validated count overflow".to_owned())
                })?;
                if let Some(key) = operation.authorization_keys.get(index).cloned() {
                    let mut authorization = read_record::<CacheEvictionAuthorization>(
                        &write,
                        CACHE_EVICTION_AUTHORIZATIONS,
                        &key,
                    )?
                    .ok_or_else(|| {
                        StoreError::Integrity(format!(
                            "cache cleanup authorization disappeared: {key}"
                        ))
                    })?;
                    validate_authorization_record(&authorization, &key, guard.repository_id())?;
                    if authorization.operation_id != operation.operation_id
                        || authorization.request_digest != operation.request_digest
                        || authorization.state != CacheEvictionAuthorizationState::Authorized
                    {
                        return Err(StoreError::Integrity(format!(
                            "cache cleanup authorization is not the next authorized row: {key}"
                        )));
                    }
                    reconcile_authorized_move(guard, &authorization)?;
                    flush_cleanup_parents(guard)?;
                    guard.validate_bound_entries()?;
                    #[cfg(feature = "cache-cleanup-test-fault")]
                    crash::hit(crash::CacheCleanupCrashPoint::AfterPhysicalDurability(
                        authorization.ordinal,
                    ));
                    authorization.state = CacheEvictionAuthorizationState::Validated;
                    operation.validated_count =
                        operation.validated_count.checked_add(1).ok_or_else(|| {
                            StoreError::Integrity(
                                "cache cleanup validated count overflow".to_owned(),
                            )
                        })?;
                    write_record(&write, CACHE_EVICTION_AUTHORIZATIONS, &key, &authorization)?;
                    write_record(
                        &write,
                        CACHE_CLEANUP_OPERATIONS,
                        operation.operation_id.as_str(),
                        &operation,
                    )?;
                    guard.commit(write)?;
                    #[cfg(feature = "cache-cleanup-test-fault")]
                    crash::hit(crash::CacheCleanupCrashPoint::AfterRowValidation(
                        authorization.ordinal,
                    ));
                    return Ok(AdvanceOutcome::Advanced);
                }

                require_active_cache_anchor_only(guard)?;
                let authenticated = validate_authenticated_quarantine(guard, &write)?;
                let expected_count = operation
                    .initial_authorization_count
                    .checked_add(operation.authorized_count())
                    .ok_or_else(|| {
                        StoreError::Integrity(
                            "cache cleanup final authorization count overflow".to_owned(),
                        )
                    })?;
                if authenticated.len() as u64 != expected_count {
                    return Err(StoreError::Integrity(
                        "cache cleanup final quarantine authorization set is not exact".to_owned(),
                    ));
                }
                let current_keys = operation
                    .authorization_keys
                    .iter()
                    .cloned()
                    .collect::<BTreeSet<_>>();
                let mut initial_rows = Vec::new();
                let mut observed_current = BTreeSet::new();
                for authorization in &authenticated {
                    let key = destination_name(authorization)?;
                    if current_keys.contains(&key) {
                        observed_current.insert(key);
                    } else {
                        initial_rows.push(authorization_set_frame(authorization));
                    }
                }
                if observed_current != current_keys
                    || initial_rows.len() as u64 != operation.initial_authorization_count
                    || CacheEvictionAuthorizationSetId::for_canonical_rows(&initial_rows)
                        != operation.initial_authorization_set_id
                {
                    return Err(StoreError::Integrity(
                        "cache cleanup final authorization provenance changed".to_owned(),
                    ));
                }
                flush_cleanup_parents(guard)?;
                guard.validate_bound_entries()?;
                #[cfg(feature = "cache-cleanup-test-fault")]
                crash::hit(crash::CacheCleanupCrashPoint::BeforeResultCommit);
                let result = CacheCleanupResult {
                    operation_id: operation.operation_id.clone(),
                    request_digest: operation.request_digest.clone(),
                };
                operation.status = CacheCleanupOperationStatus::Committed;
                operation.execution_lease = None;
                operation.recovery_reservation = None;
                operation.result = Some(result.clone());
                write_record(
                    &write,
                    CACHE_CLEANUP_OPERATIONS,
                    operation.operation_id.as_str(),
                    &operation,
                )?;
                guard.commit(write)?;
                Ok(AdvanceOutcome::Committed(result))
            })?;
            match outcome {
                AdvanceOutcome::Advanced => {}
                AdvanceOutcome::Committed(result) => return Ok(result),
            }
        }
    }
}

enum AdvanceOutcome {
    Advanced,
    Committed(CacheCleanupResult),
}

enum InterruptOutcome {
    None,
    Interrupted,
    Committed(CacheCleanupResult),
}

fn new_execution_lease(
    session: &OperationSession<'_>,
) -> Result<CacheCleanupExecutionLease, StoreError> {
    Ok(CacheCleanupExecutionLease {
        execution_attempt_id: nonce_hex()?,
        liveness: session.liveness().clone(),
    })
}

fn load_pending_cleanup(
    write: &WriteTransaction,
    session: &OperationSession<'_>,
    request_digest: &str,
    repository_id: &RepositoryId,
) -> Result<CacheCleanupOperationRecord, StoreError> {
    let operation = read_record::<CacheCleanupOperationRecord>(
        write,
        CACHE_CLEANUP_OPERATIONS,
        session.operation_id().as_str(),
    )?
    .ok_or_else(|| {
        StoreError::Integrity(format!(
            "pending cache cleanup operation disappeared: {}",
            session.operation_id().as_str()
        ))
    })?;
    validate_operation_identity(
        &operation,
        session.operation_id(),
        request_digest,
        repository_id,
    )?;
    validate_operation_shape(&operation)?;
    if operation.status == CacheCleanupOperationStatus::Committed {
        return Ok(operation);
    }
    if operation.status != CacheCleanupOperationStatus::Pending
        || operation
            .execution_lease
            .as_ref()
            .is_none_or(|lease| lease.liveness != *session.liveness())
    {
        return Err(StoreError::Integrity(format!(
            "cache cleanup is not bound to the current execution lease: {}",
            session.operation_id().as_str()
        )));
    }
    Ok(operation)
}

fn reject_non_cleanup_collision(
    write: &WriteTransaction,
    operation_id: &OperationId,
) -> Result<(), StoreError> {
    if read_record::<lumin_evidence::OperationRecord>(
        write,
        crate::gate::OPERATIONS,
        operation_id.as_str(),
    )?
    .is_some()
        || read_record::<lumin_evidence::RetentionOperationRecord>(
            write,
            crate::retention::RETENTION_OPERATIONS,
            operation_id.as_str(),
        )?
        .is_some()
    {
        return Err(StoreError::OperationConflict(
            operation_id.as_str().to_owned(),
        ));
    }
    Ok(())
}

fn reject_active_cache_mutation_reservation(
    write: &WriteTransaction,
    allowed_owner: Option<&OperationId>,
) -> Result<(), StoreError> {
    for operation in read_records::<CacheCleanupOperationRecord>(write, CACHE_CLEANUP_OPERATIONS)? {
        validate_operation_shape(&operation)?;
        reject_active_cache_mutation_reservation_for_operation(&operation, allowed_owner)?;
    }
    Ok(())
}

fn reject_active_cache_mutation_reservation_for_operation(
    operation: &CacheCleanupOperationRecord,
    allowed_owner: Option<&OperationId>,
) -> Result<(), StoreError> {
    if operation.status != CacheCleanupOperationStatus::Committed
        && allowed_owner != Some(&operation.operation_id)
    {
        return Err(StoreError::OperationBusy(
            operation.operation_id.as_str().to_owned(),
        ));
    }
    Ok(())
}

fn authorization_for_payload(
    repository_id: &RepositoryId,
    operation: &CacheCleanupOperationRecord,
    ordinal: u64,
    payload: PreparedCachePayload,
) -> Result<CacheEvictionAuthorization, StoreError> {
    let digest = manifest_digest(&payload.manifest);
    let destination = format!("{}.{ordinal:016x}.{digest}", operation.invocation_id);
    let destination_component = component_projection(std::ffi::OsStr::new(&destination))?;
    Ok(CacheEvictionAuthorization {
        schema_version: CACHE_EVICTION_AUTHORIZATION_SCHEMA.to_owned(),
        repository_id: repository_id.clone(),
        operation_id: operation.operation_id.clone(),
        request_digest: operation.request_digest.clone(),
        invocation_id: operation.invocation_id.clone(),
        ordinal,
        source_component: payload.source_component,
        destination_component,
        manifest_digest: digest,
        expected_manifest: payload.manifest,
        state: CacheEvictionAuthorizationState::Authorized,
    })
}

fn validate_authenticated_quarantine(
    guard: &NamespaceGuard,
    write: &WriteTransaction,
) -> Result<Vec<CacheEvictionAuthorization>, StoreError> {
    let authorizations =
        read_records::<CacheEvictionAuthorization>(write, CACHE_EVICTION_AUTHORIZATIONS)?;
    let mut by_name = BTreeMap::new();
    for authorization in authorizations {
        let name = destination_name(&authorization)?;
        validate_authorization_record(&authorization, &name, guard.repository_id())?;
        if authorization.state != CacheEvictionAuthorizationState::Validated {
            return Err(StoreError::OperationBusy(
                authorization.operation_id.as_str().to_owned(),
            ));
        }
        if by_name.insert(name.clone(), authorization).is_some() {
            return Err(StoreError::Integrity(format!(
                "duplicate cache eviction authorization for {name}"
            )));
        }
    }

    let names = quarantine_child_names(guard)?;
    let observed = names.iter().cloned().collect::<BTreeSet<_>>();
    let authorized = by_name.keys().cloned().collect::<BTreeSet<_>>();
    if observed != authorized {
        let foreign = observed
            .symmetric_difference(&authorized)
            .cloned()
            .collect::<Vec<_>>();
        return Err(StoreError::Integrity(format!(
            "cache quarantine authorization/child bijection changed: {}",
            foreign.join(", ")
        )));
    }
    for name in &names {
        let authorization = by_name.get(name).ok_or_else(|| {
            StoreError::Integrity(format!(
                "cache quarantine child has no authorization: {name}"
            ))
        })?;
        validate_quarantine_child(guard, name, authorization)?;
    }
    names
        .into_iter()
        .map(|name| {
            by_name.remove(&name).ok_or_else(|| {
                StoreError::Integrity(format!(
                    "cache quarantine authorization disappeared during validation: {name}"
                ))
            })
        })
        .collect()
}

pub(crate) fn validate_external_snapshot(
    guard: &NamespaceGuard,
    operation_rows: &BTreeMap<String, Vec<u8>>,
    authorization_rows: &BTreeMap<String, Vec<u8>>,
) -> Result<(), StoreError> {
    for (key, bytes) in operation_rows {
        let operation =
            serde_json::from_slice::<CacheCleanupOperationRecord>(bytes).map_err(|error| {
                StoreError::Integrity(format!(
                    "cache-cleanup-operations record {key} is malformed: {error}"
                ))
            })?;
        if operation.operation_id.as_str() != key
            || operation.repository_id != *guard.repository_id()
        {
            return Err(StoreError::Integrity(format!(
                "cache cleanup operation {key} changed repository ownership"
            )));
        }
        validate_operation_shape(&operation)?;
    }

    let mut expected_quarantine = BTreeSet::new();
    for (key, bytes) in authorization_rows {
        let authorization =
            serde_json::from_slice::<CacheEvictionAuthorization>(bytes).map_err(|error| {
                StoreError::Integrity(format!(
                    "cache-eviction-authorizations record {key} is malformed: {error}"
                ))
            })?;
        validate_authorization_record(&authorization, key, guard.repository_id())?;
        let at_destination = validate_authorized_location(guard, &authorization)?;
        if authorization.state == CacheEvictionAuthorizationState::Validated && !at_destination {
            return Err(StoreError::Integrity(format!(
                "validated cache eviction authorization remains in the active cache: {key}"
            )));
        }
        if at_destination {
            expected_quarantine.insert(key.clone());
        }
    }
    let observed_quarantine = quarantine_child_names(guard)?
        .into_iter()
        .collect::<BTreeSet<_>>();
    if observed_quarantine != expected_quarantine {
        return Err(StoreError::Integrity(
            "cache quarantine changed outside its durable authorization set".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_authorization_record(
    authorization: &CacheEvictionAuthorization,
    key: &str,
    repository_id: &RepositoryId,
) -> Result<(), StoreError> {
    if authorization.schema_version != CACHE_EVICTION_AUTHORIZATION_SCHEMA
        || &authorization.repository_id != repository_id
        || destination_name(authorization)? != key
        || authorization.manifest_digest != manifest_digest(&authorization.expected_manifest)
        || authorization.request_digest.is_empty()
        || authorization.invocation_id.len() != 32
        || !is_lower_hex(&authorization.invocation_id)
    {
        return Err(StoreError::Integrity(format!(
            "cache eviction authorization is invalid: {key}"
        )));
    }
    let expected_name = format!(
        "{}.{:016x}.{}",
        authorization.invocation_id, authorization.ordinal, authorization.manifest_digest
    );
    if key != expected_name {
        return Err(StoreError::Integrity(format!(
            "cache eviction authorization name disagrees with its fields: {key}"
        )));
    }
    Ok(())
}

pub(crate) fn destination_name(
    authorization: &CacheEvictionAuthorization,
) -> Result<String, StoreError> {
    portable_path_component(&authorization.destination_component.canonical)
        .map_err(|error| {
            StoreError::Integrity(format!("cache destination component is invalid: {error}"))
        })?
        .ok_or_else(|| StoreError::Integrity("cache destination must be portable UTF-8".to_owned()))
}

pub(crate) fn authorization_set_frame(authorization: &CacheEvictionAuthorization) -> Vec<u8> {
    let mut framed = Vec::new();
    append_length_prefixed(&mut framed, authorization.repository_id.as_str().as_bytes());
    append_length_prefixed(&mut framed, authorization.operation_id.as_str().as_bytes());
    append_length_prefixed(&mut framed, authorization.request_digest.as_bytes());
    append_length_prefixed(&mut framed, authorization.invocation_id.as_bytes());
    framed.extend_from_slice(&authorization.ordinal.to_be_bytes());
    append_length_prefixed(&mut framed, &authorization.source_component.canonical);
    append_length_prefixed(&mut framed, &authorization.destination_component.canonical);
    append_length_prefixed(&mut framed, authorization.manifest_digest.as_bytes());
    framed.push(match authorization.state {
        CacheEvictionAuthorizationState::Authorized => 1,
        CacheEvictionAuthorizationState::Validated => 2,
    });
    framed
}

fn validate_operation_identity(
    operation: &CacheCleanupOperationRecord,
    operation_id: &OperationId,
    request_digest: &str,
    repository_id: &RepositoryId,
) -> Result<(), StoreError> {
    if operation.operation_id != *operation_id
        || operation.request_digest != request_digest
        || operation.repository_id != *repository_id
    {
        return Err(StoreError::OperationConflict(
            operation_id.as_str().to_owned(),
        ));
    }
    Ok(())
}

fn next_cleanup_interruption_count(current: u64) -> Result<u64, StoreError> {
    let next = current.checked_add(1).ok_or_else(|| {
        StoreError::Integrity("cache cleanup interruption count overflow".to_owned())
    })?;
    if next == u64::MAX {
        return Err(StoreError::Integrity(
            "cache cleanup interruption count exhausted".to_owned(),
        ));
    }
    Ok(next)
}

pub(crate) fn validate_operation_shape(
    operation: &CacheCleanupOperationRecord,
) -> Result<(), StoreError> {
    let unique = operation
        .authorization_keys
        .iter()
        .collect::<BTreeSet<_>>()
        .len()
        == operation.authorization_keys.len();
    let counts_valid = operation.validated_count <= operation.authorized_count();
    let plan_valid = operation.plan_initialized
        || (operation.authorization_keys.is_empty() && operation.validated_count == 0);
    let interruption_count_valid = match operation.status {
        CacheCleanupOperationStatus::Pending => operation.interruption_count < u64::MAX,
        CacheCleanupOperationStatus::Interrupted => {
            (1..u64::MAX).contains(&operation.interruption_count)
        }
        CacheCleanupOperationStatus::Committed => true,
    };
    let delivery_sequences_valid = operation.greatest_allocated_delivery_sequence != u64::MAX
        && operation
            .delivery_completions
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
        && operation.delivery_completions.iter().all(|completion| {
            completion.sequence > 0
                && completion.sequence <= operation.greatest_allocated_delivery_sequence
        })
        && operation.greatest_completed_delivery_sequence
            == operation
                .delivery_completions
                .last()
                .map(|completion| completion.sequence)
        && (operation.greatest_allocated_delivery_sequence > 0
            || operation.delivery_completions.is_empty());
    let state_valid = match operation.status {
        CacheCleanupOperationStatus::Pending => {
            operation.execution_lease.is_some()
                && operation.recovery_reservation.is_none()
                && operation.result.is_none()
                && operation.greatest_allocated_delivery_sequence == 0
                && operation.greatest_completed_delivery_sequence.is_none()
                && operation.delivery_completions.is_empty()
        }
        CacheCleanupOperationStatus::Interrupted => {
            operation.execution_lease.is_none()
                && operation.recovery_reservation.is_some()
                && operation.result.is_none()
                && operation.greatest_allocated_delivery_sequence == 0
                && operation.greatest_completed_delivery_sequence.is_none()
                && operation.delivery_completions.is_empty()
        }
        CacheCleanupOperationStatus::Committed => {
            operation.execution_lease.is_none()
                && operation.recovery_reservation.is_none()
                && operation.result.as_ref().is_some_and(|result| {
                    result.operation_id == operation.operation_id
                        && result.request_digest == operation.request_digest
                })
                && operation.plan_initialized
                && operation.validated_count == operation.authorized_count()
        }
    };
    if operation.schema_version != CACHE_CLEANUP_OPERATION_SCHEMA
        || operation.request_digest.is_empty()
        || operation.invocation_id.len() != 32
        || !is_lower_hex(&operation.invocation_id)
        || !unique
        || !counts_valid
        || !plan_valid
        || !interruption_count_valid
        || !delivery_sequences_valid
        || !state_valid
    {
        return Err(StoreError::Integrity(format!(
            "cache cleanup operation record is incoherent: {}",
            operation.operation_id.as_str()
        )));
    }
    Ok(())
}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
