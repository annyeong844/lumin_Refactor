use super::*;
use crate::namespace::database::tests::{Counts, take_counts};

#[test]
fn empty_latest_derivation_skips_only_the_second_backend_and_write()
-> Result<(), Box<dyn std::error::Error>> {
    for existing_table in [false, true] {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        store.with_exclusive_lock(|guard| {
            if existing_table {
                seed_index(guard, &LatestPointer::default())?;
            }
            let before = observation(guard)?;
            for _ in 0..2 {
                take_counts();
                ensure_profiled(
                    &store,
                    guard,
                    #[cfg(feature = "audit-lifecycle-test-profile")]
                    None,
                )?;
                assert_eq!(
                    take_counts(),
                    Counts {
                        opens: 1,
                        reads: 1,
                        writes: 0,
                        commits: 0
                    }
                );
                assert_eq!(observation(guard)?, before);
                assert!(!store.state_dir.join(LATEST_NAME).exists());
                assert!(!store.state_dir.join("latest.json.pending").exists());
            }
            Ok(())
        })?;
    }
    Ok(())
}

#[test]
fn canonical_empty_latest_document_retains_the_index_abort()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    store.with_exclusive_lock(|guard| {
        files::write_json(
            &store.state_dir.join(LATEST_NAME),
            guard.state_directory_entry(),
            "latest pointer",
            &LatestPointer::default(),
        )?;
        let before = observation(guard)?;
        let document = std::fs::read(store.state_dir.join(LATEST_NAME)).map_err(crate::io_error)?;
        take_counts();
        ensure_profiled(
            &store,
            guard,
            #[cfg(feature = "audit-lifecycle-test-profile")]
            None,
        )?;
        assert_eq!(
            take_counts(),
            Counts {
                opens: 1,
                reads: 0,
                writes: 1,
                commits: 0
            }
        );
        assert_eq!(observation(guard)?, before);
        assert_eq!(
            std::fs::read(store.state_dir.join(LATEST_NAME)).map_err(crate::io_error)?,
            document
        );
        Ok(())
    })?;
    Ok(())
}

#[test]
fn nonempty_latest_derivation_preserves_exact_pointer_fields()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = populated_store(root.path())?;
    store.with_exclusive_lock(|guard| {
        for expected in [
            pointer(Some(3), None),
            pointer(None, Some(2)),
            pointer(Some(3), Some(2)),
        ] {
            seed_index(guard, &expected)?;
            let before = observation(guard)?;
            take_counts();
            let derived = derive_latest_with_hooks(
                &store,
                guard,
                |_| Ok(()),
                |_| {
                    Err(StoreError::Integrity(
                        "nonempty read reached the empty return hook".to_owned(),
                    ))
                },
                #[cfg(feature = "audit-lifecycle-test-profile")]
                None,
            )?;
            assert!(!derived.empty_index);
            assert_eq!(derived.latest, expected);
            assert_eq!(
                take_counts(),
                Counts {
                    opens: 1,
                    reads: 1 + usize::from(expected.latest_completed.is_some()),
                    writes: 0,
                    commits: 0
                }
            );
            assert_eq!(observation(guard)?, before);
        }
        seed_index(guard, &pointer(Some(3), Some(2)))
    })?;
    Ok(())
}

#[test]
fn invalid_latest_index_never_becomes_empty_or_poisoned_without_a_failed_proof()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    store.with_exclusive_lock(|guard| {
        for (key, bytes, diagnostic) in [
            ("opaque", b"opaque".as_slice(), "unknown latest pointer index key"),
            ("latest-attempt", b"\xff".as_slice(), "legacy latest-attempt"),
            ("latest-completed", b"\xff".as_slice(), "legacy latest-completed"),
        ] {
            seed_index(guard, &LatestPointer::default())?;
            let before = observation(guard)?;
            {
                let database = guard.open_database()?;
                let write = database.begin_write()?;
                write.open_table(POINTERS).map_err(backend_error)?.insert(key, bytes).map_err(backend_error)?;
                guard.commit(write)?;
            }
            take_counts();
            let result = derive_latest_with_hooks(&store, guard, |_| Ok(()), |_| Err(StoreError::Integrity("invalid read reached the empty return hook".to_owned())),
                #[cfg(feature = "audit-lifecycle-test-profile")]
                None,
            );
            assert!(matches!(result, Err(StoreError::Integrity(message)) if message.contains(diagnostic)));
            assert_eq!(take_counts(), Counts { opens: 1, reads: 1, writes: 0, commits: 0 });
            guard.require_backend_access()?;
            {
                let database = guard.open_database()?;
                let read = database.begin_read()?;
                let table = read.open_table(POINTERS).map_err(backend_error)?;
                assert_eq!(table.len().map_err(backend_error)?, 1);
                assert_eq!(table.get(key).map_err(backend_error)?.ok_or_else(|| StoreError::Integrity("injected pointer disappeared".to_owned()))?.value(), bytes);
            }
            {
                let database = guard.open_database()?;
                let write = database.begin_write()?;
                write.open_table(POINTERS).map_err(backend_error)?.remove(key).map_err(backend_error)?;
                guard.commit(write)?;
            }
            assert_eq!(observation(guard)?, before);
        }
        Ok(())
    })?;
    Ok(())
}

