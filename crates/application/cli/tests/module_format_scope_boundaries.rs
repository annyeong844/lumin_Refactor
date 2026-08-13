use std::fs;
use std::path::Path;

use serde_json::{Value, json};

mod support;

use support::{assert_status, field, run};

const COMMONJS_EXPORT_LOWERING_UNSUPPORTED: &str =
    "CommonJS export lowering is not implemented in the first audit increment";
const REQUIRE_ATTRIBUTION_OPAQUE: &str = "shadowed, mutated, dynamically resolved, or escaped require makes CommonJS module-use attribution opaque";
const CONTROL_FLOW_SOURCE: &str = concat!(
    "if (enabled) {\n",
    "  require('@scope/dep/before');\n",
    "  require = customLoader;\n",
    "  require('@scope/dep/after');\n",
    "}\n",
);
const ARGUMENTS_SOURCE: &str = concat!(
    "arguments[1] = customLoader;\n",
    "require('@scope/dep/arguments-after');\n",
);
const VAR_ARGUMENTS_SOURCE: &str = concat!(
    "var arguments;\n",
    "arguments[1] = customLoader;\n",
    "require('@scope/dep/var-arguments-after');\n",
);
const STRICT_ARGUMENTS_SOURCE: &str = concat!(
    "'use strict';\n",
    "arguments[1] = customLoader;\n",
    "require('@scope/dep/strict-arguments');\n",
);
const SHADOWED_ARGUMENTS_SOURCE: &str = concat!(
    "const arguments = [];\n",
    "arguments[1] = customLoader;\n",
    "require('@scope/dep/shadowed-arguments');\n",
);
const COMPUTED_MODULE_SOURCE: &str = concat!(
    "const key = 'exports';\n",
    "module[key] = { publicValue: 1 };\n",
);
const ESCAPED_ARGUMENTS_SOURCE: &str = concat!(
    "require('@scope/dep/escape-before');\n",
    "function replace(value) { value[1] = customLoader; }\n",
    "replace(arguments, require('@scope/dep/escape-during'));\n",
    "require('@scope/dep/escape-after');\n",
);
const UNARY_ARGUMENTS_SOURCE: &str = concat!(
    "require('@scope/dep/unary-before');\n",
    "arguments.valueOf = function () { this[1] = customLoader; return 0; };\n",
    "+arguments;\n",
    "require('@scope/dep/unary-after');\n",
);
const NONCOERCIVE_UNARY_ARGUMENTS_SOURCE: &str = concat!(
    "const inspected = !arguments;\n",
    "consume(typeof arguments);\n",
    "const ignored = void arguments;\n",
    "require('@scope/dep/unary-grounded');\n",
);
const BINARY_ARGUMENTS_SOURCE: &str = concat!(
    "require('@scope/dep/binary-before');\n",
    "arguments.valueOf = function () { this[1] = customLoader; return 0; };\n",
    "(condition ? arguments : 0) + 1;\n",
    "require('@scope/dep/binary-after');\n",
);
const NONCOERCIVE_BINARY_ARGUMENTS_SOURCE: &str = concat!(
    "consume(arguments === candidate);\n",
    "consume(arguments == null);\n",
    "consume('1' in arguments);\n",
    "try { arguments instanceof null; } catch {}\n",
    "try { arguments instanceof (1 + 2); } catch {}\n",
    "require('@scope/dep/binary-grounded');\n",
);
const UPDATE_ARGUMENTS_SOURCE: &str = concat!(
    "arguments.valueOf = function () { this[1] = customLoader; return 0; };\n",
    "try { arguments instanceof arguments++; } catch {}\n",
    "require('@scope/dep/update-after');\n",
);
const WRAPPER_THIS_SOURCE: &str = "this.publicValue = 1;\n";

