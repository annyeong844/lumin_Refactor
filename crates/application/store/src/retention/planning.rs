mod gates;
mod runs;

use std::collections::BTreeMap;

use lumin_evidence::{
    OperationRecord, RetentionExclusionReason, RetentionItemKind, RetentionMutationResult,
    RetentionOperationKind, RetentionOperationRecord, RetentionOperationResult,
    RetentionOperationStatus, RetentionPlanExclusion, RetentionPlanItem, RetentionPlanRecord,
    RetentionPlanScope, RetentionPlanState,
};
use lumin_model::{OperationId, RetentionPlanId, append_length_prefixed, digest_hex};
use redb::{ReadableTable, TableDefinition, TableError, WriteTransaction};
use serde::de::DeserializeOwned;

use crate::gate::OPERATIONS;
use crate::namespace::NamespaceGuard;
use crate::{RepositoryStore, StoreError, backend_error, nonce_hex, unix_millis};

use super::RetentionPlanRequest;
use super::records::{
    OPERATION_SCHEMA, PLAN_SCHEMA, StoredRetentionPlan, StoredTombstone,
    canonical_content_identity, ensure_result_matches, next_sequence, read_plan,
    read_retention_operation, retention_operation_result, tombstone_key, write_plan,
    write_retention_operation,
};

pub(super) struct PlanContents {
    pub(super) items: Vec<RetentionPlanItem>,
    pub(super) exclusions: Vec<RetentionPlanExclusion>,
}

pub(super) fn prepare(
    store: &RepositoryStore,
    request: &RetentionPlanRequest,
) -> Result<RetentionMutationResult, StoreError> {
    store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        let kind = plan_operation_kind(&request.scope);
        let request_digest = plan_request_digest(&request.scope)?;

        if let Some(operation) = read_retention_operation(&write, &request.operation_id)? {
            ensure_result_matches(&operation, kind, &request_digest)?;
            return retention_operation_result(operation);
        }
        reject_gate_operation_collision(&write, &request.operation_id)?;

        let contents = build_contents(guard, &write, &request.scope)?;
        let plan_sequence = next_sequence(&write, "retention-plan")?;
        let catalog_revision = next_sequence(&write, "retention-catalog")?;
        let plan_id = RetentionPlanId::from_string(format!("retention_plan_{plan_sequence:016x}"));
        let mut record = RetentionPlanRecord {
            schema_version: PLAN_SCHEMA.to_owned(),
            repository_id: guard.repository_id().clone(),
            plan_id: plan_id.clone(),
            content_identity: lumin_model::RetentionContentIdentity::from_string(String::new()),
            scope: request.scope.clone(),
            created_unix_millis: unix_millis()?,
            catalog_revision,
            state: RetentionPlanState::Prepared,
            items: contents.items,
            exclusions: contents.exclusions,
            confirmation_operation_id: None,
            recoverable_state: None,
            tombstone_identity: None,
            physical_reclamation_pending: false,
        };
        record.items.sort();
        record.exclusions.sort();
        record.content_identity = canonical_content_identity(&record)?;
        let result = RetentionMutationResult::Prepared {
            plan_id,
            content_identity: record.content_identity.clone(),
        };
        let plan = StoredRetentionPlan {
            record,
            trash_nonce: nonce_hex()?,
            progress: None,
        };
        let operation = RetentionOperationRecord {
            schema_version: OPERATION_SCHEMA.to_owned(),
            operation_id: request.operation_id.clone(),
            kind,
            request_digest,
            status: RetentionOperationStatus::Committed,
            plan_id: Some(plan.record.plan_id.clone()),
            result: RetentionOperationResult::Retention {
                result: result.clone(),
            },
        };
        write_plan(&write, &plan)?;
        write_retention_operation(&write, &operation)?;
        guard.commit(write)?;
        Ok(result)
    })
}

pub(super) fn build_contents(
    guard: &NamespaceGuard,
    write: &WriteTransaction,
    scope: &RetentionPlanScope,
) -> Result<PlanContents, StoreError> {
    let mut contents = match scope {
        RetentionPlanScope::Runs { before_unix_millis } => {
            runs::collect(guard, write, *before_unix_millis)
        }
        RetentionPlanScope::Gates {
            terminal_before_unix_millis,
        } => gates::collect(guard, write, *terminal_before_unix_millis),
    }?;
    exclude_retention_owned_items(write, &mut contents)?;
    contents.items.sort();
    contents.exclusions.sort();
    contents.exclusions.dedup();
    Ok(contents)
}