#[test]
fn latest_read_error_is_finished_and_preserved_without_poisoning_the_guard()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    store.with_exclusive_lock(|guard| {
        let before = observation(guard)?;
        take_counts();
        let result = derive_latest_with_hooks(&store, guard,
            |_| Err(StoreError::Integrity("forced latest read error".to_owned())),
            |_| Err(StoreError::Integrity("failed read reached the empty return hook".to_owned())),
            #[cfg(feature = "audit-lifecycle-test-profile")]
            None,
        );
        assert!(matches!(result, Err(StoreError::Integrity(message)) if message == "forced latest read error"));
        assert_eq!(take_counts(), Counts { opens: 1, reads: 0, writes: 0, commits: 0 });
        guard.require_backend_access()?;
        assert_eq!(observation(guard)?, before);
        let derived = derive_latest_with_hooks(&store, guard, |_| Ok(()), |_| Ok(()),
            #[cfg(feature = "audit-lifecycle-test-profile")]
            None,
        )?;
        assert!(derived.empty_index);
        Ok(())
    })?;
    Ok(())
}

#[test]
fn latest_arrivals_are_preserved_at_both_read_boundaries() -> Result<(), Box<dyn std::error::Error>>
{
    for after_open in [false, true] {
        for name in ["latest.json", "latest.json.pending"] {
            let root = tempfile::tempdir()?;
            let store = open_store(root.path())?;
            store.with_exclusive_lock(|guard| {
                let before = observation(guard)?;
                let path = store.state_dir.join(name);
                let insert = || std::fs::write(&path, b"unowned arrival").map_err(crate::io_error);
                let result = derive_latest_with_hooks(&store, guard,
                    |_| if after_open { insert() } else { Ok(()) },
                    |_| if after_open { Ok(()) } else { insert() },
                    #[cfg(feature = "audit-lifecycle-test-profile")]
                    None,
                );
                assert!(matches!(result, Err(StoreError::Integrity(message)) if message.contains("latest pointer appeared")));
                assert_eq!(std::fs::read(&path).map_err(crate::io_error)?, b"unowned arrival");
                assert_eq!(observation(guard)?, before);
                std::fs::remove_file(path).map_err(crate::io_error)?;
                Ok(())
            })?;
        }
    }
    Ok(())
}

#[cfg(feature = "audit-lifecycle-test-profile")]
#[test]
fn audit_lifecycle_empty_latest_has_one_read_tail_and_no_index_write()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::audit_lifecycle_profile::LifecycleProfiler;
    use lumin_model::audit_lifecycle_diagnostic::{AuditLifecycleContext, AuditLifecycleCost};
    for existing_table in [false, true] {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        store.with_exclusive_lock(|guard| {
            if existing_table {
                seed_index(guard, &LatestPointer::default())?;
            }
            let before = observation(guard)?;
            let mut recorder = LifecycleProfiler::new(AuditLifecycleContext::OpenRecoveryLatest);
            ensure_profiled(&store, guard, Some(&mut recorder))?;
            let row = recorder.finish().map_err(StoreError::Integrity)?;
            for cost in AuditLifecycleCost::ALL {
                let count = row.costs[cost as usize].calls;
                match cost {
                    AuditLifecycleCost::NamespaceValidation
                    | AuditLifecycleCost::StoreValidation => assert!(count > 0),
                    AuditLifecycleCost::StoreHandleOpen
                    | AuditLifecycleCost::BackendOpen
                    | AuditLifecycleCost::ReadAdmission
                    | AuditLifecycleCost::DatabaseReturnTail => assert_eq!(count, 1),
                    _ => assert_eq!(count, 0),
                }
            }
            assert_eq!(observation(guard)?, before);
            Ok(())
        })?;
    }
    Ok(())
}
