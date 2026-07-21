mod runs;

use lumin_evidence::{
    GateRecord, LifecycleOperationRecord, RecordLookup, RetentionMutationResult,
    RetentionOperationRecord, RetentionPlanExclusion, RetentionPlanItem, RetentionPlanRecord,
    RetentionPlanScope, RetentionPlanState, RetentionTombstoneEnvelope, RunPinRecord,
};
use lumin_model::{OperationId, RetentionContentIdentity, RetentionPlanId};
use serde::{Deserialize, Serialize};

use crate::cursor::{decode_cursor_payload, encode_cursor_payload};
use crate::{
    GateShowResponseDto, OperationShowResponseDto, ProtocolError, gate_show_response,
    operation_show_response,
};

pub const RETENTION_PLAN_ITEMS_ORDERING: &str = "retention-plan-items.v1";
pub const RETENTION_PLAN_PAGE_SIZE: usize = 100;

pub use runs::{
    RUNS_ORDERING, RUNS_PAGE_SIZE, RunCatalogCollectionDto, RunCatalogItemDto, run_catalog_item,
    run_catalog_response,
};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionMutationResponseDto {
    pub schema_version: &'static str,
    pub result: RetentionMutationResult,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPinResponseDto {
    pub schema_version: &'static str,
    pub pin: RunPinRecord,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionPlanCollectionDto {
    pub schema_version: &'static str,
    pub plan_id: RetentionPlanId,
    pub content_identity: RetentionContentIdentity,
    pub scope: RetentionPlanScope,
    pub state: RetentionPlanState,
    pub created_unix_millis: u128,
    pub catalog_revision: u64,
    pub ordering: &'static str,
    pub total: usize,
    pub returned: usize,
    pub truncated: bool,
    pub next_cursor: Option<String>,
    pub items: Vec<RetentionPlanItem>,
    pub exclusions: Vec<RetentionPlanExclusion>,
    pub physical_reclamation_pending: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum GateLookupResponseDto {
    Live(GateShowResponseDto),
    Tombstone(LookupTombstoneResponseDto),
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum LookupTombstoneResponseDto {
    Pruning {
        tombstone: RetentionTombstoneEnvelope,
    },
    Pruned {
        tombstone: RetentionTombstoneEnvelope,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum LifecycleOperationShowResponseDto {
    Gate(OperationShowResponseDto),
    Retention(RetentionOperationResponseDto),
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionOperationResponseDto {
    pub schema_version: &'static str,
    pub operation_id: OperationId,
    pub operation: RetentionOperationRecord,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RetentionCursorDto {
    schema_version: String,
    plan_id: RetentionPlanId,
    content_identity: RetentionContentIdentity,
    ordering: String,
    last_item: RetentionPlanItem,
}

pub fn retention_mutation_response(
    result: &RetentionMutationResult,
) -> RetentionMutationResponseDto {
    RetentionMutationResponseDto {
        schema_version: "lumin.retention-mutation.v1",
        result: result.clone(),
    }
}

pub fn run_pin_response(pin: &RunPinRecord) -> RunPinResponseDto {
    RunPinResponseDto {
        schema_version: "lumin.run-pin.v1",
        pin: pin.clone(),
    }
}

pub fn retention_plan_response(
    plan: &RetentionPlanRecord,
    cursor: Option<&str>,
) -> Result<RetentionPlanCollectionDto, ProtocolError> {
    let start = match cursor {
        Some(cursor) => {
            let cursor = decode_cursor(cursor)?;
            if cursor.plan_id != plan.plan_id
                || cursor.content_identity != plan.content_identity
                || cursor.ordering != RETENTION_PLAN_ITEMS_ORDERING
            {
                return Err(ProtocolError::CursorScopeMismatch);
            }
            plan.items
                .iter()
                .position(|item| item == &cursor.last_item)
                .map(|index| index + 1)
                .ok_or(ProtocolError::CursorAnchorMissing)?
        }
        None => 0,
    };
    let end = start
        .saturating_add(RETENTION_PLAN_PAGE_SIZE)
        .min(plan.items.len());
    let items = plan.items[start..end].to_vec();
    let truncated = end < plan.items.len();
    let next_cursor = if truncated {
        items
            .last()
            .map(|item| encode_cursor(plan, item))
            .transpose()?
    } else {
        None
    };
    Ok(RetentionPlanCollectionDto {
        schema_version: "lumin.retention-plan.v1",
        plan_id: plan.plan_id.clone(),
        content_identity: plan.content_identity.clone(),
        scope: plan.scope.clone(),
        state: plan.state,
        created_unix_millis: plan.created_unix_millis,
        catalog_revision: plan.catalog_revision,
        ordering: RETENTION_PLAN_ITEMS_ORDERING,
        total: plan.items.len(),
        returned: items.len(),
        truncated,
        next_cursor,
        items,
        exclusions: plan.exclusions.clone(),
        physical_reclamation_pending: plan.physical_reclamation_pending,
    })
}

pub fn gate_lookup_response(lookup: RecordLookup<GateRecord>) -> GateLookupResponseDto {
    match lookup {
        RecordLookup::Live(gate) => GateLookupResponseDto::Live(gate_show_response(&gate)),
        RecordLookup::Pruning(tombstone) => {
            GateLookupResponseDto::Tombstone(LookupTombstoneResponseDto::Pruning { tombstone })
        }
        RecordLookup::Pruned(tombstone) => {
            GateLookupResponseDto::Tombstone(LookupTombstoneResponseDto::Pruned { tombstone })
        }
    }
}

pub fn lifecycle_operation_response(
    operation: &LifecycleOperationRecord,
) -> LifecycleOperationShowResponseDto {
    match operation {
        LifecycleOperationRecord::Gate(operation) => {
            LifecycleOperationShowResponseDto::Gate(operation_show_response(operation))
        }
        LifecycleOperationRecord::Retention(operation) => {
            LifecycleOperationShowResponseDto::Retention(RetentionOperationResponseDto {
                schema_version: "lumin.retention-operation.v1",
                operation_id: operation.operation_id.clone(),
                operation: operation.as_ref().clone(),
            })
        }
    }
}

fn encode_cursor(
    plan: &RetentionPlanRecord,
    last_item: &RetentionPlanItem,
) -> Result<String, ProtocolError> {
    let cursor = RetentionCursorDto {
        schema_version: "lumin-retention-cursor.v1".to_owned(),
        plan_id: plan.plan_id.clone(),
        content_identity: plan.content_identity.clone(),
        ordering: RETENTION_PLAN_ITEMS_ORDERING.to_owned(),
        last_item: last_item.clone(),
    };
    encode_cursor_payload(&cursor)
}

fn decode_cursor(value: &str) -> Result<RetentionCursorDto, ProtocolError> {
    let cursor: RetentionCursorDto = decode_cursor_payload(value)?;
    if cursor.schema_version != "lumin-retention-cursor.v1" {
        return Err(ProtocolError::CursorScopeMismatch);
    }
    Ok(cursor)
}

#[cfg(test)]
mod tests;
