use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

#[test]
fn relative_extends_uses_exact_then_one_json_fallback() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(root.path(), "config/explicit.json", "{}\n")?;
    write(root.path(), "config/exact", "{}\n")?;
    write(root.path(), "config/exact.json", "{malformed\n")?;
    write(root.path(), "config/fallback.json", "{}\n")?;
    write(root.path(), "config/backslash.json", "{}\n")?;
    write(root.path(), "config/parent.json", "{}\n")?;

    write_app(
        root.path(),
        "apps/explicit",
        r#"{"extends":"../../config/explicit.json"}"#,
    )?;
    write_app(
        root.path(),
        "apps/exact",
        r#"{"extends":"../../config/exact"}"#,
    )?;
    write_app(
        root.path(),
        "apps/fallback",
        r#"{"extends":"../../config/fallback"}"#,
    )?;
    write_app(
        root.path(),
        "apps/backslash",
        r#"{"extends":"..\\..\\config\\backslash"}"#,
    )?;
    write_app(
        root.path(),
        "apps/nested/child",
        r#"{"extends":"../../../config/parent.json"}"#,
    )?;

    let overview = audit_overview(root.path(), "complete")?;
    assert_eq!(
        overview.get("limitationCount").and_then(Value::as_u64),
        Some(0)
    );
    Ok(())
}

#[test]
fn unsupported_extends_forms_create_no_hidden_probe() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"workspaces":["packages/*"]}"#,
    )?;
    write(
        root.path(),
        "packages/config/package.json",
        r#"{"name":"@acme/config","tsconfig":"poison.json"}"#,
    )?;
    write(root.path(), "packages/config/poison.json", "{malformed\n")?;
    write(
        root.path(),
        "node_modules/external-config/poison.json",
        "{malformed\n",
    )?;

    write_app(
        root.path(),
        "apps/empty-component",
        r#"{"extends":".//poison.json"}"#,
    )?;
    write(
        root.path(),
        "apps/empty-component/poison.json",
        "{malformed\n",
    )?;
    write_app(
        root.path(),
        "apps/trailing-separator",
        r#"{"extends":"./poison/"}"#,
    )?;
    write(
        root.path(),
        "apps/trailing-separator/poison.json",
        "{malformed\n",
    )?;
    write_app(
        root.path(),
        "apps/package-subpath",
        r#"{"extends":"@acme/config/base"}"#,
    )?;
    write_app(
        root.path(),
        "apps/external",
        r#"{"extends":"external-config"}"#,
    )?;

    write(root.path(), "poison/rooted.json", "{malformed\n")?;
    let rooted = native_config_path(&fs::canonicalize(root.path().join("poison/rooted.json"))?)?;
    write_app(
        root.path(),
        "apps/rooted",
        &serde_json::json!({"extends": rooted}).to_string(),
    )?;

    let overview = audit_overview(root.path(), "incomplete")?;
    assert_eq!(
        overview.get("limitationCount").and_then(Value::as_u64),
        Some(5)
    );
    assert_eq!(
        limitation_reasons(&overview)?,
        BTreeSet::from(["tsconfig-semantics-unsupported".to_owned()])
    );
    Ok(())
}

