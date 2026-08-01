use std::collections::BTreeSet;
use std::fs;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

type FindingView = (String, String, String, String, Option<String>);

#[test]
fn source_role_classification_persists_rule_reason_and_source()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::create_dir_all(root.path().join("tests"))?;
    fs::create_dir_all(root.path().join("vendor"))?;
    fs::write(
        root.path().join("lumin.json"),
        concat!(
            r#"{"schemaVersion":"lumin-config.v1","scan":{"roles":["#,
            r#"{"pattern":"src/authored.ts","role":"authored"},"#,
            r#"{"pattern":"src/vendor.ts","role":"vendor"}"#,
            "]}}",
        ),
    )?;
    for (path, source) in [
        ("tests/default.ts", "export const defaultTest = 1;\n"),
        (
            "tests/production.ts",
            "export const productionOverride = 1;\n",
        ),
        (
            "src/generated.ts",
            "// @generated\nexport const generated = 1;\n",
        ),
        (
            "src/authored.ts",
            "// @generated\nexport const authored = 1;\n",
        ),
        ("src/vendor.ts", "export const vendored = 1;\n"),
        (
            "src/types.d.ts",
            "export declare const declaration: string;\n",
        ),
        (
            "vendor/ordinary.ts",
            "export const ordinaryDirectoryName = 1;\n",
        ),
    ] {
        fs::write(root.path().join(path), source)?;
    }

    let audit = run(
        root.path(),
        &[
            "audit",
            "--jobs",
            "1",
            "--role-at",
            "tests/production.ts",
            "production",
        ],
    )?;
    assert_status(&audit, 0);
    let run_id = field(&audit.stdout, "runId")?;

    assert_eq!(
        classifications(root.path(), &run_id, "tests/default.ts")?,
        serde_json::json!([classification("test", "test-path-rule", "compiled-default")])
    );
    assert_eq!(
        classifications(root.path(), &run_id, "tests/production.ts")?,
        serde_json::json!([
            classification("test", "test-path-rule", "compiled-default"),
            classification("production", "explicit-production-role", "invocation")
        ])
    );
    assert_eq!(
        classifications(root.path(), &run_id, "src/generated.ts")?,
        serde_json::json!([classification(
            "generated",
            "leading-generated-comment",
            "compiled-default"
        )])
    );
    assert_eq!(
        classifications(root.path(), &run_id, "src/authored.ts")?,
        serde_json::json!([
            classification("generated", "leading-generated-comment", "compiled-default"),
            classification("authored", "explicit-authored-role", "configuration")
        ])
    );
    assert_eq!(
        classifications(root.path(), &run_id, "src/vendor.ts")?,
        serde_json::json!([classification(
            "vendor",
            "explicit-vendor-role",
            "configuration"
        )])
    );
    assert_eq!(
        classifications(root.path(), &run_id, "src/types.d.ts")?,
        serde_json::json!([classification(
            "declaration",
            "declaration-extension",
            "compiled-default"
        )])
    );
    assert_eq!(
        classifications(root.path(), &run_id, "vendor/ordinary.ts")?,
        serde_json::json!([])
    );
    Ok(())
}

#[test]
fn source_role_findings_remain_visible_and_only_explicit_filtering_narrows()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(
        root.path().join("lumin.json"),
        concat!(
            r#"{"schemaVersion":"lumin-config.v1","scan":{"roles":["#,
            r#"{"pattern":"src/vendor.ts","role":"vendor"},"#,
            r#"{"pattern":"src/override.ts","role":"authored"}"#,
            "]}}",
        ),
    )?;
    fs::write(
        root.path().join("src/authored.ts"),
        "export const dead = 1;\n",
    )?;
    fs::write(
        root.path().join("src/generated.ts"),
        "// @generated\nexport const dead = 1;\n",
    )?;
    fs::write(
        root.path().join("src/vendor.ts"),
        "export const dead = 1;\n",
    )?;
    fs::write(
        root.path().join("src/override.ts"),
        "// @generated\nexport const dead = 1;\n",
    )?;

    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
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
    assert_eq!(
        audit_json.get("findingCount").and_then(Value::as_u64),
        Some(4)
    );
    let run_id = field(&audit.stdout, "runId")?;

    let findings = run(
        root.path(),
        &["findings", "--run", &run_id, "--area", "dead-code"],
    )?;
    assert_status(&findings, 0);
    let response: Value = serde_json::from_str(&findings.stdout)?;
    assert_collection_counts(&response, 4, 4)?;
    assert_eq!(response.get("filters"), Some(&serde_json::json!({})));
    assert_eq!(
        response.get("truncated").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        finding_views(&response)?,
        BTreeSet::from([
            view("src/authored.ts", "review-candidate", None),
            view("src/generated.ts", "review-only", Some("generated-source")),
            view("src/override.ts", "review-candidate", None),
            view("src/vendor.ts", "review-only", Some("vendored-source")),
        ])
    );

    let generated_source_id = response
        .get("items")
        .and_then(Value::as_array)
        .and_then(|items| {
            items.iter().find(|item| {
                item.pointer("/path/display").and_then(Value::as_str) == Some("src/generated.ts")
            })
        })
        .and_then(|item| item.get("sourceId"))
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("generated finding sourceId is missing"))?;
    let filtered = run(
        root.path(),
        &["files", "--run", &run_id, "src/generated.ts"],
    )?;
    assert_status(&filtered, 0);
    let filtered_json: Value = serde_json::from_str(&filtered.stdout)?;
    assert_collection_counts(&filtered_json, 4, 1)?;
    assert_eq!(
        filtered_json.get("filters"),
        Some(&serde_json::json!({"path": [generated_source_id]}))
    );
    assert_eq!(
        finding_views(&filtered_json)?,
        BTreeSet::from([view(
            "src/generated.ts",
            "review-only",
            Some("generated-source"),
        )])
    );
    Ok(())
}