#[test]
fn commonjs_wrapper_mutations_preserve_only_grounded_public_edges()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let audit = run(
        root.path(),
        &["audit", "--jobs", "1", "--resolution-profile", "node16"],
    )?;
    assert_status(&audit, 0);
    assert_eq!(field(&audit.stdout, "status")?, "incomplete");
    let run_id = field(&audit.stdout, "runId")?;

    let control_flow = file_response(root.path(), &run_id, "src/control-flow.cjs")?;
    let control_flow_resolutions = resolutions(&control_flow);
    assert_eq!(control_flow_resolutions.len(), 1);
    let resolution = &control_flow_resolutions[0];
    assert_eq!(
        resolution
            .pointer("/sourceUse/specifier")
            .and_then(Value::as_str),
        Some("@scope/dep/before")
    );
    assert_eq!(
        resolution.pointer("/outcome/kind").and_then(Value::as_str),
        Some("internal")
    );
    assert_eq!(
        resolution
            .pointer("/outcome/target")
            .and_then(Value::as_str),
        source_id(root.path(), &run_id, "packages/dep/before-require.ts")?.as_deref()
    );

    let arguments = file_response(root.path(), &run_id, "src/arguments.cjs")?;
    assert!(resolutions(&arguments).is_empty());
    let var_arguments = file_response(root.path(), &run_id, "src/var-arguments.cjs")?;
    assert!(resolutions(&var_arguments).is_empty());
    let escaped_arguments = file_response(root.path(), &run_id, "src/escaped-arguments.cjs")?;
    let escaped_specifiers = resolutions(&escaped_arguments)
        .iter()
        .filter_map(|resolution| {
            resolution
                .pointer("/sourceUse/specifier")
                .and_then(Value::as_str)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        escaped_specifiers,
        ["@scope/dep/escape-before", "@scope/dep/escape-during"]
    );
    for (specifier, target) in [
        (
            "@scope/dep/escape-before",
            "packages/dep/escape-before-require.ts",
        ),
        (
            "@scope/dep/escape-during",
            "packages/dep/escape-during-require.ts",
        ),
    ] {
        assert_resolution_target(root.path(), &run_id, &escaped_arguments, specifier, target)?;
    }
    let unary_arguments = file_response(root.path(), &run_id, "src/unary-arguments.cjs")?;
    let unary_specifiers = resolutions(&unary_arguments)
        .iter()
        .filter_map(|resolution| {
            resolution
                .pointer("/sourceUse/specifier")
                .and_then(Value::as_str)
        })
        .collect::<Vec<_>>();
    assert_eq!(unary_specifiers, ["@scope/dep/unary-before"]);
    assert_resolution_target(
        root.path(),
        &run_id,
        &unary_arguments,
        "@scope/dep/unary-before",
        "packages/dep/unary-before-require.ts",
    )?;
    let binary_arguments = file_response(root.path(), &run_id, "src/binary-arguments.cjs")?;
    let binary_specifiers = resolutions(&binary_arguments)
        .iter()
        .filter_map(|resolution| {
            resolution
                .pointer("/sourceUse/specifier")
                .and_then(Value::as_str)
        })
        .collect::<Vec<_>>();
    assert_eq!(binary_specifiers, ["@scope/dep/binary-before"]);
    assert_resolution_target(
        root.path(),
        &run_id,
        &binary_arguments,
        "@scope/dep/binary-before",
        "packages/dep/binary-before-require.ts",
    )?;
    let update_arguments = file_response(root.path(), &run_id, "src/update-arguments.cjs")?;
    assert!(resolutions(&update_arguments).is_empty());
    for (path, specifier, target) in [
        (
            "src/strict-arguments.cjs",
            "@scope/dep/strict-arguments",
            "packages/dep/strict-arguments-require.ts",
        ),
        (
            "src/shadowed-arguments.cjs",
            "@scope/dep/shadowed-arguments",
            "packages/dep/shadowed-arguments-require.ts",
        ),
        (
            "src/noncoercive-unary-arguments.cjs",
            "@scope/dep/unary-grounded",
            "packages/dep/unary-grounded-require.ts",
        ),
        (
            "src/noncoercive-binary-arguments.cjs",
            "@scope/dep/binary-grounded",
            "packages/dep/binary-grounded-require.ts",
        ),
    ] {
        let file = file_response(root.path(), &run_id, path)?;
        let file_resolutions = resolutions(&file);
        assert_eq!(
            file_resolutions.len(),
            1,
            "unexpected resolutions for {path}"
        );
        assert_eq!(
            file_resolutions[0]
                .pointer("/sourceUse/specifier")
                .and_then(Value::as_str),
            Some(specifier)
        );
        assert_eq!(
            file_resolutions[0]
                .pointer("/outcome/target")
                .and_then(Value::as_str),
            source_id(root.path(), &run_id, target)?.as_deref()
        );
    }

    let overview = run(root.path(), &["overview", "--run", &run_id])?;
    assert_status(&overview, 0);
    let overview: Value = serde_json::from_str(&overview.stdout)?;
    let limitations = overview
        .get("limitations")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("limitations are missing"))?;
    let mut details = limitations
        .iter()
        .map(|limitation| {
            limitation
                .get("detail")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("limitation detail is missing"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    details.sort_unstable();
    let mut expected = vec![COMMONJS_EXPORT_LOWERING_UNSUPPORTED; 13];
    expected.extend([REQUIRE_ATTRIBUTION_OPAQUE; 7]);
    expected.sort_unstable();
    assert_eq!(details, expected);
    Ok(())
}

#[test]
fn effective_profiles_drive_physical_and_embedded_commonjs_extraction()
-> Result<(), Box<dyn std::error::Error>> {
    let root = profile_fixture()?;
    let configured = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&configured, 0);
    let configured_run = field(&configured.stdout, "runId")?;
    for (path, specifier, target) in [
        (
            "apps/configured/main.ts",
            "@scope/dep/configured",
            "packages/dep/configured-require.ts",
        ),
        (
            "apps/configured/App.vue",
            "@scope/dep/embedded",
            "packages/dep/embedded-require.ts",
        ),
    ] {
        let source = file_response(root.path(), &configured_run, path)?;
        assert_eq!(
            source
                .pointer("/resolutionProfile/profile")
                .and_then(Value::as_str),
            Some("node16")
        );
        assert_eq!(
            source
                .pointer("/resolutionProfile/source/kind")
                .and_then(Value::as_str),
            Some("config")
        );
        assert_resolution_target(root.path(), &configured_run, &source, specifier, target)?;
    }

    let legacy = run(
        root.path(),
        &["audit", "--jobs", "1", "--resolution-profile", "node"],
    )?;
    assert_status(&legacy, 0);
    let legacy_run = field(&legacy.stdout, "runId")?;
    let legacy_source = file_response(root.path(), &legacy_run, "src/legacy.ts")?;
    assert_eq!(
        legacy_source
            .pointer("/resolutionProfile/profile")
            .and_then(Value::as_str),
        Some("node")
    );
    assert_resolution_target(
        root.path(),
        &legacy_run,
        &legacy_source,
        "@scope/dep/legacy",
        "packages/dep/legacy.ts",
    )
}

fn fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"root-app","private":true,"workspaces":["packages/*"]}"#,
    )?;
    write(root.path(), "src/control-flow.cjs", CONTROL_FLOW_SOURCE)?;
    write(root.path(), "src/arguments.cjs", ARGUMENTS_SOURCE)?;
    write(root.path(), "src/var-arguments.cjs", VAR_ARGUMENTS_SOURCE)?;
    write(
        root.path(),
        "src/strict-arguments.cjs",
        STRICT_ARGUMENTS_SOURCE,
    )?;
    write(
        root.path(),
        "src/shadowed-arguments.cjs",
        SHADOWED_ARGUMENTS_SOURCE,
    )?;
    write(
        root.path(),
        "src/computed-module.ts",
        COMPUTED_MODULE_SOURCE,
    )?;
    write(
        root.path(),
        "src/escaped-arguments.cjs",
        ESCAPED_ARGUMENTS_SOURCE,
    )?;
    write(
        root.path(),
        "src/unary-arguments.cjs",
        UNARY_ARGUMENTS_SOURCE,
    )?;
    write(
        root.path(),
        "src/noncoercive-unary-arguments.cjs",
        NONCOERCIVE_UNARY_ARGUMENTS_SOURCE,
    )?;
    write(
        root.path(),
        "src/binary-arguments.cjs",
        BINARY_ARGUMENTS_SOURCE,
    )?;
    write(
        root.path(),
        "src/noncoercive-binary-arguments.cjs",
        NONCOERCIVE_BINARY_ARGUMENTS_SOURCE,
    )?;
    write(
        root.path(),
        "src/update-arguments.cjs",
        UPDATE_ARGUMENTS_SOURCE,
    )?;
    write(root.path(), "src/wrapper-this.ts", WRAPPER_THIS_SOURCE)?;
    write(
        root.path(),
        "packages/dep/package.json",
        &json!({
            "name": "@scope/dep",
            "private": true,
            "exports": {
                "./before": {
                    "import": "./before-import.js",
                    "require": "./before-require.js",
                },
                "./after": {
                    "import": "./after-import.js",
                    "require": "./after-require.js",
                },
                "./arguments-after": {
                    "import": "./arguments-after-import.js",
                    "require": "./arguments-after-require.js",
                },
                "./var-arguments-after": {
                    "import": "./var-arguments-after-import.js",
                    "require": "./var-arguments-after-require.js",
                },
                "./strict-arguments": {
                    "import": "./strict-arguments-import.js",
                    "require": "./strict-arguments-require.js",
                },
                "./shadowed-arguments": {
                    "import": "./shadowed-arguments-import.js",
                    "require": "./shadowed-arguments-require.js",
                },
                "./escape-before": {
                    "import": "./escape-before-import.js",
                    "require": "./escape-before-require.js",
                },
                "./escape-during": {
                    "import": "./escape-during-import.js",
                    "require": "./escape-during-require.js",
                },
                "./escape-after": {
                    "import": "./escape-after-import.js",
                    "require": "./escape-after-require.js",
                },
                "./unary-before": {
                    "import": "./unary-before-import.js",
                    "require": "./unary-before-require.js",
                },
                "./unary-after": {
                    "import": "./unary-after-import.js",
                    "require": "./unary-after-require.js",
                },
                "./unary-grounded": {
                    "import": "./unary-grounded-import.js",
                    "require": "./unary-grounded-require.js",
                },
                "./binary-before": {
                    "import": "./binary-before-import.js",
                    "require": "./binary-before-require.js",
                },
                "./binary-after": {
                    "import": "./binary-after-import.js",
                    "require": "./binary-after-require.js",
                },
                "./binary-grounded": {
                    "import": "./binary-grounded-import.js",
                    "require": "./binary-grounded-require.js",
                },
                "./update-after": {
                    "import": "./update-after-import.js",
                    "require": "./update-after-require.js",
                },
            },
        })
        .to_string(),
    )?;
    for path in [
        "before-import",
        "before-require",
        "after-import",
        "after-require",
        "arguments-after-import",
        "arguments-after-require",
        "var-arguments-after-import",
        "var-arguments-after-require",
        "strict-arguments-import",
        "strict-arguments-require",
        "shadowed-arguments-import",
        "shadowed-arguments-require",
        "escape-before-import",
        "escape-before-require",
        "escape-during-import",
        "escape-during-require",
        "escape-after-import",
        "escape-after-require",
        "unary-before-import",
        "unary-before-require",
        "unary-after-import",
        "unary-after-require",
        "unary-grounded-import",
        "unary-grounded-require",
        "binary-before-import",
        "binary-before-require",
        "binary-after-import",
        "binary-after-require",
        "binary-grounded-import",
        "binary-grounded-require",
        "update-after-import",
        "update-after-require",
    ] {
        write(
            root.path(),
            &format!("packages/dep/{path}.ts"),
            &format!("export const {} = 1;\n", path.replace('-', "_")),
        )?;
    }
    Ok(root)
}

