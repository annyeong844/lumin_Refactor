use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

use super::support::{assert_status, field, run};

const APP_BEFORE: &str = concat!(
    "<template><article>One</article></template>\n",
    "<script lang=\"ts\">\n",
    "export const vueOwnerFact = 1;\n",
    "</script>\n",
);
const APP_AFTER: &str = concat!(
    "<template><article>Two</article></template>\n",
    "<script lang=\"ts\">\n",
    "const vueOwnerFact = 2; console.log(vueOwnerFact);\n",
    "</script>\n",
);

#[test]
fn mixed_js_and_vue_changes_share_one_gate_and_preserve_owner_facts()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let opened = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "mixed-vue-open",
            "--path",
            "src/lib.ts",
            "--path",
            "src/App.vue",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);
    assert_eq!(field(&opened.stdout, "decision")?, "allow-with-warnings");
    let gate_id = field(&opened.stdout, "gateId")?;
    assert_eq!(
        path_set(&opened.stdout, "/leasedWriteSet")?,
        BTreeSet::from(["src/App.vue".to_owned(), "src/lib.ts".to_owned()])
    );
    assert_owner_facts(root.path(), &gate_id, "0")?;

    fs::write(
        root.path().join("src/lib.ts"),
        "const jsOwnerFact = 2; console.log(jsOwnerFact);\n",
    )?;
    fs::write(root.path().join("src/App.vue"), APP_AFTER)?;

    let closed = run(
        root.path(),
        &["post-write", &gate_id, "--operation-id", "mixed-vue-close"],
    )?;
    assert_status(&closed, 0);
    assert_eq!(field(&closed.stdout, "decision")?, "allow");
    assert_eq!(field(&closed.stdout, "lifecycle")?, "closed");
    assert_eq!(
        path_set(&closed.stdout, "/actualWriteSet/paths")?,
        BTreeSet::from(["src/App.vue".to_owned(), "src/lib.ts".to_owned()])
    );
    assert_resolved_owner_deltas(&closed.stdout)?;
    assert_owner_facts(root.path(), &gate_id, "0")?;
    assert_no_findings(root.path(), &gate_id, "1")?;
    Ok(())
}

fn fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(
        root.path().join("package.json"),
        r#"{"name":"mixed-vue-gate","private":true,"type":"module"}"#,
    )?;
    fs::write(
        root.path().join("src/main.ts"),
        "import App from './App.vue'; console.log(App);\n",
    )?;
    fs::write(
        root.path().join("src/lib.ts"),
        "export const jsOwnerFact = 1;\n",
    )?;
    fs::write(root.path().join("src/App.vue"), APP_BEFORE)?;
    Ok(root)
}

fn assert_owner_facts(
    root: &Path,
    gate_id: &str,
    revision: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let findings = run(root, &["gate", "findings", gate_id, "--revision", revision])?;
    assert_status(&findings, 0);
    let response: Value = serde_json::from_str(&findings.stdout)?;
    let owners = response
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("gate findings items are missing"))?
        .iter()
        .map(|finding| {
            let path = finding
                .pointer("/path/display")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("finding path is missing"))?;
            let exported_name = finding
                .get("exportedName")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("finding exportedName is missing"))?;
            Ok((path.to_owned(), exported_name.to_owned()))
        })
        .collect::<Result<BTreeSet<_>, std::io::Error>>()?;
    assert_eq!(
        owners,
        BTreeSet::from([
            ("src/App.vue".to_owned(), "vueOwnerFact".to_owned()),
            ("src/lib.ts".to_owned(), "jsOwnerFact".to_owned()),
        ])
    );
    Ok(())
}

fn assert_resolved_owner_deltas(json: &str) -> Result<(), Box<dyn std::error::Error>> {
    let response: Value = serde_json::from_str(json)?;
    let deltas = response
        .get("deltas")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("gate deltas are missing"))?;
    assert_eq!(deltas.len(), 2);
    assert!(deltas.iter().all(|delta| {
        delta
            .pointer("/classification/kind")
            .and_then(Value::as_str)
            == Some("resolved")
    }));
    Ok(())
}

fn assert_no_findings(
    root: &Path,
    gate_id: &str,
    revision: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let findings = run(root, &["gate", "findings", gate_id, "--revision", revision])?;
    assert_status(&findings, 0);
    let response: Value = serde_json::from_str(&findings.stdout)?;
    assert_eq!(
        response
            .get("items")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );
    Ok(())
}

fn path_set(json: &str, pointer: &str) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
    let response: Value = serde_json::from_str(json)?;
    response
        .pointer(pointer)
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other(format!("path array is missing at {pointer}")))?
        .iter()
        .map(|path| {
            path.pointer("/path/display")
                .or_else(|| path.get("display"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| {
                    std::io::Error::other(format!("path display is missing at {pointer}"))
                })
        })
        .collect::<Result<_, _>>()
        .map_err(Into::into)
}
