use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

type FindingKey = (String, String, String);

const MAIN_SOURCE: &str = concat!(
    "const key = process.argv[2] ?? 'first';\n",
    "console.log(require('./member.js')[key]);\n",
    "const { [key]: selected } = require('./destructure.js');\n",
    "console.log(selected);\n",
);

const TEST_SOURCE: &str = concat!(
    "const key = process.argv[2] ?? 'first';\n",
    "console.log(require('../src/test-target.js')[key]);\n",
);

#[test]
fn computed_commonjs_access_is_module_scoped_broad_value_evidence()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let audit = run(
        root.path(),
        &["audit", "--jobs", "1", "--resolution-profile", "node16"],
    )?;
    assert_status(&audit, 0);
    let audit_json: Value = serde_json::from_str(&audit.stdout)?;
    assert_eq!(
        audit_json.get("status").and_then(Value::as_str),
        Some("incomplete")
    );
    assert_eq!(
        audit_json.get("limitationCount").and_then(Value::as_u64),
        Some(3)
    );
    let run_id = field(&audit.stdout, "runId")?;

    let main = file_response(root.path(), &run_id, "src/main.ts")?;
    let test = file_response(root.path(), &run_id, "tests/consumer.test.ts")?;
    let member = file_response(root.path(), &run_id, "src/member.ts")?;
    let destructure = file_response(root.path(), &run_id, "src/destructure.ts")?;
    let test_target = file_response(root.path(), &run_id, "src/test-target.ts")?;
    let member_id = required_str(&member, "/sourceContext/sourceId")?;
    let destructure_id = required_str(&destructure, "/sourceContext/sourceId")?;
    let test_target_id = required_str(&test_target, "/sourceContext/sourceId")?;

    assert_computed_resolution(&main, "./member.js", member_id)?;
    assert_computed_resolution(&main, "./destructure.js", destructure_id)?;
    assert_computed_resolution(&test, "../src/test-target.js", test_target_id)?;

    let overview = overview(root.path(), &run_id)?;
    let limitations = required_array(&overview, "/limitations")?
        .iter()
        .filter(|limitation| {
            limitation.get("reason").and_then(Value::as_str) == Some("common-js-computed-member")
        })
        .collect::<Vec<_>>();
    assert_eq!(limitations.len(), 3);
    assert_limitation(
        &limitations,
        "./member.js",
        member_id,
        MAIN_SOURCE
            .find("require('./member.js')")
            .ok_or_else(|| std::io::Error::other("member require is missing"))? as u64,
    )?;
    assert_limitation(
        &limitations,
        "./destructure.js",
        destructure_id,
        MAIN_SOURCE
            .find("require('./destructure.js')")
            .ok_or_else(|| std::io::Error::other("destructure require is missing"))? as u64,
    )?;
    assert_limitation(
        &limitations,
        "../src/test-target.js",
        test_target_id,
        TEST_SOURCE
            .find("require('../src/test-target.js')")
            .ok_or_else(|| std::io::Error::other("test require is missing"))? as u64,
    )?;

    let findings = finding_map(root.path(), &run_id)?;
    assert_eq!(
        findings.keys().cloned().collect::<BTreeSet<_>>(),
        BTreeSet::from([
            finding_key("src/destructure.ts", "DestructureType", "type"),
            finding_key("src/member.ts", "MemberType", "type"),
            finding_key("src/test-target.ts", "TestType", "type"),
            finding_key("src/test-target.ts", "testA", "value"),
            finding_key("src/test-target.ts", "testB", "value"),
            finding_key("src/unrelated.ts", "UnrelatedType", "type"),
            finding_key("src/unrelated.ts", "unrelatedValue", "value"),
        ]),
        "computed require escaped its resolved module/value namespace or lost broad fan-in",
    );
    for name in ["testA", "testB"] {
        assert!(
            findings[&finding_key("src/test-target.ts", name, "value")].contains("test-like"),
            "test-like computed fan-in was not preserved for {name}",
        );
    }
    assert!(
        !findings[&finding_key("src/test-target.ts", "TestType", "type")].contains("test-like"),
        "runtime CommonJS opacity leaked into the type namespace",
    );

    let gate = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-cjs-computed",
            "--path",
            "src/main.ts",
            "--jobs",
            "1",
            "--resolution-profile",
            "node16",
        ],
    )?;
    assert_status(&gate, 0);
    assert_eq!(field(&gate.stdout, "decision")?, "allow-with-warnings");
    let gate: Value = serde_json::from_str(&gate.stdout)?;
    assert!(
        !gate
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some("required-evidence-incomplete")
            })),
        "resolved-module CommonJS opacity became an unbounded required gap: {gate:#?}",
    );
    Ok(())
}

