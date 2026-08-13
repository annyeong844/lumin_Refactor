use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

type FindingView = (String, String, String);

#[test]
fn bundler_excludes_node_in_value_and_type_lanes() -> Result<(), Box<dyn std::error::Error>> {
    let root = package_fixture()?;

    let bundler = audit_findings(root.path(), "bundler")?;
    assert_eq!(
        bundler,
        BTreeSet::from([
            view("packages/lib/default.ts", "DefaultDeadType", "type"),
            view("packages/lib/default.ts", "defaultDeadValue", "value"),
            view("packages/lib/node.ts", "NodeDeadType", "type"),
            view("packages/lib/node.ts", "UsedType", "type"),
            view("packages/lib/node.ts", "nodeDeadValue", "value"),
            view("packages/lib/node.ts", "usedValue", "value"),
        ])
    );

    let node_expected = BTreeSet::from([
        view("packages/lib/default.ts", "DefaultDeadType", "type"),
        view("packages/lib/default.ts", "UsedType", "type"),
        view("packages/lib/default.ts", "defaultDeadValue", "value"),
        view("packages/lib/default.ts", "usedValue", "value"),
        view("packages/lib/node.ts", "NodeDeadType", "type"),
        view("packages/lib/node.ts", "nodeDeadValue", "value"),
    ]);
    assert_eq!(audit_findings(root.path(), "node16")?, node_expected);
    assert_eq!(audit_findings(root.path(), "nodenext")?, node_expected);
    Ok(())
}

#[test]
fn supported_public_condition_lanes_union_only_selected_identity_namespaces()
-> Result<(), Box<dyn std::error::Error>> {
    let root = public_union_fixture()?;

    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    let audit_json: Value = serde_json::from_str(&audit.stdout)?;
    assert_eq!(
        audit_json.get("status").and_then(Value::as_str),
        Some("complete")
    );
    assert_eq!(
        audit_json.get("findingCount").and_then(Value::as_u64),
        Some(12)
    );
    assert_eq!(
        audit_json.get("limitationCount").and_then(Value::as_u64),
        Some(0)
    );
    let run_id = field(&audit.stdout, "runId")?;

    assert_eq!(
        finding_views(root.path(), &run_id, 12)?,
        BTreeSet::from([
            view("packages/lib/bundler.ts", "BundlerDeadType", "type"),
            view("packages/lib/import.ts", "ImportDeadType", "type"),
            view("packages/lib/node.ts", "NodeDeadType", "type"),
            view("packages/lib/require.ts", "RequireDeadType", "type"),
            view(
                "packages/lib/runtime-default.ts",
                "RuntimeDefaultDeadType",
                "type",
            ),
            view(
                "packages/lib/runtime-default.ts",
                "runtimeDefaultDeadValue",
                "value",
            ),
            view(
                "packages/lib/runtime-types.ts",
                "runtimeTypesDeadValue",
                "value",
            ),
            view(
                "packages/lib/shadowed-require.ts",
                "ShadowedRequireDeadType",
                "type",
            ),
            view(
                "packages/lib/shadowed-require.ts",
                "shadowedRequireDeadValue",
                "value",
            ),
            view(
                "packages/lib/syntax-default.ts",
                "SyntaxDefaultDeadType",
                "type",
            ),
            view(
                "packages/lib/syntax-default.ts",
                "syntaxDefaultDeadValue",
                "value",
            ),
            view(
                "packages/lib/syntax-types.ts",
                "syntaxTypesDeadValue",
                "value",
            ),
        ])
    );
    Ok(())
}

fn audit_findings(
    root: &Path,
    profile: &str,
) -> Result<BTreeSet<FindingView>, Box<dyn std::error::Error>> {
    let audit = run(
        root,
        &["audit", "--jobs", "1", "--resolution-profile", profile],
    )?;
    assert_status(&audit, 0);
    let audit_json: Value = serde_json::from_str(&audit.stdout)?;
    assert_eq!(
        audit_json.get("status").and_then(Value::as_str),
        Some("complete")
    );
    assert_eq!(
        audit_json.get("findingCount").and_then(Value::as_u64),
        Some(6)
    );
    assert_eq!(
        audit_json.get("limitationCount").and_then(Value::as_u64),
        Some(0)
    );
    let run_id = field(&audit.stdout, "runId")?;

    let overview = run(root, &["overview", "--run", &run_id])?;
    assert_status(&overview, 0);
    let overview_json: Value = serde_json::from_str(&overview.stdout)?;
    assert_eq!(
        overview_json.get("limitations"),
        Some(&serde_json::json!([]))
    );
    let profiles = overview_json
        .get("resolutionProfiles")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("resolutionProfiles are missing"))?;
    assert!(!profiles.is_empty());
    let expected_profile = if profile == "nodenext" {
        "node-next"
    } else {
        profile
    };
    assert!(profiles.iter().all(|selected| {
        selected.get("profile").and_then(Value::as_str) == Some(expected_profile)
            && selected.pointer("/source/kind").and_then(Value::as_str) == Some("invocation")
    }));

    finding_views(root, &run_id, 6)
}

