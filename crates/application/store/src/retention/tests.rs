mod faults;
mod gates;
mod pins;

use lumin_evidence::{
    CapabilityRecord, RecordLookup, RetentionExclusionReason, RetentionItemKind,
    RetentionMutationResult, RetentionPlanScope, RetentionPlanState, RunEvidence,
};
use lumin_model::{CapabilityState, OperationId, RetentionPlanId, RunId};
use tempfile::TempDir;

use super::*;

#[test]
fn run_plan_prunes_only_nonlatest_unpinned_run() -> Result<(), Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    let store = open_store(root.path())?;
    let first = publish(&store)?;
    let second = publish(&store)?;
    let initial_catalog = store.list_runs(None, 100)?;
    let initial_revision = initial_catalog.revision;
    assert_eq!(
        initial_catalog
            .runs
            .iter()
            .map(|run| run.run_id.as_str())
            .collect::<Vec<_>>(),
        vec![second.run_id.as_str(), first.run_id.as_str()]
    );

    let result = store.prepare_retention_plan(&RetentionPlanRequest {
        scope: RetentionPlanScope::Runs {
            before_unix_millis: 9_000_000_000_000,
        },
        operation_id: operation("plan-runs"),
    })?;
    let plan_id = prepared_plan_id(&result)?;
    let plan = store.load_retention_plan(&plan_id)?;
    assert!(plan.items.iter().any(|item| {
        item.kind == RetentionItemKind::Run && item.record_id == first.run_id.as_str()
    }));
    assert!(!plan.items.iter().any(|item| {
        item.kind == RetentionItemKind::Run && item.record_id == second.run_id.as_str()
    }));
    assert!(plan.exclusions.iter().any(|exclusion| {
        exclusion.record_id == second.run_id.as_str()
            && exclusion.reason == RetentionExclusionReason::LatestCompleted
    }));

    let confirm_id = operation("confirm-runs");
    let pruned = store.confirm_retention_plan(&plan_id, &confirm_id)?;
    assert!(matches!(
        pruned,
        RetentionMutationResult::Pruned {
            physical_reclamation_pending: true,
            ..
        }
    ));
    assert!(
        !store
            .load_retention_plan(&plan_id)?
            .physical_reclamation_pending
    );
    assert!(matches!(
        store.lookup_run(&first.run_id)?,
        RecordLookup::Pruned(_)
    ));
    assert!(matches!(
        store.lookup_run(&second.run_id)?,
        RecordLookup::Live(_)
    ));
    let pruned_catalog = store.list_runs(None, 100)?;
    assert!(pruned_catalog.revision > initial_revision);
    assert_eq!(
        pruned_catalog
            .runs
            .iter()
            .map(|run| run.run_id.as_str())
            .collect::<Vec<_>>(),
        vec![second.run_id.as_str()]
    );
    assert_eq!(store.confirm_retention_plan(&plan_id, &confirm_id)?, pruned);
    Ok(())
}

