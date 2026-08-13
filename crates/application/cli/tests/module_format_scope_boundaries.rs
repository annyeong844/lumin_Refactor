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
    let mut expected = vec![COMMONJS_EXPORT_LOWERING_UNSUPPORTED; 5];
    expected.extend([REQUIRE_ATTRIBUTION_OPAQUE; 3]);
    expected.sort_unstable();
    assert_eq!(details, expected);
    Ok(())
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
    ] {
        write(
            root.path(),
            &format!("packages/dep/{path}.ts"),
            &format!("export const {} = 1;\n", path.replace('-', "_")),
        )?;
    }
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

fn write(root: &Path, path: &str, contents: &str) -> Result<(), std::io::Error> {
    let path = root.join(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}
