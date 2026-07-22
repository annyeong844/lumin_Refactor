use lumin_evidence::{
    RetentionItemKind, RetentionMutationResult, RetentionOperationKind, RetentionOperationRecord,
    RetentionOperationResult, RetentionOperationStatus, RetentionPlanRecord, RetentionPlanState,
    RetentionTombstoneEnvelope,
};
use lumin_model::{
    OperationId, PhysicalFileIdentity, RetentionContentIdentity, RetentionPlanId,
    RetentionTombstoneIdentity, append_length_prefixed, digest_hex,
};
use redb::{ReadTransaction, ReadableTable, TableError, WriteTransaction};
use serde::{Deserialize, Serialize};

use crate::gate::records::{load_record, read_record, write_record};
use crate::namespace::{
    StoreDatabase,
    records::{ManagedStateParentBinding, ManagedStateParentKind},
};
use crate::{RepositoryStore, StoreError, backend_error, serialization_error};

use super::{RETENTION_OPERATIONS, RETENTION_PLANS, RETENTION_TOMBSTONES};

pub(crate) use validation::{validate_plan, validate_retention_operation, validate_tombstone};

pub(super) const PLAN_SCHEMA: &str = "lumin-retention-plan.v1";
pub(super) const OPERATION_SCHEMA: &str = "lumin-retention-operation.v1";
pub(super) const TOMBSTONE_SCHEMA: &str = "lumin-retention-tombstone.v1";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StoredRetentionPlan {
    pub(crate) record: RetentionPlanRecord,
    pub(crate) trash_nonce: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) progress: Option<RetentionProgress>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RetentionProgress {
    pub(crate) source_parent_bindings: Vec<ManagedStateParentBinding>,
    pub(crate) trash_parent_binding: ManagedStateParentBinding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) trash_directory: Option<TrashDirectoryBinding>,
    pub(crate) moves: Vec<RetentionPayloadMove>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrashDirectoryBinding {
    pub(crate) directory_physical_identity: PhysicalFileIdentity,
    pub(crate) anchor_physical_identity: PhysicalFileIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RetentionPayloadMove {
    pub(crate) kind: RetentionItemKind,
    pub(crate) record_id: String,
    pub(crate) source_parent: ManagedStateParentKind,
    pub(crate) source_child: String,
    pub(crate) trash_child: String,
    pub(crate) physical_identity: PhysicalFileIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StoredTombstone {
    pub(crate) schema_version: String,
    pub(crate) envelope: RetentionTombstoneEnvelope,
    pub(crate) identity_sha256: String,
    pub(crate) owning_sequence: u64,
}

pub(super) fn load_public_plan(
    store: &RepositoryStore,
    plan_id: &RetentionPlanId,
) -> Result<RetentionPlanRecord, StoreError> {
    store.with_shared_lock(|guard| {
        let database = guard.open_database()?;
        let plan =
            load_record::<StoredRetentionPlan>(&database, RETENTION_PLANS, plan_id.as_str())?
                .ok_or_else(|| StoreError::RetentionPlanNotFound(plan_id.as_str().to_owned()))?;
        validate_plan(&plan)?;
        Ok(plan.record)
    })
}

pub(super) fn read_plan(
    write: &WriteTransaction,
    plan_id: &RetentionPlanId,
) -> Result<StoredRetentionPlan, StoreError> {
    let plan = read_record(write, RETENTION_PLANS, plan_id.as_str())?
        .ok_or_else(|| StoreError::RetentionPlanNotFound(plan_id.as_str().to_owned()))?;
    validate_plan(&plan)?;
    Ok(plan)
}

pub(super) fn write_plan(
    write: &WriteTransaction,
    plan: &StoredRetentionPlan,
) -> Result<(), StoreError> {
    validate_plan(plan)?;
    write_record(write, RETENTION_PLANS, plan.record.plan_id.as_str(), plan)
}

pub(super) fn read_retention_operation(
    write: &WriteTransaction,
    operation_id: &OperationId,
) -> Result<Option<RetentionOperationRecord>, StoreError> {
    let operation = read_record(write, RETENTION_OPERATIONS, operation_id.as_str())?;
    if let Some(operation) = &operation {
        validate_retention_operation(operation)?;
    }
    Ok(operation)
}

pub(crate) fn project_retention_operation(
    database: &StoreDatabase<'_>,
    operation: &RetentionOperationRecord,
) -> Result<Option<bool>, StoreError> {
    validate_retention_operation(operation)?;
    if operation.status != RetentionOperationStatus::Committed
        || !matches!(
            operation.kind,
            RetentionOperationKind::RunPruneConfirm | RetentionOperationKind::GatePruneConfirm
        )
    {
        return Ok(None);
    }

    let plan_id = operation.plan_id.as_ref().ok_or_else(|| {
        StoreError::Integrity("committed retention confirmation has no plan".to_owned())
    })?;
    let plan = load_record::<StoredRetentionPlan>(database, RETENTION_PLANS, plan_id.as_str())?
        .ok_or_else(|| {
            StoreError::Integrity(format!(
                "retention operation {} has no owner plan",
                operation.operation_id.as_str()
            ))
        })?;
    validate_plan(&plan)?;
    if plan.record.state != RetentionPlanState::Pruned
        || plan.record.confirmation_operation_id.as_ref() != Some(&operation.operation_id)
    {
        return Err(StoreError::Integrity(format!(
            "retention operation {} disagrees with its owner plan",
            operation.operation_id.as_str()
        )));
    }

    match &operation.result {
        RetentionOperationResult::Retention {
            result:
                RetentionMutationResult::Pruned {
                    plan_id: result_plan_id,
                    tombstone_identity,
                    physical_reclamation_pending,
                },
        } if result_plan_id == plan_id
            && Some(tombstone_identity) == plan.record.tombstone_identity.as_ref()
            && (!plan.record.physical_reclamation_pending || *physical_reclamation_pending) =>
        {
            Ok(Some(plan.record.physical_reclamation_pending))
        }
        _ => Err(StoreError::Integrity(format!(
            "retention operation {} disagrees with its pruned plan",
            operation.operation_id.as_str()
        ))),
    }
}

pub(super) fn write_retention_operation(
    write: &WriteTransaction,
    operation: &RetentionOperationRecord,
) -> Result<(), StoreError> {
    validate_retention_operation(operation)?;
    write_record(
        write,
        RETENTION_OPERATIONS,
        operation.operation_id.as_str(),
        operation,
    )
}

pub(super) fn write_tombstone(
    write: &WriteTransaction,
    tombstone: &StoredTombstone,
) -> Result<(), StoreError> {
    let key = tombstone_key(
        tombstone.envelope.record_kind,
        &tombstone.envelope.record_id,
    );
    if let Some(existing) = read_record::<StoredTombstone>(write, RETENTION_TOMBSTONES, &key)? {
        if existing.envelope.plan_id != tombstone.envelope.plan_id {
            return Err(StoreError::Integrity(format!(
                "retention target {key} is already owned by plan {}",
                existing.envelope.plan_id.as_str()
            )));
        }
        if existing.schema_version != tombstone.schema_version
            || existing.envelope.record_kind != tombstone.envelope.record_kind
            || existing.envelope.record_id != tombstone.envelope.record_id
            || existing.identity_sha256 != tombstone.identity_sha256
            || existing.owning_sequence != tombstone.owning_sequence
        {
            return Err(StoreError::Integrity(format!(
                "retention target {key} changed its immutable tombstone identity"
            )));
        }
    }
    write_record(write, RETENTION_TOMBSTONES, &key, tombstone)
}

pub(crate) fn read_validated_tombstone(
    read: &ReadTransaction,
    kind: RetentionItemKind,
    record_id: &str,
) -> Result<Option<StoredTombstone>, StoreError> {
    let key = tombstone_key(kind, record_id);
    let table = match read.open_table(RETENTION_TOMBSTONES) {
        Ok(table) => table,
        Err(TableError::TableDoesNotExist(_)) => return Ok(None),
        Err(error) => return Err(backend_error(error)),
    };
    let bytes = table
        .get(key.as_str())
        .map_err(backend_error)?
        .map(|value| value.value().to_vec());
    let Some(bytes) = bytes else {
        return Ok(None);
    };
    let tombstone = serde_json::from_slice(&bytes).map_err(serialization_error)?;
    validate_tombstone_owner(read, &key, &tombstone)?;
    Ok(Some(tombstone))
}

pub(super) fn read_validated_tombstone_with_owner(
    write: &WriteTransaction,
    kind: RetentionItemKind,
    record_id: &str,
) -> Result<Option<(StoredTombstone, StoredRetentionPlan)>, StoreError> {
    let key = tombstone_key(kind, record_id);
    let Some(tombstone) = read_record::<StoredTombstone>(write, RETENTION_TOMBSTONES, &key)? else {
        return Ok(None);
    };
    let owner = match read_plan(write, &tombstone.envelope.plan_id) {
        Err(StoreError::RetentionPlanNotFound(_)) => {
            return Err(StoreError::Integrity(format!(
                "retention tombstone {key} has no owner plan"
            )));
        }
        result => result?,
    };
    validate_tombstone(&key, &tombstone, &owner)?;
    Ok(Some((tombstone, owner)))
}

pub(crate) fn validate_tombstone_owner(
    read: &ReadTransaction,
    key: &str,
    tombstone: &StoredTombstone,
) -> Result<(), StoreError> {
    let table = match read.open_table(RETENTION_PLANS) {
        Ok(table) => table,
        Err(TableError::TableDoesNotExist(_)) => {
            return Err(StoreError::Integrity(format!(
                "retention tombstone {key} has no owner plan"
            )));
        }
        Err(error) => return Err(backend_error(error)),
    };
    let bytes = table
        .get(tombstone.envelope.plan_id.as_str())
        .map_err(backend_error)?
        .map(|value| value.value().to_vec())
        .ok_or_else(|| {
            StoreError::Integrity(format!("retention tombstone {key} has no owner plan"))
        })?;
    let plan: StoredRetentionPlan = serde_json::from_slice(&bytes).map_err(serialization_error)?;
    validate_plan(&plan)?;
    validate_tombstone(key, tombstone, &plan)
}

pub(super) fn write_pruning_tombstones(
    write: &WriteTransaction,
    plan: &RetentionPlanRecord,
) -> Result<(), StoreError> {
    let recoverable_state = plan
        .recoverable_state
        .ok_or_else(|| StoreError::Integrity("pruning plan has no recoverable state".to_owned()))?;
    for item in &plan.items {
        write_tombstone(
            write,
            &StoredTombstone {
                schema_version: TOMBSTONE_SCHEMA.to_owned(),
                envelope: RetentionTombstoneEnvelope {
                    record_kind: item.kind,
                    record_id: item.record_id.clone(),
                    plan_id: plan.plan_id.clone(),
                    recoverable_state: Some(recoverable_state),
                    tombstone_identity: None,
                    physical_reclamation_pending: plan.physical_reclamation_pending,
                },
                identity_sha256: item.identity_sha256.clone(),
                owning_sequence: item.owning_sequence,
            },
        )?;
    }
    Ok(())
}

pub(super) fn write_pruned_tombstones(
    write: &WriteTransaction,
    plan: &RetentionPlanRecord,
) -> Result<(), StoreError> {
    let tombstone_identity = plan
        .tombstone_identity
        .clone()
        .ok_or_else(|| StoreError::Integrity("pruned plan has no tombstone identity".to_owned()))?;
    for item in &plan.items {
        write_tombstone(
            write,
            &StoredTombstone {
                schema_version: TOMBSTONE_SCHEMA.to_owned(),
                envelope: RetentionTombstoneEnvelope {
                    record_kind: item.kind,
                    record_id: item.record_id.clone(),
                    plan_id: plan.plan_id.clone(),
                    recoverable_state: None,
                    tombstone_identity: Some(tombstone_identity.clone()),
                    physical_reclamation_pending: plan.physical_reclamation_pending,
                },
                identity_sha256: item.identity_sha256.clone(),
                owning_sequence: item.owning_sequence,
            },
        )?;
    }
    Ok(())
}

pub(crate) fn tombstone_key(kind: RetentionItemKind, record_id: &str) -> String {
    format!("{}:{record_id}", kind.rank())
}

pub(super) fn canonical_content_identity(
    plan: &RetentionPlanRecord,
) -> Result<RetentionContentIdentity, StoreError> {
    let mut bytes = Vec::new();
    append_length_prefixed(&mut bytes, b"lumin-retention-plan-content.v1");
    append_length_prefixed(&mut bytes, plan.repository_id.as_str().as_bytes());
    let scope = serde_json::to_vec(&plan.scope).map_err(crate::serialization_error)?;
    append_length_prefixed(&mut bytes, &scope);
    bytes.extend_from_slice(&(plan.items.len() as u64).to_be_bytes());
    for item in &plan.items {
        bytes.push(item.kind.rank());
        bytes.extend_from_slice(&item.owning_sequence.to_be_bytes());
        append_length_prefixed(&mut bytes, item.record_id.as_bytes());
        append_length_prefixed(&mut bytes, item.identity_sha256.as_bytes());
        bytes.extend_from_slice(&item.byte_count.to_be_bytes());
    }
    bytes.extend_from_slice(&(plan.exclusions.len() as u64).to_be_bytes());
    for exclusion in &plan.exclusions {
        let encoded = serde_json::to_vec(exclusion).map_err(crate::serialization_error)?;
        append_length_prefixed(&mut bytes, &encoded);
    }
    Ok(RetentionContentIdentity::from_string(format!(
        "retention_content_{}",
        digest_hex(&bytes)
    )))
}

pub(super) fn canonical_tombstone_identity(
    plan: &RetentionPlanRecord,
) -> RetentionTombstoneIdentity {
    let mut bytes = Vec::new();
    append_length_prefixed(&mut bytes, b"lumin-retention-tombstone.v1");
    append_length_prefixed(&mut bytes, plan.repository_id.as_str().as_bytes());
    append_length_prefixed(&mut bytes, plan.plan_id.as_str().as_bytes());
    append_length_prefixed(&mut bytes, plan.content_identity.as_str().as_bytes());
    for item in &plan.items {
        append_length_prefixed(&mut bytes, item.record_id.as_bytes());
        append_length_prefixed(&mut bytes, item.identity_sha256.as_bytes());
    }
    RetentionTombstoneIdentity::from_string(format!("tombstone_{}", digest_hex(&bytes)))
}

pub(super) fn pruning_result(
    plan: &RetentionPlanRecord,
) -> Result<RetentionMutationResult, StoreError> {
    Ok(RetentionMutationResult::Pruning {
        plan_id: plan.plan_id.clone(),
        recoverable_state: plan.recoverable_state.ok_or_else(|| {
            StoreError::Integrity("pruning retention plan has no recoverable state".to_owned())
        })?,
    })
}

pub(super) fn pruned_result(
    plan: &RetentionPlanRecord,
) -> Result<RetentionMutationResult, StoreError> {
    Ok(RetentionMutationResult::Pruned {
        plan_id: plan.plan_id.clone(),
        tombstone_identity: plan.tombstone_identity.clone().ok_or_else(|| {
            StoreError::Integrity("pruned retention plan has no tombstone identity".to_owned())
        })?,
        physical_reclamation_pending: plan.physical_reclamation_pending,
    })
}

pub(crate) fn next_sequence(write: &WriteTransaction, key: &str) -> Result<u64, StoreError> {
    let mut table = write.open_table(crate::SEQUENCES).map_err(backend_error)?;
    let current = table
        .get(key)
        .map_err(backend_error)?
        .map_or(0, |value| value.value());
    let next = current
        .checked_add(1)
        .ok_or_else(|| StoreError::Integrity(format!("{key} sequence overflow")))?;
    table.insert(key, next).map_err(backend_error)?;
    Ok(next)
}

pub(super) fn ensure_result_matches(
    operation: &RetentionOperationRecord,
    expected_kind: lumin_evidence::RetentionOperationKind,
    request_digest: &str,
) -> Result<(), StoreError> {
    if operation.kind != expected_kind || operation.request_digest != request_digest {
        return Err(StoreError::OperationConflict(
            operation.operation_id.as_str().to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn retention_operation_result(
    operation: RetentionOperationRecord,
) -> Result<RetentionMutationResult, StoreError> {
    match operation.result {
        RetentionOperationResult::Retention { result } => Ok(result),
        _ => Err(StoreError::OperationConflict(
            operation.operation_id.as_str().to_owned(),
        )),
    }
}
mod validation;
