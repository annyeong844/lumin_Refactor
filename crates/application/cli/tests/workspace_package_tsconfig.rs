use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

type FindingTuple = (String, String, String);
type LimitationTuple = (String, String, String);

#[test]
fn custom_field_fallback_and_child_override_apply_through_public_behavior()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write_workspace_root(root.path())?;

    write(
        root.path(),
        "packages/custom/package.json",
        r#"{"name":"@acme/custom","tsconfig":"configs/base.json"}"#,
    )?;
    write(
        root.path(),
        "packages/custom/configs/base.json",
        r#"{"compilerOptions":{"baseUrl":"..","paths":{"@custom":["src/selected"]}}}"#,
    )?;
    write(
        root.path(),
        "packages/custom/tsconfig.json",
        r#"{"compilerOptions":{"baseUrl":".","paths":{"@custom":["src/decoy"]}}}"#,
    )?;
    module(
        root.path(),
        "packages/custom/src/selected.ts",
        "customUsed",
        "customSelectedDead",
    )?;
    module(
        root.path(),
        "packages/custom/src/decoy.ts",
        "customUsed",
        "customDecoyDead",
    )?;

    write(
        root.path(),
        "packages/fallback/package.json",
        r#"{"name":"@acme/fallback"}"#,
    )?;
    write(
        root.path(),
        "packages/fallback/tsconfig.json",
        r#"{"compilerOptions":{"baseUrl":".","paths":{"@fallback":["src/selected"]}}}"#,
    )?;
    module(
        root.path(),
        "packages/fallback/src/selected.ts",
        "fallbackUsed",
        "fallbackSelectedDead",
    )?;

    write_app(
        root.path(),
        "apps/custom",
        "custom-app",
        r#"{"extends":"@acme/custom"}"#,
        "import { customUsed } from '@custom'; console.log(customUsed);\n",
    )?;
    write_app(
        root.path(),
        "apps/override",
        "override-app",
        r#"{"extends":"@acme/custom","compilerOptions":{"baseUrl":".","paths":{"@custom":["child"]}}}"#,
        "import { customUsed } from '@custom'; console.log(customUsed);\n",
    )?;
    module(
        root.path(),
        "apps/override/child.ts",
        "customUsed",
        "childSelectedDead",
    )?;
    write_app(
        root.path(),
        "apps/fallback",
        "fallback-app",
        r#"{"extends":"@acme/fallback"}"#,
        "import { fallbackUsed } from '@fallback'; console.log(fallbackUsed);\n",
    )?;

    let (run_id, overview) = audit_overview(root.path(), "complete")?;
    assert_eq!(
        overview.get("limitationCount").and_then(Value::as_u64),
        Some(0)
    );
    assert_eq!(
        finding_set(root.path(), &run_id)?,
        BTreeSet::from([
            finding("apps/override/child.ts", "childSelectedDead"),
            finding("packages/custom/src/decoy.ts", "customDecoyDead"),
            finding("packages/custom/src/decoy.ts", "customUsed"),
            finding("packages/custom/src/selected.ts", "customSelectedDead"),
            finding("packages/fallback/src/selected.ts", "fallbackSelectedDead"),
        ])
    );
    Ok(())
}

