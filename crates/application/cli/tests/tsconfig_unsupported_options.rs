use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

type FindingTuple = (String, String, String);

#[test]
fn module_suffixes_blocks_relative_probes_without_hiding_unaffected_fan_in()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write_workspace_root(root.path())?;
    write_package(root.path(), "affected", "@acme/affected")?;
    write_package(root.path(), "target", "@acme/target")?;
    write_package(root.path(), "clean", "@acme/clean")?;
    write_package(root.path(), "control", "@acme/control")?;

    write(
        root.path(),
        "packages/affected/tsconfig.json",
        r#"{"compilerOptions":{"moduleSuffixes":[".native",""]}}"#,
    )?;
    write(
        root.path(),
        "packages/affected/main.ts",
        concat!(
            "import { suffixUsed } from '../target/value';\n",
            "console.log(suffixUsed);\n",
        ),
    )?;
    module(
        root.path(),
        "packages/target/value.native.ts",
        "suffixUsed",
        "nativeDead",
    )?;
    module(
        root.path(),
        "packages/target/value.ts",
        "suffixUsed",
        "plainDead",
    )?;

    write(
        root.path(),
        "packages/clean/main.ts",
        concat!(
            "import { controlUsed } from '../control/value';\n",
            "console.log(controlUsed);\n",
        ),
    )?;
    module(
        root.path(),
        "packages/control/value.ts",
        "controlUsed",
        "controlDead",
    )?;

    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    assert_eq!(field(&audit.stdout, "status")?, "incomplete");
    let run_id = field(&audit.stdout, "runId")?;
    let overview = run(root.path(), &["overview", "--run", &run_id])?;
    assert_status(&overview, 0);
    let overview: Value = serde_json::from_str(&overview.stdout)?;
    let limitations = overview
        .get("limitations")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("limitations are missing"))?;
    assert_eq!(limitations.len(), 1);
    assert_eq!(
        limitations[0].get("reason").and_then(Value::as_str),
        Some("tsconfig-semantics-unsupported")
    );
    assert_eq!(
        limitations[0].get("path").and_then(Value::as_str),
        Some("packages/affected/tsconfig.json")
    );
    assert_eq!(
        limitations[0].get("detail").and_then(Value::as_str),
        Some("unsupported resolution-affecting compiler option moduleSuffixes")
    );
    assert_eq!(
        finding_set(root.path(), &run_id)?,
        BTreeSet::from([
            finding("packages/control/value.ts", "controlDead"),
            finding("packages/target/value.native.ts", "nativeDead"),
            finding("packages/target/value.native.ts", "suffixUsed"),
            finding("packages/target/value.ts", "plainDead"),
            finding("packages/target/value.ts", "suffixUsed"),
        ])
    );
    Ok(())
}

#[test]
fn module_suffixes_prewrite_withholds_authorization_and_retry_is_idempotent()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write_workspace_root(root.path())?;
    write_package(root.path(), "affected", "@acme/affected")?;
    write(
        root.path(),
        "packages/affected/tsconfig.json",
        r#"{"compilerOptions":{"moduleSuffixes":[".native",""]}}"#,
    )?;
    write(
        root.path(),
        "packages/affected/main.ts",
        "export const value = 1;\n",
    )?;

    let arguments = [
        "pre-write",
        "--operation-id",
        "op-module-suffixes",
        "--path",
        "packages/affected/main.ts",
        "--jobs",
        "1",
    ];
    let first = run(root.path(), &arguments)?;
    assert_status(&first, 4);
    assert_eq!(field(&first.stdout, "decision")?, "incomplete");
    let response: Value = serde_json::from_str(&first.stdout)?;
    let signals = response
        .get("signals")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("signals are missing"))?;
    assert!(signals.iter().any(|signal| {
        signal.get("kind").and_then(Value::as_str) == Some("required-evidence-incomplete")
    }));
    assert!(
        response
            .get("deltas")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
    );

    let retry = run(root.path(), &arguments)?;
    assert_status(&retry, 4);
    assert_eq!(retry.stdout, first.stdout);
    Ok(())
}

fn write_workspace_root(root: &Path) -> std::io::Result<()> {
    write(
        root,
        "package.json",
        r#"{"private":true,"workspaces":["packages/*"]}"#,
    )
}

fn write_package(root: &Path, directory: &str, name: &str) -> std::io::Result<()> {
    write(
        root,
        &format!("packages/{directory}/package.json"),
        &serde_json::json!({"name":name,"private":true}).to_string(),
    )
}

fn module(root: &Path, relative: &str, used: &str, dead: &str) -> std::io::Result<()> {
    write(
        root,
        relative,
        &format!("export const {used} = 1; export const {dead} = 2;\n"),
    )
}

fn finding(path: &str, name: &str) -> FindingTuple {
    (path.to_owned(), name.to_owned(), "value".to_owned())
}

fn finding_set(
    root: &Path,
    run_id: &str,
) -> Result<BTreeSet<FindingTuple>, Box<dyn std::error::Error>> {
    let output = run(root, &["findings", "--run", run_id, "--area", "dead-code"])?;
    assert_status(&output, 0);
    let response: Value = serde_json::from_str(&output.stdout)?;
    response
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("finding items are missing"))?
        .iter()
        .map(|item| {
            let path = item
                .pointer("/path/display")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("finding path is missing"))?;
            let name = item
                .get("exportedName")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("finding name is missing"))?;
            let namespace = item
                .get("namespace")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("finding namespace is missing"))?;
            Ok((path.to_owned(), name.to_owned(), namespace.to_owned()))
        })
        .collect()
}

fn write(root: &Path, relative: &str, contents: &str) -> std::io::Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}
