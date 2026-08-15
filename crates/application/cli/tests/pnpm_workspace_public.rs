use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

type FindingView = (String, String);

#[test]
fn pnpm_precedence_and_missing_packages_are_public() -> Result<(), Box<dyn std::error::Error>> {
    assert_package_workspace_object_membership()?;
    assert_same_directory_pnpm_precedence()?;
    assert_missing_pnpm_packages_is_root_only()?;
    Ok(())
}

#[test]
fn package_configs_pinned_forms_emit_typed_limitations() -> Result<(), Box<dyn std::error::Error>> {
    for yaml in [
        "packageConfigs:\n  project-1:\n    saveExact: true\n",
        "packageConfigs:\n  - match: [project-1, project-2]\n    saveExact: true\n",
    ] {
        let root = tempfile::tempdir()?;
        write(
            root.path(),
            "package.json",
            r#"{"name":"app","private":true}"#,
        )?;
        write(root.path(), "pnpm-workspace.yaml", yaml)?;
        write(root.path(), "src/main.ts", "export const dead = 1;\n")?;

        let audit = run(root.path(), &["audit", "--jobs", "1"])?;
        assert_status(&audit, 0);
        let audit_json: Value = serde_json::from_str(&audit.stdout)?;
        assert_eq!(
            audit_json.get("status").and_then(Value::as_str),
            Some("complete")
        );
        assert_eq!(
            audit_json.get("limitationCount").and_then(Value::as_u64),
            Some(1)
        );
        let run_id = field(&audit.stdout, "runId")?;

        let overview = run(root.path(), &["overview", "--run", &run_id])?;
        assert_status(&overview, 0);
        let overview: Value = serde_json::from_str(&overview.stdout)?;
        let limitations = overview
            .get("limitations")
            .and_then(Value::as_array)
            .ok_or_else(|| std::io::Error::other("overview limitations are missing"))?;
        assert_eq!(limitations.len(), 1);
        assert_eq!(
            limitations[0].get("reason").and_then(Value::as_str),
            Some("pnpm-dependency-semantics-unsupported")
        );
        assert_eq!(
            limitations[0].get("path").and_then(Value::as_str),
            Some("pnpm-workspace.yaml")
        );
        assert_eq!(
            limitations[0].get("detail").and_then(Value::as_str),
            Some("pnpm packageConfigs semantics are unsupported")
        );
        assert_eq!(
            findings(root.path(), &run_id)?,
            BTreeSet::from([("src/main.ts".to_owned(), "dead".to_owned())]),
            "pnpm dependency-only uncertainty must not suppress dead-code evidence",
        );
    }
    Ok(())
}

#[test]
fn malformed_pnpm_hard_stops_without_fallback() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"app","workspaces":["packages/*"]}"#,
    )?;
    write(
        root.path(),
        "pnpm-workspace.yaml",
        "packages: []\npackages: [packages/*]\n",
    )?;
    write(
        root.path(),
        "packages/lib/package.json",
        r#"{"name":"@acme/lib"}"#,
    )?;
    write(
        root.path(),
        "packages/lib/index.ts",
        "export const used = 1; export const dead = 2;\n",
    )?;
    write(
        root.path(),
        "src/main.ts",
        "import { used } from '@acme/lib'; console.log(used);\n",
    )?;

    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 1);
    assert!(audit.stdout.is_empty());
    assert!(
        audit.stderr.contains("malformed configuration")
            && audit.stderr.contains("pnpm-workspace.yaml")
            && audit.stderr.contains("duplicate"),
        "malformed pnpm diagnostic was not preserved: {}",
        audit.stderr
    );
    Ok(())
}

fn assert_package_workspace_object_membership() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"app","private":true,"workspaces":{"packages":["packages/*"]}}"#,
    )?;
    write_package(root.path(), "packages/lib", "@acme/lib", "used", "dead")?;
    write(
        root.path(),
        "src/main.ts",
        "import { used } from '@acme/lib'; console.log(used);\n",
    )?;

    assert_eq!(
        complete_audit_findings(root.path())?,
        BTreeSet::from([("packages/lib/index.ts".to_owned(), "dead".to_owned())])
    );
    Ok(())
}

