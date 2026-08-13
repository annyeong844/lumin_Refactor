use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::{Value, json};

mod support;

use support::{assert_status, field, run};

type LimitationView = (String, String, String);

#[test]
fn resolution_profiles_follow_override_nearest_config_default_and_unsupported_rules()
-> Result<(), Box<dyn std::error::Error>> {
    verify_nearest_effective_configs_and_product_default()?;
    verify_unsupported_value_and_invocation_override()?;
    Ok(())
}

fn verify_nearest_effective_configs_and_product_default() -> Result<(), Box<dyn std::error::Error>>
{
    let root = mixed_profile_fixture()?;
    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    let summary: Value = serde_json::from_str(&audit.stdout)?;
    let run_id = field(&audit.stdout, "runId")?;
    let overview = run(root.path(), &["overview", "--run", &run_id])?;
    assert_status(&overview, 0);
    let overview: Value = serde_json::from_str(&overview.stdout)?;
    assert_eq!(
        summary.get("status").and_then(Value::as_str),
        Some("complete"),
        "unexpected mixed-profile audit summary: {summary:#?}\noverview: {overview:#?}",
    );
    assert_eq!(
        summary.get("limitationCount").and_then(Value::as_u64),
        Some(0)
    );
    for (path, profile, source_kind, config_path, specifier, target) in [
        (
            "apps/bundler/main.ts",
            "bundler",
            "config",
            Some("apps/bundler/tsconfig.json"),
            "@scope/dep/bundler",
            "packages/dep/bundler-import.ts",
        ),
        (
            "apps/node/main.ts",
            "node",
            "config",
            Some("apps/node/tsconfig.json"),
            "@scope/dep/node",
            "packages/dep/node.ts",
        ),
        (
            "apps/node16/main.ts",
            "node16",
            "config",
            Some("apps/node16/tsconfig.json"),
            "@scope/dep/node16",
            "packages/dep/node16-import.ts",
        ),
        (
            "apps/nodenext/main.ts",
            "node-next",
            "config",
            Some("apps/nodenext/tsconfig.json"),
            "@scope/dep/nodenext",
            "packages/dep/nodenext-import.ts",
        ),
        (
            "src/default.ts",
            "bundler",
            "product-default",
            None,
            "@scope/dep/default",
            "packages/dep/default-import.ts",
        ),
    ] {
        assert_profile_and_target(
            root.path(),
            &run_id,
            path,
            profile,
            source_kind,
            config_path,
            specifier,
            target,
        )?;
    }
    Ok(())
}

