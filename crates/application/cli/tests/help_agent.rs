#[allow(dead_code)]
mod support;

use std::collections::BTreeSet;
use std::fs;

use serde_json::Value;
use support::{assert_status, run};

#[test]
fn help_agent_owns_the_recovery_workflow_without_creating_state()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let output = run(root.path(), &["help-agent"])?;
    assert_status(&output, 0);
    assert!(output.stderr.is_empty());
    assert!(output.stdout.starts_with("Lumin agent workflow\n"));
    for command in [
        "lumin audit --jobs 1 --format json",
        "lumin overview --run <run-id> --format json",
        "lumin findings --run <run-id> --area dead-code --format json",
        "lumin explain --run <run-id> <finding-id> --format json",
        "lumin related --run <run-id> <finding-id> --format json",
        "lumin pre-write --operation-id <operation-id> --path <repo-path> --format json",
        "lumin post-write <gate-id> --operation-id <operation-id> --format json",
        "lumin gate abandon <gate-id> --operation-id <operation-id> --reason <reason> --format json",
        "lumin runs pin <run-id> --operation-id <operation-id> --reason <reason> --format json",
        "lumin runs unpin <pin-id> --operation-id <operation-id> --format json",
        "lumin runs prune plan --before <unix-millis> --operation-id <operation-id> --format json",
        "lumin runs prune confirm <plan-id> --operation-id <operation-id> --format json",
        "lumin gate prune plan --terminal-before <unix-millis> --operation-id <operation-id> --format json",
        "lumin gate prune confirm <plan-id> --operation-id <operation-id> --format json",
        "lumin cache clean --operation-id <operation-id> --format json",
        "lumin operation show <operation-id> --format json",
        "lumin store migrate --format json",
    ] {
        assert_eq!(
            output
                .stdout
                .lines()
                .filter(|line| line.trim() == command)
                .count(),
            1,
            "help-agent command projection differs for {command}"
        );
    }
    assert!(output.stdout.contains("lumin.cache-cleanup-operation.v2"));
    assert!(
        output
            .stdout
            .contains("Only decision values allow and allow-with-warnings authorize editing.")
    );
    assert!(output.stdout.contains("incomplete, and stale do not."));
    assert!(
        output
            .stdout
            .contains("status, result, and lastDeliveryStatus")
    );
    assert!(
        output
            .stdout
            .contains("not-attempted, unknown, succeeded, or failed")
    );
    assert!(
        output
            .stdout
            .contains("Operation show is read-only and never resumes work or changes liveness.")
    );
    assert!(
        output.stdout.contains(
            "gate and cache operations, consume result when status is committed; retry the"
        )
    );
    assert!(
        output.stdout.contains(
            "committed or stale; retry the identical mutation with the same operation ID"
        )
    );
    assert!(output.stdout.contains(
        "{\"schemaVersion\":\"lumin.lifecycle-store-migration.v1\",\"storeSchema\":\"lumin-lifecycle-store-header.v13\",\"status\":\"ready\"}",
    ));
    assert!(output.stdout.contains("Never read or modify .lumin."));
    assert!(!root.path().join(".lumin").exists());
    Ok(())
}

#[test]
fn help_agent_rejects_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let output = run(root.path(), &["help-agent", "extra"])?;
    assert_status(&output, 2);
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, "lumin: unknown command or argument: extra\n");
    assert!(!root.path().join(".lumin").exists());
    Ok(())
}

