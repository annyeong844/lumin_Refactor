use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

type FindingView = (String, String, String);

#[test]
fn types_versions_blocks_unspecialized_type_fallback() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::create_dir_all(root.path().join("packages/lib"))?;
    write_workspace_root(root.path())?;
    fs::write(
        root.path().join("packages/lib/package.json"),
        r#"{"name":"@acme/lib","private":true,"types":"./types.ts","typesVersions":{"*":{"*":[]}}}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/types.ts"),
        "export type Shape = string;\n",
    )?;
    fs::write(
        root.path().join("src/main.ts"),
        concat!(
            "import type { Shape } from '@acme/lib';\n",
            "const value: Shape = 'x'; console.log(value);\n",
            "export const appDead = 1;\n",
        ),
    )?;

    let evidence = audit_evidence(root.path())?;
    assert_public_surface_limitations(
        &evidence.overview,
        "packages/lib/package.json",
        "typesVersions",
    )?;
    assert_eq!(
        evidence.findings,
        BTreeSet::from([(
            "src/main.ts".to_owned(),
            "appDead".to_owned(),
            "value".to_owned(),
        )])
    );
    Ok(())
}

#[test]
fn unsupported_exports_shapes_never_select_fallbacks() -> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        (
            "nested-condition",
            r#"{"import":{"default":"./fallback.js"}}"#,
            "nested or non-string package conditions are unsupported",
        ),
        (
            "array-fallback",
            r#"["./fallback.js"]"#,
            "package exports must match exports-v1",
        ),
        (
            "mixed-subpath-condition",
            r#"{".":"./fallback.js","default":"./fallback.js"}"#,
            "exports cannot mix subpath and condition keys or use unknown conditions",
        ),
    ];

    for (name, exports, expected_detail) in cases {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("src"))?;
        fs::create_dir_all(root.path().join("packages/lib"))?;
        write_workspace_root(root.path())?;
        fs::write(
            root.path().join("packages/lib/package.json"),
            format!(r#"{{"name":"@acme/lib","private":true,"exports":{exports}}}"#),
        )?;
        fs::write(
            root.path().join("packages/lib/fallback.ts"),
            "export const used = 1; export const fallbackDead = 2;\n",
        )?;
        fs::write(
            root.path().join("src/main.ts"),
            concat!(
                "import { used } from '@acme/lib'; console.log(used);\n",
                "export const appDead = 1;\n",
            ),
        )?;

        let evidence = audit_evidence(root.path())?;
        assert_public_surface_limitations(
            &evidence.overview,
            "packages/lib/package.json",
            expected_detail,
        )?;
        assert_eq!(
            evidence.findings,
            BTreeSet::from([(
                "src/main.ts".to_owned(),
                "appDead".to_owned(),
                "value".to_owned(),
            )]),
            "unsupported exports case {name} selected or protected a fallback"
        );
    }
    Ok(())
}

#[test]
fn invalid_exports_subpath_components_are_package_scoped_unsupported()
-> Result<(), Box<dyn std::error::Error>> {
    for (name, key, expected_detail) in [
        (
            "dot",
            "./feature/./*",
            "invalid package target component \".\"",
        ),
        (
            "dot-dot",
            "./feature/../*",
            "invalid package target component \"..\"",
        ),
        (
            "node-modules",
            "./feature/node_modules/*",
            "invalid package target component \"node_modules\"",
        ),
    ] {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("src"))?;
        fs::create_dir_all(root.path().join("packages/lib"))?;
        write_workspace_root(root.path())?;
        fs::write(
            root.path().join("packages/lib/package.json"),
            format!(
                r#"{{"name":"@acme/lib","private":true,"exports":{{"{key}":"./fallback.js"}}}}"#
            ),
        )?;
        fs::write(
            root.path().join("packages/lib/fallback.ts"),
            "export const used = 1; export const fallbackDead = 2;\n",
        )?;
        fs::write(
            root.path().join("src/main.ts"),
            concat!(
                "import { used } from '@acme/lib/feature/x'; console.log(used);\n",
                "export const appDead = 1;\n",
            ),
        )?;

        let evidence = audit_evidence(root.path())?;
        assert_public_surface_limitations(
            &evidence.overview,
            "packages/lib/package.json",
            expected_detail,
        )?;
        let source = run(
            root.path(),
            &["files", "--run", &evidence.run_id, "src/main.ts"],
        )?;
        assert_status(&source, 0);
        let source: Value = serde_json::from_str(&source.stdout)?;
        let resolution = source
            .get("resolutions")
            .and_then(Value::as_array)
            .and_then(|resolutions| {
                resolutions.iter().find(|resolution| {
                    resolution
                        .pointer("/sourceUse/specifier")
                        .and_then(Value::as_str)
                        == Some("@acme/lib/feature/x")
                })
            })
            .ok_or_else(|| std::io::Error::other("package resolution is missing"))?;
        assert_eq!(
            resolution.pointer("/outcome/kind").and_then(Value::as_str),
            Some("unsupported"),
            "invalid component case {name} did not retain an unsupported outcome"
        );
        assert!(
            resolution
                .pointer("/outcome/reason")
                .and_then(Value::as_str)
                .is_some_and(|detail| detail.contains(expected_detail)),
            "invalid component case {name} lost its typed detail"
        );
        assert!(resolution.pointer("/outcome/target").is_none());
        assert!(resolution.pointer("/outcome/candidates").is_none());
        assert_eq!(
            evidence.findings,
            BTreeSet::from([(
                "src/main.ts".to_owned(),
                "appDead".to_owned(),
                "value".to_owned(),
            )]),
            "invalid component case {name} selected or protected its target"
        );
    }
    Ok(())
}