fn verify_unsupported_value_and_invocation_override() -> Result<(), Box<dyn std::error::Error>> {
    let root = unsupported_profile_fixture()?;
    let incomplete = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&incomplete, 0);
    let summary: Value = serde_json::from_str(&incomplete.stdout)?;
    assert_eq!(
        summary.get("status").and_then(Value::as_str),
        Some("incomplete")
    );
    assert_eq!(
        summary.get("limitationCount").and_then(Value::as_u64),
        Some(1)
    );
    let incomplete_run = field(&incomplete.stdout, "runId")?;

    let overview = run(root.path(), &["overview", "--run", &incomplete_run])?;
    assert_status(&overview, 0);
    let overview: Value = serde_json::from_str(&overview.stdout)?;
    assert_eq!(
        limitation_set(&overview)?,
        BTreeSet::from([limitation(
            "tsconfig-semantics-unsupported",
            "apps/override/tsconfig.json",
            "unsupported moduleResolution value classic",
        )])
    );
    for (path, specifier) in [
        ("apps/override/main.ts", "@scope/dep/physical"),
        ("apps/override/App.vue", "@scope/dep/embedded"),
    ] {
        let source = file_response(root.path(), &incomplete_run, path)?;
        assert!(
            source.get("resolutionProfile").is_none()
                || source.get("resolutionProfile") == Some(&Value::Null),
            "unsupported profile must not masquerade as product-default: {source:#?}",
        );
        let resolution = resolution(&source, specifier)?;
        assert_eq!(
            resolution.pointer("/outcome/kind").and_then(Value::as_str),
            Some("unsupported")
        );
        assert_eq!(
            resolution
                .pointer("/outcome/reason")
                .and_then(Value::as_str),
            Some("the importer's semantic configuration is incomplete")
        );
        assert!(resolution.pointer("/outcome/target").is_none());
    }

    let overridden = run(
        root.path(),
        &["audit", "--jobs", "1", "--resolution-profile", "bundler"],
    )?;
    assert_status(&overridden, 0);
    let summary: Value = serde_json::from_str(&overridden.stdout)?;
    assert_eq!(
        summary.get("status").and_then(Value::as_str),
        Some("complete")
    );
    assert_eq!(
        summary.get("limitationCount").and_then(Value::as_u64),
        Some(0)
    );
    let overridden_run = field(&overridden.stdout, "runId")?;
    for (path, specifier, target) in [
        (
            "apps/override/main.ts",
            "@scope/dep/physical",
            "packages/dep/physical-import.ts",
        ),
        (
            "apps/override/App.vue",
            "@scope/dep/embedded",
            "packages/dep/embedded-import.ts",
        ),
    ] {
        assert_profile_and_target(
            root.path(),
            &overridden_run,
            path,
            "bundler",
            "invocation",
            None,
            specifier,
            target,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn assert_profile_and_target(
    root: &Path,
    run_id: &str,
    path: &str,
    profile: &str,
    source_kind: &str,
    config_path: Option<&str>,
    specifier: &str,
    target: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let source = file_response(root, run_id, path)?;
    assert_eq!(
        source
            .pointer("/resolutionProfile/profile")
            .and_then(Value::as_str),
        Some(profile),
        "wrong profile for {path}",
    );
    assert_eq!(
        source
            .pointer("/resolutionProfile/source/kind")
            .and_then(Value::as_str),
        Some(source_kind),
        "wrong profile source for {path}",
    );
    assert_eq!(
        source
            .pointer("/resolutionProfile/source/path_display")
            .and_then(Value::as_str),
        config_path,
        "wrong controlling config for {path}",
    );
    let resolution = resolution(&source, specifier)?;
    assert_eq!(
        resolution.pointer("/outcome/kind").and_then(Value::as_str),
        Some("internal")
    );
    assert_eq!(
        required_str(resolution, "/outcome/target")?,
        source_id(root, run_id, target)?,
        "wrong target for {path}",
    );
    Ok(())
}

fn mixed_profile_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write_root_manifest(root.path())?;
    write(
        root.path(),
        "configs/classic-base.json",
        r#"{"compilerOptions":{"moduleResolution":"classic"}}"#,
    )?;
    for (directory, config) in [
        (
            "bundler",
            r#"{"compilerOptions":{"moduleResolution":"bundler","module":"esnext"}}"#,
        ),
        (
            "node",
            r#"{"compilerOptions":{"moduleResolution":"node10","module":"commonjs"}}"#,
        ),
        (
            "node16",
            r#"{"extends":"../../configs/classic-base.json","compilerOptions":{"moduleResolution":"node16","module":"node16"}}"#,
        ),
        (
            "nodenext",
            r#"{"compilerOptions":{"moduleResolution":"nodenext","module":"nodenext"}}"#,
        ),
    ] {
        write(
            root.path(),
            &format!("apps/{directory}/tsconfig.json"),
            config,
        )?;
        write(
            root.path(),
            &format!("apps/{directory}/main.ts"),
            &format!(
                "import {{ selected }} from '@scope/dep/{directory}'; console.log(selected);\n"
            ),
        )?;
    }
    write(
        root.path(),
        "src/default.ts",
        "import { selected } from '@scope/dep/default'; console.log(selected);\n",
    )?;
    write_condition_package(
        root.path(),
        &["bundler", "node", "node16", "nodenext", "default"],
    )?;
    write(
        root.path(),
        "packages/dep/node.ts",
        "export const selected = 1;\n",
    )?;
    Ok(root)
}

fn unsupported_profile_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write_root_manifest(root.path())?;
    write(
        root.path(),
        "configs/bundler-base.json",
        r#"{"compilerOptions":{"moduleResolution":"bundler","module":"esnext"}}"#,
    )?;
    write(
        root.path(),
        "apps/override/tsconfig.json",
        r#"{"extends":"../../configs/bundler-base.json","compilerOptions":{"moduleResolution":"classic"}}"#,
    )?;
    write(
        root.path(),
        "apps/override/main.ts",
        "import { selected } from '@scope/dep/physical'; console.log(selected);\n",
    )?;
    write(
        root.path(),
        "apps/override/App.vue",
        concat!(
            "<script lang=\"ts\">\n",
            "import { selected } from '@scope/dep/embedded'; console.log(selected);\n",
            "</script>\n<template><div /></template>\n",
        ),
    )?;
    write_condition_package(root.path(), &["physical", "embedded"])?;
    Ok(root)
}