#[test]
fn overlapping_plans_cannot_replace_an_active_retention_owner()
-> Result<(), Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    let store = open_store(root.path())?;
    let first = publish(&store)?;
    let _latest = publish(&store)?;
    let scope = RetentionPlanScope::Runs {
        before_unix_millis: 9_000_000_000_000,
    };
    let first_plan = store.prepare_retention_plan(&RetentionPlanRequest {
        scope: scope.clone(),
        operation_id: operation("plan-owner-first"),
    })?;
    let second_plan = store.prepare_retention_plan(&RetentionPlanRequest {
        scope: scope.clone(),
        operation_id: operation("plan-owner-second"),
    })?;
    let first_plan_id = prepared_plan_id(&first_plan)?;
    let second_plan_id = prepared_plan_id(&second_plan)?;
    let first_confirmation = operation("confirm-owner-first");
    store.with_exclusive_lock(|guard| {
        let result = confirmation::admit_or_resume(guard, &first_plan_id, &first_confirmation)?;
        assert!(matches!(result, RetentionMutationResult::Pruning { .. }));
        Ok(())
    })?;

    assert!(matches!(
        store.confirm_retention_plan(&second_plan_id, &operation("confirm-owner-second"))?,
        RetentionMutationResult::Stale { .. }
    ));
    assert!(matches!(
        store.lookup_run(&first.run_id)?,
        RecordLookup::Pruning(tombstone) if tombstone.plan_id == first_plan_id
    ));

    let later_plan = store.prepare_retention_plan(&RetentionPlanRequest {
        scope,
        operation_id: operation("plan-owner-later"),
    })?;
    let later_plan_id = prepared_plan_id(&later_plan)?;
    let later_plan = store.load_retention_plan(&later_plan_id)?;
    assert!(later_plan.exclusions.iter().any(|exclusion| {
        exclusion.record_id == first.run_id.as_str()
            && exclusion.reason
                == RetentionExclusionReason::RetentionInProgress {
                    plan_id: first_plan_id.clone(),
                }
    }));

    assert!(matches!(
        store.confirm_retention_plan(&first_plan_id, &first_confirmation)?,
        RetentionMutationResult::Pruned { .. }
    ));
    Ok(())
}

#[test]
fn planning_rejects_a_tombstone_without_its_active_owner_plan()
-> Result<(), Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    let store = open_store(root.path())?;
    let first = publish(&store)?;
    let _latest = publish(&store)?;
    let scope = RetentionPlanScope::Runs {
        before_unix_millis: 9_000_000_000_000,
    };
    let plan = store.prepare_retention_plan(&RetentionPlanRequest {
        scope: scope.clone(),
        operation_id: operation("plan-orphan-owner"),
    })?;
    let plan_id = prepared_plan_id(&plan)?;
    store.with_exclusive_lock(|guard| {
        let result =
            confirmation::admit_or_resume(guard, &plan_id, &operation("confirm-orphan-owner"))?;
        assert!(matches!(result, RetentionMutationResult::Pruning { .. }));
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        let key = records::tombstone_key(RetentionItemKind::Run, first.run_id.as_str());
        let mut tombstone = crate::gate::records::read_record::<records::StoredTombstone>(
            &write,
            RETENTION_TOMBSTONES,
            &key,
        )?
        .ok_or_else(|| StoreError::Integrity("run tombstone is missing".to_owned()))?;
        tombstone.envelope.plan_id =
            RetentionPlanId::from_string("retention_plan_missing_owner".to_owned());
        crate::gate::records::write_record(&write, RETENTION_TOMBSTONES, &key, &tombstone)?;
        guard.commit(write)
    })?;

    assert!(matches!(
        store.lookup_run(&first.run_id),
        Err(StoreError::Integrity(message)) if message.contains("has no owner plan")
    ));
    assert!(matches!(
        store.list_runs(None, 100),
        Err(StoreError::Integrity(message)) if message.contains("has no owner plan")
    ));
    assert!(matches!(
        store.prepare_retention_plan(&RetentionPlanRequest {
            scope,
            operation_id: operation("plan-after-orphan-owner"),
        }),
        Err(StoreError::Integrity(message)) if message.contains("has no owner plan")
    ));
    Ok(())
}

