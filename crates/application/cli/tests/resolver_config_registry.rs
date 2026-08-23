use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

#[path = "support/gate.rs"]
mod gate;

use gate::{assert_incomplete_prewrite_retry, assert_probe_candidates_excluded};
use support::{assert_status, run};

type FindingView = (String, String, String);
type LimitationView = (String, String, String);

const FROZEN_ANALYSIS_CONTRACT: &str =
    "51f0b5add00f36c04ad6823ae2c81e87ba6558e26180412b964a65d0d49b75a1";

#[test]
fn resolver_artifact_identity_is_public_and_frozen() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"app","private":true}"#,
    )?;
    write(root.path(), "src/main.ts", "export const value = 1;\n")?;

    let opened = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-resolver-artifact-identity",
            "--path",
            "src/main.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);
    let opened: Value = serde_json::from_str(&opened.stdout)?;
    let gate_id = required_string(&opened, "gateId")?;

    let shown = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&shown, 0);
    let shown: Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(
        shown
            .pointer("/baseline/analysisContract")
            .and_then(Value::as_str),
        Some(FROZEN_ANALYSIS_CONTRACT),
    );
    assert_eq!(
        shown
            .pointer("/baseline/limitationCount")
            .and_then(Value::as_u64),
        Some(0),
    );
    Ok(())
}

#[test]
fn supported_and_neutral_fields_follow_registry() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"app","private":true,"type":"module"}"#,
    )?;
    write(
        root.path(),
        "tsconfig.json",
        concat!(
            r#"{"compilerOptions":{"moduleResolution":"bundler","module":"esnext","baseUrl":".","paths":{"@lib":["src/lib"]},"strict":true,"target":"es2022"}}"#,
            "\n",
        ),
    )?;
    module(root.path(), "src/lib.ts", "used", "dead")?;
    write(
        root.path(),
        "src/main.ts",
        "import { used } from '@lib'; console.log(used);\n",
    )?;

    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    let audit: Value = serde_json::from_str(&audit.stdout)?;
    assert_eq!(
        audit.get("status").and_then(Value::as_str),
        Some("complete")
    );
    assert_eq!(
        audit.get("limitationCount").and_then(Value::as_u64),
        Some(0)
    );
    let run_id = required_string(&audit, "runId")?;

    let overview = overview(root.path(), &run_id)?;
    assert_eq!(overview.get("limitations"), Some(&serde_json::json!([])));
    let profiles = overview
        .get("resolutionProfiles")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("resolutionProfiles are missing"))?;
    assert!(!profiles.is_empty());
    assert!(
        profiles.iter().all(|profile| {
            profile.get("profile").and_then(Value::as_str) == Some("bundler")
                && profile.pointer("/source/kind").and_then(Value::as_str) == Some("config")
                && profile
                    .pointer("/source/path_display")
                    .and_then(Value::as_str)
                    == Some("tsconfig.json")
        }),
        "unexpected config profile DTOs: {profiles:#?}",
    );
    assert_eq!(
        finding_set(root.path(), &run_id)?,
        BTreeSet::from([finding("src/lib.ts", "dead")])
    );
    Ok(())
}

#[test]
fn registry_failures_block_before_probing_and_override_cannot_hide()
-> Result<(), Box<dyn std::error::Error>> {
    assert_registry_failure_matrix()?;
    assert_tsconfig_probe_is_blocked()?;
    assert_unknown_condition_probe_is_blocked()?;
    Ok(())
}