#[test]
fn malformed_and_package_escaping_fields_create_no_hidden_probe()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write_workspace_root(root.path())?;

    write(
        root.path(),
        "packages/empty/package.json",
        r#"{"name":"@acme/empty","tsconfig":""}"#,
    )?;
    write(
        root.path(),
        "packages/shape/package.json",
        r#"{"name":"@acme/shape","tsconfig":["poison.json"]}"#,
    )?;
    write(root.path(), "packages/shape/poison.json", "{malformed\n")?;
    write(
        root.path(),
        "packages/escape/package.json",
        r#"{"name":"@acme/escape","tsconfig":"../poison.json"}"#,
    )?;
    write(root.path(), "packages/poison.json", "{malformed\n")?;

    write(root.path(), "packages/rooted/poison.json", "{malformed\n")?;
    let rooted_target = native_config_path(&fs::canonicalize(
        root.path().join("packages/rooted/poison.json"),
    )?)?;
    write(
        root.path(),
        "packages/rooted/package.json",
        &serde_json::json!({"name":"@acme/rooted","tsconfig":rooted_target}).to_string(),
    )?;

    for (directory, package) in [
        ("empty", "@acme/empty"),
        ("shape", "@acme/shape"),
        ("escape", "@acme/escape"),
        ("rooted", "@acme/rooted"),
    ] {
        write_app(
            root.path(),
            &format!("apps/{directory}"),
            &format!("{directory}-app"),
            &serde_json::json!({"extends":package}).to_string(),
            "export const value = 1;\n",
        )?;
    }

    let (_, overview) = audit_overview(root.path(), "incomplete")?;
    let limitations = limitation_tuples(&overview)?;
    assert_eq!(limitations.len(), 4);
    assert_eq!(
        limitations.iter().cloned().collect::<BTreeSet<_>>(),
        BTreeSet::from([
            limitation(
                "packages/empty/package.json",
                "workspace package tsconfig field must be a nonempty string",
            ),
            limitation(
                "packages/escape/package.json",
                "workspace package tsconfig target escapes the package root",
            ),
            limitation(
                "packages/rooted/package.json",
                "workspace package tsconfig target must be package-relative",
            ),
            limitation(
                "packages/shape/package.json",
                "workspace package tsconfig field must be a nonempty string",
            ),
        ])
    );
    Ok(())
}

#[test]
fn repository_escaping_workspace_targets_hard_stop() -> Result<(), Box<dyn std::error::Error>> {
    let lexical = tempfile::tempdir()?;
    write_workspace_consumer(
        lexical.path(),
        r#"{"name":"@acme/config","tsconfig":"../../../outside.json"}"#,
    )?;
    assert_audit_hard_stop(
        lexical.path(),
        "workspace package tsconfig target escapes the repository root",
    )?;

    let rooted = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    let outside_target = native_config_path(&fs::canonicalize(outside.path())?.join("base.json"))?;
    write_workspace_consumer(
        rooted.path(),
        &serde_json::json!({"name":"@acme/config","tsconfig":outside_target}).to_string(),
    )?;
    assert_audit_hard_stop(
        rooted.path(),
        "workspace package tsconfig target escapes the repository root",
    )?;

    let physical = tempfile::tempdir()?;
    let physical_outside = tempfile::tempdir()?;
    write(physical_outside.path(), "base.json", "{}\n")?;
    write_workspace_consumer(
        physical.path(),
        r#"{"name":"@acme/config","tsconfig":"alias/base.json"}"#,
    )?;
    let alias = physical
        .path()
        .join("packages")
        .join("config")
        .join("alias");
    create_directory_alias(physical_outside.path(), &alias)?;
    let result = assert_audit_hard_stop(
        physical.path(),
        "config path resolves outside the repository root",
    );
    remove_directory_alias(&alias)?;
    result
}

#[test]
fn missing_nonregular_and_cyclic_targets_are_scoped() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write_workspace_root(root.path())?;
    write(
        root.path(),
        "packages/missing/package.json",
        r#"{"name":"@acme/missing","tsconfig":"configs/missing.json"}"#,
    )?;
    write(
        root.path(),
        "packages/nonregular/package.json",
        r#"{"name":"@acme/nonregular","tsconfig":"configs"}"#,
    )?;
    fs::create_dir_all(root.path().join("packages/nonregular/configs"))?;
    write(
        root.path(),
        "packages/cycle/package.json",
        r#"{"name":"@acme/cycle","tsconfig":"configs/base.json"}"#,
    )?;
    write(
        root.path(),
        "packages/cycle/configs/base.json",
        r#"{"extends":"@acme/cycle"}"#,
    )?;
    for (directory, package) in [
        ("missing", "@acme/missing"),
        ("nonregular", "@acme/nonregular"),
        ("cycle", "@acme/cycle"),
    ] {
        write_app(
            root.path(),
            &format!("apps/{directory}"),
            &format!("{directory}-app"),
            &serde_json::json!({"extends":package}).to_string(),
            "export const value = 1;\n",
        )?;
    }

    let (_, overview) = audit_overview(root.path(), "incomplete")?;
    assert_eq!(
        limitation_tuples(&overview)?
            .into_iter()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            limitation("packages/cycle/configs/base.json", "config extends cycle",),
            limitation(
                "packages/missing/configs/missing.json",
                "selected config target is missing or non-regular",
            ),
            limitation(
                "packages/nonregular/configs",
                "selected config target is missing or non-regular",
            ),
        ])
    );
    Ok(())
}