fn profile_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"root-app","private":true,"workspaces":["apps/*","packages/*"]}"#,
    )?;
    write(
        root.path(),
        "apps/configured/tsconfig.json",
        r#"{"extends":"../../configs/node16.json"}"#,
    )?;
    write(
        root.path(),
        "configs/node16.json",
        r#"{"compilerOptions":{"moduleResolution":"node16","module":"node16"}}"#,
    )?;
    write(
        root.path(),
        "apps/configured/package.json",
        r#"{"name":"configured","private":true,"type":"commonjs"}"#,
    )?;
    write(
        root.path(),
        "apps/configured/main.ts",
        "var require; const configured = require('@scope/dep/configured');\n",
    )?;
    write(
        root.path(),
        "apps/configured/App.vue",
        concat!(
            "<script lang=\"ts\">\n",
            "var require; const embedded = require('@scope/dep/embedded');\n",
            "</script>\n<template><div /></template>\n",
        ),
    )?;
    write(
        root.path(),
        "src/legacy.ts",
        "var require; const legacy = require('@scope/dep/legacy');\n",
    )?;
    write(
        root.path(),
        "packages/dep/package.json",
        &json!({
            "name": "@scope/dep",
            "private": true,
            "exports": {
                "./configured": {
                    "import": "./configured-import.js",
                    "require": "./configured-require.js",
                },
                "./embedded": {
                    "import": "./embedded-import.js",
                    "require": "./embedded-require.js",
                },
            },
        })
        .to_string(),
    )?;
    for target in [
        "configured-import",
        "configured-require",
        "embedded-import",
        "embedded-require",
    ] {
        write(
            root.path(),
            &format!("packages/dep/{target}.ts"),
            &format!("export const {} = 1;\n", target.replace('-', "_")),
        )?;
    }
    write(
        root.path(),
        "packages/dep/legacy.ts",
        "export const legacy = 1;\n",
    )?;
    Ok(root)
}