#[test]
fn malformed_and_root_escaping_extends_hard_stop() -> Result<(), Box<dyn std::error::Error>> {
    let empty = hard_stop_fixture("tsconfig.json", r#"{"extends":""}"#)?;
    assert_audit_hard_stop(empty.path(), "extends must be a nonempty NUL-free string")?;

    let nul = hard_stop_fixture("tsconfig.json", r#"{"extends":"\u0000"}"#)?;
    assert_audit_hard_stop(nul.path(), "extends must be a nonempty NUL-free string")?;

    let lexical = hard_stop_fixture(
        "nested/tsconfig.json",
        r#"{"extends":"../../outside.json"}"#,
    )?;
    assert_audit_hard_stop(lexical.path(), "extends escapes the repository root")?;

    let rooted = tempfile::tempdir()?;
    let rooted_outside = tempfile::tempdir()?;
    write(rooted.path(), "main.ts", "export const value = 1;\n")?;
    let outside_path =
        native_config_path(&fs::canonicalize(rooted_outside.path())?.join("base.json"))?;
    write(
        rooted.path(),
        "tsconfig.json",
        &serde_json::json!({"extends": outside_path}).to_string(),
    )?;
    assert_audit_hard_stop(rooted.path(), "extends path escapes the repository root")?;

    let physical = tempfile::tempdir()?;
    let physical_outside = tempfile::tempdir()?;
    write(physical.path(), "main.ts", "export const value = 1;\n")?;
    write(physical_outside.path(), "base.json", "{}\n")?;
    let alias = physical.path().join("alias");
    create_directory_alias(physical_outside.path(), &alias)?;
    write(
        physical.path(),
        "tsconfig.json",
        r#"{"extends":"./alias/base.json"}"#,
    )?;
    let result = assert_audit_hard_stop(
        physical.path(),
        "config path resolves outside the repository root",
    );
    remove_directory_alias(&alias)?;
    result
}

#[test]
fn workspace_identity_is_exact_and_duplicate_identity_keeps_inventory_ownership()
-> Result<(), Box<dyn std::error::Error>> {
    let exact = tempfile::tempdir()?;
    write(
        exact.path(),
        "package.json",
        r#"{"workspaces":["packages/*","apps/*"]}"#,
    )?;
    write(
        exact.path(),
        "packages/scoped/package.json",
        r#"{"name":"@acme/tsconfig"}"#,
    )?;
    write(exact.path(), "packages/scoped/tsconfig.json", "{}\n")?;
    write(
        exact.path(),
        "packages/unscoped/package.json",
        r#"{"name":"tooling-config"}"#,
    )?;
    write(exact.path(), "packages/unscoped/tsconfig.json", "{}\n")?;
    write(
        exact.path(),
        "apps/scoped/package.json",
        r#"{"name":"scoped-app","private":true}"#,
    )?;
    write_app(
        exact.path(),
        "apps/scoped",
        r#"{"extends":"@acme/tsconfig"}"#,
    )?;
    write(
        exact.path(),
        "apps/unscoped/package.json",
        r#"{"name":"unscoped-app","private":true}"#,
    )?;
    write_app(
        exact.path(),
        "apps/unscoped",
        r#"{"extends":"tooling-config"}"#,
    )?;
    let overview = audit_overview(exact.path(), "complete")?;
    assert_eq!(
        overview.get("limitationCount").and_then(Value::as_u64),
        Some(0)
    );

    let duplicate = tempfile::tempdir()?;
    write(
        duplicate.path(),
        "package.json",
        r#"{"workspaces":["packages/*"]}"#,
    )?;
    for package in ["one", "two"] {
        write(
            duplicate.path(),
            &format!("packages/{package}/package.json"),
            r#"{"name":"@acme/tsconfig","tsconfig":"poison.json"}"#,
        )?;
        write(
            duplicate.path(),
            &format!("packages/{package}/tsconfig.json"),
            "{}\n",
        )?;
        write(
            duplicate.path(),
            &format!("packages/{package}/poison.json"),
            "{malformed\n",
        )?;
    }
    write(
        duplicate.path(),
        "tsconfig.json",
        r#"{"extends":"@acme/tsconfig"}"#,
    )?;
    write(duplicate.path(), "main.ts", "export const value = 1;\n")?;
    let overview = audit_overview(duplicate.path(), "incomplete")?;
    assert_eq!(
        overview.get("limitationCount").and_then(Value::as_u64),
        Some(2)
    );
    assert_eq!(
        limitation_reasons(&overview)?,
        BTreeSet::from(["package-identity-unsupported".to_owned()])
    );
    Ok(())
}

#[test]
fn missing_extends_reservation_conflicts_through_parent_alias_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(root.path(), "src/a.ts", "export const value = 1;\n")?;
    write(
        root.path(),
        "config/helper.ts",
        "export const helper = 1;\n",
    )?;
    let alias = root.path().join("alias");
    create_directory_alias(&root.path().join("config"), &alias)?;

    let writer = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-parent-writer",
            "--path",
            "config",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&writer, 0);
    let writer_gate = field(&writer.stdout, "gateId")?;

    write(
        root.path(),
        "src/tsconfig.json",
        r#"{"extends":"../alias/base"}"#,
    )?;
    let reader = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-parent-reader",
            "--path",
            "src/new.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&reader, 4);
    assert_eq!(field(&reader.stdout, "decision")?, "incomplete");
    let response: Value = serde_json::from_str(&reader.stdout)?;
    let conflict = response
        .get("signals")
        .and_then(Value::as_array)
        .and_then(|signals| {
            signals.iter().find(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some("semantic-input-conflict")
            })
        })
        .ok_or_else(|| {
            std::io::Error::other(format!(
                "semantic input conflict is missing: {}",
                reader.stdout
            ))
        })?;
    assert_eq!(
        conflict.pointer("/paths/0/display").and_then(Value::as_str),
        Some("alias/base")
    );
    assert_eq!(
        conflict.pointer("/gateIds/0").and_then(Value::as_str),
        Some(writer_gate.as_str())
    );

    let retry = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-parent-reader",
            "--path",
            "src/new.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&retry, 4);
    assert_eq!(retry.stdout, reader.stdout);

    remove_directory_alias(&alias)?;
    Ok(())
}

