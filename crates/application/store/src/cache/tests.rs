use std::fs;
use std::path::Path;

use lumin_evidence::{
    CacheCleanupDeliveryOutcome, CacheCleanupDeliveryStatus, CacheCleanupOperationStatus,
    LifecycleOperationRecord,
};
use lumin_model::{OperationId, append_length_prefixed, digest_hex};

use super::*;

fn open_store(root: &Path) -> Result<RepositoryStore, Box<dyn std::error::Error>> {
    let admission = lumin_inventory::repository_admission(root)?;
    Ok(RepositoryStore::open(
        &admission.canonical_root,
        &admission.binding,
    )?)
}

fn digest(store: &RepositoryStore) -> Result<String, StoreError> {
    let repository_id = store.repository_id()?;
    let mut framed = Vec::new();
    append_length_prefixed(&mut framed, b"lumin-cache-clean-request.v2");
    append_length_prefixed(&mut framed, repository_id.as_str().as_bytes());
    append_length_prefixed(&mut framed, b"cache-clean");
    Ok(digest_hex(&framed))
}

#[test]
fn cleanup_quarantines_payloads_and_replays_one_committed_result()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let cache = root.path().join(".lumin/cache");
    fs::create_dir_all(cache.join("nested/deep"))?;
    fs::write(cache.join("nested/deep/payload.bin"), b"nested")?;
    fs::write(cache.join("direct.bin"), b"direct")?;
    let operation_id = OperationId::from_string("cache-clean-1".to_owned());
    let request_digest = digest(&store)?;

    let first = store.clean_cache_payloads(&operation_id, &request_digest)?;
    let replay = store.clean_cache_payloads(&operation_id, &request_digest)?;
    assert_eq!(first, replay);
    assert_eq!(
        fs::read_dir(&cache)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<Result<Vec<_>, _>>()?,
        [std::ffi::OsString::from("namespace.anchor")]
    );
    let quarantine = root.path().join(".lumin/trash/cache-evictions");
    assert_eq!(fs::read_dir(&quarantine)?.count(), 3);

    let operation = store.load_cache_cleanup_operation(&operation_id)?;
    assert_eq!(operation.status, CacheCleanupOperationStatus::Committed);
    assert_eq!(operation.authorized_count(), 2);
    assert_eq!(operation.validated_count, 2);
    assert!(matches!(
        store.load_lifecycle_operation(&operation_id)?,
        LifecycleOperationRecord::CacheCleanup(_)
    ));
    Ok(())
}

#[test]
fn delivery_ledger_orders_attempts_before_transport_and_ignores_late_lower_completion()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let operation_id = OperationId::from_string("cache-clean-delivery-ledger".to_owned());
    let request_digest = digest(&store)?;
    store.clean_cache_payloads(&operation_id, &request_digest)?;

    let committed = store.load_cache_cleanup_operation(&operation_id)?;
    assert_eq!(
        committed.last_delivery_status(),
        CacheCleanupDeliveryStatus::NotAttempted
    );

    let lower = store.allocate_cache_cleanup_delivery(&operation_id, &request_digest)?;
    let greater = store.allocate_cache_cleanup_delivery(&operation_id, &request_digest)?;
    assert_eq!((lower, greater), (1, 2));
    assert_eq!(
        store
            .load_cache_cleanup_operation(&operation_id)?
            .last_delivery_status(),
        CacheCleanupDeliveryStatus::Unknown
    );

    store.record_cache_cleanup_delivery(
        &operation_id,
        &request_digest,
        greater,
        CacheCleanupDeliveryOutcome::Succeeded,
    )?;
    let before_late = store.load_cache_cleanup_operation(&operation_id)?;
    assert_eq!(
        before_late.last_delivery_status(),
        CacheCleanupDeliveryStatus::Succeeded
    );
    assert_eq!(before_late.greatest_completed_delivery_sequence, Some(2));

    store.record_cache_cleanup_delivery(
        &operation_id,
        &request_digest,
        lower,
        CacheCleanupDeliveryOutcome::Failed,
    )?;
    let after_late = store.load_cache_cleanup_operation(&operation_id)?;
    assert_eq!(
        after_late.last_delivery_status(),
        CacheCleanupDeliveryStatus::Succeeded
    );
    assert_eq!(after_late.greatest_allocated_delivery_sequence, 2);
    assert_eq!(after_late.greatest_completed_delivery_sequence, Some(2));
    assert_eq!(after_late.delivery_completions.len(), 2);

    store.record_cache_cleanup_delivery(
        &operation_id,
        &request_digest,
        lower,
        CacheCleanupDeliveryOutcome::Failed,
    )?;
    assert!(matches!(
        store.record_cache_cleanup_delivery(
            &operation_id,
            &request_digest,
            lower,
            CacheCleanupDeliveryOutcome::Succeeded,
        ),
        Err(StoreError::Integrity(message)) if message.contains("changed outcome")
    ));
    Ok(())
}

