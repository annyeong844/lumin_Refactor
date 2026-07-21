use std::fs;

use serde_json::Value;

#[test]
fn public_run_retention_flow_keeps_tombstone_visible() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("lib.ts"), "export const first = 1;")?;
    let first = audit(root.path());
    let first_run = run_id(&first)?;

    fs::write(root.path().join("lib.ts"), "export const second = 2;")?;
    let second = audit(root.path());
    let second_run = run_id(&second)?;
    assert_run_catalog(root.path(), &[&second_run, &first_run])?;

    let plan = crate::execute(
        root.path(),
        vec![
            "runs".into(),
            "prune".into(),
            "plan".into(),
            "--before".into(),
            "9000000000000".into(),
            "--operation-id".into(),
            "cli-plan".into(),
        ],
    );
    assert_eq!(plan.exit_code, 0, "{}", plan.stderr);
    let plan_json: Value = serde_json::from_str(&plan.stdout)?;
    let plan_id = plan_json
        .pointer("/result/planId")
        .and_then(Value::as_str)
        .ok_or("plan response omitted planId")?
        .to_owned();
    let show = crate::execute(
        root.path(),
        vec![
            "runs".into(),
            "prune".into(),
            "plan".into(),
            "show".into(),
            plan_id.clone().into(),
        ],
    );
    assert_eq!(show.exit_code, 0, "{}", show.stderr);
    let show_json: Value = serde_json::from_str(&show.stdout)?;
    assert!(show_json.get("items").and_then(Value::as_array).is_some());

    let confirm = crate::execute(
        root.path(),
        vec![
            "runs".into(),
            "prune".into(),
            "confirm".into(),
            plan_id.into(),
            "--operation-id".into(),
            "cli-confirm".into(),
        ],
    );
    assert_eq!(confirm.exit_code, 0, "{}", confirm.stderr);
    assert_run_catalog(root.path(), &[&second_run])?;

    let overview = crate::execute(
        root.path(),
        vec!["overview".into(), "--run".into(), first_run.clone().into()],
    );
    assert_eq!(overview.exit_code, 0, "{}", overview.stderr);
    let overview_json: Value = serde_json::from_str(&overview.stdout)?;
    assert_eq!(
        overview_json.get("status").and_then(Value::as_str),
        Some("pruned")
    );
    let findings = crate::execute(
        root.path(),
        vec![
            "findings".into(),
            "--run".into(),
            first_run.into(),
            "--area".into(),
            "dead-code".into(),
        ],
    );
    assert_eq!(findings.exit_code, 0, "{}", findings.stderr);
    let findings_json: Value = serde_json::from_str(&findings.stdout)?;
    assert_eq!(
        findings_json.get("status").and_then(Value::as_str),
        Some("pruned")
    );

    let operation = crate::execute(
        root.path(),
        vec!["operation".into(), "show".into(), "cli-confirm".into()],
    );
    assert_eq!(operation.exit_code, 0, "{}", operation.stderr);
    let operation_json: Value = serde_json::from_str(&operation.stdout)?;
    assert_eq!(
        operation_json.get("schemaVersion").and_then(Value::as_str),
        Some("lumin.retention-operation.v1")
    );
    Ok(())
}

fn audit(root: &std::path::Path) -> crate::CommandOutput {
    let output = crate::execute(root, vec!["audit".into(), "--jobs".into(), "1".into()]);
    assert_eq!(output.exit_code, 0, "{}", output.stderr);
    output
}

fn run_id(output: &crate::CommandOutput) -> Result<String, Box<dyn std::error::Error>> {
    let body: Value = serde_json::from_str(&output.stdout)?;
    body.get("runId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "audit omitted runId".into())
}

fn assert_run_catalog(
    root: &std::path::Path,
    expected_run_ids: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    let output = crate::execute(root, vec!["runs".into(), "list".into()]);
    assert_eq!(output.exit_code, 0, "{}", output.stderr);
    let body: Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(
        body.get("schemaVersion").and_then(Value::as_str),
        Some("lumin.runs.v1")
    );
    assert_eq!(
        body.get("ordering").and_then(Value::as_str),
        Some("runs.v1")
    );
    assert_eq!(
        body.get("total").and_then(Value::as_u64),
        Some(expected_run_ids.len() as u64)
    );
    let actual = body
        .get("runs")
        .and_then(Value::as_array)
        .ok_or("run catalog omitted runs")?
        .iter()
        .map(|run| {
            run.get("runId")
                .and_then(Value::as_str)
                .ok_or("run omitted runId")
        })
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(actual, expected_run_ids);
    Ok(())
}
