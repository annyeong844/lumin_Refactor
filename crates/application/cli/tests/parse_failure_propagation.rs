use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

type FindingKey = (String, String, String);

#[test]
fn recoverable_parse_failures_preserve_module_uses_and_remain_file_scoped()
-> Result<(), Box<dyn std::error::Error>> {
    let root = recoverable_fixture()?;
    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    let audit_json: Value = serde_json::from_str(&audit.stdout)?;
    assert_eq!(required_str(&audit_json, "/status")?, "incomplete");
    assert_eq!(required_u64(&audit_json, "/limitationCount")?, 1);
    let run_id = field(&audit.stdout, "runId")?;

    let broken = file_response(root.path(), &run_id, "src/broken.ts")?;
    let target = file_response(root.path(), &run_id, "src/target.ts")?;
    let broken_id = required_str(&broken, "/sourceContext/sourceId")?;
    let target_id = required_str(&target, "/sourceContext/sourceId")?;
    let resolution = required_array(&broken, "/resolutions")?
        .iter()
        .find(|resolution| {
            resolution
                .pointer("/sourceUse/specifier")
                .and_then(Value::as_str)
                == Some("./target.js")
        })
        .ok_or_else(|| std::io::Error::other("recoverable import resolution is missing"))?;
    assert_eq!(required_str(resolution, "/outcome/kind")?, "internal");
    assert_eq!(required_str(resolution, "/outcome/target")?, target_id);

    let overview = overview(root.path(), &run_id)?;
    let limitations = required_array(&overview, "/limitations")?;
    assert_eq!(limitations.len(), 1);
    assert_eq!(
        required_str(&limitations[0], "/reason")?,
        "js-recoverable-parse-local"
    );
    assert_eq!(required_str(&limitations[0], "/source_id")?, broken_id);

    assert_eq!(
        finding_keys(root.path(), &run_id)?,
        BTreeSet::from([
            finding("src/target.ts", "deadSibling"),
            finding("src/unrelated.ts", "unrelatedDead"),
        ]),
        "the recovered import was lost or the file-local gap escaped its source",
    );

    let unrelated_gate = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-recoverable-unrelated",
            "--path",
            "src/safe.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&unrelated_gate, 0);
    let unrelated_gate: Value = serde_json::from_str(&unrelated_gate.stdout)?;
    assert!(
        !has_signal(&unrelated_gate, "required-evidence-incomplete"),
        "a file-local parse gap blocked an unrelated write: {unrelated_gate:#?}",
    );

    let consumer_gate = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-recoverable-consumer",
            "--path",
            "src/consumer.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&consumer_gate, 4);
    let consumer_gate: Value = serde_json::from_str(&consumer_gate.stdout)?;
    assert_eq!(required_str(&consumer_gate, "/decision")?, "incomplete");
    assert!(has_signal(&consumer_gate, "required-evidence-incomplete"));

    let broken_gate = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-recoverable-broken",
            "--path",
            "src/broken.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&broken_gate, 4);
    let broken_gate: Value = serde_json::from_str(&broken_gate.stdout)?;
    assert_eq!(required_str(&broken_gate, "/decision")?, "incomplete");
    assert!(has_signal(&broken_gate, "required-evidence-incomplete"));
    Ok(())
}

#[test]
fn unrecoverable_parse_failures_block_workspace_absence_and_gates()
-> Result<(), Box<dyn std::error::Error>> {
    let root = unrecoverable_fixture()?;
    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    let audit_json: Value = serde_json::from_str(&audit.stdout)?;
    assert_eq!(required_str(&audit_json, "/status")?, "incomplete");
    assert_eq!(required_u64(&audit_json, "/findingCount")?, 0);
    assert_eq!(required_u64(&audit_json, "/limitationCount")?, 1);
    let run_id = field(&audit.stdout, "runId")?;

    let broken = file_response(root.path(), &run_id, "src/broken.ts")?;
    let broken_id = required_str(&broken, "/sourceContext/sourceId")?;
    let overview = overview(root.path(), &run_id)?;
    let limitations = required_array(&overview, "/limitations")?;
    assert_eq!(limitations.len(), 1);
    assert_eq!(
        required_str(&limitations[0], "/reason")?,
        "js-module-use-unknown"
    );
    assert_eq!(required_str(&limitations[0], "/source_id")?, broken_id);
    assert!(finding_keys(root.path(), &run_id)?.is_empty());

    let gate = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-unrecoverable-unrelated",
            "--path",
            "src/safe.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&gate, 4);
    let gate: Value = serde_json::from_str(&gate.stdout)?;
    assert_eq!(required_str(&gate, "/decision")?, "incomplete");
    assert!(has_signal(&gate, "required-evidence-incomplete"));
    Ok(())
}

