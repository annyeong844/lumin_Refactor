use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};

use crate::backend;
use crate::model::{BackendKind, FaultCaseResult};

pub fn run_backend_contract_cases(
    backend_kind: BackendKind,
    backend_root: &Path,
) -> Vec<FaultCaseResult> {
    [
        capture("indexed-query", || {
            run_indexed_query_case(backend_kind, backend_root)
        }),
        capture("corruption-visible", || {
            run_corruption_case(backend_kind, backend_root)
        }),
    ]
    .into_iter()
    .collect()
}

fn capture<F>(case: &str, operation: F) -> FaultCaseResult
where
    F: FnOnce() -> Result<serde_json::Value>,
{
    let started = Instant::now();
    match operation() {
        Ok(observation) => FaultCaseResult {
            domain: "backend-contract".to_owned(),
            crash_point: case.to_owned(),
            status: "PASS".to_owned(),
            error: None,
            elapsed_micros: started.elapsed().as_micros(),
            observation,
        },
        Err(error) => FaultCaseResult {
            domain: "backend-contract".to_owned(),
            crash_point: case.to_owned(),
            status: "FAIL".to_owned(),
            error: Some(format!("{error:#}")),
            elapsed_micros: started.elapsed().as_micros(),
            observation: serde_json::Value::Null,
        },
    }
}

fn run_indexed_query_case(
    backend_kind: BackendKind,
    backend_root: &Path,
) -> Result<serde_json::Value> {
    let root = backend_root.join("backend-contract").join("indexed-query");
    reset_directory(&root)?;
    let database = root.join(backend_kind.file_name());
    backend::initialize(backend_kind, &database)?;
    let records = (0..1_000_u32)
        .map(|index| {
            let key = format!("record-{index:08}");
            let value = Sha256::digest(key.as_bytes()).to_vec();
            (key, value)
        })
        .collect::<Vec<_>>();
    backend::insert_records(backend_kind, &database, &records)?;

    let mut cursor = None;
    let mut observed = Vec::new();
    let mut pages = 0_u32;
    loop {
        let page = backend::query_records(backend_kind, &database, cursor.as_deref(), 37)?;
        ensure!(page.len() <= 37);
        if page.is_empty() {
            break;
        }
        cursor = page.last().map(|(key, _)| key.clone());
        observed.extend(page);
        pages += 1;
    }
    ensure!(
        observed == records,
        "cursor traversal changed order or payload"
    );
    ensure!(pages == 28, "unexpected page count {pages}");
    Ok(serde_json::json!({
        "records": observed.len(),
        "page_size": 37,
        "pages": pages,
        "first": observed.first().map(|row| &row.0),
        "last": observed.last().map(|row| &row.0)
    }))
}

fn run_corruption_case(
    backend_kind: BackendKind,
    backend_root: &Path,
) -> Result<serde_json::Value> {
    let root = backend_root
        .join("backend-contract")
        .join("corruption-visible");
    reset_directory(&root)?;
    let database = root.join(backend_kind.file_name());
    backend::initialize(backend_kind, &database)?;
    let catalog = br#"{"generation":1,"sentinel":"must-not-become-empty"}"#;
    ensure!(backend::compare_exchange_catalog(
        backend_kind,
        &database,
        None,
        catalog
    )?);
    backend::prepare_for_replace(backend_kind, &database)?;
    let original_bytes = fs::read(&database)?;
    ensure!(original_bytes.len() >= 64);
    let mut file = OpenOptions::new().write(true).open(&database)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&[0_u8; 32])?;
    file.sync_all()?;
    drop(file);

    let diagnostic = match backend::read_catalog(backend_kind, &database) {
        Ok(value) => anyhow::bail!("corrupt store was accepted as {value:?}"),
        Err(error) => format!("{error:#}"),
    };
    Ok(serde_json::json!({
        "original_bytes": original_bytes.len(),
        "overwritten_header_bytes": 32,
        "visible_error": diagnostic
    }))
}

fn reset_directory(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path)
            .with_context(|| format!("remove previous fixture {}", path.display()))?;
    }
    fs::create_dir_all(path)?;
    Ok(())
}
