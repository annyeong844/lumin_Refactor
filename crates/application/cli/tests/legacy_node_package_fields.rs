use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

#[test]
fn legacy_node_ignores_valid_and_malformed_fields_and_uses_main_and_typings()
-> Result<(), Box<dyn std::error::Error>> {
    for (case, exports, imports) in [
        (
            "valid",
            r#"{"default":"./wrong-exports.js"}"#,
            r##"{"#internal":"./wrong-local.js"}"##,
        ),
        ("malformed", "7", "7"),
    ] {
        let root = package_fixture(exports, imports)?;
        let run_id = audit(root.path(), "node", "complete")?;
        let overview = overview(root.path(), &run_id)?;
        assert_eq!(
            overview.get("limitations"),
            Some(&serde_json::json!([])),
            "legacy node consulted {case} exports or imports"
        );

        let source = file_response(root.path(), &run_id, "src/main.ts")?;
        assert_eq!(
            source
                .get("resolutions")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(3),
            "unexpected resolution count for {case} fields"
        );
        assert_internal_target(
            &source,
            "@acme/lib",
            "value",
            &source_id(root.path(), &run_id, "packages/lib/main.ts")?,
        )?;
        assert_internal_target(
            &source,
            "@acme/lib",
            "type",
            &source_id(root.path(), &run_id, "packages/lib/preferred.d.ts")?,
        )?;
        let internal = resolution(&source, "#internal", "value")?;
        assert_eq!(
            internal.pointer("/outcome/kind").and_then(Value::as_str),
            Some("external")
        );
        assert_eq!(
            internal.pointer("/outcome/package").and_then(Value::as_str),
            Some("#internal")
        );
    }
    Ok(())
}

#[test]
fn enabled_profile_retains_field_applicability_after_legacy_run()
-> Result<(), Box<dyn std::error::Error>> {
    let root = package_fixture("7", "7")?;

    let legacy_run = audit(root.path(), "node", "complete")?;
    assert_eq!(
        overview(root.path(), &legacy_run)?.get("limitations"),
        Some(&serde_json::json!([]))
    );

    let bundler_run = audit(root.path(), "bundler", "incomplete")?;
    let bundler_overview = overview(root.path(), &bundler_run)?;
    let limitations = bundler_overview
        .get("limitations")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("overview limitations are missing"))?;
    let observed = limitations
        .iter()
        .map(|limitation| {
            Ok((
                required_str(limitation, "/reason")?,
                required_str(limitation, "/path")?,
            ))
        })
        .collect::<Result<BTreeSet<_>, std::io::Error>>()?;
    assert_eq!(
        observed,
        BTreeSet::from([
            (
                "package-imports-unsupported".to_owned(),
                "package.json".to_owned(),
            ),
            (
                "public-surface-unsupported".to_owned(),
                "packages/lib/package.json".to_owned(),
            ),
        ])
    );

    let source = file_response(root.path(), &bundler_run, "src/main.ts")?;
    for namespace in ["value", "type"] {
        let package = resolution(&source, "@acme/lib", namespace)?;
        assert_eq!(
            package.pointer("/outcome/kind").and_then(Value::as_str),
            Some("unsupported")
        );
        assert_eq!(
            package.pointer("/outcome/reason").and_then(Value::as_str),
            Some("package exports must match exports-v1")
        );
    }
    let internal = resolution(&source, "#internal", "value")?;
    assert_eq!(
        internal.pointer("/outcome/kind").and_then(Value::as_str),
        Some("unsupported")
    );
    assert_eq!(
        internal.pointer("/outcome/reason").and_then(Value::as_str),
        Some("package imports are unsupported")
    );
    Ok(())
}

fn package_fixture(
    exports: &str,
    imports: &str,
) -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        &format!(
            r#"{{"name":"app","private":true,"workspaces":["packages/*"],"imports":{imports}}}"#
        ),
    )?;
    write(
        root.path(),
        "packages/lib/package.json",
        &format!(
            concat!(
                r#"{{"name":"@acme/lib","private":true,"exports":{},"#,
                r#""module":"./wrong-module.js","main":"./main.js","#,
                r#""typings":"./preferred.d.ts","types":"./wrong-types.d.ts"}}"#,
            ),
            exports,
        ),
    )?;
    write(
        root.path(),
        "packages/lib/main.ts",
        "export const usedValue = 1; export const mainDead = 2;\n",
    )?;
    write(
        root.path(),
        "packages/lib/preferred.d.ts",
        "export type UsedType = string; export type PreferredDead = number;\n",
    )?;
    for path in [
        "packages/lib/wrong-exports.ts",
        "packages/lib/wrong-module.ts",
        "packages/lib/wrong-types.d.ts",
        "wrong-local.ts",
    ] {
        write(root.path(), path, "export const wrong = 1;\n")?;
    }
    write(
        root.path(),
        "src/main.ts",
        concat!(
            "import { usedValue, type UsedType } from '@acme/lib';\n",
            "import { local } from '#internal';\n",
            "const typed: UsedType = String(usedValue); console.log(typed, local);\n",
        ),
    )?;
    Ok(root)
}

fn audit(
    root: &Path,
    profile: &str,
    expected_status: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let output = run(
        root,
        &["audit", "--jobs", "1", "--resolution-profile", profile],
    )?;
    assert_status(&output, 0);
    assert_eq!(field(&output.stdout, "status")?, expected_status);
    let response: Value = serde_json::from_str(&output.stdout)?;
    let limitation_count = response
        .get("limitationCount")
        .and_then(Value::as_u64)
        .ok_or_else(|| std::io::Error::other("audit limitationCount is missing"))?;
    if expected_status == "complete" {
        assert_eq!(limitation_count, 0);
    } else {
        assert!(limitation_count > 0);
    }
    field(&output.stdout, "runId")
}

fn overview(root: &Path, run_id: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let output = run(root, &["overview", "--run", run_id])?;
    assert_status(&output, 0);
    Ok(serde_json::from_str(&output.stdout)?)
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

fn source_id(root: &Path, run_id: &str, path: &str) -> Result<String, Box<dyn std::error::Error>> {
    required_str(
        &file_response(root, run_id, path)?,
        "/sourceContext/sourceId",
    )
    .map_err(Into::into)
}

fn assert_internal_target(
    source: &Value,
    specifier: &str,
    namespace: &str,
    expected_target: &str,
) -> Result<(), std::io::Error> {
    let resolution = resolution(source, specifier, namespace)?;
    assert_eq!(
        resolution.pointer("/outcome/kind").and_then(Value::as_str),
        Some("internal")
    );
    assert_eq!(
        required_str(resolution, "/outcome/target")?,
        expected_target
    );
    Ok(())
}

fn resolution<'a>(
    source: &'a Value,
    specifier: &str,
    namespace: &str,
) -> Result<&'a Value, std::io::Error> {
    source
        .get("resolutions")
        .and_then(Value::as_array)
        .and_then(|resolutions| {
            resolutions.iter().find(|resolution| {
                resolution
                    .pointer("/sourceUse/specifier")
                    .and_then(Value::as_str)
                    == Some(specifier)
                    && resolution
                        .pointer("/sourceUse/namespace")
                        .and_then(Value::as_str)
                        == Some(namespace)
            })
        })
        .ok_or_else(|| {
            std::io::Error::other(format!(
                "resolution for {specifier} in {namespace} namespace is missing"
            ))
        })
}

fn required_str(value: &Value, pointer: &str) -> Result<String, std::io::Error> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other(format!("missing string {pointer}")))
}

fn write(root: &Path, relative: &str, contents: &str) -> std::io::Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}
