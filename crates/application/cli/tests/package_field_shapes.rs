use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

type LimitationView = (String, String, String);

#[test]
fn inventory_owned_shape_families_emit_exact_limitations_before_resolution()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"app","private":true,"workspaces":["packages/*"]}"#,
    )?;
    write(
        root.path(),
        "packages/dependencies/package.json",
        concat!(
            r#"{"name":"@acme/dependencies","private":true,"dependencies":[],"#,
            r#""devDependencies":{"tool":7},"optionalDependencies":false,"#,
            r#""peerDependencies":["peer"]}"#,
        ),
    )?;
    write(
        root.path(),
        "packages/dependencies/index.ts",
        "export const value = 1;\n",
    )?;
    write(
        root.path(),
        "packages/privacy/package.json",
        r#"{"name":"@acme/privacy","private":"yes"}"#,
    )?;
    write(
        root.path(),
        "packages/privacy/index.ts",
        "export const value = 1;\n",
    )?;
    write(
        root.path(),
        "packages/identity/package.json",
        r#"{"name":7,"private":true}"#,
    )?;
    write(
        root.path(),
        "packages/identity/index.ts",
        "export const value = 1;\n",
    )?;
    write(
        root.path(),
        "src/main.ts",
        concat!(
            "import { value as dependencyValue } from '@acme/dependencies';\n",
            "import { value as privacyValue } from '@acme/privacy';\n",
            "import { value as identityValue } from '@acme/identity';\n",
            "console.log(dependencyValue, privacyValue, identityValue);\n",
        ),
    )?;

    let (run_id, overview) = audit_overview(root.path(), &[])?;
    assert_eq!(
        limitation_set(&overview)?,
        BTreeSet::from([
            limitation(
                "dependency-owner-ambiguous",
                "packages/dependencies/package.json",
                "package dependencies field must be object<string,string>",
            ),
            limitation(
                "dependency-owner-ambiguous",
                "packages/dependencies/package.json",
                "package devDependencies field must be object<string,string>",
            ),
            limitation(
                "dependency-owner-ambiguous",
                "packages/dependencies/package.json",
                "package optionalDependencies field must be object<string,string>",
            ),
            limitation(
                "dependency-owner-ambiguous",
                "packages/dependencies/package.json",
                "package peerDependencies field must be object<string,string>",
            ),
            limitation(
                "package-identity-unsupported",
                "packages/identity/package.json",
                "package name does not match package-name.v1",
            ),
            limitation(
                "package-privacy-unsupported",
                "packages/privacy/package.json",
                "package private field must be boolean",
            ),
        ])
    );
    let source = file_response(root.path(), &run_id, "src/main.ts")?;
    assert_resolution_kind(&source, "@acme/dependencies", "internal")?;
    assert_resolution_kind(&source, "@acme/privacy", "internal")?;
    let identity = resolution(&source, "@acme/identity")?;
    assert_eq!(
        identity.pointer("/outcome/kind").and_then(Value::as_str),
        Some("external")
    );
    assert_eq!(
        identity.pointer("/outcome/package").and_then(Value::as_str),
        Some("@acme/identity")
    );

    let workspace = tempfile::tempdir()?;
    write(
        workspace.path(),
        "package.json",
        r#"{"name":"workspace-app","private":true,"workspaces":{"packages":"packages/*"}}"#,
    )?;
    write(
        workspace.path(),
        "packages/lib/package.json",
        r#"{"name":"@acme/lib","private":true}"#,
    )?;
    write(
        workspace.path(),
        "packages/lib/index.ts",
        "export const value = 1;\n",
    )?;
    write(
        workspace.path(),
        "src/main.ts",
        "import { value } from '@acme/lib'; console.log(value);\n",
    )?;

    let (run_id, overview) = audit_overview(workspace.path(), &[])?;
    assert_eq!(
        limitation_set(&overview)?,
        BTreeSet::from([limitation(
            "workspace-ownership-unsupported",
            "package.json",
            "workspaces object must contain packages: array<string>",
        )])
    );
    let source = file_response(workspace.path(), &run_id, "src/main.ts")?;
    let workspace_import = resolution(&source, "@acme/lib")?;
    assert_eq!(
        workspace_import
            .pointer("/outcome/kind")
            .and_then(Value::as_str),
        Some("external")
    );
    assert_eq!(
        workspace_import
            .pointer("/outcome/package")
            .and_then(Value::as_str),
        Some("@acme/lib")
    );
    Ok(())
}

