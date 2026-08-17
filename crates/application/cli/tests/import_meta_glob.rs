use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

type FindingView = (String, String, String, String);

#[test]
fn relative_import_meta_globs_expand_and_unsupported_patterns_remain_scoped()
-> Result<(), Box<dyn std::error::Error>> {
    verify_supported_patterns_and_roles()?;
    verify_unsupported_scopes_and_gate_decisions()?;
    verify_embedded_limitation_span()?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn native_only_glob_matches_preserve_value_liveness() -> Result<(), Box<dyn std::error::Error>> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"glob-native","private":true,"type":"module"}"#,
    )?;
    write(
        root.path(),
        "src/main.ts",
        "const pages = import.meta.glob('./pages/*.ts'); console.log(pages);\n",
    )?;
    let mut native_relative = std::path::PathBuf::from("src/pages");
    native_relative.push(OsString::from_vec(b"\x80.ts".to_vec()));
    fs::create_dir_all(root.path().join("src/pages"))?;
    fs::write(
        root.path().join(&native_relative),
        "export const nativeValue = 1; export type NativeType = string;\n",
    )?;
    let display = lumin_model::RepoPath::from_native_relative(&native_relative)?.display_escaped();

    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    let run_id = field(&audit.stdout, "runId")?;
    assert_eq!(
        findings(root.path(), &run_id)?,
        BTreeSet::from([finding(
            &display,
            "NativeType",
            "type",
            "zero grounded exact fan-in",
        )]),
    );
    Ok(())
}

fn verify_supported_patterns_and_roles() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"glob-supported","private":true,"type":"module"}"#,
    )?;
    write(
        root.path(),
        "src/main.ts",
        concat!(
            "const pages = import.meta.glob([",
            "'./pages/*.ts', './pages/nested/*.ts', '!./pages/private/*.ts'",
            "]);\n",
            "const scripts = import.meta.glob('./scripts/*.js');\n",
            "console.log(pages, scripts);\n",
        ),
    )?;
    write(
        root.path(),
        "tests/loader.test.ts",
        "const targets = import.meta.glob('../src/test-target/*.ts'); console.log(targets);\n",
    )?;
    write_exports(root.path(), "src/pages/one.ts", "oneValue", "OneType")?;
    write_exports(
        root.path(),
        "src/pages/nested/two.ts",
        "twoValue",
        "TwoType",
    )?;
    write_exports(
        root.path(),
        "src/pages/private/secret.ts",
        "secretValue",
        "SecretType",
    )?;
    write(
        root.path(),
        "src/scripts/dual.js",
        "export const javascriptValue = 1;\n",
    )?;
    write_exports(
        root.path(),
        "src/scripts/dual.ts",
        "typescriptValue",
        "TypeScriptType",
    )?;
    write_exports(
        root.path(),
        "src/test-target/only.ts",
        "testOnlyValue",
        "TestOnlyType",
    )?;
    write_exports(
        root.path(),
        "src/unrelated.ts",
        "unrelatedValue",
        "UnrelatedType",
    )?;

    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    let audit_json: Value = serde_json::from_str(&audit.stdout)?;
    let run_id = field(&audit.stdout, "runId")?;
    let audit_overview = overview(root.path(), &run_id)?;
    assert_eq!(
        audit_json.get("status").and_then(Value::as_str),
        Some("complete"),
        "supported glob audit was not complete: {audit_overview:#?}",
    );
    assert_eq!(
        audit_json.get("limitationCount").and_then(Value::as_u64),
        Some(0)
    );
    let observed = findings(root.path(), &run_id)?;
    assert_eq!(
        observed,
        BTreeSet::from([
            finding(
                "src/pages/one.ts",
                "OneType",
                "type",
                "zero grounded exact fan-in"
            ),
            finding(
                "src/pages/nested/two.ts",
                "TwoType",
                "type",
                "zero grounded exact fan-in",
            ),
            finding(
                "src/pages/private/secret.ts",
                "secretValue",
                "value",
                "zero grounded exact fan-in",
            ),
            finding(
                "src/pages/private/secret.ts",
                "SecretType",
                "type",
                "zero grounded exact fan-in",
            ),
            finding(
                "src/scripts/dual.ts",
                "typescriptValue",
                "value",
                "zero grounded exact fan-in",
            ),
            finding(
                "src/scripts/dual.ts",
                "TypeScriptType",
                "type",
                "zero grounded exact fan-in",
            ),
            finding(
                "src/test-target/only.ts",
                "testOnlyValue",
                "value",
                "zero production fan-in and is consumed only by test-like sources",
            ),
            finding(
                "src/test-target/only.ts",
                "TestOnlyType",
                "type",
                "zero grounded exact fan-in",
            ),
            finding(
                "src/unrelated.ts",
                "unrelatedValue",
                "value",
                "zero grounded exact fan-in",
            ),
            finding(
                "src/unrelated.ts",
                "UnrelatedType",
                "type",
                "zero grounded exact fan-in",
            ),
        ])
    );

    let main = file_response(root.path(), &run_id, "src/main.ts")?;
    assert_glob_resolutions(
        &main,
        &[
            "./pages/nested/two.ts",
            "./pages/one.ts",
            "./scripts/dual.js",
        ],
    )?;
    assert_resolution_target(
        root.path(),
        &run_id,
        &main,
        "./scripts/dual.js",
        "src/scripts/dual.js",
    )?;
    assert_glob_resolutions(
        &file_response(root.path(), &run_id, "tests/loader.test.ts")?,
        &["../src/test-target/only.ts"],
    )?;
    Ok(())
}

