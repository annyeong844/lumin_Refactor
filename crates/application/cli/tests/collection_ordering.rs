mod support;

use std::fs;
use std::path::Path;

use serde_json::Value;

fn json(stdout: &str) -> Result<Value, Box<dyn std::error::Error>> {
    serde_json::from_str(stdout).map_err(Into::into)
}

// --- `lumin related` ---

#[test]
fn related_returns_relation_collection_for_run_finding() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    create_related_fixture(root.path())?;
    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);
    let run_id = support::field(&audit.stdout, "runId")?;

    // Get first finding that has nested collections
    let findings = support::run(
        root.path(),
        &["findings", "--run", &run_id, "--area", "dead-code"],
    )?;
    support::assert_status(&findings, 0);
    let findings_json = json(&findings.stdout)?;
    let items = findings_json
        .get("items")
        .and_then(Value::as_array)
        .ok_or("missing items")?;
    let finding_id = items[0]
        .get("findingId")
        .and_then(Value::as_str)
        .ok_or("missing findingId")?;

    let related = support::run(
        root.path(),
        &["related", "--run", &run_id, finding_id, "--format", "json"],
    )?;
    support::assert_status(&related, 0);
    let response = json(&related.stdout)?;
    assert_eq!(
        response.get("schemaVersion").and_then(Value::as_str),
        Some("lumin.collection.v1")
    );
    assert_eq!(
        response.get("ordering").and_then(Value::as_str),
        Some("relations.v1")
    );
    assert!(response.get("scopeTotal").and_then(Value::as_u64).is_some());
    assert!(response.get("total").and_then(Value::as_u64).is_some());
    assert!(response.get("items").and_then(Value::as_array).is_some());
    Ok(())
}

#[test]
fn related_missing_run_exits_2() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    create_related_fixture(root.path())?;
    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);

    let result = support::run(
        root.path(),
        &["related", "--run", "nonexistent-run", "finding-id"],
    )?;
    support::assert_status(&result, 2);
    Ok(())
}

// --- `lumin files` ---

#[test]
fn files_returns_file_findings_collection() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(root.path().join("src/a.ts"), "export const dead = 1;\n")?;
    fs::write(root.path().join("src/b.ts"), "export const other = 2;\n")?;
    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);
    let run_id = support::field(&audit.stdout, "runId")?;

    let files_result = support::run(
        root.path(),
        &["files", "--run", &run_id, "src/a.ts", "--format", "json"],
    )?;
    support::assert_status(&files_result, 0);
    let response = json(&files_result.stdout)?;
    assert_eq!(
        response.get("schemaVersion").and_then(Value::as_str),
        Some("lumin.collection.v1")
    );
    assert_eq!(
        response.get("ordering").and_then(Value::as_str),
        Some("file-findings.v1")
    );
    // scopeTotal = all run findings (2), total = matches for src/a.ts (1)
    assert_eq!(response.get("scopeTotal").and_then(Value::as_u64), Some(2));
    assert_eq!(response.get("total").and_then(Value::as_u64), Some(1));
    let items = response
        .get("items")
        .and_then(Value::as_array)
        .ok_or("missing items")?;
    assert_eq!(items.len(), 1);
    Ok(())
}

#[test]
fn files_zero_match_exits_0_empty() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("lib.ts"), "export const dead = 1;\n")?;
    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);
    let run_id = support::field(&audit.stdout, "runId")?;

    let files_result = support::run(root.path(), &["files", "--run", &run_id, "nonexistent.ts"])?;
    support::assert_status(&files_result, 0);
    let response = json(&files_result.stdout)?;
    assert_eq!(response.get("total").and_then(Value::as_u64), Some(0));
    assert_eq!(response.get("returned").and_then(Value::as_u64), Some(0));
    Ok(())
}

#[test]
fn files_invalid_path_exits_2() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("lib.ts"), "export const dead = 1;\n")?;
    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);
    let run_id = support::field(&audit.stdout, "runId")?;

    // Absolute path is invalid
    let result = support::run(
        root.path(),
        &["files", "--run", &run_id, "/absolute/path.ts"],
    )?;
    support::assert_status(&result, 2);
    Ok(())
}

// --- `lumin gate list` ---

#[test]
fn gate_list_requires_active_flag() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("lib.ts"), "export const dead = 1;\n")?;
    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);

    let result = support::run(root.path(), &["gate", "list"])?;
    support::assert_status(&result, 2);
    assert!(result.stdout.is_empty());
    Ok(())
}

#[test]
fn gate_list_active_returns_empty_collection() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("lib.ts"), "export const dead = 1;\n")?;
    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);

    let result = support::run(
        root.path(),
        &["gate", "list", "--active", "--format", "json"],
    )?;
    support::assert_status(&result, 0);
    let response = json(&result.stdout)?;
    assert_eq!(
        response.get("schemaVersion").and_then(Value::as_str),
        Some("lumin.active-gates.v1")
    );
    assert_eq!(
        response.get("ordering").and_then(Value::as_str),
        Some("active-gates.v1")
    );
    assert_eq!(response.get("scopeTotal").and_then(Value::as_u64), Some(0));
    assert_eq!(response.get("total").and_then(Value::as_u64), Some(0));
    assert_eq!(response.get("returned").and_then(Value::as_u64), Some(0));
    assert_eq!(
        response.get("truncated").and_then(Value::as_bool),
        Some(false)
    );
    assert!(
        response.get("nextCursor").is_none() || response.get("nextCursor") == Some(&Value::Null)
    );
    Ok(())
}

#[test]
fn gate_list_active_shows_open_gate() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("lib.ts"), "export const dead = 1;\n")?;
    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);

    // Open a gate
    let pre_write = support::run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-gate-list-1",
            "--path",
            "lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    support::assert_status(&pre_write, 0);
    let pre_write_json = json(&pre_write.stdout)?;
    let gate_id = pre_write_json
        .get("gateId")
        .and_then(Value::as_str)
        .ok_or("missing gateId")?;
    let lifecycle = pre_write_json
        .get("lifecycle")
        .and_then(Value::as_str)
        .ok_or("missing lifecycle")?;
    assert_eq!(lifecycle, "active");

    let result = support::run(root.path(), &["gate", "list", "--active"])?;
    support::assert_status(&result, 0);
    let response = json(&result.stdout)?;
    assert_eq!(response.get("scopeTotal").and_then(Value::as_u64), Some(1));
    assert_eq!(response.get("total").and_then(Value::as_u64), Some(1));
    let items = response
        .get("items")
        .and_then(Value::as_array)
        .ok_or("missing items")?;
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].get("gateId").and_then(Value::as_str),
        Some(gate_id)
    );
    assert!(
        items[0]
            .get("openingTransitionSequence")
            .and_then(Value::as_u64)
            .is_some()
    );
    Ok(())
}

#[test]
fn gate_list_active_malformed_cursor_exits_2() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("lib.ts"), "export const dead = 1;\n")?;
    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);

    let result = support::run(
        root.path(),
        &["gate", "list", "--active", "--cursor", "not-valid-base64!"],
    )?;
    support::assert_status(&result, 2);
    assert!(result.stdout.is_empty());
    Ok(())
}

// --- Helpers ---

fn create_related_fixture(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(root.join("src"))?;
    // src/lib.ts exports "dead" with zero production fan-in.
    fs::write(root.join("src/lib.ts"), "export const dead = 1;\n")?;
    // One test file that re-exports "dead" from lib.ts.
    fs::write(
        root.join("src/test.test.ts"),
        "export { dead as testDead } from './lib.js';\n",
    )?;
    Ok(())
}