fn file_response(
    root: &Path,
    run_id: &str,
    path: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let output = run(root, &["files", "--run", run_id, path])?;
    assert_status(&output, 0);
    Ok(serde_json::from_str(&output.stdout)?)
}

fn resolutions(file: &Value) -> &[Value] {
    file.get("resolutions")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn source_id(
    root: &Path,
    run_id: &str,
    path: &str,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    Ok(file_response(root, run_id, path)?
        .pointer("/sourceContext/sourceId")
        .and_then(Value::as_str)
        .map(str::to_owned))
}

fn assert_resolution_target(
    root: &Path,
    run_id: &str,
    source: &Value,
    specifier: &str,
    target: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let resolution = resolutions(source)
        .iter()
        .find(|resolution| {
            resolution
                .pointer("/sourceUse/specifier")
                .and_then(Value::as_str)
                == Some(specifier)
        })
        .ok_or_else(|| std::io::Error::other(format!("resolution is missing: {specifier}")))?;
    assert_eq!(
        resolution
            .pointer("/outcome/target")
            .and_then(Value::as_str),
        source_id(root, run_id, target)?.as_deref(),
        "wrong resolution target for {specifier}: {resolution}",
    );
    Ok(())
}

fn write(root: &Path, path: &str, contents: &str) -> Result<(), std::io::Error> {
    let path = root.join(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}