fn verify_unsupported_scopes_and_gate_decisions() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"glob-workspace","private":true,"workspaces":["packages/*"]}"#,
    )?;
    for package in ["explicit", "opaque", "other"] {
        write(
            root.path(),
            &format!("packages/{package}/package.json"),
            &format!(r#"{{"name":"@fixture/{package}","private":true,"type":"module"}}"#),
        )?;
    }
    write(
        root.path(),
        "packages/explicit/src/main.ts",
        concat!(
            "const modules = import.meta.glob('./targets/*.ts', { eager: true });\n",
            "const cross = import.meta.glob('../../other/src/*.{ts,tsx}');\n",
            "console.log(modules, cross);\n",
        ),
    )?;
    write_exports(
        root.path(),
        "packages/explicit/src/targets/one.ts",
        "explicitValue",
        "ExplicitType",
    )?;
    write_exports(
        root.path(),
        "packages/explicit/src/unrelated.ts",
        "explicitDeadValue",
        "ExplicitDeadType",
    )?;
    write(
        root.path(),
        "packages/opaque/src/main.ts",
        concat!(
            "const modules = import.meta.glob('@opaque/*.ts');\n",
            "const excluded = import.meta.glob('./.lumin/*.ts');\n",
            "const wildcardExcluded = import.meta.glob('./node*/**/*.ts');\n",
            "const escaped = import.meta.glob('../../../../outside/*.ts');\n",
            "console.log(modules, excluded, wildcardExcluded, escaped);\n",
        ),
    )?;
    write_exports(
        root.path(),
        "packages/opaque/src/blocked.ts",
        "opaqueBlockedValue",
        "OpaqueBlockedType",
    )?;
    write_exports(
        root.path(),
        "packages/other/src/dead.ts",
        "otherDeadValue",
        "OtherDeadType",
    )?;

    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    let audit_json: Value = serde_json::from_str(&audit.stdout)?;
    assert_eq!(
        audit_json.get("status").and_then(Value::as_str),
        Some("incomplete")
    );
    assert_eq!(
        audit_json.get("limitationCount").and_then(Value::as_u64),
        Some(6)
    );
    let run_id = field(&audit.stdout, "runId")?;

    assert_eq!(
        findings(root.path(), &run_id)?,
        BTreeSet::from([
            finding(
                "packages/explicit/src/targets/one.ts",
                "ExplicitType",
                "type",
                "zero grounded exact fan-in",
            ),
            finding(
                "packages/explicit/src/unrelated.ts",
                "explicitDeadValue",
                "value",
                "zero grounded exact fan-in",
            ),
            finding(
                "packages/explicit/src/unrelated.ts",
                "ExplicitDeadType",
                "type",
                "zero grounded exact fan-in",
            ),
            finding(
                "packages/other/src/dead.ts",
                "OtherDeadType",
                "type",
                "zero grounded exact fan-in",
            ),
        ]),
        "glob opacity escaped its target/package absence domain",
    );

    let overview = overview(root.path(), &run_id)?;
    let limitations = required_array(&overview, "/limitations")?;
    let explicit = limitation_for_pattern(limitations, "./targets/*.ts")?;
    assert_eq!(
        required_str(explicit, "/targetScope/kind")?,
        "explicit-targets"
    );
    assert_eq!(required_array(explicit, "/candidates")?.len(), 1);
    assert_eq!(
        required_array(explicit, "/candidates")?[0]
            .as_str()
            .ok_or_else(|| std::io::Error::other("candidate source ID is missing"))?,
        required_str(
            &file_response(root.path(), &run_id, "packages/explicit/src/targets/one.ts",)?,
            "/sourceContext/sourceId",
        )?,
    );
    let cross = limitation_for_pattern(limitations, "../../other/src/*.{ts,tsx}")?;
    assert_eq!(
        required_str(cross, "/targetScope/kind")?,
        "explicit-targets"
    );
    assert_eq!(required_array(cross, "/candidates")?.len(), 1);
    assert_eq!(
        required_array(cross, "/candidates")?[0]
            .as_str()
            .ok_or_else(|| std::io::Error::other("cross-package candidate is missing"))?,
        required_str(
            &file_response(root.path(), &run_id, "packages/other/src/dead.ts")?,
            "/sourceContext/sourceId",
        )?,
    );
    let package = limitation_for_pattern(limitations, "@opaque/*.ts")?;
    assert_eq!(required_str(package, "/targetScope/kind")?, "package");
    assert!(required_array(package, "/candidates")?.is_empty());
    let excluded = limitation_for_pattern(limitations, "./.lumin/*.ts")?;
    assert_eq!(required_str(excluded, "/targetScope/kind")?, "package");
    assert!(required_array(excluded, "/candidates")?.is_empty());
    let wildcard_excluded = limitation_for_pattern(limitations, "./node*/**/*.ts")?;
    assert_eq!(
        required_str(wildcard_excluded, "/targetScope/kind")?,
        "package"
    );
    assert!(required_array(wildcard_excluded, "/candidates")?.is_empty());
    let escaped = limitation_for_pattern(limitations, "../../../../outside/*.ts")?;
    assert_eq!(required_str(escaped, "/targetScope/kind")?, "package");
    assert!(required_array(escaped, "/candidates")?.is_empty());

    let explicit_gate = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-import-meta-explicit",
            "--path",
            "packages/explicit/src/main.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&explicit_gate, 0);
    assert_eq!(
        field(&explicit_gate.stdout, "decision")?,
        "allow-with-warnings"
    );
    assert!(!has_required_gap(&serde_json::from_str(
        &explicit_gate.stdout
    )?));

    let opaque_gate = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-import-meta-package",
            "--path",
            "packages/opaque/src/main.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opaque_gate, 4);
    assert_eq!(field(&opaque_gate.stdout, "decision")?, "incomplete");
    assert_eq!(field(&opaque_gate.stdout, "lifecycle")?, "rejected");
    assert!(has_required_gap(&serde_json::from_str(
        &opaque_gate.stdout
    )?));
    Ok(())
}

