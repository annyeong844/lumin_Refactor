use std::collections::BTreeSet;
use std::fs;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

type FindingTuple = (String, String, String);

#[test]
fn plain_esm_preserves_namespace_and_side_effect_distinctions()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(
        root.path().join("src/lib.ts"),
        concat!(
            "export const namedUsed = 1;\n",
            "const defaultValue = 2; export default defaultValue;\n",
            "export type TypeUsed = string;\n",
            "export const dead = 3;\n",
        ),
    )?;
    fs::write(
        root.path().join("src/side.ts"),
        "export const sideOnly = 1;\n",
    )?;
    fs::write(
        root.path().join("src/main.ts"),
        concat!(
            "import defaultValue, { namedUsed, type TypeUsed } from './lib.js';\n",
            "import './side.js';\n",
            "const typed: TypeUsed = String(namedUsed + defaultValue); console.log(typed);\n",
        ),
    )?;

    let findings = audit_findings(root.path())?;
    assert_eq!(
        findings,
        BTreeSet::from([
            (
                "src/lib.ts".to_owned(),
                "dead".to_owned(),
                "value".to_owned()
            ),
            (
                "src/side.ts".to_owned(),
                "sideOnly".to_owned(),
                "value".to_owned(),
            ),
        ])
    );
    Ok(())
}

#[test]
fn reachable_module_keeps_zero_fan_in_sibling() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(
        root.path().join("src/lib.ts"),
        "export const used = 1; export const deadSibling = 2;\n",
    )?;
    fs::write(
        root.path().join("src/main.ts"),
        "import { used } from './lib.js'; console.log(used);\n",
    )?;

    assert_eq!(
        audit_findings(root.path())?,
        BTreeSet::from([(
            "src/lib.ts".to_owned(),
            "deadSibling".to_owned(),
            "value".to_owned(),
        )])
    );
    Ok(())
}

#[test]
fn public_reexport_protects_only_selected_identity() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(
        root.path().join("package.json"),
        r#"{"name":"public-reexport-pkg","exports":"./src/index.js"}"#,
    )?;
    fs::write(
        root.path().join("src/index.ts"),
        "export { publicOne } from './lib.js';\n",
    )?;
    fs::write(
        root.path().join("src/lib.ts"),
        concat!(
            "export const publicOne = 1;\n",
            "export const deadA = 2;\n",
            "export const deadB = 3;\n",
            "export const deadC = 4;\n",
        ),
    )?;

    assert_eq!(
        audit_findings(root.path())?,
        BTreeSet::from([
            (
                "src/lib.ts".to_owned(),
                "deadA".to_owned(),
                "value".to_owned()
            ),
            (
                "src/lib.ts".to_owned(),
                "deadB".to_owned(),
                "value".to_owned()
            ),
            (
                "src/lib.ts".to_owned(),
                "deadC".to_owned(),
                "value".to_owned()
            ),
        ])
    );
    Ok(())
}

fn audit_findings(
    root: &std::path::Path,
) -> Result<BTreeSet<FindingTuple>, Box<dyn std::error::Error>> {
    let audit = run(root, &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    let audit_json: Value = serde_json::from_str(&audit.stdout)?;
    assert_eq!(
        audit_json.get("status").and_then(Value::as_str),
        Some("complete")
    );
    assert_eq!(
        audit_json.get("limitationCount").and_then(Value::as_u64),
        Some(0)
    );
    let run_id = field(&audit.stdout, "runId")?;
    let result = run(root, &["findings", "--run", &run_id, "--area", "dead-code"])?;
    assert_status(&result, 0);
    let response: Value = serde_json::from_str(&result.stdout)?;
    assert_eq!(
        response.get("truncated").and_then(Value::as_bool),
        Some(false)
    );
    let items = response
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("findings items are missing"))?;
    assert_eq!(
        response.get("total").and_then(Value::as_u64),
        Some(items.len() as u64)
    );
    items
        .iter()
        .map(|item| {
            assert_eq!(
                item.get("ruleId").and_then(Value::as_str),
                Some("dead-code/zero-exact-fan-in.v1")
            );
            assert_eq!(
                item.pointer("/path/encoding").and_then(Value::as_str),
                Some("repo-path.v1")
            );
            let path = item
                .pointer("/path/display")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("finding path is missing"))?;
            let exported_name = item
                .get("exportedName")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("exportedName is missing"))?;
            let namespace = item
                .get("namespace")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("namespace is missing"))?;
            Ok((
                path.to_owned(),
                exported_name.to_owned(),
                namespace.to_owned(),
            ))
        })
        .collect()
}