fn assert_registry_failure_matrix() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write_workspace_root(root.path())?;

    write_package(root.path(), "unknown", "@acme/unknown", None)?;
    write(
        root.path(),
        "packages/unknown/tsconfig.json",
        r#"{"compilerOptions":{"madeUpFlag":true}}"#,
    )?;
    write_relative_pair(root.path(), "unknown")?;

    write_package(root.path(), "unsupported", "@acme/unsupported", None)?;
    write(
        root.path(),
        "packages/unsupported/tsconfig.json",
        r#"{"compilerOptions":{"moduleSuffixes":[".native",""]}}"#,
    )?;
    write_relative_pair(root.path(), "unsupported")?;

    write_package(root.path(), "malformed", "@acme/malformed", None)?;
    write(
        root.path(),
        "packages/malformed/tsconfig.json",
        r#"{"compilerOptions":{"strict":"yes"}}"#,
    )?;
    write_relative_pair(root.path(), "malformed")?;

    write_package(root.path(), "condition-app", "@acme/condition-app", None)?;
    write(
        root.path(),
        "packages/condition-app/main.ts",
        concat!(
            "import { selected } from '@acme/condition-lib';\n",
            "console.log(selected);\n",
        ),
    )?;
    write_package(
        root.path(),
        "condition-lib",
        "@acme/condition-lib",
        Some(r#"{"custom":"./custom.js","default":"./default.js"}"#),
    )?;
    module(
        root.path(),
        "packages/condition-lib/custom.ts",
        "selected",
        "customDead",
    )?;
    module(
        root.path(),
        "packages/condition-lib/default.ts",
        "selected",
        "defaultDead",
    )?;

    write_package(root.path(), "clean", "@acme/clean", None)?;
    write_relative_pair(root.path(), "clean")?;

    let audit = run(
        root.path(),
        &["audit", "--jobs", "1", "--resolution-profile", "bundler"],
    )?;
    assert_status(&audit, 0);
    let audit: Value = serde_json::from_str(&audit.stdout)?;
    assert_eq!(
        audit.get("status").and_then(Value::as_str),
        Some("incomplete")
    );
    assert_eq!(
        audit.get("limitationCount").and_then(Value::as_u64),
        Some(4)
    );
    let run_id = required_string(&audit, "runId")?;

    let overview = overview(root.path(), &run_id)?;
    assert_eq!(
        limitation_set(&overview)?,
        BTreeSet::from([
            limitation(
                "tsconfig-semantics-unsupported",
                "packages/unknown/tsconfig.json",
                "unknown compiler option madeUpFlag",
            ),
            limitation(
                "tsconfig-semantics-unsupported",
                "packages/unsupported/tsconfig.json",
                "unsupported resolution-affecting compiler option moduleSuffixes",
            ),
            limitation(
                "tsconfig-semantics-unsupported",
                "packages/malformed/tsconfig.json",
                "compiler option strict has the wrong shape",
            ),
            limitation(
                "public-surface-unsupported",
                "packages/condition-lib/package.json",
                "exports cannot mix subpath and condition keys or use unknown conditions",
            ),
        ])
    );
    let profiles = overview
        .get("resolutionProfiles")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("resolutionProfiles are missing"))?;
    assert!(!profiles.is_empty());
    assert!(profiles.iter().all(|profile| {
        profile.get("profile").and_then(Value::as_str) == Some("bundler")
            && profile.pointer("/source/kind").and_then(Value::as_str) == Some("invocation")
    }));
    assert_eq!(
        finding_set(root.path(), &run_id)?,
        BTreeSet::from([finding("packages/clean/target.ts", "dead")])
    );
    Ok(())
}

fn assert_tsconfig_probe_is_blocked() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"app","private":true}"#,
    )?;
    module(root.path(), "src/target.ts", "used", "dead")?;
    let writer = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-registry-tsconfig-writer",
            "--path",
            "src/target.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&writer, 0);

    write(
        root.path(),
        "tsconfig.json",
        concat!(
            r#"{"compilerOptions":{"madeUpFlag":true,"moduleSuffixes":[".native",""],"strict":"yes"}}"#,
            "\n",
        ),
    )?;
    write(
        root.path(),
        "src/main.ts",
        "import { used } from './target'; console.log(used);\n",
    )?;

    let rejected = assert_incomplete_prewrite_retry(
        root.path(),
        "op-registry-tsconfig-reader",
        "src/main.ts",
        &["--resolution-profile", "bundler"],
    )?;
    assert_probe_candidates_excluded(&rejected, 1)?;
    Ok(())
}