struct AuditEvidence {
    run_id: String,
    overview: Value,
    findings: BTreeSet<FindingView>,
}

fn audit_evidence(root: &Path) -> Result<AuditEvidence, Box<dyn std::error::Error>> {
    let audit = run(root, &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    let audit_json: Value = serde_json::from_str(&audit.stdout)?;
    assert_eq!(
        audit_json.get("status").and_then(Value::as_str),
        Some("incomplete")
    );
    assert_eq!(
        audit_json.get("findingCount").and_then(Value::as_u64),
        Some(1)
    );
    assert!(
        audit_json
            .get("limitationCount")
            .and_then(Value::as_u64)
            .is_some_and(|count| count > 0)
    );
    let run_id = field(&audit.stdout, "runId")?;

    let overview = run(root, &["overview", "--run", &run_id])?;
    assert_status(&overview, 0);
    let overview_json: Value = serde_json::from_str(&overview.stdout)?;
    assert_eq!(
        overview_json.get("schemaVersion").and_then(Value::as_str),
        Some("lumin.overview.v1")
    );
    assert_eq!(
        overview_json.get("findingCount").and_then(Value::as_u64),
        Some(1)
    );
    let limitations = overview_json
        .get("limitations")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("overview limitations are missing"))?;
    assert_eq!(
        overview_json.get("limitationCount").and_then(Value::as_u64),
        Some(limitations.len() as u64)
    );
    assert!(!limitations.is_empty());

    let findings = run(root, &["findings", "--run", &run_id, "--area", "dead-code"])?;
    assert_status(&findings, 0);
    let findings_json: Value = serde_json::from_str(&findings.stdout)?;
    assert_eq!(findings_json.get("filters"), Some(&serde_json::json!({})));
    assert_eq!(
        findings_json.get("scopeTotal").and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(findings_json.get("total").and_then(Value::as_u64), Some(1));
    assert_eq!(
        findings_json.get("truncated").and_then(Value::as_bool),
        Some(false)
    );
    let finding_views = findings_json
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
        .collect::<Result<_, std::io::Error>>()?;

    Ok(AuditEvidence {
        run_id,
        overview: overview_json,
        findings: finding_views,
    })
}

fn assert_public_surface_limitations(
    overview: &Value,
    expected_path: &str,
    expected_detail: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let limitations = overview
        .get("limitations")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("overview limitations are missing"))?;
    assert!(limitations.iter().any(|limitation| {
        limitation.get("reason").and_then(Value::as_str) == Some("public-surface-unsupported")
            && limitation.get("path").and_then(Value::as_str) == Some(expected_path)
            && limitation
                .get("detail")
                .and_then(Value::as_str)
                .is_some_and(|detail| detail.contains(expected_detail))
    }));
    assert!(limitations.iter().all(|limitation| {
        limitation.get("reason").and_then(Value::as_str) == Some("public-surface-unsupported")
            && limitation.get("path").and_then(Value::as_str) == Some(expected_path)
    }));
    Ok(())
}

fn write_workspace_root(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::write(
        root.join("package.json"),
        r#"{"name":"app","private":true,"workspaces":["packages/*"]}"#,
    )?;
    Ok(())
}
