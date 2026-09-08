use crate::namespace::database::tests::{Counts, take_counts};
use crate::{ATTEMPT_LEASES, backend_error};

use super::*;

#[test]
fn attempt_session_reads_one_generation_bound_database_without_application_writes()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let attempt = store.begin_attempt()?;
    store.with_exclusive_lock(|guard| {
        let before = crate::namespace::complete_logical_observation_for_test(guard)?;
        for _ in 0..2 {
            take_counts();
            attempt.validate(guard)?;
            assert_eq!(
                take_counts(),
                Counts {
                    opens: 1,
                    reads: 1,
                    writes: 0,
                    commits: 0
                }
            );
            assert_eq!(
                crate::namespace::complete_logical_observation_for_test(guard)?,
                before
            );
        }
        Ok(())
    })?;
    Ok(())
}

#[test]
fn attempt_session_missing_and_malformed_rows_still_finish_the_original_read()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let attempt = store.begin_attempt()?;
    let original = serde_json::to_vec(&attempt.lease)?;
    store.with_exclusive_lock(|guard| {
        let before = crate::namespace::complete_logical_observation_for_test(guard)?;
        for bytes in [None, Some(b"{malformed".as_slice()), Some(b"{}".as_slice())] {
            put_lease(guard, attempt.attempt_id(), bytes)?;
            let mut finished = false;
            take_counts();
            let result = records::read_session_with_hooks(
                guard,
                attempt.generation(),
                attempt.attempt_id(),
                |_| Ok(()),
                |_| {
                    finished = true;
                    Ok(())
                },
            );
            assert!(result.is_err());
            assert!(finished, "failed row read skipped original-held finishing");
            assert_eq!(
                take_counts(),
                Counts {
                    opens: 1,
                    reads: 1,
                    writes: 0,
                    commits: 0
                }
            );
            guard.require_backend_access()?;
            assert!(attempt.validate(guard).is_err());
            attempt.require_backend_access()?;
            put_lease(guard, attempt.attempt_id(), Some(&original))?;
            attempt.validate(guard)?;
            assert_eq!(
                crate::namespace::complete_logical_observation_for_test(guard)?,
                before
            );
        }
        Ok(())
    })?;
    Ok(())
}

#[test]
fn attempt_session_compares_the_complete_persisted_lease_and_external_lock()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let attempt = store.begin_attempt()?;
    let original = serde_json::to_vec(&attempt.lease)?;
    let mut wrong_owner = attempt.lease.clone();
    wrong_owner.owner_process_id = wrong_owner.owner_process_id.wrapping_add(1);
    let mut wrong_nonce = attempt.lease.clone();
    wrong_nonce.lease_nonce = "a".repeat(32);
    if wrong_nonce.lease_nonce == attempt.lease.lease_nonce {
        wrong_nonce.lease_nonce = "b".repeat(32);
    }
    wrong_nonce.lock_name = format!("attempt-liveness-{}.lock", wrong_nonce.lease_nonce);
    let mut releasing = attempt.lease.clone();
    releasing.state = AttemptLeaseState::Releasing;
    store.with_exclusive_lock(|guard| {
        let before = crate::namespace::complete_logical_observation_for_test(guard)?;
        for changed in [&wrong_owner, &wrong_nonce, &releasing] {
            let bytes = serde_json::to_vec(changed).map_err(crate::serialization_error)?;
            put_lease(guard, attempt.attempt_id(), Some(&bytes))?;
            let result = attempt.validate(guard);
            assert!(matches!(result, Err(StoreError::Integrity(message))
                if message.starts_with("attempt process-liveness lease changed:")));
            put_lease(guard, attempt.attempt_id(), Some(&original))?;
        }
        let lock = attempt
            .lock_file
            .as_ref()
            .ok_or_else(|| StoreError::Integrity("test attempt lock missing".to_owned()))?;
        let original_lock = lock.read_all()?;
        lock.replace_contents(b"corrupted attempt lock")?;
        assert!(attempt.validate(guard).is_err());
        lock.replace_contents(&original_lock)?;
        attempt.validate(guard)?;
        assert_eq!(
            crate::namespace::complete_logical_observation_for_test(guard)?,
            before
        );
        Ok(())
    })?;
    Ok(())
}

