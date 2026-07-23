use std::fs;
use std::path::Path;

use serde_json::Value;

#[path = "support/retention_plan.rs"]
mod retention_plan_support;
#[path = "support/retention.rs"]
mod retention_support;
mod support;

use retention_plan_support::contains_exclusion;
use retention_support::{audit, json};
use support::{assert_status, run};

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
    assert_committed_operation(root.path(), &plan_id)?;
    Ok(())
}

#[test]
fn latest_attempt_and_completed_closures_survive_stale_confirmation()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("lib.ts"), "export const first = 1;\n")?;
    let completed_run = audit(root.path())?;
    let completed_overview = overview(root.path())?;
    let completed_attempt = required_string(&completed_overview, "/latestAttempt/attemptId")?;

    fs::write(root.path().join("lumin.json"), b"{\n")?;
    let failed = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&failed, 1);
    let failed_overview = overview(root.path())?;
    let failed_attempt = required_string(&failed_overview, "/latestAttempt/attemptId")?;
    assert_eq!(
        failed_overview
            .pointer("/latestAttempt/status")
            .and_then(Value::as_str),
        Some("failed")
    );
    assert_eq!(
        failed_overview.pointer("/scope/id").and_then(Value::as_str),
        Some(completed_run.as_str())
    );

    let plan = prepare_plan(root.path())?;
    let plan_id = required_string(&json(&plan.stdout)?, "/result/planId")?;
    assert_latest_exclusions(
        root.path(),
        &plan_id,
        &failed_attempt,
        &completed_attempt,
        &completed_run,
    )?;

    fs::remove_file(root.path().join("lumin.json"))?;
    fs::write(root.path().join("lib.ts"), "export const newest = 2;\n")?;
    let newest_run = audit(root.path())?;
    let newest_overview = overview(root.path())?;
    let newest_attempt = required_string(&newest_overview, "/latestAttempt/attemptId")?;

    let stale = run(
        root.path(),
        &[
            "runs",
            "prune",
            "confirm",
            &plan_id,
            "--operation-id",
            "public-latest-protection-confirm",
        ],
    )?;
    assert_status(&stale, 5);
    let stale_body = json(&stale.stdout)?;
    assert_eq!(
        stale_body.pointer("/result/status").and_then(Value::as_str),
        Some("stale")
    );
    assert!(
        stale_body
            .pointer("/result/changedInputs")
            .and_then(Value::as_array)
            .is_some_and(|inputs| !inputs.is_empty())
    );

    assert_stale_views(
        root.path(),
        &plan_id,
        &completed_run,
        &newest_run,
        &newest_attempt,
    )?;
    Ok(())
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
        body.pointer("/operation/result/result/physicalReclamationPending")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        body.get("currentPhysicalReclamationPending")
            .and_then(Value::as_bool),
        Some(false)
    );
    Ok(())
}

fn overview(root: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    let output = run(root, &["overview"])?;
    assert_status(&output, 0);
    json(&output.stdout).map_err(Into::into)
}

fn required_string(value: &Value, pointer: &str) -> Result<String, Box<dyn std::error::Error>> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other(format!("response omitted {pointer}")).into())
}

fn assert_latest_exclusions(
    root: &Path,
    plan_id: &str,
    failed_attempt: &str,
    completed_attempt: &str,
    completed_run: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let shown = run(root, &["runs", "prune", "plan", "show", plan_id])?;
    assert_status(&shown, 0);
    let body = json(&shown.stdout)?;
    assert_eq!(body.get("state").and_then(Value::as_str), Some("prepared"));
    assert!(
        body.get("items")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
    );
    assert!(contains_exclusion(
        &body,
        "attempt",
        failed_attempt,
        "latest-attempt"
    ));
    assert!(contains_exclusion(
        &body,
        "attempt",
        completed_attempt,
        "latest-completed"
    ));
    assert!(contains_exclusion(
        &body,
        "run",
        completed_run,
        "latest-completed"
    ));
    Ok(())
}

fn assert_stale_views(
    root: &Path,
    plan_id: &str,
    completed_run: &str,
    newest_run: &str,
    newest_attempt: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let shown = run(root, &["runs", "prune", "plan", "show", plan_id])?;
    assert_status(&shown, 0);
    assert_eq!(
        json(&shown.stdout)?.get("state").and_then(Value::as_str),
        Some("prepared")
    );

    let operation = run(
        root,
        &["operation", "show", "public-latest-protection-confirm"],
    )?;
    assert_status(&operation, 0);
    let operation = json(&operation.stdout)?;
    assert_eq!(
        operation
            .pointer("/operation/status")
            .and_then(Value::as_str),
        Some("stale")
    );

    let latest = overview(root)?;
    assert_eq!(
        latest.pointer("/scope/id").and_then(Value::as_str),
        Some(newest_run)
    );
    assert_eq!(
        latest
            .pointer("/latestAttempt/status")
            .and_then(Value::as_str),
        Some("completed")
    );
    assert_eq!(
        latest
            .pointer("/latestAttempt/attemptId")
            .and_then(Value::as_str),
        Some(newest_attempt)
    );

    let retained = run(root, &["overview", "--run", completed_run])?;
    assert_status(&retained, 0);
    assert_eq!(
        json(&retained.stdout)?
            .pointer("/scope/id")
            .and_then(Value::as_str),
        Some(completed_run)
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