#[test]
fn malformed_package_type_blocks_node_resolution_before_target_selection()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"app","private":true,"type":false}"#,
    )?;
    write(
        root.path(),
        "src/main.ts",
        "import { used } from './target.js'; console.log(used);\n",
    )?;
    write(
        root.path(),
        "src/target.ts",
        "export const used = 1; export const dead = 2;\n",
    )?;

    let (run_id, overview) = audit_overview(root.path(), &["--resolution-profile", "node16"])?;
    assert_eq!(
        limitation_set(&overview)?,
        BTreeSet::from([limitation(
            "importer-format-unsupported",
            "package.json",
            "package type must be module or commonjs for Node profiles",
        )])
    );
    let source = file_response(root.path(), &run_id, "src/main.ts")?;
    assert_eq!(
        source
            .pointer("/resolutionProfile/profile")
            .and_then(Value::as_str),
        Some("node16")
    );
    let relative = resolution(&source, "./target.js")?;
    assert_eq!(
        relative.pointer("/outcome/kind").and_then(Value::as_str),
        Some("unsupported")
    );
    assert_eq!(
        relative.pointer("/outcome/reason").and_then(Value::as_str),
        Some("the importer's semantic configuration is incomplete")
    );
    assert!(relative.pointer("/outcome/target").is_none());
    Ok(())
}

#[test]
fn malformed_public_entry_fields_stop_before_later_fallbacks()
-> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        (
            "module",
            r#"{"name":"@acme/lib","private":true,"module":[],"main":"./fallback.js"}"#,
            "import { used } from '@acme/lib'; console.log(used);\n",
        ),
        (
            "main",
            r#"{"name":"@acme/lib","private":true,"main":[]}"#,
            "import { used } from '@acme/lib'; console.log(used);\n",
        ),
        (
            "typings",
            r#"{"name":"@acme/lib","private":true,"typings":[],"types":"./fallback.d.ts"}"#,
            concat!(
                "import type { Shape } from '@acme/lib';\n",
                "const value: Shape = 'x'; console.log(value);\n",
            ),
        ),
        (
            "types",
            r#"{"name":"@acme/lib","private":true,"types":[],"main":"./fallback.js"}"#,
            concat!(
                "import type { Shape } from '@acme/lib';\n",
                "const value: Shape = 'x'; console.log(value);\n",
            ),
        ),
    ];

    for (field_name, manifest, importer) in cases {
        let root = tempfile::tempdir()?;
        write(
            root.path(),
            "package.json",
            r#"{"name":"app","private":true,"workspaces":["packages/*"]}"#,
        )?;
        write(root.path(), "packages/lib/package.json", manifest)?;
        write(
            root.path(),
            "packages/lib/index.ts",
            "export const used = 1;\n",
        )?;
        write(
            root.path(),
            "packages/lib/fallback.ts",
            "export const used = 1;\n",
        )?;
        write(
            root.path(),
            "packages/lib/fallback.d.ts",
            "export type Shape = string;\n",
        )?;
        write(root.path(), "src/main.ts", importer)?;

        let (run_id, overview) = audit_overview(root.path(), &[])?;
        let expected_detail = format!("package {field_name} field must be a nonempty string");
        assert_eq!(
            limitation_set(&overview)?,
            BTreeSet::from([limitation(
                "public-surface-unsupported",
                "packages/lib/package.json",
                &expected_detail,
            )]),
            "malformed {field_name} did not emit its exact limitation",
        );
        let source = file_response(root.path(), &run_id, "src/main.ts")?;
        let package = resolution(&source, "@acme/lib")?;
        assert_eq!(
            package.pointer("/outcome/kind").and_then(Value::as_str),
            Some("unsupported"),
            "malformed {field_name} selected a later fallback",
        );
        assert_eq!(
            package.pointer("/outcome/reason").and_then(Value::as_str),
            Some(expected_detail.as_str())
        );
        assert!(package.pointer("/outcome/target").is_none());
    }
    Ok(())
}

fn audit_overview(
    root: &Path,
    extra_arguments: &[&str],
) -> Result<(String, Value), Box<dyn std::error::Error>> {
    let mut arguments = vec!["audit", "--jobs", "1"];
    arguments.extend_from_slice(extra_arguments);
    let audit = run(root, &arguments)?;
    assert_status(&audit, 0);
    assert_eq!(field(&audit.stdout, "status")?, "incomplete");
    let run_id = field(&audit.stdout, "runId")?;
    let overview = run(root, &["overview", "--run", &run_id])?;
    assert_status(&overview, 0);
    let overview: Value = serde_json::from_str(&overview.stdout)?;
    assert_eq!(
        overview.get("limitationCount").and_then(Value::as_u64),
        overview
            .get("limitations")
            .and_then(Value::as_array)
            .map(|limitations| limitations.len() as u64)
    );
    Ok((run_id, overview))
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

fn assert_resolution_kind(
    source: &Value,
    specifier: &str,
    expected_kind: &str,
) -> Result<(), std::io::Error> {
    assert_eq!(
        resolution(source, specifier)?
            .pointer("/outcome/kind")
            .and_then(Value::as_str),
        Some(expected_kind)
    );
    Ok(())
}

fn limitation_set(overview: &Value) -> Result<BTreeSet<LimitationView>, std::io::Error> {
    overview
        .get("limitations")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("overview limitations are missing"))?
        .iter()
        .map(|value| {
            let required = |field: &str| {
                value
                    .get(field)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| std::io::Error::other(format!("limitation {field} is missing")))
            };
            Ok((required("reason")?, required("path")?, required("detail")?))
        })
        .collect()
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
