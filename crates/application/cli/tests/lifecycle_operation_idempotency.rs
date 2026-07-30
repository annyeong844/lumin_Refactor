use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

mod support;

use support::{ProcessResult, assert_status, field, run, run_with_env};

const DELIVERY_FAILURE_ENV: &str = "LUMIN_TEST_FAIL_RESULT_DELIVERY";

#[path = "lifecycle_operation_idempotency/gate.rs"]
mod gate;
#[path = "lifecycle_operation_idempotency/gate_retention.rs"]
mod gate_retention;
#[path = "lifecycle_operation_idempotency/run_retention.rs"]
mod run_retention;

fn fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(
        root.path().join("src/main.ts"),
        "import { value } from './lib';\nconsole.log(value);\n",
    )?;
    fs::write(root.path().join("src/lib.ts"), "export const value = 1;\n")?;
    fs::write(
        root.path().join("src/other.ts"),
        "export const other = 1;\n",
    )?;
    Ok(root)
}

fn open_gate(
    root: &Path,
    operation_id: &str,
    path: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let opened = run(
        root,
        &[
            "pre-write",
            "--operation-id",
            operation_id,
            "--path",
            path,
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);
    field(&opened.stdout, "gateId")
}

fn assert_delivery_failure(
    root: &Path,
    arguments: &[&str],
    operation_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let failed = run_with_env(root, arguments, &[(DELIVERY_FAILURE_ENV, operation_id)])?;
    assert_status(&failed, 1);
    assert!(failed.stdout.is_empty());
    assert_eq!(
        failed.stderr,
        "lumin: injected result delivery failure after commit\n"
    );
    Ok(())
}

fn recovered_gate_result(
    root: &Path,
    operation_id: &str,
    kind: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let operation = show_operation(root, operation_id)?;
    assert_eq!(required_string(&operation, "/kind")?, kind);
    assert_eq!(required_string(&operation, "/status")?, "committed");
    let result = required_value(&operation, "/result")?.clone();
    assert_eq!(required_string(&result, "/operationId")?, operation_id);
    Ok(result)
}

fn recovered_retention_result(
    root: &Path,
    operation_id: &str,
    kind: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let operation = show_operation(root, operation_id)?;
    assert_eq!(required_string(&operation, "/operation/kind")?, kind);
    assert_eq!(
        required_string(&operation, "/operation/status")?,
        "committed"
    );
    required_value(&operation, "/operation/result").cloned()
}

fn show_operation(root: &Path, operation_id: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let shown = run(root, &["operation", "show", operation_id])?;
    assert_status(&shown, 0);
    json(&shown.stdout).map_err(Into::into)
}

fn assert_gate_history(
    root: &Path,
    gate_id: &str,
    lifecycle: &str,
    operation_ids: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    let shown = run(root, &["gate", "show", gate_id])?;
    assert_status(&shown, 0);
    let shown = json(&shown.stdout)?;
    assert_eq!(required_string(&shown, "/lifecycle")?, lifecycle);
    let actual = required_value(&shown, "/revisions")?
        .as_array()
        .ok_or_else(|| std::io::Error::other("gate revisions were not an array"))?
        .iter()
        .map(|revision| required_string(revision, "/operationId"))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(actual, operation_ids);
    assert_eq!(
        required_u64(&shown, "/currentRevision")?,
        (operation_ids.len() - 1) as u64
    );
    Ok(())
}

fn show_run_plan(root: &Path, plan_id: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let shown = run(root, &["runs", "prune", "plan", "show", plan_id])?;
    assert_status(&shown, 0);
    json(&shown.stdout).map_err(Into::into)
}

fn show_gate_plan(root: &Path, plan_id: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let shown = run(root, &["gate", "prune", "plan", "show", plan_id])?;
    assert_status(&shown, 0);
    json(&shown.stdout).map_err(Into::into)
}

fn assert_plan_contains_record(
    plan: &Value,
    collection: &str,
    kind: &str,
    record_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let records = plan
        .get(collection)
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other(format!("plan omitted {collection}")))?;
    let matches = records
        .iter()
        .filter(|record| {
            record.get("kind").and_then(Value::as_str) == Some(kind)
                && record.get("recordId").and_then(Value::as_str) == Some(record_id)
        })
        .count();
    assert_eq!(
        matches, 1,
        "expected one {kind} {record_id} in {collection}"
    );
    Ok(())
}

fn assert_tombstone(
    root: &Path,
    arguments: &[&str],
    plan_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let lookup = run(root, arguments)?;
    assert_status(&lookup, 0);
    let lookup = json(&lookup.stdout)?;
    assert_eq!(lookup.get("status").and_then(Value::as_str), Some("pruned"));
    assert_eq!(
        lookup.pointer("/tombstone/planId").and_then(Value::as_str),
        Some(plan_id)
    );
    Ok(())
}

fn active_pin_ids(
    plan: &Value,
    kind: &str,
    record_id: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let exclusions = required_value(plan, "/exclusions")?
        .as_array()
        .ok_or_else(|| std::io::Error::other("plan exclusions were not an array"))?;
    let matches = exclusions
        .iter()
        .filter(|exclusion| {
            exclusion.get("kind").and_then(Value::as_str) == Some(kind)
                && exclusion.get("recordId").and_then(Value::as_str) == Some(record_id)
                && exclusion.pointer("/reason/reason").and_then(Value::as_str) == Some("active-pin")
        })
        .collect::<Vec<_>>();
    assert_eq!(matches.len(), 1, "expected one active-pin exclusion");
    matches[0]
        .pointer("/reason/pinIds")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("active-pin exclusion omitted pinIds"))?
        .iter()
        .map(|pin| {
            pin.as_str()
                .map(str::to_owned)
                .ok_or_else(|| std::io::Error::other("pinId was not a string"))
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn assert_conflict(result: ProcessResult) {
    assert_status(&result, 2);
    assert!(result.stdout.is_empty());
    assert!(result.stderr.contains("reused with a different request"));
}

fn json(value: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(value)
}

fn required_value<'a>(
    value: &'a Value,
    pointer: &str,
) -> Result<&'a Value, Box<dyn std::error::Error>> {
    value
        .pointer(pointer)
        .ok_or_else(|| std::io::Error::other(format!("response omitted {pointer}")).into())
}

fn required_string(value: &Value, pointer: &str) -> Result<String, Box<dyn std::error::Error>> {
    required_value(value, pointer)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other(format!("{pointer} was not a string")).into())
}

fn unix_millis() -> Result<u64, Box<dyn std::error::Error>> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .try_into()
        .map_err(Into::into)
}

fn required_u64(value: &Value, pointer: &str) -> Result<u64, Box<dyn std::error::Error>> {
    required_value(value, pointer)?
        .as_u64()
        .ok_or_else(|| std::io::Error::other(format!("{pointer} was not a u64")).into())
}