fn exclude_retention_owned_items(
    write: &WriteTransaction,
    contents: &mut PlanContents,
) -> Result<(), StoreError> {
    let mut retained = Vec::with_capacity(contents.items.len());
    for item in contents.items.drain(..) {
        let key = tombstone_key(item.kind, &item.record_id);
        let tombstone = crate::gate::records::read_record::<StoredTombstone>(
            write,
            super::RETENTION_TOMBSTONES,
            &key,
        )?;
        let Some(tombstone) = tombstone else {
            retained.push(item);
            continue;
        };
        let owner_plan = match read_plan(write, &tombstone.envelope.plan_id) {
            Err(StoreError::RetentionPlanNotFound(_)) => {
                return Err(StoreError::Integrity(format!(
                    "retention tombstone {key} has no owner plan"
                )));
            }
            result => result?,
        };
        super::records::validate_tombstone(&key, &tombstone, &owner_plan)?;
        let owner_matches = owner_plan.record.state == RetentionPlanState::Pruning
            && owner_plan
                .record
                .items
                .iter()
                .any(|owner_item| owner_item == &item);
        if !owner_matches {
            return Err(StoreError::Integrity(format!(
                "retention tombstone {key} disagrees with its active owner plan"
            )));
        }
        contents.exclusions.push(RetentionPlanExclusion {
            kind: item.kind,
            record_id: item.record_id,
            reason: RetentionExclusionReason::RetentionInProgress {
                plan_id: tombstone.envelope.plan_id,
            },
        });
    }
    contents.items = retained;
    Ok(())
}

pub(super) fn plan_request_digest(scope: &RetentionPlanScope) -> Result<String, StoreError> {
    let mut bytes = Vec::new();
    append_length_prefixed(&mut bytes, b"lumin-retention-plan-request.v1");
    let scope = serde_json::to_vec(scope).map_err(crate::serialization_error)?;
    append_length_prefixed(&mut bytes, &scope);
    Ok(digest_hex(&bytes))
}

pub(super) fn confirm_request_digest(plan_id: &RetentionPlanId) -> String {
    let mut bytes = Vec::new();
    append_length_prefixed(&mut bytes, b"lumin-retention-confirm-request.v1");
    append_length_prefixed(&mut bytes, plan_id.as_str().as_bytes());
    digest_hex(&bytes)
}

pub(super) fn plan_operation_kind(scope: &RetentionPlanScope) -> RetentionOperationKind {
    match scope {
        RetentionPlanScope::Runs { .. } => RetentionOperationKind::RunPrunePlan,
        RetentionPlanScope::Gates { .. } => RetentionOperationKind::GatePrunePlan,
    }
}

pub(super) fn confirm_operation_kind(scope: &RetentionPlanScope) -> RetentionOperationKind {
    match scope {
        RetentionPlanScope::Runs { .. } => RetentionOperationKind::RunPruneConfirm,
        RetentionPlanScope::Gates { .. } => RetentionOperationKind::GatePruneConfirm,
    }
}

pub(super) fn reject_gate_operation_collision(
    write: &WriteTransaction,
    operation_id: &OperationId,
) -> Result<(), StoreError> {
    if crate::gate::records::read_record::<OperationRecord>(
        write,
        OPERATIONS,
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

pub(super) fn read_raw_records<T: DeserializeOwned>(
    write: &WriteTransaction,
    definition: TableDefinition<'static, &str, &[u8]>,
    table_name: &str,
) -> Result<BTreeMap<String, (T, Vec<u8>)>, StoreError> {
    let table = match write.open_table(definition) {
        Ok(table) => table,
        Err(TableError::TableDoesNotExist(_)) => return Ok(BTreeMap::new()),
        Err(error) => return Err(backend_error(error)),
    };
    let mut records = BTreeMap::new();
    for row in table.iter().map_err(backend_error)? {
        let (key, value) = row.map_err(backend_error)?;
        let key = key.value().to_owned();
        let bytes = value.value().to_vec();
        let record = serde_json::from_slice(&bytes).map_err(|error| {
            StoreError::Integrity(format!("{table_name} record {key} is malformed: {error}"))
        })?;
        records.insert(key, (record, bytes));
    }
    Ok(records)
}

pub(super) fn raw_pointer(
    write: &WriteTransaction,
    key: &str,
) -> Result<Option<String>, StoreError> {
    let table = write.open_table(crate::POINTERS).map_err(backend_error)?;
    let value = table
        .get(key)
        .map_err(backend_error)?
        .map(|value| value.value().to_vec());
    value
        .map(|bytes| {
            String::from_utf8(bytes).map_err(|error| {
                StoreError::Integrity(format!("{key} pointer is not UTF-8: {error}"))
            })
        })
        .transpose()
}

pub(super) fn orphan_payload_identity(path: &std::path::Path) -> Result<(String, u64), StoreError> {
    runs::directory_payload_identity(path)
}

pub(super) fn retention_item_from_bytes(
    kind: RetentionItemKind,
    owning_sequence: u64,
    record_id: String,
    bytes: &[u8],
) -> RetentionPlanItem {
    RetentionPlanItem {
        kind,
        owning_sequence,
        record_id,
        identity_sha256: digest_hex(bytes),
        byte_count: bytes.len() as u64,
    }
}