fn verify_embedded_limitation_span() -> Result<(), Box<dyn std::error::Error>> {
    const CALL: &str = "import.meta.glob('./views/*.ts', { eager: true })";
    let source = format!(
        "<template><div /></template>\n<script setup lang=\"ts\">\nconst views = {CALL};\nconsole.log(views);\n</script>\n"
    );
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"glob-vue","private":true,"type":"module"}"#,
    )?;
    write(root.path(), "src/App.vue", &source)?;
    write_exports(root.path(), "src/views/one.ts", "viewValue", "ViewType")?;

    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    let run_id = field(&audit.stdout, "runId")?;
    let overview = overview(root.path(), &run_id)?;
    let limitation =
        limitation_for_pattern(required_array(&overview, "/limitations")?, "./views/*.ts")?;
    let start = source
        .find(CALL)
        .ok_or_else(|| std::io::Error::other("embedded call is missing"))?;
    assert_eq!(
        limitation.pointer("/span/start").and_then(Value::as_u64),
        Some(start as u64),
    );
    assert_eq!(
        limitation.pointer("/span/end").and_then(Value::as_u64),
        Some((start + CALL.len()) as u64),
    );
    assert_eq!(
        required_str(limitation, "/targetScope/kind")?,
        "explicit-targets"
    );
    assert_eq!(
        capability_state(&overview, "sfc/vue.v1"),
        Some("incomplete")
    );
    Ok(())
}