#[test]
fn command_help_exposes_owned_syntax_without_creating_state()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    for (command, required) in [
        ("audit", "lumin audit --jobs <count> --format json"),
        ("overview", "lumin overview --run <run-id> --format json"),
        (
            "findings",
            "lumin findings --run <run-id> --area dead-code --cursor <cursor> --format json",
        ),
        (
            "explain",
            "lumin explain --run <run-id> <finding-id> --evidence-cursor <cursor> --format json",
        ),
        (
            "related",
            "lumin related --run <run-id> <finding-id> --cursor <cursor> --format json",
        ),
        (
            "files",
            "lumin files --run <run-id> --cursor <cursor> --format json -- <repo-path>",
        ),
        (
            "capabilities",
            "lumin capabilities --run <run-id> --cursor <cursor> --format json",
        ),
        (
            "pre-write",
            "lumin pre-write --operation-id <operation-id> --path <repo-path> --format json",
        ),
        (
            "post-write",
            "lumin post-write <gate-id> --operation-id <operation-id> --format json",
        ),
        (
            "gate",
            "lumin gate findings <gate-id> --revision <revision> --cursor <cursor> --format json",
        ),
        (
            "operation",
            "lumin operation show -- <operation-id> --format json",
        ),
        ("runs", "lumin runs list --cursor <cursor> --format json"),
        (
            "cache",
            "lumin cache clean --operation-id <operation-id> --format json",
        ),
        ("store", "lumin store migrate --format json"),
        ("help-agent", "lumin help-agent"),
    ] {
        let output = run(root.path(), &[command, "--help"])?;
        assert_stage_status(&format!("{command} help"), &output, 0);
        assert!(output.stderr.is_empty());
        assert!(
            output
                .stdout
                .starts_with(&format!("Lumin command help: {command}\n")),
            "unexpected {command} help heading: {}",
            output.stdout
        );
        assert!(
            output.stdout.lines().any(|line| line.trim() == required),
            "{command} help omitted {required}: {}",
            output.stdout
        );
        assert!(!root.path().join(".lumin").exists());
    }
    for command in ["audit", "pre-write"] {
        let output = run(root.path(), &[command, "--help"])?;
        assert_stage_status(&format!("{command} override help"), &output, 0);
        for required in [
            "<role>: test | production | generated | vendor | authored",
            "<profile>: bundler | node | node10 | node16 | nodenext",
        ] {
            assert!(
                output.stdout.lines().any(|line| line.trim() == required),
                "{command} help omitted {required}: {}",
                output.stdout
            );
        }
    }
    let operation = run(root.path(), &["operation", "--help"])?;
    assert_stage_status("operation state help", &operation, 0);
    for required in [
        "Gate/cache committed: consume result without retrying the mutation.",
        "Gate/cache pending or interrupted: retry the identical mutation with the same operation ID.",
        "Retention committed or stale: consume result without retrying the mutation.",
        "Retention pruning: retry the identical mutation with the same operation ID.",
        "Operation show is read-only; repeated shows do not resume work or change liveness.",
    ] {
        assert!(
            operation.stdout.lines().any(|line| line.trim() == required),
            "operation help omitted {required}: {}",
            operation.stdout
        );
    }
    assert!(!root.path().join(".lumin").exists());
    Ok(())
}