fn finding_views(
    root: &Path,
    run_id: &str,
    expected_count: u64,
) -> Result<BTreeSet<FindingView>, Box<dyn std::error::Error>> {
    let findings = run(root, &["findings", "--run", run_id, "--area", "dead-code"])?;
    assert_status(&findings, 0);
    let response: Value = serde_json::from_str(&findings.stdout)?;
    assert_eq!(response.get("filters"), Some(&serde_json::json!({})));
    assert_eq!(
        response.get("scopeTotal").and_then(Value::as_u64),
        Some(expected_count)
    );
    assert_eq!(
        response.get("total").and_then(Value::as_u64),
        Some(expected_count)
    );
    assert_eq!(
        response.get("truncated").and_then(Value::as_bool),
        Some(false)
    );
    response
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("findings are missing"))?
        .iter()
        .map(|item| {
            assert_eq!(
                item.get("ruleId").and_then(Value::as_str),
                Some("dead-code/zero-exact-fan-in.v1")
            );
            let required = |pointer: &str| {
                item.pointer(pointer)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| std::io::Error::other(format!("missing {pointer}")))
            };
            Ok((
                required("/path/display")?,
                required("/exportedName")?,
                required("/namespace")?,
            ))
        })
        .collect::<Result<_, std::io::Error>>()
        .map_err(Into::into)
}

fn public_union_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("packages/lib"))?;
    fs::write(
        root.path().join("package.json"),
        r#"{"name":"app","private":true,"workspaces":["packages/*"]}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/package.json"),
        concat!(
            r#"{"name":"@acme/lib","exports":{"./runtime":{"types":"./runtime-types.js","node":"./node.js","import":"./bundler.js","require":"./shadowed-require.js","default":"./runtime-default.js"},"#,
            r#""./syntax":{"types":"./syntax-types.js","import":"./import.js","require":"./require.js","default":"./syntax-default.js"}}}"#,
        ),
    )?;
    for (file, value, ty) in [
        ("bundler.ts", "bundlerSelectedValue", "BundlerDeadType"),
        ("node.ts", "nodeSelectedValue", "NodeDeadType"),
        ("import.ts", "importSelectedValue", "ImportDeadType"),
        ("require.ts", "requireSelectedValue", "RequireDeadType"),
        (
            "shadowed-require.ts",
            "shadowedRequireDeadValue",
            "ShadowedRequireDeadType",
        ),
        (
            "runtime-default.ts",
            "runtimeDefaultDeadValue",
            "RuntimeDefaultDeadType",
        ),
        (
            "runtime-types.ts",
            "runtimeTypesDeadValue",
            "RuntimeSelectedType",
        ),
        (
            "syntax-default.ts",
            "syntaxDefaultDeadValue",
            "SyntaxDefaultDeadType",
        ),
        (
            "syntax-types.ts",
            "syntaxTypesDeadValue",
            "SyntaxSelectedType",
        ),
    ] {
        fs::write(
            root.path().join("packages/lib").join(file),
            format!("export const {value} = 1;\nexport type {ty} = string;\n"),
        )?;
    }
    Ok(root)
}

fn package_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::create_dir_all(root.path().join("packages/lib"))?;
    fs::write(
        root.path().join("package.json"),
        r#"{"name":"app","private":true,"type":"module","workspaces":["packages/*"]}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/package.json"),
        r#"{"name":"@acme/lib","private":true,"exports":{"node":"./node.js","default":"./default.js"}}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/node.ts"),
        concat!(
            "export const usedValue = 1;\n",
            "export type UsedType = string;\n",
            "export const nodeDeadValue = 2;\n",
            "export type NodeDeadType = number;\n",
        ),
    )?;
    fs::write(
        root.path().join("packages/lib/default.ts"),
        concat!(
            "export const usedValue = 1;\n",
            "export type UsedType = string;\n",
            "export const defaultDeadValue = 2;\n",
            "export type DefaultDeadType = number;\n",
        ),
    )?;
    fs::write(
        root.path().join("src/main.ts"),
        concat!(
            "import { usedValue, type UsedType } from '@acme/lib';\n",
            "const typed: UsedType = String(usedValue); console.log(typed);\n",
        ),
    )?;
    Ok(root)
}

fn view(path: &str, name: &str, namespace: &str) -> FindingView {
    (path.to_owned(), name.to_owned(), namespace.to_owned())
}