fn assert_glob_resolutions(source: &Value, expected: &[&str]) -> Result<(), std::io::Error> {
    let resolutions = required_array(source, "/resolutions")?;
    let observed = resolutions
        .iter()
        .map(|resolution| {
            assert_eq!(
                required_str(resolution, "/sourceUse/requestKind")?,
                "import-meta-glob",
            );
            assert_eq!(
                required_str(resolution, "/sourceUse/kind")?,
                "dynamic-broad"
            );
            assert_eq!(required_str(resolution, "/outcome/kind")?, "internal");
            required_str(resolution, "/sourceUse/specifier")
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    assert_eq!(
        observed,
        expected.iter().map(|value| (*value).to_owned()).collect(),
    );
    Ok(())
}

fn assert_resolution_target(
    root: &Path,
    run_id: &str,
    source: &Value,
    specifier: &str,
    target_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let resolution = required_array(source, "/resolutions")?
        .iter()
        .find(|resolution| {
            resolution
                .pointer("/sourceUse/specifier")
                .and_then(Value::as_str)
                == Some(specifier)
        })
        .ok_or_else(|| std::io::Error::other(format!("resolution for {specifier} is missing")))?;
    let expected_target = required_str(
        &file_response(root, run_id, target_path)?,
        "/sourceContext/sourceId",
    )?;
    assert_eq!(
        required_str(resolution, "/outcome/target")?,
        expected_target,
        "glob target was re-resolved instead of preserving the expanded source",
    );
    Ok(())
}

fn findings(
    root: &Path,
    run_id: &str,
) -> Result<BTreeSet<FindingView>, Box<dyn std::error::Error>> {
    let output = run(root, &["findings", "--run", run_id, "--area", "dead-code"])?;
    assert_status(&output, 0);
    let response: Value = serde_json::from_str(&output.stdout)?;
    required_array(&response, "/items")?
        .iter()
        .map(|item| {
            Ok((
                required_str(item, "/path/display")?,
                required_str(item, "/exportedName")?,
                required_str(item, "/namespace")?,
                required_str(item, "/claim")?,
            ))
        })
        .collect::<Result<_, std::io::Error>>()
        .map_err(Into::into)
}

fn finding(path: &str, name: &str, namespace: &str, claim: &str) -> FindingView {
    (
        path.to_owned(),
        name.to_owned(),
        namespace.to_owned(),
        format!("export `{name}` has {claim}"),
    )
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

fn limitation_for_pattern<'a>(
    limitations: &'a [Value],
    pattern: &str,
) -> Result<&'a Value, std::io::Error> {
    limitations
        .iter()
        .find(|limitation| {
            limitation
                .get("patterns")
                .and_then(Value::as_array)
                .is_some_and(|patterns| {
                    patterns.iter().any(|value| value.as_str() == Some(pattern))
                })
        })
        .ok_or_else(|| std::io::Error::other(format!("limitation for {pattern} is missing")))
}

fn has_required_gap(response: &Value) -> bool {
    response
        .get("signals")
        .and_then(Value::as_array)
        .is_some_and(|signals| {
            signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some("required-evidence-incomplete")
            })
        })
}

fn capability_state<'a>(overview: &'a Value, capability_id: &str) -> Option<&'a str> {
    overview
        .get("capabilityStates")?
        .as_array()?
        .iter()
        .find(|row| row.get("capabilityId").and_then(Value::as_str) == Some(capability_id))?
        .get("state")?
        .as_str()
}

fn required_array<'a>(value: &'a Value, pointer: &str) -> Result<&'a Vec<Value>, std::io::Error> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other(format!("missing array {pointer}")))
}

fn required_str(value: &Value, pointer: &str) -> Result<String, std::io::Error> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other(format!("missing string {pointer}")))
}

fn write_exports(
    root: &Path,
    path: &str,
    value_name: &str,
    type_name: &str,
) -> Result<(), std::io::Error> {
    write(
        root,
        path,
        &format!("export const {value_name} = 1; export type {type_name} = string;\n"),
    )
}

fn write(root: &Path, path: &str, contents: &str) -> Result<(), std::io::Error> {
    let path = root.join(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}
