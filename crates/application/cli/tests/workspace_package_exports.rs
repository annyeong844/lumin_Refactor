use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

type FindingView = (String, String, String);

#[test]
fn overlapping_patterns_follow_comparator_independent_of_source_order()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"app","private":true,"workspaces":["packages/*"]}"#,
    )?;
    for (package, exports) in [
        (
            "forward",
            concat!(
                r#"{"./features/*":"./broad/*","./features/internal/*":"./prefix/*","#,
                r#""./features/*.js":"./suffix/*"}"#,
            ),
        ),
        (
            "reverse",
            concat!(
                r#"{"./features/*.js":"./suffix/*","./features/internal/*":"./prefix/*","#,
                r#""./features/*":"./broad/*"}"#,
            ),
        ),
    ] {
        write(
            root.path(),
            &format!("packages/{package}/package.json"),
            &format!(r#"{{"name":"@acme/{package}","private":true,"exports":{exports}}}"#),
        )?;
        for (path, name) in [
            ("prefix/x.ts", format!("{package}Prefix")),
            ("suffix/button.ts", format!("{package}Suffix")),
            ("broad/internal/x.ts", format!("wrong{package}Prefix")),
            ("broad/button.ts", format!("wrong{package}Suffix")),
        ] {
            write(
                root.path(),
                &format!("packages/{package}/{path}"),
                &format!("export const {name} = 1;\n"),
            )?;
        }
    }
    let main_source = concat!(
        "import { forwardPrefix } from '@acme/forward/features/internal/x';\n",
        "import { forwardSuffix } from '@acme/forward/features/button.js';\n",
        "import { reversePrefix } from '@acme/reverse/features/internal/x';\n",
        "import { reverseSuffix } from '@acme/reverse/features/button.js';\n",
        "console.log(forwardPrefix, forwardSuffix, reversePrefix, reverseSuffix);\n",
    );
    write(root.path(), "src/main.ts", main_source)?;

    let run_id = audit(root.path())?;
    let source = file_response(root.path(), &run_id, "src/main.ts")?;
    for (specifier, binding, target) in [
        (
            "@acme/forward/features/internal/x",
            "forwardPrefix",
            "packages/forward/prefix/x.ts",
        ),
        (
            "@acme/forward/features/button.js",
            "forwardSuffix",
            "packages/forward/suffix/button.ts",
        ),
        (
            "@acme/reverse/features/internal/x",
            "reversePrefix",
            "packages/reverse/prefix/x.ts",
        ),
        (
            "@acme/reverse/features/button.js",
            "reverseSuffix",
            "packages/reverse/suffix/button.ts",
        ),
    ] {
        assert_eq!(
            resolution_target(
                &source,
                specifier,
                "named",
                "static-import",
                expected_span(main_source, binding)?,
            )?,
            source_id(root.path(), &run_id, target)?
        );
    }
    assert_eq!(
        finding_set(root.path(), &run_id)?,
        BTreeSet::from([
            finding("packages/forward/broad/button.ts", "wrongforwardSuffix"),
            finding("packages/forward/broad/internal/x.ts", "wrongforwardPrefix",),
            finding("packages/reverse/broad/button.ts", "wrongreverseSuffix"),
            finding("packages/reverse/broad/internal/x.ts", "wrongreversePrefix",),
        ])
    );
    Ok(())
}

