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
    write_package(root.path(), "target", "@acme/target")?;
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

    let writer = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-module-suffixes-candidate-writer",
            "--path",
            "packages/target/value.native.ts",
            "--path",
            "packages/target/value.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&writer, 0);
    let writer_gate = field(&writer.stdout, "gateId")?;

    write_package(root.path(), "affected", "@acme/affected")?;
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

    let rejected_gate = assert_prewrite_incomplete_retry(
        root.path(),
        "op-module-suffixes",
        "packages/affected/main.ts",
    )?;
    let semantic_input_count = rejected_gate
        .pointer("/baseline/semanticInputCount")
        .and_then(Value::as_u64)
        .ok_or_else(|| std::io::Error::other("baseline semantic input count is missing"))?;
    let protected_input_count = rejected_gate
        .pointer("/baseline/protectedSemanticInputCount")
        .and_then(Value::as_u64)
        .ok_or_else(|| std::io::Error::other("baseline protected input count is missing"))?;
    assert_eq!(
        semantic_input_count.checked_sub(protected_input_count),
        Some(2),
        "the two relative probe candidates entered the protected read closure",
    );
    assert_eq!(
        rejected_gate
            .get("protectedSemanticInputCount")
            .and_then(Value::as_u64),
        Some(protected_input_count),
    );
    assert_eq!(
        rejected_gate
            .pointer("/revisions/0/protectedSemanticInputCount")
            .and_then(Value::as_u64),
        Some(protected_input_count),
    );

    write_package(root.path(), "control", "@acme/control")?;
    write(
        root.path(),
        "packages/control/main.ts",
        concat!(
            "import { suffixUsed } from '../target/value';\n",
            "console.log(suffixUsed);\n",
        ),
    )?;
    let control = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-module-suffixes-probe-control",
            "--path",
            "packages/control/main.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&control, 4);
    let control: Value = serde_json::from_str(&control.stdout)?;
    let conflict = control
        .get("signals")
        .and_then(Value::as_array)
        .and_then(|signals| {
            signals
                .iter()
                .find(|signal| signal.get("kind").and_then(Value::as_str) == Some("write-conflict"))
        })
        .ok_or_else(|| {
            std::io::Error::other(format!(
                "probe control did not observe its target read: {control}"
            ))
        })?;
    assert_eq!(
        conflict.pointer("/paths/0/display").and_then(Value::as_str),
        Some("packages/target/value.ts")
    );
    assert_eq!(
        conflict.pointer("/gateIds/0").and_then(Value::as_str),
        Some(writer_gate.as_str())
    );
    Ok(())
}

#[test]
fn custom_conditions_blocks_node_and_default_without_hiding_unaffected_selection()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"root","private":true,"type":"module","workspaces":["packages/*"]}"#,
    )?;
    write_esm_package(root.path(), "affected", "@acme/affected")?;
    write_esm_package(root.path(), "clean", "@acme/clean")?;
    write_condition_package(root.path(), "target", "@acme/target")?;
    write_condition_package(root.path(), "control", "@acme/control")?;

    write(
        root.path(),
        "packages/affected/tsconfig.json",
        r#"{"compilerOptions":{"customConditions":["custom"]}}"#,
    )?;
    write(
        root.path(),
        "packages/affected/main.ts",
        concat!(
            "import { conditionUsed } from '@acme/target';\n",
            "console.log(conditionUsed);\n",
        ),
    )?;
    module(
        root.path(),
        "packages/target/node.ts",
        "conditionUsed",
        "targetNodeDead",
    )?;
    module(
        root.path(),
        "packages/target/default.ts",
        "conditionUsed",
        "targetDefaultDead",
    )?;

    write(
        root.path(),
        "packages/clean/main.ts",
        concat!(
            "import { controlUsed } from '@acme/control';\n",
            "console.log(controlUsed);\n",
        ),
    )?;
    module(
        root.path(),
        "packages/control/node.ts",
        "controlUsed",
        "controlNodeDead",
    )?;
    module(
        root.path(),
        "packages/control/default.ts",
        "controlUsed",
        "controlDefaultDead",
    )?;

    for (profile, expected) in [
        (
            "bundler",
            BTreeSet::from([
                finding("packages/control/default.ts", "controlDefaultDead"),
                finding("packages/control/node.ts", "controlNodeDead"),
                finding("packages/control/node.ts", "controlUsed"),
                finding("packages/target/default.ts", "conditionUsed"),
                finding("packages/target/default.ts", "targetDefaultDead"),
                finding("packages/target/node.ts", "conditionUsed"),
                finding("packages/target/node.ts", "targetNodeDead"),
            ]),
        ),
        (
            "node16",
            BTreeSet::from([
                finding("packages/control/default.ts", "controlDefaultDead"),
                finding("packages/control/default.ts", "controlUsed"),
                finding("packages/control/node.ts", "controlNodeDead"),
                finding("packages/target/default.ts", "conditionUsed"),
                finding("packages/target/default.ts", "targetDefaultDead"),
                finding("packages/target/node.ts", "conditionUsed"),
                finding("packages/target/node.ts", "targetNodeDead"),
            ]),
        ),
    ] {
        let audit = run(
            root.path(),
            &["audit", "--jobs", "1", "--resolution-profile", profile],
        )?;
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
            Some("unsupported resolution-affecting compiler option customConditions")
        );
        assert_eq!(finding_set(root.path(), &run_id)?, expected);
    }
    Ok(())
}

