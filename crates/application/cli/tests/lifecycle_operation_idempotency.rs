use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

mod support;

use support::{ProcessResult, assert_status, field, run, run_with_env};

const DELIVERY_FAILURE_ENV: &str = "LUMIN_TEST_FAIL_RESULT_DELIVERY";

#[test]
fn gate_mutations_recover_post_commit_delivery_failure_without_duplication()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let pre_args = [
        "pre-write",
        "--operation-id",
        "lifecycle-pre",
        "--path",
        "src/lib.ts",
        "--jobs",
        "1",
    ];
    assert_delivery_failure(root.path(), &pre_args, "lifecycle-pre")?;
    let pre_result = recovered_gate_result(root.path(), "lifecycle-pre", "pre-write")?;
    let gate_id = required_string(&pre_result, "/gateId")?;
    let pre_retry = run(root.path(), &pre_args)?;
    assert_status(&pre_retry, 0);
    assert_eq!(json(&pre_retry.stdout)?, pre_result);
    assert_conflict(run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "lifecycle-pre",
            "--path",
            "src/other.ts",
            "--jobs",
            "1",
        ],
    )?);
    assert_gate_history(root.path(), &gate_id, "active", &["lifecycle-pre"])?;

    fs::write(root.path().join("src/lib.ts"), "export const value = 2;\n")?;
    let post_args = [
        "post-write",
        gate_id.as_str(),
        "--operation-id",
        "lifecycle-post",
    ];
    assert_delivery_failure(root.path(), &post_args, "lifecycle-post")?;
    let post_result = recovered_gate_result(root.path(), "lifecycle-post", "post-write")?;
    assert_eq!(required_u64(&post_result, "/revision")?, 1);
    let post_retry = run(root.path(), &post_args)?;
    assert_status(&post_retry, 0);
    assert_eq!(json(&post_retry.stdout)?, post_result);
    assert_gate_history(
        root.path(),
        &gate_id,
        "closed",
        &["lifecycle-pre", "lifecycle-post"],
    )?;

    let abandoned_gate = open_gate(root.path(), "lifecycle-abandon-pre", "src/other.ts")?;
    assert_conflict(run(
        root.path(),
        &[
            "post-write",
            abandoned_gate.as_str(),
            "--operation-id",
            "lifecycle-post",
        ],
    )?);
    let abandon_args = [
        "gate",
        "abandon",
        abandoned_gate.as_str(),
        "--operation-id",
        "lifecycle-abandon",
        "--reason",
        "cancelled edit",
    ];
    assert_delivery_failure(root.path(), &abandon_args, "lifecycle-abandon")?;
    let abandon_result = recovered_gate_result(root.path(), "lifecycle-abandon", "gate-abandon")?;
    assert_eq!(
        required_string(&abandon_result, "/reason")?,
        "cancelled edit"
    );
    let abandon_retry = run(root.path(), &abandon_args)?;
    assert_status(&abandon_retry, 0);
    assert_eq!(json(&abandon_retry.stdout)?, abandon_result);
    assert_conflict(run(
        root.path(),
        &[
            "gate",
            "abandon",
            abandoned_gate.as_str(),
            "--operation-id",
            "lifecycle-abandon",
            "--reason",
            "different reason",
        ],
    )?);
    assert_gate_history(
        root.path(),
        &abandoned_gate,
        "abandoned",
        &["lifecycle-abandon-pre", "lifecycle-abandon"],
    )?;
    Ok(())
}