fn assert_unknown_condition_probe_is_blocked() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write_workspace_root(root.path())?;
    write_package(root.path(), "app", "@acme/app", None)?;
    write_package(root.path(), "lib", "@acme/lib", None)?;
    module(
        root.path(),
        "packages/lib/custom.ts",
        "selected",
        "customDead",
    )?;
    module(
        root.path(),
        "packages/lib/default.ts",
        "selected",
        "defaultDead",
    )?;
    let writer = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-registry-condition-writer",
            "--path",
            "packages/lib/custom.ts",
            "--path",
            "packages/lib/default.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&writer, 0);

    write_package(
        root.path(),
        "lib",
        "@acme/lib",
        Some(r#"{"custom":"./custom.js","default":"./default.js"}"#),
    )?;
    write(
        root.path(),
        "packages/app/main.ts",
        "import { selected } from '@acme/lib'; console.log(selected);\n",
    )?;

    let rejected = assert_incomplete_prewrite_retry(
        root.path(),
        "op-registry-condition-reader",
        "packages/app/main.ts",
        &["--resolution-profile", "bundler"],
    )?;
    assert_probe_candidates_excluded(&rejected, 2)?;
    Ok(())
}

fn overview(root: &Path, run_id: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let output = run(root, &["overview", "--run", run_id])?;
    assert_status(&output, 0);
    serde_json::from_str(&output.stdout).map_err(Into::into)
}

fn limitation_set(overview: &Value) -> Result<BTreeSet<LimitationView>, std::io::Error> {
    overview
        .get("limitations")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("limitations are missing"))?
        .iter()
        .map(|item| {
            Ok((
                required_string(item, "reason")?,
                required_string(item, "path")?,
                required_string(item, "detail")?,
            ))
        })
        .collect()
}

fn finding_set(
    root: &Path,
    run_id: &str,
) -> Result<BTreeSet<FindingView>, Box<dyn std::error::Error>> {
    let output = run(root, &["findings", "--run", run_id, "--area", "dead-code"])?;
    assert_status(&output, 0);
    let response: Value = serde_json::from_str(&output.stdout)?;
    response
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("finding items are missing"))?
        .iter()
        .map(|item| {
            Ok((
                item.pointer("/path/display")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| std::io::Error::other("finding path is missing"))?,
                required_string(item, "exportedName")?,
                required_string(item, "namespace")?,
            ))
        })
        .collect::<Result<_, std::io::Error>>()
        .map_err(Into::into)
}

fn required_string(value: &Value, field: &str) -> Result<String, std::io::Error> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other(format!("missing string field {field}")))
}

fn write_workspace_root(root: &Path) -> std::io::Result<()> {
    write(
        root,
        "package.json",
        r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
    )
}

fn write_package(
    root: &Path,
    directory: &str,
    name: &str,
    exports: Option<&str>,
) -> std::io::Result<()> {
    let exports = exports.map_or_else(String::new, |exports| format!(r#", "exports":{exports}"#));
    write(
        root,
        &format!("packages/{directory}/package.json"),
        &format!(r#"{{"name":"{name}","private":true{exports}}}"#),
    )
}

fn write_relative_pair(root: &Path, directory: &str) -> std::io::Result<()> {
    module(
        root,
        &format!("packages/{directory}/target.ts"),
        "used",
        "dead",
    )?;
    write(
        root,
        &format!("packages/{directory}/main.ts"),
        "import { used } from './target'; console.log(used);\n",
    )
}

fn module(root: &Path, relative: &str, used: &str, dead: &str) -> std::io::Result<()> {
    write(
        root,
        relative,
        &format!("export const {used} = 1; export const {dead} = 2;\n"),
    )
}

fn limitation(reason: &str, path: &str, detail: &str) -> LimitationView {
    (reason.to_owned(), path.to_owned(), detail.to_owned())
}

fn finding(path: &str, name: &str) -> FindingView {
    (path.to_owned(), name.to_owned(), "value".to_owned())
}

fn write(root: &Path, relative: &str, contents: &str) -> std::io::Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}
