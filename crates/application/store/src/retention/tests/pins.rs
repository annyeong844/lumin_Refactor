use lumin_evidence::{
    RecordLookup, RetentionExclusionReason, RetentionMutationResult, RetentionPlanRecord,
    RetentionPlanScope, RetentionPlanState,
};
use lumin_model::{PinId, RunId};

use super::*;

#[test]
fn independent_pins_keep_a_run_protected_until_each_is_removed()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::TempDir::new()?;
    let store = open_store(root.path())?;
    let first = publish(&store)?;
    let _latest = publish(&store)?;
    let result = store.prepare_retention_plan(&RetentionPlanRequest {
        scope: run_scope(),
        operation_id: operation("plan-before-pins"),
    })?;
    let stale_plan_id = prepared_plan_id(&result)?;
    let first_pin = store.pin_run(
        &first.run_id,
        &operation("pin-first-release"),
        "release baseline",
    )?;
    let second_pin = store.pin_run(
        &first.run_id,
        &operation("pin-first-investigation"),
        "active investigation",
    )?;

    assert!(matches!(
        store.confirm_retention_plan(&stale_plan_id, &operation("confirm-stale"))?,
        RetentionMutationResult::Stale { .. }
    ));
    assert_eq!(
        store.load_retention_plan(&stale_plan_id)?.state,
        RetentionPlanState::Prepared
    );

    let both_plan = prepare_plan(&store, "plan-with-both-pins")?;
    let mut expected = vec![first_pin.pin_id.clone(), second_pin.pin_id.clone()];
    expected.sort();
    assert_eq!(
        active_pin_ids(&both_plan, &first.run_id),
        Some(expected.as_slice())
    );

    let removed = store.unpin_run(&first_pin.pin_id, &operation("unpin-first-release"))?;
    assert!(!removed.is_active());
    assert!(matches!(
        store.lookup_run_pin(&first_pin.pin_id)?,
        RecordLookup::Live(record) if !record.is_active()
    ));
    let one_plan = prepare_plan(&store, "plan-with-one-pin")?;
    assert_eq!(
        active_pin_ids(&one_plan, &first.run_id),
        Some(std::slice::from_ref(&second_pin.pin_id))
    );

    store.unpin_run(&second_pin.pin_id, &operation("unpin-first-investigation"))?;
    let unpinned_plan = prepare_plan(&store, "plan-after-unpin")?;
    assert!(active_pin_ids(&unpinned_plan, &first.run_id).is_none());
    assert!(unpinned_plan.items.iter().any(|item| {
        item.kind == lumin_evidence::RetentionItemKind::Run
            && item.record_id == first.run_id.as_str()
    }));
    Ok(())
}

fn run_scope() -> RetentionPlanScope {
    RetentionPlanScope::Runs {
        before_unix_millis: 9_000_000_000_000,
    }
}

fn prepare_plan(
    store: &crate::RepositoryStore,
    operation_id: &str,
) -> Result<RetentionPlanRecord, crate::StoreError> {
    let result = store.prepare_retention_plan(&RetentionPlanRequest {
        scope: run_scope(),
        operation_id: operation(operation_id),
    })?;
    let plan_id = prepared_plan_id(&result)
        .map_err(|message| crate::StoreError::Integrity(message.to_owned()))?;
    store.load_retention_plan(&plan_id)
}

fn active_pin_ids<'a>(plan: &'a RetentionPlanRecord, run_id: &RunId) -> Option<&'a [PinId]> {
    plan.exclusions.iter().find_map(|exclusion| {
        if exclusion.record_id != run_id.as_str() {
            return None;
        }
        match &exclusion.reason {
            RetentionExclusionReason::ActivePin { pin_ids } => Some(pin_ids.as_slice()),
            _ => None,
        }
    })
}