fn fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"cjs-computed","private":true,"type":"commonjs"}"#,
    )?;
    write(root.path(), "src/main.ts", MAIN_SOURCE)?;
    write(root.path(), "tests/consumer.test.ts", TEST_SOURCE)?;
    write(
        root.path(),
        "src/member.ts",
        "export const memberA = 1; export const memberB = 2; export type MemberType = string;\n",
    )?;
    write(
        root.path(),
        "src/destructure.ts",
        "export const destructureA = 1; export const destructureB = 2; export type DestructureType = string;\n",
    )?;
    write(
        root.path(),
        "src/test-target.ts",
        "export const testA = 1; export const testB = 2; export type TestType = string;\n",
    )?;
    write(
        root.path(),
        "src/unrelated.ts",
        "export const unrelatedValue = 1; export type UnrelatedType = string;\n",
    )?;
    Ok(root)
}

fn assert_computed_resolution(
    source: &Value,
    specifier: &str,
    target: &str,
) -> Result<(), std::io::Error> {
    let resolution = required_array(source, "/resolutions")?
        .iter()
        .find(|resolution| {
            resolution
                .pointer("/sourceUse/specifier")
                .and_then(Value::as_str)
                == Some(specifier)
        })
        .ok_or_else(|| std::io::Error::other(format!("resolution for {specifier} is missing")))?;
    assert_eq!(
        required_str(resolution, "/sourceUse/kind")?,
        "common-js-computed"
    );
    assert_eq!(
        required_str(resolution, "/sourceUse/requestKind")?,
        "require"
    );
    assert_eq!(required_str(resolution, "/outcome/kind")?, "internal");
    assert_eq!(required_str(resolution, "/outcome/target")?, target);
    Ok(())
}

fn assert_limitation(
    limitations: &[&Value],
    specifier: &str,
    target: &str,
    start: u64,
) -> Result<(), std::io::Error> {
    let limitation = limitations
        .iter()
        .copied()
        .find(|limitation| limitation.get("specifier").and_then(Value::as_str) == Some(specifier))
        .ok_or_else(|| std::io::Error::other(format!("limitation for {specifier} is missing")))?;
    assert_eq!(required_str(limitation, "/target")?, target);
    assert_eq!(required_u64(limitation, "/span/start")?, start);
    Ok(())
}

fn finding_map(
    root: &Path,
    run_id: &str,
) -> Result<BTreeMap<FindingKey, String>, Box<dyn std::error::Error>> {
    let output = run(root, &["findings", "--run", run_id, "--area", "dead-code"])?;
    assert_status(&output, 0);
    let response: Value = serde_json::from_str(&output.stdout)?;
    required_array(&response, "/items")?
        .iter()
        .map(|finding| {
            Ok((
                (
                    required_str(finding, "/path/display")?.to_owned(),
                    required_str(finding, "/exportedName")?.to_owned(),
                    required_str(finding, "/namespace")?.to_owned(),
                ),
                required_str(finding, "/claim")?.to_owned(),
            ))
        })
        .collect::<Result<_, std::io::Error>>()
        .map_err(Into::into)
}

fn finding_key(path: &str, name: &str, namespace: &str) -> FindingKey {
    (path.to_owned(), name.to_owned(), namespace.to_owned())
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

fn write(root: &Path, path: &str, contents: &str) -> Result<(), std::io::Error> {
    let path = root.join(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}
