use std::collections::BTreeMap;
use std::path::Path;

use lumin_evidence::{
    CapabilityRecord, RUN_EVIDENCE_CAPABILITY_IDS, RUN_EVIDENCE_SCHEMA_VERSION, RunEvidence,
};
use lumin_model::CapabilityState;
use serde_json::Value;

use super::*;

#[test]
fn unchanged_empty_index_aborts_without_creating_or_committing_a_table()
-> Result<(), Box<dyn std::error::Error>> {
    for existing_table in [false, true] {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        store.with_exclusive_lock(|guard| {
            if existing_table {
                seed_index(guard, &LatestPointer::default())?;
            }
            let before = observation(guard)?;
            assert_eq!(
                before["tableNames"]
                    .as_array()
                    .ok_or_else(|| StoreError::Integrity("missing table inventory".to_owned()))?
                    .iter()
                    .any(|name| name == "pointers"),
                existing_table
            );
            for _ in 0..2 {
                let mut commits = 0;
                sync_index_with_commit(guard, &LatestPointer::default(), |write| {
                    commits += 1;
                    guard.commit(write)
                })?;
                assert_eq!(commits, 0);
                assert_eq!(observation(guard)?, before);
            }
            Ok(())
        })?;
    }
    Ok(())
}

#[test]
fn unchanged_populated_index_aborts_without_changing_any_logical_state()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = populated_store(root.path())?;
    store.with_exclusive_lock(|guard| {
        let before = observation(guard)?;
        let expected = pointer(Some(3), Some(2));
        assert_eq!(before["records"]["pointers"], pointer_rows(&expected));
        let mut commits = 0;
        sync_index_with_commit(guard, &expected, |write| {
            commits += 1;
            guard.commit(write)
        })?;
        assert_eq!(commits, 0);
        assert_eq!(observation(guard)?, before);
        Ok(())
    })?;
    Ok(())
}

#[test]
fn changed_index_commits_both_exact_fields_once_and_preserves_other_rows()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = populated_store(root.path())?;
    for (old, expected) in [
        (pointer(Some(2), Some(2)), pointer(Some(3), Some(2))),
        (pointer(Some(3), Some(1)), pointer(Some(3), Some(2))),
        (pointer(None, Some(2)), pointer(Some(3), Some(2))),
        (pointer(Some(3), None), pointer(Some(3), Some(2))),
        (pointer(None, None), pointer(Some(3), Some(2))),
        (pointer(Some(3), Some(2)), pointer(None, Some(2))),
        (pointer(Some(3), Some(2)), pointer(Some(3), None)),
        (pointer(Some(3), Some(2)), pointer(None, None)),
    ] {
        store.with_exclusive_lock(|guard| {
            seed_index(guard, &old)?;
            let mut expected_observation = observation(guard)?;
            expected_observation["records"]["pointers"] = pointer_rows(&expected);
            let mut commits = 0;
            sync_index_with_commit(guard, &expected, |write| {
                commits += 1;
                guard.commit(write)
            })?;
            assert_eq!(commits, 1);
            assert_eq!(observation(guard)?, expected_observation);
            let mut repeated_commits = 0;
            sync_index_with_commit(guard, &expected, |write| {
                repeated_commits += 1;
                guard.commit(write)
            })?;
            assert_eq!(repeated_commits, 0);
            assert_eq!(observation(guard)?, expected_observation);
            Ok(())
        })?;
    }
    Ok(())
}

#[test]
fn failure_before_required_index_commit_does_not_publish_either_field()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = populated_store(root.path())?;
    store.with_exclusive_lock(|guard| {
        seed_index(guard, &pointer(Some(1), Some(1)))?;
        let before = observation(guard)?;
        let mut commits = 0;
        let result = sync_index_with_commit(guard, &pointer(Some(3), Some(2)), |_write| {
            commits += 1;
            Err(StoreError::Integrity(
                "injected before real commit".to_owned(),
            ))
        });
        assert!(matches!(result, Err(StoreError::Integrity(message))
            if message == "injected before real commit"));
        assert_eq!(commits, 1);
        assert_eq!(observation(guard)?, before);
        Ok(())
    })?;
    Ok(())
}

