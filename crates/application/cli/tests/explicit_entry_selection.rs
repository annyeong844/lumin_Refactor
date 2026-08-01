use std::collections::BTreeMap;
use std::fs;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

#[test]
fn explicit_entries_replace_deduplicate_and_preserve_alias_contexts()
-> Result<(), Box<dyn std::error::Error>> {
    assert_available_entry_selection_and_package_roots()?;
    assert_unavailable_entries_remain_typed_and_deduplicated()?;
    Ok(())
}

fn assert_available_entry_selection_and_package_roots() -> Result<(), Box<dyn std::error::Error>> {
    let root = workspace_fixture()?;
    let opened = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-explicit-entries",
            "--path",
            "src/write.ts",
            "--entry",
            "src/from-invocation.ts",
            "--entry",
            "src/from-invocation.ts",
            "--entry",
            "src/original.ts",
            "--entry",
            "src/alias.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);
    assert_eq!(field(&opened.stdout, "decision")?, "allow-with-warnings");
    let gate_id = field(&opened.stdout, "gateId")?;

    let shown = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&shown, 0);
    let shown_json: Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(
        shown_json
            .pointer("/baseline/limitationCount")
            .and_then(Value::as_u64),
        Some(0)
    );
    let selections = shown_json
        .pointer("/baseline/entrySelections")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("baseline entrySelections are missing"))?;
    assert_eq!(selections.len(), 3);
    let expected_paths = ["src/alias.ts", "src/from-invocation.ts", "src/original.ts"];
    let selected_by_path = selections
        .iter()
        .map(|selection| {
            assert_eq!(
                selection
                    .pointer("/path/schemaVersion")
                    .and_then(Value::as_str),
                Some("repo-path.v1")
            );
            assert_eq!(
                selection.get("source").and_then(Value::as_str),
                Some("invocation")
            );
            assert!(selection.get("unavailableReason").is_none());
            selection
                .pointer("/path/display")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| std::io::Error::other("entry selection path is missing"))
        })
        .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
    assert_eq!(
        selected_by_path,
        expected_paths.into_iter().map(str::to_owned).collect()
    );
    assert!(!selections.iter().any(|selection| {
        selection.pointer("/path/display").and_then(Value::as_str) == Some("src/from-config.ts")
            || selection.get("source").and_then(Value::as_str) == Some("configuration")
    }));

    let findings = run(
        root.path(),
        &["gate", "findings", &gate_id, "--revision", "0"],
    )?;
    assert_status(&findings, 0);
    let findings_json: Value = serde_json::from_str(&findings.stdout)?;
    let items = findings_json
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("gate findings items are missing"))?;
    let by_path = items
        .iter()
        .map(|item| {
            let path = item
                .pointer("/path/display")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("finding path is missing"))?;
            Ok((path.to_owned(), item))
        })
        .collect::<Result<BTreeMap<_, _>, Box<dyn std::error::Error>>>()?;

    for alias_path in ["src/original.ts", "src/alias.ts"] {
        let finding = by_path
            .get(alias_path)
            .ok_or_else(|| std::io::Error::other(format!("missing finding for {alias_path}")))?;
        assert_eq!(
            finding.get("exportedName").and_then(Value::as_str),
            Some("aliasedDead")
        );
    }
    assert!(
        !by_path.contains_key("packages/public-pkg/src/index.ts"),
        "public package target lost its independent surface protection"
    );

    let private_finding = by_path
        .get("packages/private-pkg/src/orphan.ts")
        .ok_or_else(|| std::io::Error::other("private package export-level finding is missing"))?;
    assert_eq!(
        private_finding.get("ruleId").and_then(Value::as_str),
        Some("dead-code/zero-exact-fan-in.v1")
    );
    assert_eq!(
        private_finding
            .pointer("/disposition/kind")
            .and_then(Value::as_str),
        Some("review-candidate")
    );
    assert!(
        private_finding
            .get("claim")
            .and_then(Value::as_str)
            .is_some_and(|claim| claim.contains("zero grounded exact fan-in"))
    );
    assert!(items.iter().all(|item| {
        item.get("ruleId").and_then(Value::as_str) == Some("dead-code/zero-exact-fan-in.v1")
            && !item
                .get("claim")
                .and_then(Value::as_str)
                .is_some_and(|claim| claim.contains("unreachable"))
    }));
    Ok(())
}

fn assert_unavailable_entries_remain_typed_and_deduplicated()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(
        root.path().join("src/excluded.ts"),
        "export const excluded = 1;\n",
    )?;
    let audit = run(
        root.path(),
        &[
            "audit",
            "--entry",
            "src/missing.ts",
            "--entry",
            "src/missing.ts",
            "--entry",
            "src/excluded.ts",
            "--exclude",
            "src/excluded.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&audit, 0);
    let run_id = field(&audit.stdout, "runId")?;
    let overview = run(root.path(), &["overview", "--run", &run_id])?;
    assert_status(&overview, 0);
    let overview_json: Value = serde_json::from_str(&overview.stdout)?;
    let limitations = overview_json
        .get("limitations")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("overview limitations are missing"))?;
    assert_eq!(limitations.len(), 2);
    let reasons = limitations
        .iter()
        .map(|limitation| {
            assert_eq!(
                limitation.get("reason").and_then(Value::as_str),
                Some("explicit-entry-unavailable")
            );
            assert_eq!(
                limitation.get("source").and_then(Value::as_str),
                Some("invocation")
            );
            let path = limitation
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("limitation path is missing"))?;
            let reason = limitation
                .get("unavailable_reason")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("unavailable reason is missing"))?;
            Ok((path.to_owned(), reason.to_owned()))
        })
        .collect::<Result<BTreeMap<_, _>, Box<dyn std::error::Error>>>()?;
    assert_eq!(
        reasons.get("src/missing.ts").map(String::as_str),
        Some("missing")
    );
    assert_eq!(
        reasons.get("src/excluded.ts").map(String::as_str),
        Some("excluded")
    );
    Ok(())
}

fn workspace_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    for directory in ["src", "packages/public-pkg/src", "packages/private-pkg/src"] {
        fs::create_dir_all(root.path().join(directory))?;
    }
    fs::write(
        root.path().join("package.json"),
        r#"{"private":true,"workspaces":["packages/*"]}"#,
    )?;
    fs::write(
        root.path().join("lumin.json"),
        r#"{"schemaVersion":"lumin-config.v1","entries":["src/from-config.ts"]}"#,
    )?;
    fs::write(
        root.path().join("src/from-config.ts"),
        "export const configuredDead = 1;\n",
    )?;
    fs::write(
        root.path().join("src/from-invocation.ts"),
        "export const invocationDead = 1;\n",
    )?;
    fs::write(root.path().join("src/write.ts"), "console.log('write');\n")?;
    fs::write(
        root.path().join("src/original.ts"),
        "export const aliasedDead = 1;\n",
    )?;
    fs::hard_link(
        root.path().join("src/original.ts"),
        root.path().join("src/alias.ts"),
    )?;
    fs::write(
        root.path().join("packages/public-pkg/package.json"),
        r#"{"name":"public-pkg","exports":"./src/index.js"}"#,
    )?;
    fs::write(
        root.path().join("packages/public-pkg/src/index.ts"),
        "export const publicApi = 1;\n",
    )?;
    fs::write(
        root.path().join("packages/private-pkg/package.json"),
        r#"{"name":"private-pkg","private":true}"#,
    )?;
    fs::write(
        root.path().join("packages/private-pkg/src/orphan.ts"),
        "export const privateDead = 1;\n",
    )?;
    Ok(root)
}