#[test]
fn gate_retention_mutations_recover_post_commit_delivery_failure_without_duplication()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let gate_id = open_gate(root.path(), "lifecycle-gate-retention-open", "src/lib.ts")?;
    fs::write(root.path().join("src/lib.ts"), "export const value = 3;\n")?;
    let closed = run(
        root.path(),
        &[
            "post-write",
            gate_id.as_str(),
            "--operation-id",
            "lifecycle-gate-retention-close",
        ],
    )?;
    assert_status(&closed, 0);

    let plan_args = [
        "gate",
        "prune",
        "plan",
        "--terminal-before",
        "9000000000000",
        "--operation-id",
        "lifecycle-gate-plan",
    ];
    assert_delivery_failure(root.path(), &plan_args, "lifecycle-gate-plan")?;
    let plan_operation =
        recovered_retention_result(root.path(), "lifecycle-gate-plan", "gate-prune-plan")?;
    let plan_result = required_value(&plan_operation, "/result")?;
    let plan_id = required_string(plan_result, "/planId")?;
    let plan_retry = run(root.path(), &plan_args)?;
    assert_status(&plan_retry, 0);
    assert_eq!(
        json(&plan_retry.stdout)?.pointer("/result"),
        Some(plan_result)
    );
    assert_conflict(run(
        root.path(),
        &[
            "gate",
            "prune",
            "plan",
            "--terminal-before",
            "8999999999999",
            "--operation-id",
            "lifecycle-gate-plan",
        ],
    )?);
    let shown_plan = show_gate_plan(root.path(), &plan_id)?;
    assert_plan_contains_record(&shown_plan, "items", "gate", &gate_id)?;

    let second_plan_output = run(
        root.path(),
        &[
            "gate",
            "prune",
            "plan",
            "--terminal-before",
            "9000000000000",
            "--operation-id",
            "lifecycle-gate-plan-secondary",
        ],
    )?;
    assert_status(&second_plan_output, 0);
    let second_plan = required_string(&json(&second_plan_output.stdout)?, "/result/planId")?;

    let confirm_args = [
        "gate",
        "prune",
        "confirm",
        plan_id.as_str(),
        "--operation-id",
        "lifecycle-gate-confirm",
    ];
    assert_delivery_failure(root.path(), &confirm_args, "lifecycle-gate-confirm")?;
    let confirm_operation =
        recovered_retention_result(root.path(), "lifecycle-gate-confirm", "gate-prune-confirm")?;
    let confirm_result = required_value(&confirm_operation, "/result")?;
    assert_eq!(required_string(confirm_result, "/status")?, "pruned");
    assert_eq!(required_string(confirm_result, "/planId")?, plan_id);
    let confirm_retry = run(root.path(), &confirm_args)?;
    assert_status(&confirm_retry, 0);
    assert_eq!(
        json(&confirm_retry.stdout)?.pointer("/result"),
        Some(confirm_result)
    );
    assert_conflict(run(
        root.path(),
        &[
            "gate",
            "prune",
            "confirm",
            second_plan.as_str(),
            "--operation-id",
            "lifecycle-gate-confirm",
        ],
    )?);
    assert_eq!(
        required_string(&show_gate_plan(root.path(), &plan_id)?, "/state")?,
        "pruned"
    );
    assert_eq!(
        required_string(&show_gate_plan(root.path(), &second_plan)?, "/state")?,
        "prepared"
    );
    assert_tombstone(root.path(), &["gate", "show", gate_id.as_str()], &plan_id)?;
    Ok(())
}