fn write_root_manifest(root: &Path) -> std::io::Result<()> {
    write(
        root,
        "package.json",
        r#"{"name":"root-app","private":true,"type":"module","workspaces":["packages/*"]}"#,
    )
}

fn write_condition_package(root: &Path, subpaths: &[&str]) -> std::io::Result<()> {
    let exports = subpaths
        .iter()
        .map(|subpath| {
            (
                format!("./{subpath}"),
                json!({
                    "import": format!("./{subpath}-import.js"),
                    "require": format!("./{subpath}-require.js"),
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    write(
        root,
        "packages/dep/package.json",
        &json!({
            "name": "@scope/dep",
            "private": true,
            "type": "module",
            "exports": exports,
        })
        .to_string(),
    )?;
    for subpath in subpaths {
        for condition in ["import", "require"] {
            write(
                root,
                &format!("packages/dep/{subpath}-{condition}.ts"),
                "export const selected = 1;\n",
            )?;
        }
    }
    Ok(())
}

fn file_response(
    root: &Path,
    run_id: &str,
    path: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let output = run(root, &["files", "--run", run_id, path])?;
    assert_status(&output, 0);
    serde_json::from_str(&output.stdout).map_err(Into::into)
}

fn source_id(root: &Path, run_id: &str, path: &str) -> Result<String, Box<dyn std::error::Error>> {
    required_str(
        &file_response(root, run_id, path)?,
        "/sourceContext/sourceId",
    )
    .map_err(Into::into)
}

fn resolution<'a>(source: &'a Value, specifier: &str) -> Result<&'a Value, std::io::Error> {
    source
        .get("resolutions")
        .and_then(Value::as_array)
        .and_then(|resolutions| {
            resolutions.iter().find(|resolution| {
                resolution
                    .pointer("/sourceUse/specifier")
                    .and_then(Value::as_str)
                    == Some(specifier)
            })
        })
        .ok_or_else(|| std::io::Error::other(format!("resolution for {specifier} is missing")))
}

fn limitation_set(overview: &Value) -> Result<BTreeSet<LimitationView>, std::io::Error> {
    overview
        .get("limitations")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("limitations are missing"))?
        .iter()
        .map(|item| {
            Ok((
                required_str(item, "/reason")?,
                required_str(item, "/path")?,
                required_str(item, "/detail")?,
            ))
        })
        .collect()
}

fn required_str(value: &Value, pointer: &str) -> Result<String, std::io::Error> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other(format!("missing string {pointer}")))
}

fn limitation(reason: &str, path: &str, detail: &str) -> LimitationView {
    (reason.to_owned(), path.to_owned(), detail.to_owned())
}

fn write(root: &Path, relative: &str, contents: &str) -> std::io::Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}