#[test]
fn help_agent_query_examples_execute_through_the_public_binary()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(
        root.path().join("src/lib.ts"),
        format!(
            "export const used = 1;\n{}",
            (0..101)
                .map(|index| format!("export const dead{index:03} = {index};\n"))
                .collect::<String>()
        ),
    )?;
    fs::write(
        root.path().join("src/main.ts"),
        "import { used } from './lib.js'; console.log(used);\n",
    )?;
    fs::write(
        root.path().join("--generated.ts"),
        "console.log('generated');\n",
    )?;

    let help = run(root.path(), &["help-agent"])?;
    assert_stage_status("help-agent", &help, 0);
    let audit_line = help
        .stdout
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("lumin audit "))
        .ok_or("help-agent omitted its audit command")?;
    let audit_arguments = command_arguments(audit_line, "<run-id>", None, None);
    let audit_refs = audit_arguments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let audit = run(root.path(), &audit_refs)?;
    assert_stage_status("audit", &audit, 0);
    let run_id = support::field(&audit.stdout, "runId")?;

    let files_help = run(root.path(), &["files", "--help"])?;
    assert_stage_status("files command help", &files_help, 0);
    let option_path_line = files_help
        .stdout
        .lines()
        .map(str::trim)
        .find(|line| {
            line.starts_with("lumin files ")
                && line.ends_with("-- <repo-path>")
                && !line.contains("--cursor")
        })
        .ok_or("files command help omitted its option-shaped path form")?;
    let option_path_arguments = option_path_line
        .replace("<run-id>", &run_id)
        .replace("<repo-path>", "--generated.ts")
        .split_whitespace()
        .skip(1)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let option_path_refs = option_path_arguments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let option_path = run(root.path(), &option_path_refs)?;
    assert_stage_status("option-shaped files query", &option_path, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&option_path.stdout)?
            .pointer("/sourceContext/path/display")
            .and_then(Value::as_str),
        Some("--generated.ts")
    );

    let findings_line = help
        .stdout
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("lumin findings "))
        .ok_or("help-agent omitted its findings command")?;
    let findings_arguments = command_arguments(findings_line, &run_id, None, None);
    let findings_refs = findings_arguments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let findings = run(root.path(), &findings_refs)?;
    assert_stage_status("findings example", &findings, 0);
    let findings_json = serde_json::from_str::<Value>(&findings.stdout)?;
    assert_eq!(
        findings_json.get("total").and_then(Value::as_u64),
        Some(101)
    );
    assert_eq!(
        findings_json
            .get("items")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(100)
    );
    assert_eq!(
        findings_json.get("truncated").and_then(Value::as_bool),
        Some(true)
    );
    let cursor = findings_json
        .get("nextCursor")
        .and_then(Value::as_str)
        .ok_or("findings first page omitted nextCursor")?;

    let findings_help = run(root.path(), &["findings", "--help"])?;
    assert_stage_status("findings command help", &findings_help, 0);
    let continuation_line = findings_help
        .stdout
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("lumin findings ") && line.contains("--cursor <cursor>"))
        .ok_or("findings command help omitted its continuation form")?;
    let continuation_arguments = command_arguments(continuation_line, &run_id, None, Some(cursor));
    let continuation_refs = continuation_arguments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let continuation = run(root.path(), &continuation_refs)?;
    assert_stage_status("findings continuation", &continuation, 0);
    let continuation_json = serde_json::from_str::<Value>(&continuation.stdout)?;
    assert_eq!(
        continuation_json.get("total").and_then(Value::as_u64),
        Some(101)
    );
    assert_eq!(
        continuation_json
            .get("items")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        continuation_json.get("truncated").and_then(Value::as_bool),
        Some(false)
    );
    assert!(
        continuation_json
            .get("nextCursor")
            .is_none_or(Value::is_null)
    );
    let finding_ids = findings_json
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .chain(
            continuation_json
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        )
        .map(|item| {
            item.get("findingId")
                .and_then(Value::as_str)
                .ok_or("findings page item omitted findingId")
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    assert_eq!(finding_ids.len(), 101);

    let finding_id = findings_json
        .pointer("/items/0/findingId")
        .and_then(Value::as_str)
        .ok_or("help-agent findings command returned no finding ID")?
        .to_owned();

    let explain_line = help
        .stdout
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("lumin explain "))
        .ok_or("help-agent omitted its explain command")?;
    let explain_arguments = command_arguments(explain_line, &run_id, Some(&finding_id), None);
    let explain_refs = explain_arguments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let explain = run(root.path(), &explain_refs)?;
    assert_stage_status("explain example", &explain, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&explain.stdout)?
            .pointer("/finding/findingId")
            .and_then(Value::as_str),
        Some(finding_id.as_str())
    );
    Ok(())
}

fn assert_stage_status(stage: &str, result: &support::ProcessResult, expected: i32) {
    assert_eq!(
        result.status, expected,
        "{stage}: stdout={}\nstderr={}",
        result.stdout, result.stderr
    );
}

fn command_arguments(
    line: &str,
    run_id: &str,
    finding_id: Option<&str>,
    cursor: Option<&str>,
) -> Vec<String> {
    line.replace("<run-id>", run_id)
        .replace("<finding-id>", finding_id.unwrap_or("<finding-id>"))
        .replace("<cursor>", cursor.unwrap_or("<cursor>"))
        .split_whitespace()
        .skip(1)
        .map(str::to_owned)
        .collect()
}
