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
    DecodedRunCatalogCursor, RUNS_ORDERING, RUNS_PAGE_SIZE, RunCatalogCollectionDto,
    RunCatalogItemDto, decode_run_catalog_cursor, run_catalog_item, run_catalog_response,
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
    anchor: RetentionCursorAnchor,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
enum RetentionCursorAnchor {
    Item { item: RetentionPlanItem },
    Exclusion { exclusion: RetentionPlanExclusion },
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
    let item_count = plan.items.len();
    let total = item_count.saturating_add(plan.exclusions.len());
    let start = match cursor {
        Some(cursor) => {
            let cursor = decode_cursor(cursor)?;
            if cursor.plan_id != plan.plan_id
                || cursor.content_identity != plan.content_identity
                || cursor.ordering != RETENTION_PLAN_ITEMS_ORDERING
            {
                return Err(ProtocolError::CursorScopeMismatch);
            }
            match cursor.anchor {
                RetentionCursorAnchor::Item { item } => plan
                    .items
                    .iter()
                    .position(|candidate| candidate == &item)
                    .map(|index| index + 1)
                    .ok_or(ProtocolError::CursorAnchorMissing)?,
                RetentionCursorAnchor::Exclusion { exclusion } => plan
                    .exclusions
                    .iter()
                    .position(|candidate| candidate == &exclusion)
                    .map(|index| item_count + index + 1)
                    .ok_or(ProtocolError::CursorAnchorMissing)?,
            }
        }
        None => 0,
    };
    let end = start.saturating_add(RETENTION_PLAN_PAGE_SIZE).min(total);
    let item_start = start.min(item_count);
    let item_end = end.min(item_count);
    let items = plan.items[item_start..item_end].to_vec();
    let exclusion_start = start.saturating_sub(item_count);
    let exclusion_end = end.saturating_sub(item_count).min(plan.exclusions.len());
    let exclusions = plan.exclusions[exclusion_start..exclusion_end].to_vec();
    let truncated = end < total;
    let next_cursor = if truncated {
        let anchor = if end <= item_count {
            RetentionCursorAnchor::Item {
                item: plan.items[end - 1].clone(),
            }
        } else {
            RetentionCursorAnchor::Exclusion {
                exclusion: plan.exclusions[end - item_count - 1].clone(),
            }
        };
        Some(encode_cursor(plan, anchor)?)
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
        total,
        returned: items.len() + exclusions.len(),
        truncated,
        next_cursor,
        items,
        exclusions,
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
    anchor: RetentionCursorAnchor,
) -> Result<String, ProtocolError> {
    let cursor = RetentionCursorDto {
        schema_version: "lumin-retention-cursor.v2".to_owned(),
        plan_id: plan.plan_id.clone(),
        content_identity: plan.content_identity.clone(),
        ordering: RETENTION_PLAN_ITEMS_ORDERING.to_owned(),
        anchor,
    };
    encode_cursor_payload(&cursor)
}

fn decode_cursor(value: &str) -> Result<RetentionCursorDto, ProtocolError> {
    let cursor: RetentionCursorDto = decode_cursor_payload(value)?;
    if cursor.schema_version != "lumin-retention-cursor.v2" {
        return Err(ProtocolError::CursorScopeMismatch);
    }
    Ok(cursor)
}

#[cfg(test)]
mod tests;
