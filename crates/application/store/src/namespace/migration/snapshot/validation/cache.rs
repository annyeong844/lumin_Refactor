use std::collections::{BTreeMap, BTreeSet};

use lumin_evidence::{
    CacheCleanupOperationRecord, CacheCleanupOperationStatus, CacheEvictionAuthorization,
    CacheEvictionAuthorizationState, OperationRecord,
};
use lumin_model::CacheEvictionAuthorizationSetId;

use crate::StoreError;

use super::{LogicalStoreSnapshot, parse_record};

pub(super) fn validate_cache(
    snapshot: &LogicalStoreSnapshot,
    gate_operations: &BTreeMap<&str, OperationRecord>,
) -> Result<(), StoreError> {
    let mut operations = BTreeMap::new();
    for (key, bytes) in &snapshot.cache_cleanup_operations {
        if gate_operations.contains_key(key.as_str())
            || snapshot.retention_operations.contains_key(key)
        {
            return Err(StoreError::Integrity(format!(
                "operation ID {key} is owned by multiple lifecycle tables"
            )));
        }
        let operation =
            parse_record::<CacheCleanupOperationRecord>("cache-cleanup-operations", key, bytes)?;
        if operation.operation_id.as_str() != key {
            return Err(StoreError::Integrity(format!(
                "cache cleanup operation key {key} disagrees with its record"
            )));
        }
        crate::cache::validate_operation_shape(&operation)?;
        let expected_digest =
            lumin_evidence::cache_cleanup_request_digest(&operation.repository_id);
        if operation.request_digest != expected_digest {
            return Err(StoreError::Integrity(format!(
                "cache cleanup operation {key} has an unauthenticated request digest"
            )));
        }
        operations.insert(key.as_str(), operation);
    }

    let mut referenced = BTreeSet::new();
    let mut authorizations = BTreeMap::new();
    for operation in operations.values() {
        let validated_count = usize::try_from(operation.validated_count).map_err(|_| {
            StoreError::Integrity(format!(
                "cache cleanup operation {} validated count exceeds platform limits",
                operation.operation_id.as_str()
            ))
        })?;
        for (index, key) in operation.authorization_keys.iter().enumerate() {
            if !referenced.insert(key.as_str()) {
                return Err(StoreError::Integrity(format!(
                    "cache eviction authorization {key} has multiple owners"
                )));
            }
            let bytes = snapshot
                .cache_eviction_authorizations
                .get(key)
                .ok_or_else(|| {
                    StoreError::Integrity(format!(
                        "cache cleanup operation {} references a missing authorization {key}",
                        operation.operation_id.as_str()
                    ))
                })?;
            let authorization = parse_record::<CacheEvictionAuthorization>(
                "cache-eviction-authorizations",
                key,
                bytes,
            )?;
            crate::cache::validate_authorization_record(
                &authorization,
                key,
                &operation.repository_id,
            )?;
            let ordinal = u64::try_from(index).map_err(|_| {
                StoreError::Integrity("cache authorization ordinal exceeds u64".to_owned())
            })?;
            let expected_state = if index < validated_count {
                CacheEvictionAuthorizationState::Validated
            } else {
                CacheEvictionAuthorizationState::Authorized
            };
            if authorization.operation_id != operation.operation_id
                || authorization.request_digest != operation.request_digest
                || authorization.invocation_id != operation.invocation_id
                || authorization.ordinal != ordinal
                || authorization.state != expected_state
            {
                return Err(StoreError::Integrity(format!(
                    "cache eviction authorization {key} disagrees with its owning operation"
                )));
            }
            authorizations.insert(key.as_str(), authorization);
        }
    }

    let stored = snapshot
        .cache_eviction_authorizations
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if referenced != stored {
        return Err(StoreError::Integrity(
            "cache eviction authorization ownership is not bijective".to_owned(),
        ));
    }
    let unfinished = operations
        .values()
        .filter(|operation| {
            matches!(
                operation.status,
                CacheCleanupOperationStatus::Pending | CacheCleanupOperationStatus::Interrupted
            )
        })
        .collect::<Vec<_>>();
    if unfinished.len() > 1 {
        return Err(StoreError::Integrity(
            "multiple cache cleanup operations retained the exclusive mutation reservation"
                .to_owned(),
        ));
    }
    for operation in unfinished {
        let current = operation
            .authorization_keys
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let initial_rows = authorizations
            .iter()
            .filter(|(key, _)| !current.contains(*key))
            .map(|(_, authorization)| crate::cache::authorization_set_frame(authorization))
            .collect::<Vec<_>>();
        let initial_count = u64::try_from(initial_rows.len()).map_err(|_| {
            StoreError::Integrity(
                "cache cleanup initial authorization count exceeds u64".to_owned(),
            )
        })?;
        if initial_count != operation.initial_authorization_count
            || CacheEvictionAuthorizationSetId::for_canonical_rows(&initial_rows)
                != operation.initial_authorization_set_id
        {
            return Err(StoreError::Integrity(format!(
                "cache cleanup operation {} initial authorization provenance is not exact",
                operation.operation_id.as_str()
            )));
        }
    }
    Ok(())
}
