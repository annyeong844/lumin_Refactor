use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

struct ProcessResult {
    status: i32,
    stdout: String,
    stderr: String,
}

#[test]
fn pre_and_post_survive_process_reopen() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let pre = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-open",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&pre, 0);
    let gate_id = field(&pre.stdout, "gateId")?;

    let retry = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-open",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&retry, 0);
    assert_eq!(pre.stdout, retry.stdout);

    fs::write(root.path().join("src/lib.ts"), "export const used = 2;\n")?;
    let post = run(
        root.path(),
        &["post-write", &gate_id, "--operation-id", "op-close"],
    )?;
    assert_status(&post, 0);
    let post_json: Value = serde_json::from_str(&post.stdout)?;
    assert_eq!(
        post_json.get("decision").and_then(Value::as_str),
        Some("allow")
    );
    assert_eq!(
        post_json.get("lifecycle").and_then(Value::as_str),
        Some("closed")
    );
    let post_retry = run(
        root.path(),
        &["post-write", &gate_id, "--operation-id", "op-close"],
    )?;
    assert_status(&post_retry, 0);
    assert_eq!(post.stdout, post_retry.stdout);

    let shown = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&shown, 0);
    let shown_json: Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(
        shown_json.get("lifecycle").and_then(Value::as_str),
        Some("closed")
    );
    assert_eq!(
        shown_json.get("currentRevision").and_then(Value::as_u64),
        Some(1)
    );

    let operation = run(root.path(), &["operation", "show", "op-close"])?;
    assert_status(&operation, 0);
    let operation_json: Value = serde_json::from_str(&operation.stdout)?;
    assert_eq!(
        operation_json.get("status").and_then(Value::as_str),
        Some("committed")
    );
    assert_eq!(
        operation_json
            .pointer("/result/decision")
            .and_then(Value::as_str),
        Some("allow")
    );
    Ok(())
}

#[test]
fn overlapping_gate_is_rejected_and_operation_reuse_is_malformed()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let first = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-first",
            "--path",
            "src/main.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&first, 0);

    let overlap = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-overlap",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&overlap, 4);
    let overlap_json: Value = serde_json::from_str(&overlap.stdout)?;
    assert_eq!(
        overlap_json.get("lifecycle").and_then(Value::as_str),
        Some("rejected")
    );
    assert_eq!(
        overlap_json
            .pointer("/signals/0/kind")
            .and_then(Value::as_str),
        Some("write-conflict")
    );

    let reused = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-first",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&reused, 2);
    assert!(reused.stderr.contains("reused with a different request"));
    Ok(())
}

#[test]
fn changed_semantic_evidence_is_incomplete_until_delta_policy_exists()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let pre = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-open",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&pre, 0);
    let gate_id = field(&pre.stdout, "gateId")?;
    fs::write(
        root.path().join("src/lib.ts"),
        "export const renamed = 1;\n",
    )?;

    let post = run(
        root.path(),
        &["post-write", &gate_id, "--operation-id", "op-close"],
    )?;
    assert_status(&post, 4);
    let post_json: Value = serde_json::from_str(&post.stdout)?;
    assert_eq!(
        post_json.get("decision").and_then(Value::as_str),
        Some("incomplete")
    );
    assert_eq!(
        post_json.get("lifecycle").and_then(Value::as_str),
        Some("active")
    );
    assert!(
        post_json
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some("semantic-delta-unsupported")
            }))
    );
    Ok(())
}

#[test]
fn unexpected_new_source_denies_and_keeps_the_gate_active() -> Result<(), Box<dyn std::error::Error>>
{
    let root = fixture()?;
    let pre = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-open",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&pre, 0);
    let gate_id = field(&pre.stdout, "gateId")?;
    fs::write(
        root.path().join("src/extra.ts"),
        "export const extra = 1;\n",
    )?;

    let post = run(
        root.path(),
        &["post-write", &gate_id, "--operation-id", "op-close"],
    )?;
    assert_status(&post, 3);
    let post_json: Value = serde_json::from_str(&post.stdout)?;
    assert_eq!(
        post_json.get("decision").and_then(Value::as_str),
        Some("deny")
    );
    assert_eq!(
        post_json.get("lifecycle").and_then(Value::as_str),
        Some("active")
    );
    assert!(
        post_json
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some("unplanned-write")
            }))
    );
    Ok(())
}

#[test]
fn protected_input_drift_is_stale() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let pre = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-open",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&pre, 0);
    let gate_id = field(&pre.stdout, "gateId")?;
    fs::write(
        root.path().join("src/main.ts"),
        "import { used } from './lib';\nconsole.log(used);\n",
    )?;

    let post = run(
        root.path(),
        &["post-write", &gate_id, "--operation-id", "op-close"],
    )?;
    assert_status(&post, 5);
    let post_json: Value = serde_json::from_str(&post.stdout)?;
    assert_eq!(
        post_json.get("decision").and_then(Value::as_str),
        Some("stale")
    );
    assert_eq!(
        post_json.get("lifecycle").and_then(Value::as_str),
        Some("active")
    );
    Ok(())
}

#[test]
fn unsupported_non_source_path_is_queryable_incomplete() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    fs::write(root.path().join("notes.md"), "not a source input\n")?;
    let pre = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-notes",
            "--path",
            "notes.md",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&pre, 4);
    let pre_json: Value = serde_json::from_str(&pre.stdout)?;
    assert_eq!(
        pre_json.get("decision").and_then(Value::as_str),
        Some("incomplete")
    );
    assert_eq!(
        pre_json.get("lifecycle").and_then(Value::as_str),
        Some("rejected")
    );
    assert_eq!(
        pre_json
            .pointer("/signals/0/reason")
            .and_then(Value::as_str),
        Some("not-analyzed-source")
    );

    let operation = run(root.path(), &["operation", "show", "op-notes"])?;
    assert_status(&operation, 0);
    let operation_json: Value = serde_json::from_str(&operation.stdout)?;
    assert_eq!(
        operation_json
            .pointer("/result/decision")
            .and_then(Value::as_str),
        Some("incomplete")
    );
    Ok(())
}

#[test]
fn missing_operation_is_a_typed_hard_stop() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let operation = run(root.path(), &["operation", "show", "op-missing"])?;

    assert_status(&operation, 2);
    assert!(operation.stdout.is_empty());
    assert_eq!(
        operation.stderr,
        "lumin: operation does not exist: op-missing\n"
    );

    let gate = run(root.path(), &["gate", "show", "gate_missing"])?;
    assert_status(&gate, 2);
    assert!(gate.stdout.is_empty());
    assert_eq!(gate.stderr, "lumin: gate does not exist: gate_missing\n");
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

fn run(root: &Path, arguments: &[&str]) -> Result<ProcessResult, Box<dyn std::error::Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_lumin"))
        .current_dir(root)
        .args(arguments)
        .output()?;
    Ok(ProcessResult {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8(output.stdout)?,
        stderr: String::from_utf8(output.stderr)?,
    })
}

fn assert_status(result: &ProcessResult, expected: i32) {
    assert_eq!(
        result.status, expected,
        "stdout={}\nstderr={}",
        result.stdout, result.stderr
    );
}

fn field(json: &str, name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let value: Value = serde_json::from_str(json)?;
    value
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other(format!("missing string field {name}")).into())
}
