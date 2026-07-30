use std::collections::BTreeSet;
use std::fs;

use serde_json::Value;

use super::support::{assert_status, field, run};

const FINDING_COUNT: usize = 101;

#[test]
fn gate_reopens_exact_revision_and_pages_immutable_evidence()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let opened = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "reopen-open",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);
    let gate_id = field(&opened.stdout, "gateId")?;
    assert_eq!(required_u64(&json(&opened.stdout)?, "/revision")?, 0);

    let closed = run(
        root.path(),
        &["post-write", &gate_id, "--operation-id", "reopen-close"],
    )?;
    assert_status(&closed, 0);
    let closed_json = json(&closed.stdout)?;
    assert_eq!(required_u64(&closed_json, "/revision")?, 1);
    assert_eq!(required_string(&closed_json, "/lifecycle")?, "closed");

    let shown = run(root.path(), &["gate", "show", &gate_id, "--revision", "1"])?;
    assert_status(&shown, 0);
    let shown_json = json(&shown.stdout)?;
    assert_eq!(required_string(&shown_json, "/gateId")?, gate_id);
    assert_eq!(required_u64(&shown_json, "/currentRevision")?, 1);
    assert_eq!(required_u64(&shown_json, "/selectedRevision")?, 1);
    assert_eq!(required_string(&shown_json, "/lifecycle")?, "closed");
    assert_eq!(required_u64(&shown_json, "/revisions/1/revision")?, 1);
    assert_eq!(
        required_string(&shown_json, "/revisions/1/operationId")?,
        "reopen-close"
    );

    let first = run(
        root.path(),
        &["gate", "findings", &gate_id, "--revision", "1"],
    )?;
    assert_status(&first, 0);
    let first_json = json(&first.stdout)?;
    assert_gate_scope(&first_json, &gate_id, 1)?;
    assert_eq!(required_string(&first_json, "/ordering")?, "findings.v1");
    assert_eq!(
        required_u64(&first_json, "/scopeTotal")?,
        FINDING_COUNT as u64
    );
    assert_eq!(required_u64(&first_json, "/total")?, FINDING_COUNT as u64);
    assert_eq!(required_u64(&first_json, "/returned")?, 100);
    assert!(required_bool(&first_json, "/truncated")?);
    let cursor = required_string(&first_json, "/nextCursor")?.to_owned();
    let mut finding_ids = item_ids(&first_json, "findingId")?;

    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    let run_id = field(&audit.stdout, "runId")?;

    let other_opened = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "reopen-other-open",
            "--path",
            "src/other.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&other_opened, 0);
    let other_gate_id = field(&other_opened.stdout, "gateId")?;
    let other_closed = run(
        root.path(),
        &[
            "post-write",
            &other_gate_id,
            "--operation-id",
            "reopen-other-close",
        ],
    )?;
    assert_status(&other_closed, 0);

    let cross_gate = run(
        root.path(),
        &[
            "gate",
            "findings",
            &other_gate_id,
            "--revision",
            "1",
            "--cursor",
            &cursor,
        ],
    )?;
    assert_status(&cross_gate, 2);
    assert!(cross_gate.stderr.contains("cursor scope"));

    let cross_revision = run(
        root.path(),
        &[
            "gate",
            "findings",
            &gate_id,
            "--revision",
            "0",
            "--cursor",
            &cursor,
        ],
    )?;
    assert_status(&cross_revision, 2);
    assert!(cross_revision.stderr.contains("cursor scope"));

    let cross_run = run(
        root.path(),
        &[
            "findings",
            "--run",
            &run_id,
            "--area",
            "dead-code",
            "--cursor",
            &cursor,
        ],
    )?;
    assert_status(&cross_run, 2);

    let second = run(
        root.path(),
        &[
            "gate",
            "findings",
            &gate_id,
            "--revision",
            "1",
            "--cursor",
            &cursor,
        ],
    )?;
    assert_status(&second, 0);
    let second_json = json(&second.stdout)?;
    assert_gate_scope(&second_json, &gate_id, 1)?;
    assert_eq!(
        required_u64(&second_json, "/scopeTotal")?,
        FINDING_COUNT as u64
    );
    assert_eq!(required_u64(&second_json, "/total")?, FINDING_COUNT as u64);
    assert_eq!(required_u64(&second_json, "/returned")?, 1);
    assert!(!required_bool(&second_json, "/truncated")?);
    assert!(
        second_json
            .pointer("/nextCursor")
            .is_some_and(Value::is_null)
    );
    finding_ids.extend(item_ids(&second_json, "findingId")?);
    assert_eq!(finding_ids.len(), FINDING_COUNT);
    assert_eq!(
        finding_ids.iter().collect::<BTreeSet<_>>().len(),
        FINDING_COUNT
    );

    let finding_id = finding_ids
        .first()
        .ok_or_else(|| std::io::Error::other("gate finding pages were empty"))?;
    let explained = run(
        root.path(),
        &["gate", "explain", &gate_id, "--revision", "1", finding_id],
    )?;
    assert_status(&explained, 0);
    let explained_json = json(&explained.stdout)?;
    assert_gate_scope(&explained_json, &gate_id, 1)?;
    assert_eq!(
        required_string(&explained_json, "/finding/findingId")?,
        finding_id
    );
    assert_nested_collection(&explained_json, "/evidence", "evidence.v1", 1)?;
    assert_nested_collection(&explained_json, "/relations", "relations.v1", 0)?;
    assert_eq!(
        required_string(&explained_json, "/evidence/items/0/kind")?,
        "definition"
    );
    assert!(
        required_string(&explained_json, "/evidence/items/0/evidenceId")?.starts_with("evidence_")
    );
    assert_eq!(
        required_string(&explained_json, "/evidence/items/0/payloadSha256")?.len(),
        64
    );
    Ok(())
}