#[test]
fn invalid_payload_prevents_authorization_and_movement() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let cache = root.path().join(".lumin/cache");
    let ordinary = root.path().join("ordinary.bin");
    fs::write(cache.join("a-valid.bin"), b"valid")?;
    fs::write(&ordinary, b"shared")?;
    fs::hard_link(&ordinary, cache.join("z-shared.bin"))?;
    let operation_id = OperationId::from_string("cache-clean-invalid".to_owned());
    let request_digest = digest(&store)?;

    assert!(
        store
            .clean_cache_payloads(&operation_id, &request_digest)
            .is_err()
    );
    assert!(cache.join("a-valid.bin").is_file());
    assert!(cache.join("z-shared.bin").is_file());
    assert_eq!(
        fs::read_dir(root.path().join(".lumin/trash/cache-evictions"))?.count(),
        1
    );
    Ok(())
}

#[test]
fn self_consistent_foreign_quarantine_is_not_authorization()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let cache = root.path().join(".lumin/cache");
    fs::write(cache.join("foreign.bin"), b"foreign")?;
    let payload = store.with_exclusive_lock(|guard| {
        let mut payloads = prepare_active_payloads(guard, None)?;
        if payloads.len() != 1 {
            return Err(StoreError::Integrity(
                "foreign quarantine fixture produced the wrong payload count".to_owned(),
            ));
        }
        Ok(payloads.remove(0))
    })?;
    let manifest_digest = manifest_digest(&payload.manifest);
    let name = format!(
        "0123456789abcdef0123456789abcdef.{:016x}.{manifest_digest}",
        0
    );
    let quarantine = root.path().join(".lumin/trash/cache-evictions");
    fs::rename(cache.join("foreign.bin"), quarantine.join(&name))?;

    let operation_id = OperationId::from_string("cache-clean-foreign".to_owned());
    let error = match store.clean_cache_payloads(&operation_id, &digest(&store)?) {
        Ok(_) => return Err("foreign quarantine unexpectedly authorized cleanup".into()),
        Err(error) => error,
    };
    assert!(
        matches!(error, StoreError::Integrity(message) if message.contains("authorization/child bijection"))
    );
    assert_eq!(fs::read(quarantine.join(name))?, b"foreign");
    assert!(matches!(
        store.load_cache_cleanup_operation(&operation_id),
        Err(StoreError::OperationNotFound(_))
    ));
    Ok(())
}

#[test]
fn lifecycle_migration_preserves_cleanup_authorizations_and_result()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    fs::write(root.path().join(".lumin/cache/payload.bin"), b"payload")?;
    let operation_id = OperationId::from_string("cache-clean-migration".to_owned());
    let request_digest = digest(&store)?;
    let result = store.clean_cache_payloads(&operation_id, &request_digest)?;
    rewrite_cleanup_operation_as_v1(&store, &operation_id)?;
    store.rewrite_current_store_header_as_prior_for_test()?;

    store.migrate_lifecycle_store()?;

    let migrated = store.load_cache_cleanup_operation(&operation_id)?;
    assert_eq!(migrated.result, Some(result.clone()));
    assert_eq!(migrated.greatest_allocated_delivery_sequence, 1);
    assert_eq!(migrated.greatest_completed_delivery_sequence, None);
    assert!(migrated.delivery_completions.is_empty());
    assert_eq!(
        migrated.last_delivery_status(),
        CacheCleanupDeliveryStatus::Unknown
    );
    assert_eq!(
        store.clean_cache_payloads(&operation_id, &request_digest)?,
        result
    );
    assert_eq!(
        fs::read_dir(root.path().join(".lumin/trash/cache-evictions"))?.count(),
        2
    );
    Ok(())
}

fn rewrite_cleanup_operation_as_v1(
    store: &RepositoryStore,
    operation_id: &OperationId,
) -> Result<(), StoreError> {
    store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        let operation = read_record::<lumin_evidence::CacheCleanupOperationRecord>(
            &write,
            CACHE_CLEANUP_OPERATIONS,
            operation_id.as_str(),
        )?
        .ok_or_else(|| StoreError::OperationNotFound(operation_id.as_str().to_owned()))?;
        if operation.greatest_allocated_delivery_sequence != 0
            || operation.greatest_completed_delivery_sequence.is_some()
            || !operation.delivery_completions.is_empty()
        {
            return Err(StoreError::Integrity(
                "legacy cleanup fixture already has delivery-v2 evidence".to_owned(),
            ));
        }
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
            "lastDeliveryStatus": "not-attempted"
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