fn recoverable_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"recoverable-parse","private":true,"type":"module"}"#,
    )?;
    write(
        root.path(),
        "src/broken.ts",
        concat!(
            "import { used } from './target.js';\n",
            "console.log(used);\n",
            "export const visible = 1;\n",
            "export const hiddenLocal;\n",
        ),
    )?;
    write(
        root.path(),
        "src/consumer.ts",
        "import { visible } from './broken.js'; console.log(visible);\n",
    )?;
    write(
        root.path(),
        "src/target.ts",
        "export const used = 1; export const deadSibling = 2;\n",
    )?;
    write(
        root.path(),
        "src/unrelated.ts",
        "export const unrelatedDead = 1;\n",
    )?;
    write(root.path(), "src/safe.ts", "console.log('safe');\n")?;
    Ok(root)
}

fn unrecoverable_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"unrecoverable-parse","private":true,"type":"module"}"#,
    )?;
    write(root.path(), "src/broken.ts", "export const = ;\n")?;
    write(
        root.path(),
        "src/unrelated.ts",
        "export const mustNotBecomeADeletionCandidate = 1;\n",
    )?;
    write(root.path(), "src/safe.ts", "console.log('safe');\n")?;
    Ok(root)
}

fn finding_keys(
    root: &Path,
    run_id: &str,
) -> Result<BTreeSet<FindingKey>, Box<dyn std::error::Error>> {
    let output = run(root, &["findings", "--run", run_id, "--area", "dead-code"])?;
    assert_status(&output, 0);
    let response: Value = serde_json::from_str(&output.stdout)?;
    required_array(&response, "/items")?
        .iter()
        .map(|item| {
            Ok((
                required_str(item, "/path/display")?.to_owned(),
                required_str(item, "/exportedName")?.to_owned(),
                required_str(item, "/namespace")?.to_owned(),
            ))
        })
        .collect::<Result<_, std::io::Error>>()
        .map_err(Into::into)
}

fn finding(path: &str, name: &str) -> FindingKey {
    (path.to_owned(), name.to_owned(), "value".to_owned())
}

fn has_signal(response: &Value, kind: &str) -> bool {
    response
        .get("signals")
        .and_then(Value::as_array)
        .is_some_and(|signals| {
            signals
                .iter()
                .any(|signal| signal.get("kind").and_then(Value::as_str) == Some(kind))
        })
}

fn overview(root: &Path, run_id: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let output = run(root, &["overview", "--run", run_id])?;
    assert_status(&output, 0);
    Ok(serde_json::from_str(&output.stdout)?)
}

fn file_response(
    root: &Path,
    run_id: &str,
    path: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let output = run(root, &["files", "--run", run_id, path])?;
    assert_status(&output, 0);
    Ok(serde_json::from_str(&output.stdout)?)
}

fn required_array<'a>(value: &'a Value, pointer: &str) -> Result<&'a Vec<Value>, std::io::Error> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other(format!("missing array at {pointer}")))
}

fn required_str<'a>(value: &'a Value, pointer: &str) -> Result<&'a str, std::io::Error> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other(format!("missing string at {pointer}")))
}

fn required_u64(value: &Value, pointer: &str) -> Result<u64, std::io::Error> {
    value
        .pointer(pointer)
        .and_then(Value::as_u64)
        .ok_or_else(|| std::io::Error::other(format!("missing integer at {pointer}")))
}

fn write(root: &Path, relative: &str, contents: &str) -> Result<(), std::io::Error> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}
