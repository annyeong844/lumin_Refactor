use std::collections::{BTreeMap, BTreeSet};

use redb::{
    Database, ReadTransaction, ReadableTable, TableDefinition, TableError, TableHandle,
    WriteTransaction,
};

use crate::gate::{GATES, OPERATIONS, TRANSITIONS};
use crate::retention::{RETENTION_OPERATIONS, RETENTION_PLANS, RETENTION_TOMBSTONES, RUN_PINS};
use crate::{POINTERS, RUN_CATALOG, SEQUENCES, StoreError, backend_error};

use super::super::super::store_header::STORE_HEADER_TABLE_NAME;
use super::LogicalStoreSnapshot;
use super::validation::validate_referential_closure;

const KNOWN_TABLES: [&str; 11] = [
    "gates",
    "operations",
    "pointers",
    "run-catalog",
    "run-pins",
    "sequences",
    STORE_HEADER_TABLE_NAME,
    "worktree-transitions",
    "retention-operations",
    "retention-plans",
    "retention-tombstones",
];

pub(super) fn read_snapshot(read: &ReadTransaction) -> Result<LogicalStoreSnapshot, StoreError> {
    validate_table_inventory(read)?;
    let snapshot = LogicalStoreSnapshot {
        sequences: read_u64_table(read, SEQUENCES)?,
        run_catalog: read_bytes_table(read, RUN_CATALOG)?,
        pointers: read_bytes_table(read, POINTERS)?,
        gates: read_bytes_table(read, GATES)?,
        operations: read_bytes_table(read, OPERATIONS)?,
        transitions: read_bytes_table(read, TRANSITIONS)?,
        retention_plans: read_bytes_table(read, RETENTION_PLANS)?,
        retention_operations: read_bytes_table(read, RETENTION_OPERATIONS)?,
        retention_tombstones: read_bytes_table(read, RETENTION_TOMBSTONES)?,
        run_pins: read_bytes_table(read, RUN_PINS)?,
    };
    validate_referential_closure(&snapshot)?;
    Ok(snapshot)
}

pub(super) fn write_snapshot(
    database: &Database,
    snapshot: &LogicalStoreSnapshot,
) -> Result<(), StoreError> {
    let write = database.begin_write().map_err(backend_error)?;
    write_u64_table(&write, SEQUENCES, &snapshot.sequences)?;
    write_bytes_table(&write, RUN_CATALOG, &snapshot.run_catalog)?;
    write_bytes_table(&write, POINTERS, &snapshot.pointers)?;
    write_bytes_table(&write, GATES, &snapshot.gates)?;
    write_bytes_table(&write, OPERATIONS, &snapshot.operations)?;
    write_bytes_table(&write, TRANSITIONS, &snapshot.transitions)?;
    write_bytes_table(&write, RETENTION_PLANS, &snapshot.retention_plans)?;
    write_bytes_table(&write, RETENTION_OPERATIONS, &snapshot.retention_operations)?;
    write_bytes_table(&write, RETENTION_TOMBSTONES, &snapshot.retention_tombstones)?;
    write_bytes_table(&write, RUN_PINS, &snapshot.run_pins)?;
    write.commit().map_err(backend_error)
}

fn validate_table_inventory(read: &ReadTransaction) -> Result<(), StoreError> {
    let observed = read
        .list_tables()
        .map_err(backend_error)?
        .map(|table| table.name().to_owned())
        .collect::<BTreeSet<_>>();
    let known = KNOWN_TABLES
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    let unknown = observed.difference(&known).cloned().collect::<Vec<_>>();
    if !unknown.is_empty() {
        return Err(StoreError::Integrity(format!(
            "lifecycle store contains unknown tables: {}",
            unknown.join(", ")
        )));
    }
    if read
        .list_multimap_tables()
        .map_err(backend_error)?
        .next()
        .is_some()
    {
        return Err(StoreError::Integrity(
            "lifecycle store contains unsupported multimap tables".to_owned(),
        ));
    }
    Ok(())
}

fn read_u64_table(
    read: &ReadTransaction,
    definition: TableDefinition<'static, &str, u64>,
) -> Result<BTreeMap<String, u64>, StoreError> {
    let table = match read.open_table(definition) {
        Ok(table) => table,
        Err(TableError::TableDoesNotExist(_)) => return Ok(BTreeMap::new()),
        Err(error) => return Err(backend_error(error)),
    };
    let mut rows = BTreeMap::new();
    for item in table.iter().map_err(backend_error)? {
        let (key, value) = item.map_err(backend_error)?;
        rows.insert(key.value().to_owned(), value.value());
    }
    Ok(rows)
}

fn read_bytes_table(
    read: &ReadTransaction,
    definition: TableDefinition<'static, &str, &[u8]>,
) -> Result<BTreeMap<String, Vec<u8>>, StoreError> {
    let table = match read.open_table(definition) {
        Ok(table) => table,
        Err(TableError::TableDoesNotExist(_)) => return Ok(BTreeMap::new()),
        Err(error) => return Err(backend_error(error)),
    };
    let mut rows = BTreeMap::new();
    for item in table.iter().map_err(backend_error)? {
        let (key, value) = item.map_err(backend_error)?;
        rows.insert(key.value().to_owned(), value.value().to_vec());
    }
    Ok(rows)
}

fn write_u64_table(
    write: &WriteTransaction,
    definition: TableDefinition<'static, &str, u64>,
    rows: &BTreeMap<String, u64>,
) -> Result<(), StoreError> {
    let mut table = write.open_table(definition).map_err(backend_error)?;
    for (key, value) in rows {
        table.insert(key.as_str(), *value).map_err(backend_error)?;
    }
    Ok(())
}

fn write_bytes_table(
    write: &WriteTransaction,
    definition: TableDefinition<'static, &str, &[u8]>,
    rows: &BTreeMap<String, Vec<u8>>,
) -> Result<(), StoreError> {
    let mut table = write.open_table(definition).map_err(backend_error)?;
    for (key, value) in rows {
        table
            .insert(key.as_str(), value.as_slice())
            .map_err(backend_error)?;
    }
    Ok(())
}