fn fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    let mut exports = String::new();
    for index in 0..FINDING_COUNT {
        exports.push_str(&format!("export const dead{index:03} = {index};\n"));
    }
    fs::write(root.path().join("src/lib.ts"), exports)?;
    fs::write(root.path().join("src/other.ts"), "console.log('other');\n")?;
    Ok(root)
}

fn assert_gate_scope(
    value: &Value,
    gate_id: &str,
    revision: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(required_string(value, "/scope/kind")?, "gate-attempt");
    assert_eq!(required_string(value, "/scope/gateId")?, gate_id);
    assert_eq!(required_u64(value, "/scope/revision")?, revision);
    Ok(())
}

fn assert_nested_collection(
    value: &Value,
    pointer: &str,
    ordering: &str,
    total: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let collection = value
        .pointer(pointer)
        .ok_or_else(|| std::io::Error::other(format!("missing {pointer}")))?;
    assert_gate_scope(collection, required_string(value, "/scope/gateId")?, 1)?;
    assert_eq!(required_string(collection, "/ordering")?, ordering);
    assert_eq!(required_u64(collection, "/scopeTotal")?, total);
    assert_eq!(required_u64(collection, "/total")?, total);
    assert_eq!(required_u64(collection, "/returned")?, total);
    assert!(!required_bool(collection, "/truncated")?);
    assert!(
        collection
            .pointer("/nextCursor")
            .is_some_and(Value::is_null)
    );
    Ok(())
}

fn item_ids(value: &Value, field: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    value
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("collection omitted items"))?
        .iter()
        .map(|item| {
            item.get(field)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| std::io::Error::other(format!("item omitted {field}")))
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn json(stdout: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(stdout)
}

fn required_string<'a>(
    value: &'a Value,
    pointer: &str,
) -> Result<&'a str, Box<dyn std::error::Error>> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other(format!("missing string {pointer}")).into())
}

fn required_u64(value: &Value, pointer: &str) -> Result<u64, Box<dyn std::error::Error>> {
    value
        .pointer(pointer)
        .and_then(Value::as_u64)
        .ok_or_else(|| std::io::Error::other(format!("missing u64 {pointer}")).into())
}

fn required_bool(value: &Value, pointer: &str) -> Result<bool, Box<dyn std::error::Error>> {
    value
        .pointer(pointer)
        .and_then(Value::as_bool)
        .ok_or_else(|| std::io::Error::other(format!("missing bool {pointer}")).into())
}