#[test]
fn custom_conditions_prewrite_withholds_authorization_and_retry_is_idempotent()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write_workspace_root(root.path())?;
    write_package(root.path(), "affected", "@acme/affected")?;
    write(
        root.path(),
        "packages/affected/tsconfig.json",
        r#"{"compilerOptions":{"customConditions":["custom"]}}"#,
    )?;
    write(
        root.path(),
        "packages/affected/main.ts",
        "export const value = 1;\n",
    )?;

    assert_prewrite_incomplete_retry(
        root.path(),
        "op-custom-conditions",
        "packages/affected/main.ts",
    )?;
    Ok(())
}

fn assert_prewrite_incomplete_retry(
    root: &Path,
    operation_id: &str,
    path: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let arguments = [
        "pre-write",
        "--operation-id",
        operation_id,
        "--path",
        path,
        "--jobs",
        "1",
    ];
    let first = run(root, &arguments)?;
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
    assert!(!signals.iter().any(|signal| {
        signal.get("kind").and_then(Value::as_str) == Some("semantic-input-conflict")
    }));
    assert!(
        !signals
            .iter()
            .any(|signal| { signal.get("kind").and_then(Value::as_str) == Some("write-conflict") })
    );
    assert!(
        response
            .get("deltas")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
    );
    let gate_id = field(&first.stdout, "gateId")?;

    let operation_before = run(root, &["operation", "show", operation_id])?;
    assert_status(&operation_before, 0);
    let operation_before: Value = serde_json::from_str(&operation_before.stdout)?;
    assert_eq!(
        operation_before.get("kind").and_then(Value::as_str),
        Some("pre-write")
    );
    assert_eq!(
        operation_before.get("status").and_then(Value::as_str),
        Some("committed")
    );
    assert_eq!(
        operation_before.get("gateId").and_then(Value::as_str),
        Some(gate_id.as_str())
    );
    assert_eq!(
        operation_before
            .get("semanticReadReservations")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );
    assert_eq!(operation_before.get("result"), Some(&response));

    let gate_before = run(root, &["gate", "show", &gate_id])?;
    assert_status(&gate_before, 0);
    let gate_before: Value = serde_json::from_str(&gate_before.stdout)?;
    assert_eq!(
        gate_before.get("lifecycle").and_then(Value::as_str),
        Some("rejected")
    );
    assert_eq!(
        gate_before.get("currentRevision").and_then(Value::as_u64),
        Some(0)
    );
    assert_eq!(
        gate_before
            .get("revisions")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        gate_before
            .pointer("/revisions/0/operationId")
            .and_then(Value::as_str),
        Some(operation_id)
    );

    let active_before = run(root, &["gate", "list", "--active"])?;
    assert_status(&active_before, 0);
    let active_before: Value = serde_json::from_str(&active_before.stdout)?;
    assert!(
        !active_before
            .get("items")
            .and_then(Value::as_array)
            .is_some_and(|items| items.iter().any(|item| {
                item.get("gateId").and_then(Value::as_str) == Some(gate_id.as_str())
            }))
    );

    let retry = run(root, &arguments)?;
    assert_status(&retry, 4);
    assert_eq!(retry.stdout, first.stdout);

    let operation_after = run(root, &["operation", "show", operation_id])?;
    assert_status(&operation_after, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&operation_after.stdout)?,
        operation_before,
        "retry mutated the durable operation snapshot",
    );
    let gate_after = run(root, &["gate", "show", &gate_id])?;
    assert_status(&gate_after, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&gate_after.stdout)?,
        gate_before,
        "retry mutated the rejected gate revision or lifecycle",
    );
    let active_after = run(root, &["gate", "list", "--active"])?;
    assert_status(&active_after, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&active_after.stdout)?,
        active_before,
        "retry mutated the active gate catalog",
    );
    Ok(gate_before)
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

fn write_esm_package(root: &Path, directory: &str, name: &str) -> std::io::Result<()> {
    write(
        root,
        &format!("packages/{directory}/package.json"),
        &serde_json::json!({"name":name,"private":true,"type":"module"}).to_string(),
    )
}

fn write_condition_package(root: &Path, directory: &str, name: &str) -> std::io::Result<()> {
    let name = serde_json::to_string(name).map_err(std::io::Error::other)?;
    write(
        root,
        &format!("packages/{directory}/package.json"),
        &format!(
            r#"{{"name":{name},"private":true,"type":"module","exports":{{"node":"./node.js","default":"./default.js"}}}}"#,
        ),
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
