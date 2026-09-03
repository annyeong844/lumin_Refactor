#[allow(dead_code)]
mod support;

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
        "lumin overview --format json",
        "lumin findings --run <run-id> --area dead-code --format json",
        "lumin explain --run <run-id> <finding-id> --format json",
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
            .contains("status, result, and lastDeliveryStatus")
    );
    assert!(
        output
            .stdout
            .contains("not-attempted, unknown, succeeded, or failed")
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
fn help_agent_query_examples_execute_through_the_public_binary()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(
        root.path().join("src/lib.ts"),
        "export const used = 1; export const dead = 2;\n",
    )?;
    fs::write(
        root.path().join("src/main.ts"),
        "import { used } from './lib.js'; console.log(used);\n",
    )?;

    let help = run(root.path(), &["help-agent"])?;
    assert_stage_status("help-agent", &help, 0);
    let audit_line = help
        .stdout
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("lumin audit "))
        .ok_or("help-agent omitted its audit command")?;
    let audit_arguments = command_arguments(audit_line, "<run-id>", None);
    let audit_refs = audit_arguments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let audit = run(root.path(), &audit_refs)?;
    assert_stage_status("audit", &audit, 0);
    let run_id = support::field(&audit.stdout, "runId")?;

    let findings_line = help
        .stdout
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("lumin findings "))
        .ok_or("help-agent omitted its findings command")?;
    let findings_arguments = command_arguments(findings_line, &run_id, None);
    let findings_refs = findings_arguments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let findings = run(root.path(), &findings_refs)?;
    assert_stage_status("findings example", &findings, 0);
    let finding_id = serde_json::from_str::<Value>(&findings.stdout)?
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
    let explain_arguments = command_arguments(explain_line, &run_id, Some(&finding_id));
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

fn command_arguments(line: &str, run_id: &str, finding_id: Option<&str>) -> Vec<String> {
    line.replace("<run-id>", run_id)
        .replace("<finding-id>", finding_id.unwrap_or("<finding-id>"))
        .split_whitespace()
        .skip(1)
        .map(str::to_owned)
        .collect()
}