#[test]
fn exact_and_pattern_exports_follow_edge_specific_conditions()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"app","private":true,"type":"commonjs","workspaces":["packages/*"]}"#,
    )?;
    write(
        root.path(),
        "tsconfig.json",
        r#"{"compilerOptions":{"moduleResolution":"node16","module":"node16"}}"#,
    )?;
    write(
        root.path(),
        "packages/lib/package.json",
        concat!(
            r#"{"name":"@acme/lib","private":true,"exports":{"./features/*":{"#,
            r#""import":"./pattern-import/*.js","require":"./pattern-require/*.js","#,
            r#""default":"./pattern-default/*.js"},"./features/special":{"#,
            r#""import":"./exact-import.js","require":"./exact-require.js","#,
            r#""default":"./exact-default.js"}}}"#,
        ),
    )?;
    for (path, name) in [
        ("packages/lib/exact-import.ts", "dynamicExact"),
        ("packages/lib/exact-default.ts", "wrongExactDefault"),
        (
            "packages/lib/pattern-require/special.ts",
            "wrongPatternExact",
        ),
        (
            "packages/lib/pattern-import/special.ts",
            "wrongPatternExactImport",
        ),
        ("packages/lib/pattern-require/button.ts", "usedPattern"),
        (
            "packages/lib/pattern-default/button.ts",
            "wrongPatternDefault",
        ),
    ] {
        write(root.path(), path, &format!("export const {name} = 1;\n"))?;
    }
    write(
        root.path(),
        "packages/lib/exact-require.ts",
        "export const usedExact = 1; export const exactRequireDead = 2;\n",
    )?;
    write(
        root.path(),
        "packages/lib/pattern-import/button.ts",
        "export const dynamicTarget = 1;\n",
    )?;
    let main_source = concat!(
        "import { usedExact } from '@acme/lib/features/special';\n",
        "import { usedPattern } from '@acme/lib/features/button';\n",
        "void import('@acme/lib/features/special');\n",
        "void import('@acme/lib/features/button');\n",
        "console.log(usedExact, usedPattern);\n",
    );
    write(root.path(), "src/main.ts", main_source)?;

    let run_id = audit(root.path())?;
    let source = file_response(root.path(), &run_id, "src/main.ts")?;
    assert_eq!(
        source
            .get("resolutions")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(4)
    );
    assert_eq!(
        resolution_target(
            &source,
            "@acme/lib/features/special",
            "named",
            "static-import",
            expected_span(main_source, "usedExact")?,
        )?,
        source_id(root.path(), &run_id, "packages/lib/exact-require.ts")?
    );
    assert_eq!(
        resolution_target(
            &source,
            "@acme/lib/features/special",
            "dynamic-broad",
            "dynamic-import",
            expected_span(main_source, "import('@acme/lib/features/special')")?,
        )?,
        source_id(root.path(), &run_id, "packages/lib/exact-import.ts")?
    );
    assert_eq!(
        resolution_target(
            &source,
            "@acme/lib/features/button",
            "named",
            "static-import",
            expected_span(main_source, "usedPattern")?,
        )?,
        source_id(
            root.path(),
            &run_id,
            "packages/lib/pattern-require/button.ts",
        )?
    );
    assert_eq!(
        resolution_target(
            &source,
            "@acme/lib/features/button",
            "dynamic-broad",
            "dynamic-import",
            expected_span(main_source, "import('@acme/lib/features/button')")?,
        )?,
        source_id(
            root.path(),
            &run_id,
            "packages/lib/pattern-import/button.ts",
        )?
    );
    Ok(())
}

#[test]
fn closed_package_exports_do_not_withhold_unrelated_dead_findings()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"app","private":true,"type":"commonjs","workspaces":["packages/*"]}"#,
    )?;
    write(
        root.path(),
        "tsconfig.json",
        r#"{"compilerOptions":{"moduleResolution":"node16","module":"node16"}}"#,
    )?;
    write(
        root.path(),
        "packages/ui/package.json",
        r#"{"name":"@app/ui","private":true,"exports":{"./blocked":null}}"#,
    )?;
    write(root.path(), "src/candidate.ts", "export const dead = 1;\n")?;
    write(
        root.path(),
        "src/main.ts",
        concat!(
            "import { blocked } from '@app/ui/blocked';\n",
            "import { missing } from '@app/ui/missing';\n",
            "console.log(blocked, missing);\n",
        ),
    )?;

    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    let audit_json: Value = serde_json::from_str(&audit.stdout)?;
    assert_eq!(
        audit_json.get("status").and_then(Value::as_str),
        Some("incomplete")
    );
    assert_eq!(
        audit_json.get("findingCount").and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        audit_json.get("limitationCount").and_then(Value::as_u64),
        Some(2)
    );
    let run_id = field(&audit.stdout, "runId")?;

    let source = file_response(root.path(), &run_id, "src/main.ts")?;
    let resolutions = source
        .get("resolutions")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("source resolution is missing"))?;
    assert_eq!(resolutions.len(), 2);
    for resolution in resolutions {
        assert_eq!(
            resolution
                .pointer("/sourceUse/requestKind")
                .and_then(Value::as_str),
            Some("static-import")
        );
        assert_eq!(
            resolution.pointer("/outcome/kind").and_then(Value::as_str),
            Some("unresolved")
        );
        assert_eq!(
            resolution.pointer("/outcome/candidates"),
            Some(&serde_json::json!([]))
        );
        assert_eq!(
            resolution
                .pointer("/outcome/targetScope/kind")
                .and_then(Value::as_str),
            Some("known-no-target")
        );
        assert_eq!(
            resolution
                .pointer("/outcome/targetScope/package")
                .and_then(Value::as_str),
            Some("@app/ui")
        );
    }

    let overview = run(root.path(), &["overview", "--run", &run_id])?;
    assert_status(&overview, 0);
    let overview: Value = serde_json::from_str(&overview.stdout)?;
    let limitations = overview
        .get("limitations")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("limitations are missing"))?;
    assert_eq!(limitations.len(), 2);
    for limitation in limitations {
        assert_eq!(
            limitation.get("reason").and_then(Value::as_str),
            Some("internal-specifier-unresolved")
        );
        assert_eq!(limitation.get("candidates"), Some(&serde_json::json!([])));
        assert_eq!(
            limitation
                .pointer("/targetScope/kind")
                .and_then(Value::as_str),
            Some("known-no-target")
        );
        assert_eq!(
            limitation
                .pointer("/targetScope/package")
                .and_then(Value::as_str),
            Some("@app/ui")
        );
    }
    assert_eq!(
        finding_set(root.path(), &run_id)?,
        BTreeSet::from([finding("src/candidate.ts", "dead")])
    );
    Ok(())
}