#[test]
fn rejected_backend_guard_cannot_reopen_commit_or_turn_a_caught_error_into_success()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let before = store.with_exclusive_lock(observation)?;
    let result = store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        {
            let mut sequences = write.open_table(crate::SEQUENCES).map_err(backend_error)?;
            sequences.insert("attempt", 12).map_err(backend_error)?;
        }
        guard.reject_backend_access();
        assert!(guard.open_database().is_err());
        assert!(database.begin_read().is_err());
        assert!(guard.commit(write).is_err());
        assert!(database.begin_write().is_err());
        // An internal caller must not be able to swallow rejection and return success.
        Ok(())
    });
    assert!(matches!(result, Err(StoreError::Integrity(message))
        if message == "lifecycle backend access was rejected for this namespace guard"));
    assert_eq!(store.with_exclusive_lock(observation)?, before);
    store.with_exclusive_lock(|guard| sync_index(guard, &LatestPointer::default()))?;
    assert_eq!(store.with_exclusive_lock(observation)?, before);
    Ok(())
}

fn observation(guard: &NamespaceGuard) -> Result<Value, StoreError> {
    let bytes = crate::namespace::complete_logical_observation_for_test(guard)?;
    serde_json::from_slice(&bytes).map_err(crate::serialization_error)
}

fn seed_index(guard: &NamespaceGuard, pointer: &LatestPointer) -> Result<(), StoreError> {
    let database = guard.open_database()?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(POINTERS).map_err(backend_error)?;
        for key in ["latest-attempt", "latest-completed"] {
            table.remove(key).map_err(backend_error)?;
        }
        if let Some(attempt) = &pointer.latest_attempt {
            table
                .insert("latest-attempt", attempt.attempt_id.as_str().as_bytes())
                .map_err(backend_error)?;
        }
        if let Some(completed) = &pointer.latest_completed {
            table
                .insert("latest-completed", completed.run_id.as_str().as_bytes())
                .map_err(backend_error)?;
        }
    }
    guard.commit(write)
}

fn pointer_rows(pointer: &LatestPointer) -> Value {
    let mut rows = BTreeMap::new();
    if let Some(attempt) = &pointer.latest_attempt {
        rows.insert("latest-attempt", attempt.attempt_id.as_str().as_bytes());
    }
    if let Some(completed) = &pointer.latest_completed {
        rows.insert("latest-completed", completed.run_id.as_str().as_bytes());
    }
    serde_json::json!(rows)
}

fn pointer(attempt: Option<u64>, completed: Option<u64>) -> LatestPointer {
    LatestPointer {
        schema_version: LATEST_SCHEMA.to_owned(),
        latest_attempt: attempt.map(|sequence| LatestAttemptPointer {
            attempt_id: AttemptId::from_string(format!("attempt_{sequence:016x}")),
            sequence,
            status: if sequence == 3 {
                AttemptStatus::Failed
            } else {
                AttemptStatus::Completed
            },
        }),
        latest_completed: completed.map(|sequence| LatestCompletedPointer {
            run_id: RunId::from_string(format!("run_{sequence:016x}")),
            sequence,
        }),
    }
}

fn open_store(root: &Path) -> Result<RepositoryStore, StoreError> {
    let admission = lumin_inventory::repository_admission(root)
        .map_err(|error| StoreError::Integrity(error.to_string()))?;
    RepositoryStore::open(&admission.canonical_root, &admission.binding)
}

fn populated_store(root: &Path) -> Result<RepositoryStore, StoreError> {
    let store = open_store(root)?;
    let evidence = RunEvidence {
        schema_version: RUN_EVIDENCE_SCHEMA_VERSION.to_owned(),
        capabilities: RUN_EVIDENCE_CAPABILITY_IDS
            .into_iter()
            .map(|id| CapabilityRecord {
                capability_id: id.to_owned(),
                state: if matches!(id, "sfc/svelte.v1" | "sfc/astro.v1") {
                    CapabilityState::Unavailable
                } else {
                    CapabilityState::Complete
                },
            })
            .collect(),
        resolution_profiles: Vec::new(),
        source_classifications: Vec::new(),
        source_contexts: Vec::new(),
        source_observations: Vec::new(),
        dependency_owners: Vec::new(),
        resolutions: Vec::new(),
        metrics: Default::default(),
        findings: Vec::new(),
        limitations: Vec::new(),
    };
    for _ in 0..2 {
        let mut attempt = store.begin_attempt()?;
        store.publish_run(&mut attempt, &evidence, |_| Ok(()))?;
    }
    let mut attempt = store.begin_attempt()?;
    store.fail_attempt(
        &mut attempt,
        "latest failed attempt preserves completed run",
    )?;
    drop(attempt);
    Ok(store)
}