#[test]
fn retention_mutations_recover_post_commit_delivery_failure_without_duplication()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let first_audited = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&first_audited, 0);
    let prunable_run_id = field(&first_audited.stdout, "runId")?;
    fs::write(root.path().join("src/lib.ts"), "export const value = 2;\n")?;
    let audited = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audited, 0);
    let run_id = field(&audited.stdout, "runId")?;
    assert_ne!(prunable_run_id, run_id);

    let pin_args = [
        "runs",
        "pin",
        run_id.as_str(),
        "--operation-id",
        "lifecycle-pin",
        "--reason",
        "primary review",
    ];
    let pin_started_unix_millis = unix_millis()?;
    assert_delivery_failure(root.path(), &pin_args, "lifecycle-pin")?;
    let pin_finished_unix_millis = unix_millis()?;
    let pin_operation = recovered_retention_result(root.path(), "lifecycle-pin", "run-pin")?;
    let created_unix_millis = required_u64(&pin_operation, "/pin/createdUnixMillis")?;
    assert!(
        (pin_started_unix_millis..=pin_finished_unix_millis).contains(&created_unix_millis),
        "pin timestamp {created_unix_millis} fell outside {pin_started_unix_millis}..={pin_finished_unix_millis}"
    );
    let first_pin = required_string(&pin_operation, "/pin/pinId")?;
    let pin_retry = run(root.path(), &pin_args)?;
    assert_status(&pin_retry, 0);
    assert_eq!(
        json(&pin_retry.stdout)?.pointer("/pin").cloned(),
        pin_operation.pointer("/pin").cloned()
    );
    assert_conflict(run(
        root.path(),
        &[
            "runs",
            "pin",
            run_id.as_str(),
            "--operation-id",
            "lifecycle-pin",
            "--reason",
            "different review",
        ],
    )?);

    let second_pin_output = run(
        root.path(),
        &[
            "runs",
            "pin",
            run_id.as_str(),
            "--operation-id",
            "lifecycle-pin-secondary",
            "--reason",
            "secondary review",
        ],
    )?;
    assert_status(&second_pin_output, 0);
    let second_pin = required_string(&json(&second_pin_output.stdout)?, "/pin/pinId")?;
    assert_ne!(first_pin, second_pin);

    let unpin_args = [
        "runs",
        "unpin",
        first_pin.as_str(),
        "--operation-id",
        "lifecycle-unpin",
    ];
    assert_delivery_failure(root.path(), &unpin_args, "lifecycle-unpin")?;
    let unpin_operation = recovered_retention_result(root.path(), "lifecycle-unpin", "run-unpin")?;
    assert_eq!(required_string(&unpin_operation, "/pinId")?, first_pin);
    let unpin_retry = run(root.path(), &unpin_args)?;
    assert_status(&unpin_retry, 0);
    let unpin_retry = json(&unpin_retry.stdout)?;
    assert_eq!(required_string(&unpin_retry, "/pin/pinId")?, first_pin);
    assert_eq!(
        required_string(&unpin_retry, "/pin/removedOperationId")?,
        "lifecycle-unpin"
    );
    assert_conflict(run(
        root.path(),
        &[
            "runs",
            "unpin",
            second_pin.as_str(),
            "--operation-id",
            "lifecycle-unpin",
        ],
    )?);

    let plan_args = [
        "runs",
        "prune",
        "plan",
        "--before",
        "9000000000000",
        "--operation-id",
        "lifecycle-plan",
    ];
    assert_delivery_failure(root.path(), &plan_args, "lifecycle-plan")?;
    let plan_operation =
        recovered_retention_result(root.path(), "lifecycle-plan", "run-prune-plan")?;
    let plan_result = required_value(&plan_operation, "/result")?;
    let plan_id = required_string(plan_result, "/planId")?;
    let plan_retry = run(root.path(), &plan_args)?;
    assert_status(&plan_retry, 0);
    assert_eq!(
        json(&plan_retry.stdout)?.pointer("/result"),
        Some(plan_result)
    );
    assert_conflict(run(
        root.path(),
        &[
            "runs",
            "prune",
            "plan",
            "--before",
            "8999999999999",
            "--operation-id",
            "lifecycle-plan",
        ],
    )?);
    let shown_plan = show_run_plan(root.path(), &plan_id)?;
    assert_eq!(
        active_pin_ids(&shown_plan, "run", &run_id)?,
        vec![second_pin]
    );
    assert_plan_contains_record(&shown_plan, "items", "run", &prunable_run_id)?;

    let second_plan_output = run(
        root.path(),
        &[
            "runs",
            "prune",
            "plan",
            "--before",
            "9000000000000",
            "--operation-id",
            "lifecycle-plan-secondary",
        ],
    )?;
    assert_status(&second_plan_output, 0);
    let second_plan = required_string(&json(&second_plan_output.stdout)?, "/result/planId")?;

    let confirm_args = [
        "runs",
        "prune",
        "confirm",
        plan_id.as_str(),
        "--operation-id",
        "lifecycle-confirm",
    ];
    assert_delivery_failure(root.path(), &confirm_args, "lifecycle-confirm")?;
    let confirm_operation =
        recovered_retention_result(root.path(), "lifecycle-confirm", "run-prune-confirm")?;
    let confirm_result = required_value(&confirm_operation, "/result")?;
    assert_eq!(required_string(confirm_result, "/status")?, "pruned");
    assert_eq!(required_string(confirm_result, "/planId")?, plan_id);
    let confirm_retry = run(root.path(), &confirm_args)?;
    assert_status(&confirm_retry, 0);
    assert_eq!(
        json(&confirm_retry.stdout)?.pointer("/result"),
        Some(confirm_result)
    );
    assert_conflict(run(
        root.path(),
        &[
            "runs",
            "prune",
            "confirm",
            second_plan.as_str(),
            "--operation-id",
            "lifecycle-confirm",
        ],
    )?);
    assert_eq!(
        required_string(&show_run_plan(root.path(), &plan_id)?, "/state")?,
        "pruned"
    );
    assert_eq!(
        required_string(&show_run_plan(root.path(), &second_plan)?, "/state")?,
        "prepared"
    );
    assert_tombstone(
        root.path(),
        &["overview", "--run", prunable_run_id.as_str()],
        &plan_id,
    )?;
    Ok(())
}

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
