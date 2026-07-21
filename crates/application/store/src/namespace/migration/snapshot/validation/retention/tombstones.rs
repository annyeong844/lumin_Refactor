use std::collections::{BTreeMap, BTreeSet};

use lumin_evidence::RetentionPlanState;

use crate::StoreError;
use crate::retention::records::{StoredRetentionPlan, StoredTombstone, tombstone_key};

use super::{LogicalStoreSnapshot, parse_record};

pub(super) fn validate_tombstones(
    snapshot: &LogicalStoreSnapshot,
    plans: &BTreeMap<&str, StoredRetentionPlan>,
) -> Result<(), StoreError> {
    let expected = plans
        .values()
        .filter(|plan| plan.record.state != RetentionPlanState::Prepared)
        .flat_map(|plan| {
            plan.record
                .items
                .iter()
                .map(|item| tombstone_key(item.kind, &item.record_id))
        })
        .collect::<BTreeSet<_>>();
    if snapshot
        .retention_tombstones
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>()
        != expected
    {
        return Err(StoreError::Integrity(
            "retention tombstone inventory disagrees with non-prepared plans".to_owned(),
        ));
    }
    for (key, bytes) in &snapshot.retention_tombstones {
        validate_tombstone(key, bytes, plans)?;
    }
    Ok(())
}

fn validate_tombstone(
    key: &str,
    bytes: &[u8],
    plans: &BTreeMap<&str, StoredRetentionPlan>,
) -> Result<(), StoreError> {
    let tombstone = parse_record::<StoredTombstone>("retention-tombstones", key, bytes)?;
    if tombstone.schema_version != "lumin-retention-tombstone.v1"
        || tombstone_key(
            tombstone.envelope.record_kind,
            &tombstone.envelope.record_id,
        ) != key
    {
        return Err(StoreError::Integrity(format!(
            "retention tombstone key {key} disagrees with its record"
        )));
    }
    let plan = plans
        .get(tombstone.envelope.plan_id.as_str())
        .ok_or_else(|| StoreError::Integrity(format!("tombstone {key} has no plan")))?;
    let item = plan
        .record
        .items
        .iter()
        .find(|item| {
            item.kind == tombstone.envelope.record_kind
                && item.record_id == tombstone.envelope.record_id
        })
        .ok_or_else(|| StoreError::Integrity(format!("tombstone {key} has no plan item")))?;
    if tombstone.identity_sha256 != item.identity_sha256
        || tombstone.owning_sequence != item.owning_sequence
    {
        return Err(StoreError::Integrity(format!(
            "retention tombstone {key} changed its item identity"
        )));
    }
    validate_state(key, &tombstone, plan)
}

fn validate_state(
    key: &str,
    tombstone: &StoredTombstone,
    plan: &StoredRetentionPlan,
) -> Result<(), StoreError> {
    let state_matches = match plan.record.state {
        RetentionPlanState::Pruning => {
            tombstone.envelope.recoverable_state == plan.record.recoverable_state
                && tombstone.envelope.tombstone_identity.is_none()
        }
        RetentionPlanState::Pruned => {
            tombstone.envelope.recoverable_state.is_none()
                && tombstone.envelope.tombstone_identity == plan.record.tombstone_identity
        }
        RetentionPlanState::Prepared => false,
    };
    if !state_matches
        || tombstone.envelope.physical_reclamation_pending
            != plan.record.physical_reclamation_pending
    {
        return Err(StoreError::Integrity(format!(
            "retention tombstone {key} disagrees with its plan state"
        )));
    }
    Ok(())
}
