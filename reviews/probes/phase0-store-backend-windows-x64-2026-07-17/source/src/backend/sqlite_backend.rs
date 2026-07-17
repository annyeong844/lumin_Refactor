use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::model::{AdmissionOutcome, HoldPhase};
use crate::util::{create_durable_marker, wait_forever};

pub fn initialize(path: &Path) -> Result<()> {
    let connection = Connection::open(path)
        .with_context(|| format!("create SQLite database {}", path.display()))?;
    connection.busy_timeout(Duration::from_secs(30))?;
    connection.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=FULL;
         PRAGMA foreign_keys=ON;
         CREATE TABLE IF NOT EXISTS leases (
             logical_path BLOB PRIMARY KEY,
             gate_id TEXT NOT NULL
         ) WITHOUT ROWID;
         CREATE TABLE IF NOT EXISTS catalog (
             catalog_key TEXT PRIMARY KEY,
             payload BLOB NOT NULL
         ) WITHOUT ROWID;
         CREATE TABLE IF NOT EXISTS records (
             record_key BLOB PRIMARY KEY,
             payload BLOB NOT NULL
         ) WITHOUT ROWID;",
    )?;
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    drop(connection);
    Ok(())
}

pub fn insert_records(path: &Path, records: &[(String, Vec<u8>)]) -> Result<()> {
    let mut connection = open(path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    {
        let mut statement = transaction.prepare(
            "INSERT INTO records(record_key, payload) VALUES (?1, ?2)
             ON CONFLICT(record_key) DO UPDATE SET payload = excluded.payload",
        )?;
        for (key, value) in records {
            statement.execute(params![key.as_bytes(), value])?;
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
    let connection = open(path)?;
    let limit = i64::try_from(limit).context("query limit exceeds i64")?;
    let mut rows = Vec::new();
    if let Some(after) = after {
        let mut statement = connection.prepare(
            "SELECT record_key, payload FROM records
             WHERE record_key > ?1 ORDER BY record_key ASC LIMIT ?2",
        )?;
        let query = statement.query_map(params![after.as_bytes(), limit], |row| {
            let key = row.get::<_, Vec<u8>>(0)?;
            let value = row.get::<_, Vec<u8>>(1)?;
            Ok((key, value))
        })?;
        for row in query {
            let (key, value) = row?;
            rows.push((
                String::from_utf8(key).context("non-UTF-8 probe key")?,
                value,
            ));
        }
    } else {
        let mut statement = connection
            .prepare("SELECT record_key, payload FROM records ORDER BY record_key ASC LIMIT ?1")?;
        let query = statement.query_map(params![limit], |row| {
            let key = row.get::<_, Vec<u8>>(0)?;
            let value = row.get::<_, Vec<u8>>(1)?;
            Ok((key, value))
        })?;
        for row in query {
            let (key, value) = row?;
            rows.push((
                String::from_utf8(key).context("non-UTF-8 probe key")?,
                value,
            ));
        }
    }
    Ok(rows)
}

pub fn read_catalog(path: &Path) -> Result<Option<Vec<u8>>> {
    let connection = open(path)?;
    Ok(connection
        .query_row(
            "SELECT payload FROM catalog WHERE catalog_key = 'canonical'",
            [],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?)
}

pub fn compare_exchange_catalog(
    path: &Path,
    expected: Option<&[u8]>,
    replacement: &[u8],
) -> Result<bool> {
    let mut connection = open(path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = transaction
        .query_row(
            "SELECT payload FROM catalog WHERE catalog_key = 'canonical'",
            [],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?;
    if current.as_deref() != expected {
        transaction.rollback()?;
        return Ok(false);
    }
    transaction.execute(
        "INSERT INTO catalog(catalog_key, payload) VALUES ('canonical', ?1)
         ON CONFLICT(catalog_key) DO UPDATE SET payload = excluded.payload",
        params![replacement],
    )?;
    transaction.commit()?;
    Ok(true)
}

pub fn prepare_for_replace(path: &Path) -> Result<()> {
    let connection = open(path)?;
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    drop(connection);
    for suffix in ["-wal", "-shm"] {
        let sidecar = path.with_file_name(format!(
            "{}{}",
            path.file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default(),
            suffix
        ));
        if sidecar.exists() {
            let length = std::fs::metadata(&sidecar)?.len();
            ensure!(
                length == 0,
                "nonempty SQLite sidecar before replace: {}",
                sidecar.display()
            );
            std::fs::remove_file(&sidecar)?;
        }
    }
    Ok(())
}

pub fn admit(path: &Path, key: &str, gate_id: &str) -> Result<AdmissionOutcome> {
    let mut connection = open(path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let existing = transaction
        .query_row(
            "SELECT gate_id FROM leases WHERE logical_path = ?1",
            params![key.as_bytes()],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    match existing {
        Some(holder_gate_id) => {
            transaction.rollback()?;
            Ok(AdmissionOutcome::Conflict { holder_gate_id })
        }
        None => {
            transaction.execute(
                "INSERT INTO leases(logical_path, gate_id) VALUES (?1, ?2)",
                params![key.as_bytes(), gate_id],
            )?;
            transaction.commit()?;
            Ok(AdmissionOutcome::Admitted {
                gate_id: gate_id.to_owned(),
            })
        }
    }
}

pub fn read_holder(path: &Path, key: &str) -> Result<Option<String>> {
    let connection = open(path)?;
    Ok(connection
        .query_row(
            "SELECT gate_id FROM leases WHERE logical_path = ?1",
            params![key.as_bytes()],
            |row| row.get::<_, String>(0),
        )
        .optional()?)
}

pub fn hold(
    path: &Path,
    key: &str,
    gate_id: &str,
    phase: HoldPhase,
    ready_path: &Path,
) -> Result<()> {
    let mut connection = open(path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let existing = transaction
        .query_row(
            "SELECT gate_id FROM leases WHERE logical_path = ?1",
            params![key.as_bytes()],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    ensure!(existing.is_none(), "hold path already has a lease");
    transaction.execute(
        "INSERT INTO leases(logical_path, gate_id) VALUES (?1, ?2)",
        params![key.as_bytes(), gate_id],
    )?;

    match phase {
        HoldPhase::Uncommitted => {
            create_durable_marker(ready_path, b"uncommitted\n")?;
            let _transaction = transaction;
            wait_forever();
        }
        HoldPhase::Committed => {
            transaction.commit()?;
            create_durable_marker(ready_path, b"committed\n")?;
            let _connection = connection;
            wait_forever();
        }
    }
}

fn open(path: &Path) -> Result<Connection> {
    let connection = Connection::open(path)
        .with_context(|| format!("open SQLite database {}", path.display()))?;
    connection.busy_timeout(Duration::from_secs(30))?;
    connection.execute_batch("PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;")?;
    Ok(connection)
}