#[test]
fn exports_protect_only_selected_public_identities() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"app","private":true,"workspaces":["packages/*"]}"#,
    )?;
    write(
        root.path(),
        "packages/lib/package.json",
        concat!(
            r#"{"name":"@acme/lib","exports":{"./features/*":"./features/*.js","#,
            r#"".":"./index.js"}}"#,
        ),
    )?;
    write(
        root.path(),
        "packages/lib/index.ts",
        "export { publicValue } from './internal.js';\n",
    )?;
    write(
        root.path(),
        "packages/lib/internal.ts",
        "export const publicValue = 1; export const siblingDead = 2;\n",
    )?;
    write(
        root.path(),
        "packages/lib/features/button.ts",
        "export const Button = 1;\n",
    )?;
    write(
        root.path(),
        "packages/lib/hidden.ts",
        "export const hiddenDead = 1;\n",
    )?;

    let run_id = audit(root.path())?;
    assert_eq!(
        finding_set(root.path(), &run_id)?,
        BTreeSet::from([
            finding("packages/lib/hidden.ts", "hiddenDead"),
            finding("packages/lib/internal.ts", "siblingDead"),
        ])
    );
    Ok(())
}

fn audit(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let audit = run(root, &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    assert_eq!(field(&audit.stdout, "status")?, "complete");
    let run_id = field(&audit.stdout, "runId")?;
    let audit: Value = serde_json::from_str(&audit.stdout)?;
    assert_eq!(
        audit.get("limitationCount").and_then(Value::as_u64),
        Some(0)
    );
    Ok(run_id)
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

fn resolution_target(
    source: &Value,
    specifier: &str,
    use_kind: &str,
    request_kind: &str,
    expected_span: (u64, u64),
) -> Result<String, std::io::Error> {
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
                        == Some(use_kind)
                    && resolution
                        .pointer("/sourceUse/requestKind")
                        .and_then(Value::as_str)
                        == Some(request_kind)
                    && resolution
                        .pointer("/sourceUse/span/start")
                        .and_then(Value::as_u64)
                        == Some(expected_span.0)
                    && resolution
                        .pointer("/sourceUse/span/end")
                        .and_then(Value::as_u64)
                        == Some(expected_span.1)
            })
        })
        .ok_or_else(|| {
            std::io::Error::other(format!(
                "{use_kind} {request_kind} resolution for {specifier} at {expected_span:?} is missing"
            ))
        })?;
    assert_eq!(
        resolution.pointer("/outcome/kind").and_then(Value::as_str),
        Some("internal")
    );
    required_str(resolution, "/outcome/target")
}

fn expected_span(source: &str, syntax: &str) -> Result<(u64, u64), std::io::Error> {
    let start = source
        .find(syntax)
        .ok_or_else(|| std::io::Error::other(format!("{syntax:?} is missing from fixture")))?;
    let end = start + syntax.len();
    Ok((start as u64, end as u64))
}

fn finding_set(
    root: &Path,
    run_id: &str,
) -> Result<BTreeSet<FindingView>, Box<dyn std::error::Error>> {
    let output = run(root, &["findings", "--run", run_id, "--area", "dead-code"])?;
    assert_status(&output, 0);
    let response: Value = serde_json::from_str(&output.stdout)?;
    let items = response
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("findings are missing"))?;
    assert_eq!(
        response.get("scopeTotal").and_then(Value::as_u64),
        Some(items.len() as u64)
    );
    assert_eq!(
        response.get("total").and_then(Value::as_u64),
        Some(items.len() as u64)
    );
    assert_eq!(
        response.get("truncated").and_then(Value::as_bool),
        Some(false)
    );
    items
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

fn required_str(value: &Value, pointer: &str) -> Result<String, std::io::Error> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other(format!("missing string {pointer}")))
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
