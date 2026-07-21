use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

#[test]
fn retention_truth_survives_public_process_reopen() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("lib.ts"), "export const first = 1;\n")?;
    let first_run = audit(root.path())?;
    fs::write(root.path().join("lib.ts"), "export const second = 2;\n")?;
    let second_run = audit(root.path())?;

    let plan = prepare_plan(root.path())?;
    let plan_id = json(&plan.stdout)?
        .pointer("/result/planId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other("plan response omitted planId"))?;
    let plan_retry = prepare_plan(root.path())?;
    assert_eq!(plan_retry.stdout, plan.stdout);
    assert_prepared_plan(root.path(), &plan_id, &first_run, &second_run)?;

    let confirmed = confirm_plan(root.path(), &plan_id)?;
    assert_eq!(
        json(&confirmed.stdout)?
            .pointer("/result/status")
            .and_then(Value::as_str),
        Some("pruned")
    );
    let confirm_retry = confirm_plan(root.path(), &plan_id)?;
    assert_eq!(confirm_retry.stdout, confirmed.stdout);

    assert_pruned_views(root.path(), &plan_id, &first_run, &second_run)?;
    assert_committed_operation(root.path(), &plan_id, &confirmed.stdout)?;
    Ok(())
}

fn audit(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let output = run(root, &["audit", "--jobs", "1"])?;
    assert_status(&output, 0);
    field(&output.stdout, "runId")
}

fn prepare_plan(root: &Path) -> Result<support::ProcessResult, Box<dyn std::error::Error>> {
    let output = run(
        root,
        &[
            "runs",
            "prune",
            "plan",
            "--before",
            "9000000000000",
            "--operation-id",
            "public-retention-plan",
        ],
    )?;
    assert_status(&output, 0);
    Ok(output)
}

fn confirm_plan(
    root: &Path,
    plan_id: &str,
) -> Result<support::ProcessResult, Box<dyn std::error::Error>> {
    let output = run(
        root,
        &[
            "runs",
            "prune",
            "confirm",
            plan_id,
            "--operation-id",
            "public-retention-confirm",
        ],
    )?;
    assert_status(&output, 0);
    Ok(output)
}

fn assert_prepared_plan(
    root: &Path,
    plan_id: &str,
    first_run: &str,
    second_run: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let shown = run(root, &["runs", "prune", "plan", "show", plan_id])?;
    assert_status(&shown, 0);
    let body = json(&shown.stdout)?;
    assert_eq!(body.get("state").and_then(Value::as_str), Some("prepared"));
    assert!(contains_record(&body, "items", "run", first_run));
    assert!(contains_record(&body, "exclusions", "run", second_run));
    Ok(())
}

fn assert_pruned_views(
    root: &Path,
    plan_id: &str,
    first_run: &str,
    second_run: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let runs = run(root, &["runs", "list"])?;
    assert_status(&runs, 0);
    let runs_body = json(&runs.stdout)?;
    let run_ids = runs_body
        .get("runs")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("run catalog omitted runs"))?
        .iter()
        .filter_map(|run| run.get("runId").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert_eq!(run_ids, [second_run]);

    for arguments in [
        vec!["overview", "--run", first_run],
        vec!["findings", "--run", first_run, "--area", "dead-code"],
    ] {
        let lookup = run(root, &arguments)?;
        assert_status(&lookup, 0);
        let body = json(&lookup.stdout)?;
        assert_eq!(body.get("status").and_then(Value::as_str), Some("pruned"));
        assert_eq!(
            body.pointer("/tombstone/planId").and_then(Value::as_str),
            Some(plan_id)
        );
    }

    let shown = run(root, &["runs", "prune", "plan", "show", plan_id])?;
    assert_status(&shown, 0);
    let body = json(&shown.stdout)?;
    assert_eq!(body.get("state").and_then(Value::as_str), Some("pruned"));
    assert_eq!(
        body.get("physicalReclamationPending")
            .and_then(Value::as_bool),
        Some(false)
    );
    Ok(())
}

fn assert_committed_operation(
    root: &Path,
    plan_id: &str,
    committed_result: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let operation = run(root, &["operation", "show", "public-retention-confirm"])?;
    assert_status(&operation, 0);
    let body = json(&operation.stdout)?;
    assert_eq!(
        body.pointer("/operation/status").and_then(Value::as_str),
        Some("committed")
    );
    assert_eq!(
        body.pointer("/operation/result/result/planId")
            .and_then(Value::as_str),
        Some(plan_id)
    );
    assert_eq!(
        body.pointer("/operation/result/result"),
        json(committed_result)?.get("result")
    );
    Ok(())
}

fn contains_record(body: &Value, collection: &str, kind: &str, record_id: &str) -> bool {
    body.get(collection)
        .and_then(Value::as_array)
        .is_some_and(|records| {
            records.iter().any(|record| {
                record.get("kind").and_then(Value::as_str) == Some(kind)
                    && record.get("recordId").and_then(Value::as_str) == Some(record_id)
            })
        })
}

fn json(value: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(value)
}