#[test]
fn failed_attempt_session_generation_open_forbids_foreign_teardown_reopen()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let mut attempt = store.begin_attempt()?;
    let attempt_id = attempt.attempt_id().clone();
    let original_generation = attempt.generation;
    let authentic_before = store.with_exclusive_lock(|guard| {
        crate::namespace::complete_logical_observation_for_test(guard)
    })?;
    let canonical = root.path().join(".lumin/lifecycle.store");
    let foreign = root.path().join("closed-foreign.store");
    let authentic = root.path().join("held-authentic.store");
    fs::copy(&canonical, &foreign)?;
    let foreign_before = fs::read(&foreign)?;
    let foreign_identity = lumin_inventory::physical_file_identity(&foreign)?;
    let wrong_generation = StoreGeneration::INITIAL
        .checked_next()
        .ok_or_else(|| std::io::Error::other("test generation exhausted"))?;
    attempt.generation = wrong_generation;
    let result = store.with_exclusive_lock(|guard| {
        take_counts();
        let result = attempt.validate(guard);
        assert!(matches!(
            &result,
            Err(StoreError::StoreGenerationChanged { .. })
        ));
        assert!(guard.require_backend_access().is_err());
        assert!(attempt.require_backend_access().is_err());
        fs::rename(&canonical, &authentic).map_err(io_error)?;
        fs::rename(&foreign, &canonical).map_err(io_error)?;
        result
    });
    assert!(matches!(
        result,
        Err(StoreError::StoreGenerationChanged { .. })
    ));
    assert_eq!(
        take_counts(),
        Counts {
            opens: 1,
            reads: 0,
            writes: 0,
            commits: 0
        }
    );
    for _ in 0..2 {
        take_counts();
        assert_rejected_continuation(store.fail_attempt(&mut attempt, "publication rejected"));
        let mut reached_preflight = false;
        assert_rejected_continuation(store.publish_run_with_preflight(&mut attempt, |_| {
            reached_preflight = true;
            Err(StoreError::Integrity(
                "unexpected publication preflight".to_owned(),
            ))
        }));
        assert!(!reached_preflight);
        assert_eq!(take_counts(), Counts::default());
    }
    assert_eq!(
        lumin_inventory::physical_file_identity(&canonical)?,
        foreign_identity
    );
    assert_eq!(fs::read(&canonical)?, foreign_before);
    fs::rename(&canonical, &foreign)?;
    fs::rename(&authentic, &canonical)?;
    // Restoration does not reset authority already rejected by this session.
    attempt.generation = original_generation;
    take_counts();
    assert_rejected_continuation(store.fail_attempt(&mut attempt, "restored namespace"));
    let mut reached_preflight = false;
    assert_rejected_continuation(store.publish_run_with_preflight(&mut attempt, |_| {
        reached_preflight = true;
        Err(StoreError::Integrity(
            "unexpected restored publication preflight".to_owned(),
        ))
    }));
    assert!(!reached_preflight);
    assert_eq!(take_counts(), Counts::default());
    assert_eq!(
        store.with_exclusive_lock(|guard| {
            crate::namespace::complete_logical_observation_for_test(guard)
        })?,
        authentic_before
    );
    drop(attempt);
    let recovered = open_store(root.path())?;
    let latest = recovered.latest_snapshot()?;
    let envelope = latest.latest_attempt.ok_or("recovered attempt missing")?;
    assert_eq!(envelope.attempt_id, attempt_id);
    assert_eq!(envelope.state, AttemptStatus::Interrupted);
    assert!(envelope.run_id.is_none());
    assert!(latest.completed.is_none());
    Ok(())
}

fn assert_rejected_continuation<T>(result: Result<T, StoreError>) {
    assert!(matches!(result, Err(StoreError::Integrity(message))
        if message.starts_with("attempt session rejected lifecycle backend access:")));
}

fn put_lease(
    guard: &NamespaceGuard,
    id: &AttemptId,
    bytes: Option<&[u8]>,
) -> Result<(), StoreError> {
    let database = guard.open_database()?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(ATTEMPT_LEASES).map_err(backend_error)?;
        match bytes {
            Some(bytes) => {
                table.insert(id.as_str(), bytes).map_err(backend_error)?;
            }
            None => {
                table.remove(id.as_str()).map_err(backend_error)?;
            }
        }
    }
    guard.commit(write)
}

fn open_store(root: &std::path::Path) -> Result<RepositoryStore, StoreError> {
    let admission = lumin_inventory::repository_admission(root)
        .map_err(|error| StoreError::Integrity(error.to_string()))?;
    RepositoryStore::open(&admission.canonical_root, &admission.binding)
}