#[test]
fn selected_workspace_target_is_reserved_before_capture_and_retry_is_idempotent()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"workspaces":["packages/*","apps/*"]}"#,
    )?;
    write(
        root.path(),
        "packages/config/package.json",
        r#"{"name":"@acme/config","tsconfig":"configs/base.json"}"#,
    )?;
    write(root.path(), "packages/config/configs/base.json", "{}\n")?;
    fs::hard_link(
        root.path().join("packages/config/configs/base.json"),
        root.path().join("packages/config/configs/base-writer.ts"),
    )?;
    write(root.path(), "seed.ts", "export const seed = 1;\n")?;

    let writer = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-workspace-target-writer",
            "--path",
            "packages/config/configs/base-writer.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&writer, 0);
    let writer_gate = field(&writer.stdout, "gateId")?;

    write_app(
        root.path(),
        "apps/consumer",
        "consumer-app",
        r#"{"extends":"@acme/config"}"#,
        "export const value = 1;\n",
    )?;
    let reader_arguments = [
        "pre-write",
        "--operation-id",
        "op-workspace-target-reader",
        "--path",
        "apps/consumer/new.ts",
        "--jobs",
        "1",
    ];
    let reader = run(root.path(), &reader_arguments)?;
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
        .ok_or_else(|| std::io::Error::other("semantic input conflict is missing"))?;
    assert_eq!(
        conflict.pointer("/paths/0/display").and_then(Value::as_str),
        Some("packages/config/configs/base.json")
    );
    assert_eq!(
        conflict.pointer("/gateIds/0").and_then(Value::as_str),
        Some(writer_gate.as_str())
    );

    let retry = run(root.path(), &reader_arguments)?;
    assert_status(&retry, 4);
    assert_eq!(retry.stdout, reader.stdout);
    Ok(())
}

fn write_workspace_root(root: &Path) -> std::io::Result<()> {
    write(
        root,
        "package.json",
        r#"{"workspaces":["packages/*","apps/*"]}"#,
    )
}

fn write_workspace_consumer(root: &Path, package_json: &str) -> std::io::Result<()> {
    write_workspace_root(root)?;
    write(root, "packages/config/package.json", package_json)?;
    write_app(
        root,
        "apps/consumer",
        "consumer-app",
        r#"{"extends":"@acme/config"}"#,
        "export const value = 1;\n",
    )
}

fn write_app(
    root: &Path,
    directory: &str,
    name: &str,
    config: &str,
    source: &str,
) -> std::io::Result<()> {
    write(
        root,
        &format!("{directory}/package.json"),
        &serde_json::json!({"name":name,"private":true}).to_string(),
    )?;
    write(root, &format!("{directory}/tsconfig.json"), config)?;
    write(root, &format!("{directory}/main.ts"), source)
}

fn audit_overview(
    root: &Path,
    expected_status: &str,
) -> Result<(String, Value), Box<dyn std::error::Error>> {
    let audit = run(root, &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    assert_eq!(field(&audit.stdout, "status")?, expected_status);
    let run_id = field(&audit.stdout, "runId")?;
    let overview = run(root, &["overview", "--run", &run_id])?;
    assert_status(&overview, 0);
    Ok((run_id, serde_json::from_str(&overview.stdout)?))
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
        overview.pointer("/scope/kind").and_then(Value::as_str),
        Some("attempt")
    );
    assert_eq!(
        overview
            .pointer("/latestAttempt/status")
            .and_then(Value::as_str),
        Some("failed")
    );
    Ok(())
}

fn limitation(path: &str, detail: &str) -> LimitationTuple {
    (
        "tsconfig-semantics-unsupported".to_owned(),
        path.to_owned(),
        detail.to_owned(),
    )
}

fn limitation_tuples(overview: &Value) -> Result<Vec<LimitationTuple>, std::io::Error> {
    overview
        .get("limitations")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("limitations are missing"))?
        .iter()
        .map(|limitation| {
            let field = |name| {
                limitation
                    .get(name)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| std::io::Error::other(format!("{name} is missing")))
            };
            Ok((field("reason")?, field("path")?, field("detail")?))
        })
        .collect()
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

fn module(root: &Path, relative: &str, used: &str, dead: &str) -> std::io::Result<()> {
    write(
        root,
        relative,
        &format!("export const {used} = 1; export const {dead} = 2;\n"),
    )
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
