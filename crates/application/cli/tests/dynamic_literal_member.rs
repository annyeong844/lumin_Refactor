use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

const MAIN_SOURCE: &str = concat!(
    "function consume(value: unknown) { console.log(value); }\n",
    "async function run(flag: boolean) {\n",
    "  const awaited = await import('./awaited.js');\n",
    "  console.log(awaited.selectedAwait);\n",
    "  import('./then.js').then((callback) => callback.selectedThen());\n",
    "  const scoped = await import('./outer.js');\n",
    "  if (flag) {\n",
    "    const scoped = await import('./inner.js');\n",
    "    console.log(scoped.selectedInner);\n",
    "  }\n",
    "  console.log(scoped.selectedOuter);\n",
    "  console.log((await import('./direct.js')).selectedDirect);\n",
    "  const escaped = await import('./broad.js');\n",
    "  consume(escaped);\n",
    "}\n",
    "void run(true);\n",
);

type FindingView = (String, String, String);

#[test]
fn literal_dynamic_members_preserve_precision_across_bindings_callbacks_and_shadowing()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
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

    assert_eq!(
        findings(root.path(), &run_id)?,
        BTreeSet::from([
            finding("src/awaited.ts", "deadAwait"),
            finding("src/direct.ts", "deadDirect"),
            finding("src/inner.ts", "deadInner"),
            finding("src/outer.ts", "deadOuter"),
            finding("src/then.ts", "deadThen"),
        ])
    );

    let source = file_response(root.path(), &run_id, "src/main.ts")?;
    for (specifier, imported_name, local_name, syntax, target) in [
        (
            "./awaited.js",
            "selectedAwait",
            Some("awaited"),
            "awaited.selectedAwait",
            "src/awaited.ts",
        ),
        (
            "./then.js",
            "selectedThen",
            Some("callback"),
            "callback.selectedThen",
            "src/then.ts",
        ),
        (
            "./outer.js",
            "selectedOuter",
            Some("scoped"),
            "scoped.selectedOuter",
            "src/outer.ts",
        ),
        (
            "./inner.js",
            "selectedInner",
            Some("scoped"),
            "scoped.selectedInner",
            "src/inner.ts",
        ),
        (
            "./direct.js",
            "selectedDirect",
            None,
            "(await import('./direct.js')).selectedDirect",
            "src/direct.ts",
        ),
    ] {
        assert_exact_dynamic_resolution(
            &source,
            specifier,
            imported_name,
            local_name,
            expected_span(MAIN_SOURCE, syntax)?,
            &source_id(root.path(), &run_id, target)?,
        )?;
    }
    assert_broad_dynamic_resolution(
        &source,
        "./broad.js",
        Some("escaped"),
        &source_id(root.path(), &run_id, "src/broad.ts")?,
    )?;
    assert_eq!(
        source
            .get("resolutions")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(6),
        "dynamic member extraction emitted duplicate or extra resolutions",
    );
    Ok(())
}

fn fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"dynamic-member-fixture","private":true,"type":"module"}"#,
    )?;
    write(root.path(), "src/main.ts", MAIN_SOURCE)?;
    for (path, selected, dead) in [
        ("src/awaited.ts", "selectedAwait", "deadAwait"),
        ("src/then.ts", "selectedThen", "deadThen"),
        ("src/outer.ts", "selectedOuter", "deadOuter"),
        ("src/inner.ts", "selectedInner", "deadInner"),
        ("src/direct.ts", "selectedDirect", "deadDirect"),
    ] {
        write(
            root.path(),
            path,
            &format!("export const {selected} = 1; export const {dead} = 2;\n"),
        )?;
    }
    write(
        root.path(),
        "src/broad.ts",
        "export const broadOne = 1; export const broadTwo = 2;\n",
    )?;
    Ok(root)
}