fn assert_same_directory_pnpm_precedence() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"app","private":true,"workspaces":["legacy/*"]}"#,
    )?;
    write(
        root.path(),
        "pnpm-workspace.yaml",
        "packages:\n  - packages/*\n",
    )?;
    write_package(
        root.path(),
        "packages/selected",
        "@acme/selected",
        "selectedUsed",
        "selectedDead",
    )?;
    write_package(
        root.path(),
        "legacy/lib",
        "@acme/legacy",
        "legacyUsed",
        "legacyDead",
    )?;
    write(
        root.path(),
        "src/main.ts",
        concat!(
            "import { selectedUsed } from '@acme/selected';\n",
            "import { legacyUsed } from '@acme/legacy';\n",
            "console.log(selectedUsed, legacyUsed);\n",
        ),
    )?;

    assert_eq!(
        complete_audit_findings(root.path())?,
        BTreeSet::from([
            ("legacy/lib/index.ts".to_owned(), "legacyDead".to_owned(),),
            ("legacy/lib/index.ts".to_owned(), "legacyUsed".to_owned(),),
            (
                "packages/selected/index.ts".to_owned(),
                "selectedDead".to_owned(),
            ),
        ])
    );
    Ok(())
}

fn assert_missing_pnpm_packages_is_root_only() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"app","private":true,"workspaces":["packages/*"]}"#,
    )?;
    write(root.path(), "pnpm-workspace.yaml", "{}\n")?;
    write_package(root.path(), "packages/lib", "@acme/lib", "used", "dead")?;
    write(
        root.path(),
        "src/main.ts",
        "import { used } from '@acme/lib'; console.log(used);\n",
    )?;

    assert_eq!(
        complete_audit_findings(root.path())?,
        BTreeSet::from([
            ("packages/lib/index.ts".to_owned(), "dead".to_owned()),
            ("packages/lib/index.ts".to_owned(), "used".to_owned()),
        ])
    );
    Ok(())
}

fn complete_audit_findings(
    root: &Path,
) -> Result<BTreeSet<FindingView>, Box<dyn std::error::Error>> {
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
    findings(root, &run_id)
}

fn findings(
    root: &Path,
    run_id: &str,
) -> Result<BTreeSet<FindingView>, Box<dyn std::error::Error>> {
    let result = run(root, &["findings", "--run", run_id, "--area", "dead-code"])?;
    assert_status(&result, 0);
    let result: Value = serde_json::from_str(&result.stdout)?;
    let items = result
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("findings are missing"))?;
    assert_eq!(
        result.get("scopeTotal").and_then(Value::as_u64),
        Some(items.len() as u64)
    );
    assert_eq!(
        result.get("total").and_then(Value::as_u64),
        Some(items.len() as u64)
    );
    assert_eq!(
        result.get("truncated").and_then(Value::as_bool),
        Some(false)
    );
    items
        .iter()
        .map(|item| {
            let required = |pointer: &str| {
                item.pointer(pointer)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| std::io::Error::other(format!("missing {pointer}")))
            };
            Ok((required("/path/display")?, required("/exportedName")?))
        })
        .collect::<Result<_, std::io::Error>>()
        .map_err(Into::into)
}

fn write_package(
    root: &Path,
    relative: &str,
    name: &str,
    used: &str,
    dead: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    write(
        root,
        &format!("{relative}/package.json"),
        &format!(r#"{{"name":"{name}","private":true}}"#),
    )?;
    write(
        root,
        &format!("{relative}/index.ts"),
        &format!("export const {used} = 1; export const {dead} = 2;\n"),
    )
}

fn write(root: &Path, relative: &str, contents: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(())
}
