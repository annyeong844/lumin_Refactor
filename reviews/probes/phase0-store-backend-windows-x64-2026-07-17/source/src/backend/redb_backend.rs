use std::ops::Bound::{Excluded, Unbounded};
use std::path::Path;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use redb::{Database, DatabaseError, Durability, ReadableDatabase, ReadableTable, TableDefinition};

use crate::model::{AdmissionOutcome, HoldPhase};
use crate::util::{create_durable_marker, wait_forever};

const LEASES: TableDefinition<&str, &str> = TableDefinition::new("leases");
const CATALOG: TableDefinition<&str, &[u8]> = TableDefinition::new("catalog");
const CATALOG_KEY: &str = "canonical";
const RECORDS: TableDefinition<&str, &[u8]> = TableDefinition::new("records");

pub fn initialize(path: &Path) -> Result<()> {
    let database = Database::create(path)
        .with_context(|| format!("create redb database {}", path.display()))?;
    let mut transaction = database.begin_write()?;
    transaction.set_durability(Durability::Immediate)?;
    transaction.open_table(LEASES)?;
    transaction.open_table(CATALOG)?;
    transaction.open_table(RECORDS)?;
    transaction.commit()?;
    drop(database);
    Ok(())
}

pub fn insert_records(path: &Path, records: &[(String, Vec<u8>)]) -> Result<()> {
    let database = open_with_retry(path)?;
    let mut transaction = database.begin_write()?;
    transaction.set_durability(Durability::Immediate)?;
    {
        let mut table = transaction.open_table(RECORDS)?;
        for (key, value) in records {
            table.insert(key.as_str(), value.as_slice())?;
        }
    }
    transaction.commit()?;
    Ok(())
}

pub fn query_records(
    path: &Path,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<(String, Vec<u8>)>> {
    let database = open_with_retry(path)?;
    let transaction = database.begin_read()?;
    let table = transaction.open_table(RECORDS)?;
    let mut rows = Vec::with_capacity(limit);
    if let Some(after) = after {
        for item in table
            .range::<&str>((Excluded(after), Unbounded))?
            .take(limit)
        {
            let (key, value) = item?;
            rows.push((key.value().to_owned(), value.value().to_owned()));
        }
    } else {
        for item in table.iter()?.take(limit) {
            let (key, value) = item?;
            rows.push((key.value().to_owned(), value.value().to_owned()));
        }
    }
    Ok(rows)
}

pub fn read_catalog(path: &Path) -> Result<Option<Vec<u8>>> {
    let database = open_with_retry(path)?;
    let transaction = database.begin_read()?;
    let table = transaction.open_table(CATALOG)?;
    Ok(table
        .get(CATALOG_KEY)?
        .map(|guard| guard.value().to_owned()))
}

pub fn compare_exchange_catalog(
    path: &Path,
    expected: Option<&[u8]>,
    replacement: &[u8],
) -> Result<bool> {
    let database = open_with_retry(path)?;
    let mut transaction = database.begin_write()?;
    transaction.set_durability(Durability::Immediate)?;
    let matches = {
        let mut table = transaction.open_table(CATALOG)?;
        let current = table
            .get(CATALOG_KEY)?
            .map(|guard| guard.value().to_owned());
        if current.as_deref() == expected {
            table.insert(CATALOG_KEY, replacement)?;
            true
        } else {
            false
        }
    };
    if matches {
        transaction.commit()?;
    } else {
        transaction.abort()?;
    }
    Ok(matches)
}

pub fn prepare_for_replace(path: &Path) -> Result<()> {
    let database = open_with_retry(path)?;
    drop(database);
    Ok(())
}

pub fn admit(path: &Path, key: &str, gate_id: &str) -> Result<AdmissionOutcome> {
    let database = open_with_retry(path)?;
    let mut transaction = database.begin_write()?;
    transaction.set_durability(Durability::Immediate)?;
    let existing = {
        let mut table = transaction.open_table(LEASES)?;
        let existing = table.get(key)?.map(|value| value.value().to_owned());
        if existing.is_none() {
            table.insert(key, gate_id)?;
        }
        existing
    };
    match existing {
        Some(holder_gate_id) => {
            transaction.abort()?;
            Ok(AdmissionOutcome::Conflict { holder_gate_id })
        }
        None => {
            transaction.commit()?;
            Ok(AdmissionOutcome::Admitted {
                gate_id: gate_id.to_owned(),
            })
        }
    }
}

pub fn read_holder(path: &Path, key: &str) -> Result<Option<String>> {
    let database = open_with_retry(path)?;
    let transaction = database.begin_read()?;
    let table = transaction.open_table(LEASES)?;
    let value = table.get(key)?.map(|guard| guard.value().to_owned());
    Ok(value)
}

pub fn hold(
    path: &Path,
    key: &str,
    gate_id: &str,
    phase: HoldPhase,
    ready_path: &Path,
) -> Result<()> {
    let database = open_with_retry(path)?;
    let mut transaction = database.begin_write()?;
    transaction.set_durability(Durability::Immediate)?;
    {
        let mut table = transaction.open_table(LEASES)?;
        ensure!(table.get(key)?.is_none(), "hold path already has a lease");
        table.insert(key, gate_id)?;
    }

    match phase {
        HoldPhase::Uncommitted => {
            create_durable_marker(ready_path, b"uncommitted\n")?;
            let _database = database;
            let _transaction = transaction;
            wait_forever();
        }
        HoldPhase::Committed => {
            transaction.commit()?;
            create_durable_marker(ready_path, b"committed\n")?;
            let _database = database;
            wait_forever();
        }
    }
}

fn open_with_retry(path: &Path) -> Result<Database> {
    loop {
        match Database::open(path) {
            Ok(database) => return Ok(database),
            Err(DatabaseError::DatabaseAlreadyOpen) => thread::sleep(Duration::from_millis(1)),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("open redb database {}", path.display()));
            }
        }
    }
}
