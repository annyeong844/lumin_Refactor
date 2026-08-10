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
    fs::write(
        root.path().join("src/a.ts"),
        "export const first = 1;\nexport const second = 2;\n",
    )?;
    fs::write(root.path().join("src/b.ts"), "export const third = 3;\n")?;
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
    // scopeTotal = all run findings (3), total = matches for src/a.ts (2).
    assert_eq!(response.get("scopeTotal").and_then(Value::as_u64), Some(3));
    assert_eq!(response.get("total").and_then(Value::as_u64), Some(2));
    assert_eq!(response.get("returned").and_then(Value::as_u64), Some(2));
    assert_eq!(
        response.get("truncated").and_then(Value::as_bool),
        Some(false)
    );
    let items = response
        .get("items")
        .and_then(Value::as_array)
        .ok_or("missing items")?;
    assert_eq!(items.len(), 2);
    let exported_names = items
        .iter()
        .map(|item| item.get("exportedName").and_then(Value::as_str))
        .collect::<Option<Vec<_>>>()
        .ok_or("file finding omitted exportedName")?;
    assert_eq!(exported_names, ["first", "second"]);
    let spans = items
        .iter()
        .map(|item| {
            Some((
                item.pointer("/span/start")?.as_u64()?,
                item.pointer("/span/end")?.as_u64()?,
            ))
        })
        .collect::<Option<Vec<_>>>()
        .ok_or("file finding omitted source span")?;
    assert!(spans[0] < spans[1], "file-findings.v1 order regressed");
    let finding_ids = items
        .iter()
        .map(|item| item.get("findingId").and_then(Value::as_str))
        .collect::<Option<Vec<_>>>()
        .ok_or("file finding omitted findingId")?;
    assert_ne!(
        finding_ids[0], finding_ids[1],
        "file findings must traverse exactly once"
    );
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
fn gate_list_active_orders_open_gates() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(root.path().join("src/a.ts"), "console.log('a');\n")?;
    fs::write(root.path().join("src/b.ts"), "console.log('b');\n")?;
    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);

    let gate_a = open_active_gate(root.path(), "op-gate-list-a", "src/a.ts")?;
    let gate_b = open_active_gate(root.path(), "op-gate-list-b", "src/b.ts")?;

    let result = support::run(root.path(), &["gate", "list", "--active"])?;
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
    assert_eq!(response.get("scopeTotal").and_then(Value::as_u64), Some(2));
    assert_eq!(response.get("total").and_then(Value::as_u64), Some(2));
    assert_eq!(response.get("returned").and_then(Value::as_u64), Some(2));
    let items = response
        .get("items")
        .and_then(Value::as_array)
        .ok_or("missing items")?;
    assert_eq!(items.len(), 2);
    let observed_gate_ids = items
        .iter()
        .map(|item| {
            item.get("gateId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect::<Option<Vec<_>>>()
        .ok_or("active gate omitted gateId")?;
    assert_eq!(observed_gate_ids, [gate_a, gate_b]);
    let order_keys = items
        .iter()
        .map(|item| {
            Some((
                item.get("openingTransitionSequence")?.as_u64()?,
                item.get("gateId")?.as_str()?.to_owned(),
            ))
        })
        .collect::<Option<Vec<_>>>()
        .ok_or("active gate omitted ordering fields")?;
    assert!(
        order_keys[0] < order_keys[1],
        "active-gates.v1 order regressed"
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

fn open_active_gate(
    root: &Path,
    operation_id: &str,
    path: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let pre_write = support::run(
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
    support::assert_status(&pre_write, 0);
    let response = json(&pre_write.stdout)?;
    assert_eq!(
        response.get("lifecycle").and_then(Value::as_str),
        Some("active")
    );
    response
        .get("gateId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other("pre-write omitted gateId").into())
}

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