#[test]
fn run_catalog_cursor_is_bounded_and_repository_scoped() -> Result<(), Box<dyn std::error::Error>> {
    let first_root = TempDir::new()?;
    let first_store = open_store(first_root.path())?;
    let older = publish(&first_store)?;
    let newer = publish(&first_store)?;
    assert!(matches!(
        first_store.list_runs(None, 101),
        Err(StoreError::RunCatalogPageSize {
            requested: 101,
            max: 100
        })
    ));
    let first_page = first_store.list_runs(None, 1)?;
    assert_eq!(first_page.total, 2);
    assert_eq!(first_page.runs.len(), 1);
    assert!(first_page.truncated);
    assert_eq!(first_page.runs[0].run_id, newer.run_id);
    let anchor = &first_page.runs[0];
    let cursor = crate::RunCatalogCursor {
        repository_id: first_page.repository_id.clone(),
        revision: first_page.revision,
        attempt_id: anchor.attempt_id.clone(),
        run_id: anchor.run_id.clone(),
        sequence: anchor.sequence,
    };
    let second_page = first_store.list_runs(Some(&cursor), 1)?;
    assert_eq!(second_page.total, 2);
    assert_eq!(second_page.runs.len(), 1);
    assert!(!second_page.truncated);
    assert_eq!(second_page.runs[0].run_id, older.run_id);

    let second_root = TempDir::new()?;
    let second_store = open_store(second_root.path())?;
    let _second_run = publish(&second_store)?;
    assert!(matches!(
        second_store.list_runs(Some(&cursor), 1),
        Err(StoreError::RunCatalogScopeMismatch)
    ));

    let _newest = publish(&first_store)?;
    assert!(matches!(
        first_store.list_runs(Some(&cursor), 1),
        Err(StoreError::RunCatalogRevisionChanged { .. })
    ));
    Ok(())
}

#[test]
fn pruning_commit_recovers_after_store_reopen() -> Result<(), Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    let store = open_store(root.path())?;
    let first = publish(&store)?;
    let _latest = publish(&store)?;
    let result = store.prepare_retention_plan(&RetentionPlanRequest {
        scope: RetentionPlanScope::Runs {
            before_unix_millis: 9_000_000_000_000,
        },
        operation_id: operation("plan-crash"),
    })?;
    let plan_id = prepared_plan_id(&result)?;
    let confirm_id = operation("confirm-crash");

    store.with_exclusive_lock(|guard| {
        let result = confirmation::admit_or_resume(guard, &plan_id, &confirm_id)?;
        assert!(matches!(result, RetentionMutationResult::Pruning { .. }));
        Ok(())
    })?;
    drop(store);

    let reopened = open_store(root.path())?;
    let result = reopened.confirm_retention_plan(&plan_id, &confirm_id)?;
    assert!(matches!(result, RetentionMutationResult::Pruned { .. }));
    assert!(matches!(
        reopened.lookup_run(&first.run_id)?,
        RecordLookup::Pruned(_)
    ));
    Ok(())
}

#[test]
fn pruning_admission_commits_without_moving_payloads() -> Result<(), Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    let store = open_store(root.path())?;
    let _first = publish(&store)?;
    let _latest = publish(&store)?;
    let result = store.prepare_retention_plan(&RetentionPlanRequest {
        scope: RetentionPlanScope::Runs {
            before_unix_millis: 9_000_000_000_000,
        },
        operation_id: operation("plan-admission"),
    })?;
    let plan_id = prepared_plan_id(&result)?;
    let confirm_id = operation("confirm-admission");
    store.with_exclusive_lock(|guard| {
        let result = confirmation::admit_or_resume(guard, &plan_id, &confirm_id)?;
        assert!(matches!(result, RetentionMutationResult::Pruning { .. }));
        Ok(())
    })?;
    assert_eq!(
        store.load_retention_plan(&plan_id)?.state,
        RetentionPlanState::Pruning
    );
    let stored = store.with_exclusive_lock(|guard| {
        confirmation::load_plan_for_resume(guard, &plan_id, &confirm_id)
    })?;
    let progress = stored
        .progress
        .as_ref()
        .ok_or("missing retention progress")?;
    assert!(!progress.moves.is_empty());
    assert!(progress.trash_directory.is_some());
    assert_eq!(
        stored.record.recoverable_state,
        Some(lumin_evidence::RetentionRecoverableState::MovingPayloads)
    );
    let mut corrupt = stored.clone();
    corrupt
        .progress
        .as_mut()
        .and_then(|progress| progress.moves.first_mut())
        .ok_or("missing retention move")?
        .trash_child = "../escape".to_owned();
    assert!(matches!(
        records::validate_plan(&corrupt),
        Err(StoreError::Integrity(message)) if message.contains("one normal path component")
    ));
    Ok(())
}