fn hard_stop_fixture(
    config_path: &str,
    config: &str,
) -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let parent = Path::new(config_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    write(
        root.path(),
        &parent.join("main.ts").to_string_lossy(),
        "export const value = 1;\n",
    )?;
    write(root.path(), config_path, config)?;
    Ok(root)
}

fn write_app(root: &Path, directory: &str, config: &str) -> std::io::Result<()> {
    write(root, &format!("{directory}/tsconfig.json"), config)?;
    write(
        root,
        &format!("{directory}/main.ts"),
        "export const value = 1;\n",
    )
}

fn audit_overview(root: &Path, expected_status: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let audit = run(root, &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    assert_eq!(field(&audit.stdout, "status")?, expected_status);
    let run_id = field(&audit.stdout, "runId")?;
    let overview = run(root, &["overview", "--run", &run_id])?;
    assert_status(&overview, 0);
    Ok(serde_json::from_str(&overview.stdout)?)
}

fn assert_audit_hard_stop(
    root: &Path,
    expected_detail: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let audit = run(root, &["audit", "--jobs", "1"])?;
    assert_status(&audit, 1);
    assert!(audit.stdout.is_empty());
    assert!(
        audit.stderr.contains(expected_detail),
        "stderr did not contain {expected_detail:?}: {}",
        audit.stderr
    );
    let overview = run(root, &["overview"])?;
    assert_status(&overview, 0);
    let overview: Value = serde_json::from_str(&overview.stdout)?;
    assert_eq!(
        overview
            .pointer("/latestAttempt/status")
            .and_then(Value::as_str),
        Some("failed")
    );
    assert_eq!(
        overview.pointer("/scope/kind").and_then(Value::as_str),
        Some("attempt")
    );
    Ok(())
}

fn limitation_reasons(overview: &Value) -> Result<BTreeSet<String>, std::io::Error> {
    overview
        .get("limitations")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("limitations are missing"))?
        .iter()
        .map(|limitation| {
            limitation
                .get("reason")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| std::io::Error::other("limitation reason is missing"))
        })
        .collect()
}

fn native_config_path(path: &Path) -> Result<String, std::io::Error> {
    path.to_str()
        .map(|value| value.replace('\\', "/"))
        .ok_or_else(|| std::io::Error::other("temporary path is not UTF-8"))
}

fn write(root: &Path, relative: &str, contents: &str) -> std::io::Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}

#[cfg(unix)]
fn create_directory_alias(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(unix)]
fn remove_directory_alias(link: &Path) -> std::io::Result<()> {
    fs::remove_file(link)
}

#[cfg(windows)]
fn create_directory_alias(target: &Path, link: &Path) -> std::io::Result<()> {
    let status = std::process::Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "mklink /J failed with {status}"
        )))
    }
}

#[cfg(windows)]
fn remove_directory_alias(link: &Path) -> std::io::Result<()> {
    fs::remove_dir(link)
}
