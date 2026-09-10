use redb::ReadableTable;
use std::cell::Cell;

use super::*;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Counts {
    pub(crate) opens: usize,
    pub(crate) reads: usize,
    pub(crate) writes: usize,
    pub(crate) commits: usize,
}

thread_local! {
    static COUNTS: Cell<Counts> = Cell::new(Counts::default());
}

pub(super) fn observe(update: impl FnOnce(&mut Counts)) {
    COUNTS.with(|cell| {
        let mut counts = cell.get();
        update(&mut counts);
        cell.set(counts);
    });
}

pub(crate) fn take_counts() -> Counts {
    COUNTS.with(|counts| counts.replace(Counts::default()))
}

#[test]
fn attempt_session_finishing_rechecks_receipts_after_its_final_turn()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let admission = lumin_inventory::repository_admission(root.path())?;
    let store = crate::RepositoryStore::open(&admission.canonical_root, &admission.binding)?;
    let result = store.with_exclusive_lock(|guard| {
        let database = guard.open_database_for_generation(StoreGeneration::INITIAL)?;
        let result = database.finish_attempt_session_read(|| {
            // Deliberate corruption after the first complete proof. Bypass the
            // application commit's receipt refresh, using this same backend.
            let write = database.database.begin_write().map_err(backend_error)?;
            {
                let mut headers = write
                    .open_table(redb::TableDefinition::<&str, &[u8]>::new("store-header"))
                    .map_err(backend_error)?;
                let bytes = headers
                    .get("namespace")
                    .map_err(backend_error)?
                    .ok_or_else(|| StoreError::Integrity("missing test header".to_owned()))?
                    .value()
                    .to_vec();
                let header: serde_json::Value =
                    serde_json::from_slice(&bytes).map_err(crate::serialization_error)?;
                let receipt = header["validationReceiptSetId"].as_str().ok_or_else(|| {
                    StoreError::Integrity("test header omitted receipt identity".to_owned())
                })?;
                let encoded = std::str::from_utf8(&bytes).map_err(|error| {
                    StoreError::Integrity(format!("invalid test header: {error}"))
                })?;
                let bytes = encoded.replace(receipt, &"0".repeat(64)).into_bytes();
                headers
                    .insert("namespace", bytes.as_slice())
                    .map_err(backend_error)?;
            }
            write.commit().map_err(backend_error)
        });
        assert!(
            matches!(&result, Err(StoreError::Integrity(message))
            if message.contains("receipt")),
            "{result:?}"
        );
        assert!(guard.require_backend_access().is_err());
        assert!(guard.open_database().is_err());
        result
    });
    assert!(result.is_err());
    Ok(())
}

#[test]
fn latest_derivation_finishing_rechecks_generation_and_receipts_after_its_final_turn()
-> Result<(), Box<dyn std::error::Error>> {
    for generation_fault in [false, true] {
        let root = tempfile::tempdir()?;
        let admission = lumin_inventory::repository_admission(root.path())?;
        let store = crate::RepositoryStore::open(&admission.canonical_root, &admission.binding)?;
        let result = store.with_exclusive_lock(|guard| {
            let database = guard.open_database()?;
            let mut injected = Vec::new();
            take_counts();
            let result = database.finish_latest_derivation_read(|| {
                // This raw owner fixture changes only the header at the exact
                // post-proof turn, through the original held backend.
                let write = database.database.begin_write().map_err(backend_error)?;
                {
                    let mut table = write.open_table(redb::TableDefinition::<&str, &[u8]>::new("store-header")).map_err(backend_error)?;
                    let bytes = table.get("namespace").map_err(backend_error)?.ok_or_else(|| StoreError::Integrity("missing test header".to_owned()))?.value().to_vec();
                    let header: serde_json::Value = serde_json::from_slice(&bytes).map_err(crate::serialization_error)?;
                    let encoded = std::str::from_utf8(&bytes).map_err(|error| StoreError::Integrity(error.to_string()))?;
                    injected = if generation_fault {
                        let generation = header["generation"].as_u64().ok_or_else(|| StoreError::Integrity("missing test generation".to_owned()))?;
                        encoded.replace(&format!("\"generation\":{generation}"), &format!("\"generation\":{}", generation + 1)).into_bytes()
                    } else {
                        encoded.replace(header["validationReceiptSetId"].as_str().ok_or_else(|| StoreError::Integrity("missing test receipt identity".to_owned()))?, &"0".repeat(64)).into_bytes()
                    };
                    assert_ne!(injected, bytes);
                    table.insert("namespace", injected.as_slice()).map_err(backend_error)?;
                }
                write.commit().map_err(backend_error)
            },
                #[cfg(feature = "audit-lifecycle-test-profile")]
                None,
            );
            if generation_fault {
                assert!(matches!(&result, Err(StoreError::StoreGenerationChanged { .. })), "{result:?}");
            } else {
                assert!(matches!(&result, Err(StoreError::Integrity(message)) if message.contains("receipt")), "{result:?}");
            }
            assert_eq!(take_counts(), Counts::default(), "finishing must not reopen or admit an application transaction");
            assert!(guard.require_backend_access().is_err());
            assert!(guard.open_database().is_err());
            let read = database.database.begin_read().map_err(backend_error)?;
            let table = read.open_table(redb::TableDefinition::<&str, &[u8]>::new("store-header")).map_err(backend_error)?;
            assert_eq!(table.get("namespace").map_err(backend_error)?.ok_or_else(|| StoreError::Integrity("injected header disappeared".to_owned()))?.value(), injected);
            result
        });
        assert!(result.is_err());
    }
    Ok(())
}
