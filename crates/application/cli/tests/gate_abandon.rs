use std::{fs, path::Path};

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

#[test]
fn abandon_survives_process_reopen_and_refuses_a_second_terminal_revision()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let opened = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-abandon-open",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);
    let gate_id = field(&opened.stdout, "gateId")?;
    let command = [
        "gate",
        "abandon",
        gate_id.as_str(),
        "--operation-id",
        "op-abandon",
        "--reason",
        "planned edit cancelled",
    ];

    let first = run(root.path(), &command)?;
    assert_status(&first, 0);
    let first_json: Value = serde_json::from_str(&first.stdout)?;
    assert_eq!(
        first_json.get("lifecycle").and_then(Value::as_str),
        Some("abandoned")
    );
    assert_eq!(
        first_json.get("reason").and_then(Value::as_str),
        Some("planned edit cancelled")
    );
    assert_eq!(first_json.get("revision").and_then(Value::as_u64), Some(1));

    let retry = run(root.path(), &command)?;
    assert_status(&retry, 0);
    assert_eq!(retry.stdout, first.stdout);

    assert_abandon_views(root.path(), &gate_id)?;
    assert_abandon_conflicts(root.path(), &gate_id)?;
    Ok(())
}

fn assert_abandon_views(root: &Path, gate_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let shown = run(root, &["gate", "show", gate_id])?;
    assert_status(&shown, 0);
    let shown_json: Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(
        shown_json.get("lifecycle").and_then(Value::as_str),
        Some("abandoned")
    );
    assert_eq!(
        shown_json.get("currentRevision").and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        shown_json
            .pointer("/revisions/1/reason")
            .and_then(Value::as_str),
        Some("planned edit cancelled")
    );
    assert_eq!(
        shown_json
            .get("leasedWriteSet")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );

    let operation = run(root, &["operation", "show", "op-abandon"])?;
    assert_status(&operation, 0);
    let operation_json: Value = serde_json::from_str(&operation.stdout)?;
    assert_eq!(
        operation_json.get("kind").and_then(Value::as_str),
        Some("gate-abandon")
    );
    assert_eq!(
        operation_json.get("status").and_then(Value::as_str),
        Some("committed")
    );
    assert_eq!(
        operation_json.get("targetRevision").and_then(Value::as_u64),
        Some(0)
    );
    assert_eq!(
        operation_json.get("reason").and_then(Value::as_str),
        Some("planned edit cancelled")
    );
    assert_eq!(
        operation_json
            .pointer("/result/reason")
            .and_then(Value::as_str),
        Some("planned edit cancelled")
    );
    Ok(())
}

fn assert_abandon_conflicts(root: &Path, gate_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let conflicting_retry = run(
        root,
        &[
            "gate",
            "abandon",
            gate_id,
            "--operation-id",
            "op-abandon",
            "--reason",
            "different reason",
        ],
    )?;
    assert_status(&conflicting_retry, 2);
    assert!(
        conflicting_retry
            .stderr
            .contains("reused with a different request")
    );

    let second = run(
        root,
        &[
            "gate",
            "abandon",
            gate_id,
            "--operation-id",
            "op-second-abandon",
            "--reason",
            "second terminal attempt",
        ],
    )?;
    assert_status(&second, 2);
    assert!(second.stderr.contains("gate is not active"));
    let final_gate = run(root, &["gate", "show", gate_id])?;
    assert_status(&final_gate, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&final_gate.stdout)?
            .get("revisions")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(2)
    );
    Ok(())
}

#[test]
fn abandon_requires_a_nonempty_reason() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let result = run(
        root.path(),
        &[
            "gate",
            "abandon",
            "gate_1",
            "--operation-id",
            "op-abandon",
            "--reason",
            "",
        ],
    )?;
    assert_status(&result, 2);
    assert_eq!(result.stderr, "lumin: abandon reason must not be empty\n");
    Ok(())
}

#[test]
fn copied_managed_parent_hard_stops_before_public_gate_mutation()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let opened = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-parent-swap-open",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);
    let gate_id = field(&opened.stdout, "gateId")?;

    let state_dir = root.path().join(".lumin");
    let runs = state_dir.join("runs");
    let original = state_dir.join("runs.displaced");
    fs::rename(&runs, &original)?;
    copy_directory(&original, &runs)?;

    let rejected = run(
        root.path(),
        &[
            "gate",
            "abandon",
            gate_id.as_str(),
            "--operation-id",
            "op-parent-swap",
            "--reason",
            "must not reach the copied parent",
        ],
    )?;
    assert_status(&rejected, 1);
    assert!(rejected.stdout.is_empty());
    assert!(
        rejected
            .stderr
            .contains("state namespace integrity failure")
    );

    fs::remove_dir_all(&runs)?;
    fs::rename(&original, &runs)?;
    let gate = run(root.path(), &["gate", "show", gate_id.as_str()])?;
    assert_status(&gate, 0);
    let gate_json: Value = serde_json::from_str(&gate.stdout)?;
    assert_eq!(
        gate_json.get("lifecycle").and_then(Value::as_str),
        Some("active")
    );
    assert_eq!(
        gate_json.get("currentRevision").and_then(Value::as_u64),
        Some(0)
    );
    let operation = run(root.path(), &["operation", "show", "op-parent-swap"])?;
    assert_status(&operation, 2);
    Ok(())
}

fn copy_directory(source: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(root.path().join("src/lib.ts"), "export const used = 1;\n")?;
    fs::write(
        root.path().join("src/main.ts"),
        "import { used } from './lib'; console.log(used);\n",
    )?;
    Ok(root)
}
