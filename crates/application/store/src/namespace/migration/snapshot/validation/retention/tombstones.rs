use std::collections::{BTreeMap, BTreeSet};

use lumin_evidence::RetentionPlanState;

use crate::StoreError;
use crate::retention::records::{
    StoredRetentionPlan, StoredTombstone, tombstone_key,
    validate_tombstone as validate_tombstone_record,
};

use super::{LogicalStoreSnapshot, parse_record};

pub(super) fn validate_tombstones(
    snapshot: &LogicalStoreSnapshot,
    plans: &BTreeMap<&str, StoredRetentionPlan>,
) -> Result<(), StoreError> {
    let mut expected_owners = BTreeMap::new();
    for plan in plans
        .values()
        .filter(|plan| plan.record.state != RetentionPlanState::Prepared)
    {
        for item in &plan.record.items {
            let key = tombstone_key(item.kind, &item.record_id);
            if let Some(existing) = expected_owners.insert(key.clone(), &plan.record.plan_id) {
                return Err(StoreError::Integrity(format!(
                    "retention target {key} is claimed by plans {} and {}",
                    existing.as_str(),
                    plan.record.plan_id.as_str()
                )));
            }
        }
    }
    let expected = expected_owners.keys().cloned().collect::<BTreeSet<_>>();
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
        validate_tombstone_entry(key, bytes, plans)?;
    }
    Ok(())
}

fn validate_tombstone_entry(
    key: &str,
    bytes: &[u8],
    plans: &BTreeMap<&str, StoredRetentionPlan>,
) -> Result<(), StoreError> {
    let tombstone = parse_record::<StoredTombstone>("retention-tombstones", key, bytes)?;
    let plan = plans
        .get(tombstone.envelope.plan_id.as_str())
        .ok_or_else(|| StoreError::Integrity(format!("tombstone {key} has no plan")))?;
    validate_tombstone_record(key, &tombstone, plan)
}