#[test]
fn contradictory_invocation_roles_hard_stop_audit_and_pre_write_without_authorization()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(root.path().join("src/a.ts"), "export const a = 1;\n")?;

    let audit = run(
        root.path(),
        &[
            "audit",
            "--jobs",
            "1",
            "--role-at",
            "src/a.ts",
            "generated",
            "--role-at",
            "src/a.ts",
            "authored",
        ],
    )?;
    assert_status(&audit, 2);
    assert!(audit.stdout.is_empty());
    assert!(audit.stderr.contains(
        "contradictory invocation source role declarations for src/a.ts: generated conflicts with authored"
    ));

    let pre = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-role-conflict",
            "--path",
            "src/a.ts",
            "--jobs",
            "1",
            "--role-at",
            "src/a.ts",
            "authored",
            "--role-at",
            "src/a.ts",
            "generated",
        ],
    )?;
    assert_status(&pre, 4);
    assert!(pre.stderr.is_empty());
    let pre_json: Value = serde_json::from_str(&pre.stdout)?;
    assert_eq!(
        pre_json.get("lifecycle").and_then(Value::as_str),
        Some("rejected")
    );
    assert_eq!(
        pre_json.get("decision").and_then(Value::as_str),
        Some("incomplete")
    );
    assert!(pre_json
        .get("signals")
        .and_then(Value::as_array)
        .is_some_and(|signals| signals.iter().any(|signal| {
            signal.get("kind").and_then(Value::as_str) == Some("analysis-failed")
                && signal
                    .get("detail")
                    .and_then(Value::as_str)
                    .is_some_and(|detail| detail.contains(
                        "contradictory invocation source role declarations for src/a.ts: generated conflicts with authored"
                    ))
        })));
    Ok(())
}

fn assert_collection_counts(
    response: &Value,
    scope_total: u64,
    total: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        response.get("schemaVersion").and_then(Value::as_str),
        Some("lumin.collection.v1")
    );
    assert_eq!(
        response.get("scopeTotal").and_then(Value::as_u64),
        Some(scope_total)
    );
    assert_eq!(response.get("total").and_then(Value::as_u64), Some(total));
    assert_eq!(
        response.get("returned").and_then(Value::as_u64),
        Some(total)
    );
    let items = response
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("finding items are missing"))?;
    assert_eq!(items.len() as u64, total);
    Ok(())
}

fn classifications(
    root: &std::path::Path,
    run_id: &str,
    path: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let output = run(root, &["files", "--run", run_id, path])?;
    assert_status(&output, 0);
    let response: Value = serde_json::from_str(&output.stdout)?;
    let source_classification = response
        .get("sourceClassification")
        .ok_or_else(|| std::io::Error::other("sourceClassification is missing"))?;
    assert_eq!(
        source_classification
            .pointer("/path/schemaVersion")
            .and_then(Value::as_str),
        Some("repo-path.v1")
    );
    assert_eq!(
        source_classification
            .pointer("/path/display")
            .and_then(Value::as_str),
        Some(path)
    );
    assert!(
        source_classification
            .get("sourceId")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
    );
    source_classification
        .get("classifications")
        .cloned()
        .ok_or_else(|| std::io::Error::other("classifications are missing").into())
}

fn classification(role: &str, reason: &str, configuration_source: &str) -> Value {
    serde_json::json!({
        "role": role,
        "ruleVersion": "source-classification.v1",
        "reason": reason,
        "configurationSource": configuration_source,
    })
}

fn finding_views(response: &Value) -> Result<BTreeSet<FindingView>, Box<dyn std::error::Error>> {
    response
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("finding items are missing"))?
        .iter()
        .map(|item| {
            assert_eq!(
                item.get("ruleId").and_then(Value::as_str),
                Some("dead-code/zero-exact-fan-in.v1")
            );
            assert_eq!(
                item.pointer("/path/schemaVersion").and_then(Value::as_str),
                Some("repo-path.v1")
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
                required("/disposition/kind")?,
                item.pointer("/disposition/reason")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            ))
        })
        .collect::<Result<_, std::io::Error>>()
        .map_err(Into::into)
}

fn view(path: &str, disposition: &str, reason: Option<&str>) -> FindingView {
    (
        path.to_owned(),
        "dead".to_owned(),
        "value".to_owned(),
        disposition.to_owned(),
        reason.map(str::to_owned),
    )
}
