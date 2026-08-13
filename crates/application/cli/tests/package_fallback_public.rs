use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

type FindingView = (String, String, String);

#[test]
fn package_fields_without_exports_select_role_scoped_public_targets()
-> Result<(), Box<dyn std::error::Error>> {
    let root = package_fixture()?;
    let expected_findings = BTreeSet::from([
        view("packages/lib/main.ts", "MainDeadType", "type"),
        view("packages/lib/module.ts", "ModuleDeadType", "type"),
        view("packages/lib/preferred.ts", "preferredDeadValue", "value"),
        view(
            "packages/types-only/declarations.ts",
            "typesOnlyDeadValue",
            "value",
        ),
        view(
            "packages/lib/shadowed-types.ts",
            "ShadowedTypesDeadType",
            "type",
        ),
        view(
            "packages/lib/shadowed-types.ts",
            "shadowedTypesDeadValue",
            "value",
        ),
    ]);

    for (profile, expected_value_target) in [
        ("bundler", "packages/lib/module.ts"),
        ("node", "packages/lib/main.ts"),
        ("node16", "packages/lib/main.ts"),
        ("nodenext", "packages/lib/main.ts"),
    ] {
        let run_id = audit(root.path(), profile, expected_findings.len() as u64)?;
        assert_package_targets(root.path(), &run_id, expected_value_target)?;
        assert_eq!(
            finding_views(root.path(), &run_id, expected_findings.len() as u64)?,
            expected_findings,
            "wrong public identity set for {profile}",
        );
    }
    Ok(())
}

fn audit(
    root: &Path,
    profile: &str,
    expected_finding_count: u64,
) -> Result<String, Box<dyn std::error::Error>> {
    let output = run(
        root,
        &["audit", "--jobs", "1", "--resolution-profile", profile],
    )?;
    assert_status(&output, 0);
    let response: Value = serde_json::from_str(&output.stdout)?;
    let run_id = field(&output.stdout, "runId")?;
    let overview = run(root, &["overview", "--run", &run_id])?;
    assert_status(&overview, 0);
    assert_eq!(
        response.get("status").and_then(Value::as_str),
        Some("complete"),
        "audit response: {}; overview: {}",
        output.stdout,
        overview.stdout,
    );
    assert_eq!(
        response.get("findingCount").and_then(Value::as_u64),
        Some(expected_finding_count)
    );
    assert_eq!(
        response.get("limitationCount").and_then(Value::as_u64),
        Some(0)
    );
    Ok(run_id)
}

fn assert_package_targets(
    root: &Path,
    run_id: &str,
    expected_value_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let importer = file_response(root, run_id, "src/importer.ts")?;
    let expected_value = source_id(root, run_id, expected_value_path)?;
    let expected_type = source_id(root, run_id, "packages/lib/preferred.ts")?;
    let expected_fallback = source_id(root, run_id, "packages/fallback/main.ts")?;
    let expected_types_only = source_id(root, run_id, "packages/types-only/declarations.ts")?;

    assert_internal_target(&importer, "@acme/lib", "value", &expected_value)?;
    assert_internal_target(&importer, "@acme/lib", "type", &expected_type)?;
    assert_internal_target(&importer, "@acme/fallback", "value", &expected_fallback)?;
    assert_internal_target(&importer, "@acme/types-only", "type", &expected_types_only)?;
    Ok(())
}

fn finding_views(
    root: &Path,
    run_id: &str,
    expected_count: u64,
) -> Result<BTreeSet<FindingView>, Box<dyn std::error::Error>> {
    let output = run(root, &["findings", "--run", run_id, "--area", "dead-code"])?;
    assert_status(&output, 0);
    let response: Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(response.get("filters"), Some(&serde_json::json!({})));
    assert_eq!(
        response.get("scopeTotal").and_then(Value::as_u64),
        Some(expected_count)
    );
    assert_eq!(
        response.get("total").and_then(Value::as_u64),
        Some(expected_count)
    );
    assert_eq!(
        response.get("truncated").and_then(Value::as_bool),
        Some(false)
    );
    response
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("findings are missing"))?
        .iter()
        .map(|item| {
            assert_eq!(
                item.get("ruleId").and_then(Value::as_str),
                Some("dead-code/zero-exact-fan-in.v1")
            );
            Ok((
                required_str(item, "/path/display")?,
                required_str(item, "/exportedName")?,
                required_str(item, "/namespace")?,
            ))
        })
        .collect::<Result<_, std::io::Error>>()
        .map_err(Into::into)
}

fn package_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"app","private":true,"type":"module","workspaces":["packages/*"]}"#,
    )?;
    write(
        root.path(),
        "packages/lib/package.json",
        concat!(
            r#"{"name":"@acme/lib","module":"./module.js","main":"./main.js","#,
            r#""typings":"./preferred.ts","types":"./shadowed-types.ts"}"#,
        ),
    )?;
    write(
        root.path(),
        "packages/lib/module.ts",
        "export const selectedValue = 1; export type ModuleDeadType = string;\n",
    )?;
    write(
        root.path(),
        "packages/lib/main.ts",
        "export const selectedValue = 2; export type MainDeadType = string;\n",
    )?;
    write(
        root.path(),
        "packages/lib/preferred.ts",
        concat!(
            "export const preferredDeadValue = 3;\n",
            "export type SelectedType = string;\n",
            "export type UnusedPublicTypingsType = number;\n",
        ),
    )?;
    write(
        root.path(),
        "packages/lib/shadowed-types.ts",
        concat!(
            "export const shadowedTypesDeadValue = 4;\n",
            "export type ShadowedTypesDeadType = string;\n",
        ),
    )?;
    write(
        root.path(),
        "packages/fallback/package.json",
        concat!(
            r#"{"name":"@acme/fallback","module":"./missing.js","#,
            r#""main":"./main.js"}"#,
        ),
    )?;
    write(
        root.path(),
        "packages/fallback/main.ts",
        "export const fallbackValue = 1;\n",
    )?;
    write(
        root.path(),
        "packages/types-only/package.json",
        r#"{"name":"@acme/types-only","types":"./declarations.ts"}"#,
    )?;
    write(
        root.path(),
        "packages/types-only/declarations.ts",
        concat!(
            "export const typesOnlyDeadValue = 1;\n",
            "export type SelectedTypesOnly = string;\n",
            "export type UnusedPublicTypesOnly = number;\n",
        ),
    )?;
    write(
        root.path(),
        "src/importer.ts",
        concat!(
            "import { selectedValue, type SelectedType } from '@acme/lib';\n",
            "import { fallbackValue } from '@acme/fallback';\n",
            "import type { SelectedTypesOnly } from '@acme/types-only';\n",
            "const typed: SelectedType = String(selectedValue);\n",
            "const typesOnly: SelectedTypesOnly = typed;\n",
            "console.log(typed, typesOnly, fallbackValue);\n",
        ),
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
    let selected = resolution(source, specifier, namespace)?;
    assert_eq!(
        selected.pointer("/outcome/kind").and_then(Value::as_str),
        Some("internal")
    );
    assert_eq!(required_str(selected, "/outcome/target")?, expected_target);
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

fn view(path: &str, name: &str, namespace: &str) -> FindingView {
    (path.to_owned(), name.to_owned(), namespace.to_owned())
}

fn write(root: &Path, relative: &str, contents: &str) -> std::io::Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}
