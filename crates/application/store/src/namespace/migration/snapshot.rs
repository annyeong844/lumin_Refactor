mod tables;
mod validation;

use std::collections::BTreeMap;
use std::path::Path;

use redb::{Database, ReadableDatabase};

use crate::{StoreError, StoreGeneration, backend_error, io_error};

use self::tables::{read_snapshot, write_snapshot};
use super::super::platform::{EntryAccess, EntryKind, HeldEntry};
use super::super::store_header::{initialize_store, verify_store_header};
use super::super::{NamespaceGuard, entry_exists, require_state_volume};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct LogicalStoreSnapshot {
    sequences: BTreeMap<String, u64>,
    attempt_leases: BTreeMap<String, Vec<u8>>,
    run_catalog: BTreeMap<String, Vec<u8>>,
    pointers: BTreeMap<String, Vec<u8>>,
    gates: BTreeMap<String, Vec<u8>>,
    operations: BTreeMap<String, Vec<u8>>,
    transitions: BTreeMap<String, Vec<u8>>,
    retention_plans: BTreeMap<String, Vec<u8>>,
    retention_operations: BTreeMap<String, Vec<u8>>,
    retention_tombstones: BTreeMap<String, Vec<u8>>,
    run_pins: BTreeMap<String, Vec<u8>>,
}

pub(super) fn read_canonical(
    guard: &NamespaceGuard,
    expected: StoreGeneration,
) -> Result<LogicalStoreSnapshot, StoreError> {
    let database = guard.open_database_for_generation(expected)?;
    let read = database.begin_read()?;
    read_snapshot(&read)
}

pub(super) fn read_private(
    guard: &NamespaceGuard,
    path: &Path,
    expected: StoreGeneration,
) -> Result<LogicalStoreSnapshot, StoreError> {
    let entry = HeldEntry::open(
        path,
        EntryKind::RegularFile,
        EntryAccess::ReadWrite,
        true,
        "private lifecycle migration store",
    )?;
    require_state_volume(
        &entry,
        &guard.state_directory,
        "private lifecycle migration store",
    )?;
    let database = Database::builder()
        .create_file(entry.file().try_clone().map_err(io_error)?)
        .map_err(backend_error)?;
    let observed = verify_store_header(&database, &guard.state.binding)?;
    if observed != expected {
        return Err(StoreError::StoreGenerationChanged { expected, observed });
    }
    let read = database.begin_read().map_err(backend_error)?;
    let snapshot = read_snapshot(&read)?;
    drop(read);
    drop(database);
    entry.sync()?;
    Ok(snapshot)
}

pub(super) fn create_private(
    guard: &NamespaceGuard,
    path: &Path,
    generation: StoreGeneration,
    snapshot: &LogicalStoreSnapshot,
) -> Result<(), StoreError> {
    if entry_exists(path)? {
        return Err(StoreError::Integrity(format!(
            "private lifecycle migration path already exists: {}",
            path.display()
        )));
    }
    let entry = HeldEntry::create_new(path, "private lifecycle migration store")?;
    require_state_volume(
        &entry,
        &guard.state_directory,
        "private lifecycle migration store",
    )?;
    initialize_store(&entry, &guard.state.binding, generation)?;
    let database = Database::builder()
        .create_file(entry.file().try_clone().map_err(io_error)?)
        .map_err(backend_error)?;
    write_snapshot(&database, snapshot)?;
    let read = database.begin_read().map_err(backend_error)?;
    let observed = read_snapshot(&read)?;
    if &observed != snapshot {
        return Err(StoreError::Integrity(
            "private lifecycle migration store changed its logical snapshot".to_owned(),
        ));
    }
    drop(read);
    drop(database);
    entry.sync()?;
    guard.state_directory.sync_directory()
}

impl LogicalStoreSnapshot {
    pub(super) fn validate_external_references(
        &self,
        guard: &NamespaceGuard,
    ) -> Result<(), StoreError> {
        validation::validate_external_references(self, guard)
    }
}