fn open_store(root: &std::path::Path) -> Result<crate::RepositoryStore, crate::StoreError> {
    let admission = lumin_inventory::repository_admission(root)
        .map_err(|error| crate::StoreError::Integrity(error.to_string()))?;
    crate::RepositoryStore::open(&admission.canonical_root, &admission.binding)
}

fn publish(store: &crate::RepositoryStore) -> Result<crate::PublishedRun, crate::StoreError> {
    let attempt = store.begin_attempt()?;
    store.publish_run(&attempt, &evidence())
}

fn evidence() -> RunEvidence {
    RunEvidence {
        schema_version: "lumin-run-evidence.v1".to_owned(),
        capabilities: vec![CapabilityRecord {
            capability_id: "dead-code.v1".to_owned(),
            state: CapabilityState::Complete,
        }],
        resolution_profiles: Vec::new(),
        findings: Vec::new(),
        limitations: Vec::new(),
    }
}

fn operation(value: &str) -> OperationId {
    OperationId::from_string(value.to_owned())
}

fn prepared_plan_id(result: &RetentionMutationResult) -> Result<RetentionPlanId, &'static str> {
    match result {
        RetentionMutationResult::Prepared { plan_id, .. } => Ok(plan_id.clone()),
        _ => Err("retention plan was not prepared"),
    }
}

fn admit_run_pruning(
    store: &crate::RepositoryStore,
    suffix: &str,
) -> Result<(RunId, RetentionPlanId, OperationId), crate::StoreError> {
    let first = publish(store)?;
    let _latest = publish(store)?;
    let plan = store.prepare_retention_plan(&RetentionPlanRequest {
        scope: RetentionPlanScope::Runs {
            before_unix_millis: 9_000_000_000_000,
        },
        operation_id: operation(&format!("plan-{suffix}")),
    })?;
    let plan_id =
        prepared_plan_id(&plan).map_err(|message| StoreError::Integrity(message.to_owned()))?;
    let confirm_id = operation(&format!("confirm-{suffix}"));
    store.with_exclusive_lock(|guard| {
        let result = confirmation::admit_or_resume(guard, &plan_id, &confirm_id)?;
        if !matches!(result, RetentionMutationResult::Pruning { .. }) {
            return Err(StoreError::Integrity(
                "retention confirmation did not enter pruning".to_owned(),
            ));
        }
        Ok(())
    })?;
    Ok((first.run_id, plan_id, confirm_id))
}

fn first_move_paths(
    store: &crate::RepositoryStore,
    plan_id: &RetentionPlanId,
    operation_id: &OperationId,
) -> Result<(std::path::PathBuf, std::path::PathBuf), crate::StoreError> {
    store.with_exclusive_lock(|guard| {
        let plan = confirmation::load_plan_for_resume(guard, plan_id, operation_id)?;
        let progress = plan
            .progress
            .as_ref()
            .ok_or_else(|| StoreError::Integrity("retention plan has no progress".to_owned()))?;
        let movement = progress.moves.first().ok_or_else(|| {
            StoreError::Integrity("retention plan has no payload move".to_owned())
        })?;
        let source = guard.managed_child_path(movement.source_parent, &movement.source_child)?;
        let trash = guard
            .managed_child_path(
                crate::namespace::records::ManagedStateParentKind::Trash,
                plan_id.as_str(),
            )?
            .join(&movement.trash_child);
        Ok((source, trash))
    })
}

fn assert_pruning_truth(
    store: &crate::RepositoryStore,
    plan_id: &RetentionPlanId,
    run_id: &RunId,
) -> Result<(), crate::StoreError> {
    assert_eq!(
        store.load_retention_plan(plan_id)?.state,
        RetentionPlanState::Pruning
    );
    assert!(matches!(
        store.lookup_run(run_id)?,
        RecordLookup::Pruning(_)
    ));
    Ok(())
}