fn findings(
    root: &Path,
    run_id: &str,
) -> Result<BTreeSet<FindingView>, Box<dyn std::error::Error>> {
    let output = run(root, &["findings", "--run", run_id, "--area", "dead-code"])?;
    assert_status(&output, 0);
    let response: Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(
        response.get("truncated").and_then(Value::as_bool),
        Some(false)
    );
    let items = response
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("dead-code finding items are missing"))?;
    assert_eq!(
        response.get("total").and_then(Value::as_u64),
        Some(items.len() as u64)
    );
    items
        .iter()
        .map(|item| {
            Ok((
                required_str(item, "/path/display")?,
                required_str(item, "/exportedName")?,
                required_str(item, "/namespace")?,
            ))
        })
        .collect()
}

fn assert_exact_dynamic_resolution(
    source: &Value,
    specifier: &str,
    imported_name: &str,
    local_name: Option<&str>,
    span: (u64, u64),
    target: &str,
) -> Result<(), std::io::Error> {
    let resolution = resolution(source, specifier, "named", span)?;
    assert_eq!(
        resolution
            .pointer("/sourceUse/importedName")
            .and_then(Value::as_str),
        Some(imported_name)
    );
    assert_eq!(
        resolution
            .pointer("/sourceUse/localName")
            .and_then(Value::as_str),
        local_name
    );
    assert_eq!(
        resolution.pointer("/outcome/kind").and_then(Value::as_str),
        Some("internal")
    );
    assert_eq!(required_str(resolution, "/outcome/target")?, target);
    Ok(())
}

fn assert_broad_dynamic_resolution(
    source: &Value,
    specifier: &str,
    local_name: Option<&str>,
    target: &str,
) -> Result<(), std::io::Error> {
    let resolution = source
        .get("resolutions")
        .and_then(Value::as_array)
        .and_then(|resolutions| {
            resolutions.iter().find(|resolution| {
                resolution
                    .pointer("/sourceUse/specifier")
                    .and_then(Value::as_str)
                    == Some(specifier)
                    && resolution
                        .pointer("/sourceUse/kind")
                        .and_then(Value::as_str)
                        == Some("dynamic-broad")
                    && resolution
                        .pointer("/sourceUse/requestKind")
                        .and_then(Value::as_str)
                        == Some("dynamic-import")
            })
        })
        .ok_or_else(|| std::io::Error::other("broad dynamic resolution is missing"))?;
    assert_eq!(
        resolution
            .pointer("/sourceUse/importedName")
            .and_then(Value::as_str),
        None
    );
    assert_eq!(
        resolution
            .pointer("/sourceUse/localName")
            .and_then(Value::as_str),
        local_name
    );
    assert_eq!(
        resolution.pointer("/outcome/kind").and_then(Value::as_str),
        Some("internal")
    );
    assert_eq!(required_str(resolution, "/outcome/target")?, target);
    Ok(())
}

fn resolution<'a>(
    source: &'a Value,
    specifier: &str,
    kind: &str,
    span: (u64, u64),
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
                        .pointer("/sourceUse/kind")
                        .and_then(Value::as_str)
                        == Some(kind)
                    && resolution
                        .pointer("/sourceUse/requestKind")
                        .and_then(Value::as_str)
                        == Some("dynamic-import")
                    && resolution
                        .pointer("/sourceUse/span/start")
                        .and_then(Value::as_u64)
                        == Some(span.0)
                    && resolution
                        .pointer("/sourceUse/span/end")
                        .and_then(Value::as_u64)
                        == Some(span.1)
            })
        })
        .ok_or_else(|| {
            std::io::Error::other(format!(
                "{kind} dynamic resolution for {specifier} at {span:?} is missing"
            ))
        })
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

fn expected_span(source: &str, syntax: &str) -> Result<(u64, u64), std::io::Error> {
    let start = source
        .find(syntax)
        .ok_or_else(|| std::io::Error::other(format!("fixture syntax is missing: {syntax}")))?;
    Ok((start as u64, (start + syntax.len()) as u64))
}

fn finding(path: &str, exported_name: &str) -> FindingView {
    (
        path.to_owned(),
        exported_name.to_owned(),
        "value".to_owned(),
    )
}

fn required_str(value: &Value, pointer: &str) -> Result<String, std::io::Error> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other(format!("missing string {pointer}")))
}

fn write(root: &Path, path: &str, contents: &str) -> Result<(), std::io::Error> {
    let path = root.join(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}
