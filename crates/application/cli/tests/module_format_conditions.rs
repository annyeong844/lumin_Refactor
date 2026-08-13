use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde_json::{Map, Value, json};

mod support;

use support::{assert_status, field, run};

const COMMONJS_EXPORT_LIMITATION: &str =
    "CommonJS export lowering is not implemented in the first audit increment";
const REQUIRE_ATTRIBUTION_LIMITATION: &str = "shadowed, mutated, dynamically resolved, or escaped require makes CommonJS module-use attribution opaque";
const MODULE_REQUIRE_ATTRIBUTION_LIMITATION: &str =
    "module.require cannot be attributed to the CommonJS wrapper loader";

const MTS_SOURCE: &str = concat!(
    "import { mtsStatic } from '@acme/lib/mts-static';\n",
    "import {} from '@acme/lib/mts-empty-import';\n",
    "import '@acme/lib/mts-side-effect-import';\n",
    "export { marker as mtsNamed } from '@acme/lib/mts-named-export';\n",
    "export * from '@acme/lib/mts-export';\n",
    "export * as mtsNamespace from '@acme/lib/mts-namespace-export';\n",
    "export {} from '@acme/lib/mts-empty-export';\n",
    "const esmRequired = require('@acme/lib/esm-require');\n",
    "console.log(mtsStatic, esmRequired);\n",
);
const MJS_SOURCE: &str = concat!(
    "import { mjsStatic } from '@acme/lib/mjs-static';\n",
    "console.log(mjsStatic);\n",
);
const CTS_SOURCE: &str = concat!(
    "import { ctsStatic } from '@acme/lib/cts-static';\n",
    "import {} from '@acme/lib/cts-empty-import';\n",
    "import '@acme/lib/cts-side-effect-import';\n",
    "export { marker as ctsNamed } from '@acme/lib/cts-named-export';\n",
    "export * from '@acme/lib/cts-export';\n",
    "export * as ctsNamespace from '@acme/lib/cts-namespace-export';\n",
    "export {} from '@acme/lib/cts-empty-export';\n",
    "import equalsLib = require('@acme/lib/cts-import-equals');\n",
    "void import('@acme/lib/cjs-dynamic');\n",
    "console.log(ctsStatic, equalsLib);\n",
);
const CJS_SOURCE: &str = concat!(
    "const cjsRequired = require('@acme/lib/cjs-require');\n",
    "const escapedLoader = require;\n",
    "escapedLoader('@acme/lib/cjs-aliased');\n",
    "module.require('@acme/lib/cjs-module-require');\n",
    "require('@acme/lib/cjs-before-write');\n",
    "require = require('@acme/lib/cjs-rhs-write');\n",
    "require('@acme/lib/cjs-after-write');\n",
    "console.log(cjsRequired);\n",
);
const CJS_LOOP_SOURCE: &str = concat!(
    "for (var require of [require('@acme/lib/cjs-loop-rhs')]) {\n",
    "  require('@acme/lib/cjs-loop-body');\n",
    "}\n",
);
const ROOT_SOURCE: &str = concat!(
    "import { rootStatic } from '@acme/lib/root-module';\n",
    "console.log(rootStatic);\n",
);
const NEAREST_COMMONJS_SOURCE: &str = concat!(
    "import { nearestCommonJs } from '@acme/lib/nearest-commonjs';\n",
    "var require;\n",
    "const bareLoaded = require('@acme/lib/nearest-bare-var');\n",
    "export import exportedEquals = require('@acme/lib/nearest-export-import-equals');\n",
    "Object.defineProperty(exports, 'unmodeled', { value: 1 });\n",
    "console.log(nearestCommonJs, bareLoaded);\n",
);
const NEAREST_DEFAULT_SOURCE: &str = concat!(
    "import { nearestDefault } from '@acme/lib/nearest-default';\n",
    "console.log(nearestDefault);\n",
);

const TARGET_CASES: &[&str] = &[
    "mts-static",
    "mts-empty-import",
    "mts-side-effect-import",
    "mts-named-export",
    "mts-export",
    "mts-namespace-export",
    "mts-empty-export",
    "mjs-static",
    "cts-static",
    "cts-empty-import",
    "cts-side-effect-import",
    "cts-named-export",
    "cts-export",
    "cts-namespace-export",
    "cts-empty-export",
    "cts-import-equals",
    "cjs-require",
    "cjs-loop-rhs",
    "cjs-loop-body",
    "cjs-before-write",
    "cjs-rhs-write",
    "root-module",
    "nearest-commonjs",
    "nearest-bare-var",
    "nearest-export-import-equals",
    "nearest-default",
    "esm-require",
    "cjs-dynamic",
];

#[test]
fn node_profiles_select_conditions_from_importer_format_and_edge_syntax()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    for profile in ["node16", "nodenext"] {
        verify_profile(root.path(), profile)?;
    }
    verify_exported_static_identities()?;
    Ok(())
}

fn verify_profile(root: &Path, profile: &str) -> Result<(), Box<dyn std::error::Error>> {
    let audit = run(
        root,
        &["audit", "--jobs", "1", "--resolution-profile", profile],
    )?;
    assert_status(&audit, 0);
    let audit_json: Value = serde_json::from_str(&audit.stdout)?;
    assert_eq!(
        audit_json.get("status").and_then(Value::as_str),
        Some("incomplete")
    );
    assert_eq!(
        audit_json.get("limitationCount").and_then(Value::as_u64),
        Some(9)
    );
    let run_id = field(&audit.stdout, "runId")?;

    let overview = run(root, &["overview", "--run", &run_id])?;
    assert_status(&overview, 0);
    let overview: Value = serde_json::from_str(&overview.stdout)?;
    let limitations = overview
        .get("limitations")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("limitations are missing"))?;
    assert_eq!(limitations.len(), 9);
    assert!(limitations.iter().all(|limitation| {
        limitation.get("reason").and_then(Value::as_str) == Some("js-module-use-unknown")
    }));
    assert_eq!(
        limitation_detail_count(limitations, COMMONJS_EXPORT_LIMITATION),
        4
    );
    assert_eq!(
        limitation_detail_count(limitations, REQUIRE_ATTRIBUTION_LIMITATION),
        2
    );
    assert_eq!(
        limitation_detail_count(limitations, MODULE_REQUIRE_ATTRIBUTION_LIMITATION),
        1
    );
    for specifier in ["@acme/lib/mts-export", "@acme/lib/cts-export"] {
        assert_eq!(
            limitation_detail_count(
                limitations,
                &format!(
                    "export-all from {specifier} requires graph expansion not implemented in this increment"
                ),
            ),
            1
        );
    }

    let expected_profile = if profile == "nodenext" {
        "node-next"
    } else {
        profile
    };
    let profiles = overview
        .get("resolutionProfiles")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("resolutionProfiles are missing"))?;
    assert!(!profiles.is_empty());
    assert!(profiles.iter().all(|selected| {
        selected.get("profile").and_then(Value::as_str) == Some(expected_profile)
            && selected.pointer("/source/kind").and_then(Value::as_str) == Some("invocation")
    }));

    let expectations = [
        (
            "apps/ext-esm/main.mts",
            MTS_SOURCE,
            "@acme/lib/mts-static",
            "named",
            "static-import",
            "mtsStatic",
            "mts-static",
            "import",
        ),
        (
            "apps/ext-esm/main.mjs",
            MJS_SOURCE,
            "@acme/lib/mjs-static",
            "named",
            "static-import",
            "mjsStatic",
            "mjs-static",
            "import",
        ),
        (
            "apps/ext-esm/main.mts",
            MTS_SOURCE,
            "@acme/lib/mts-empty-import",
            "side-effect",
            "static-import",
            "import {} from '@acme/lib/mts-empty-import';",
            "mts-empty-import",
            "import",
        ),
        (
            "apps/ext-esm/main.mts",
            MTS_SOURCE,
            "@acme/lib/mts-side-effect-import",
            "side-effect",
            "static-import",
            "import '@acme/lib/mts-side-effect-import';",
            "mts-side-effect-import",
            "import",
        ),
        (
            "apps/ext-esm/main.mts",
            MTS_SOURCE,
            "@acme/lib/mts-named-export",
            "re-export-named",
            "static-import",
            "marker as mtsNamed",
            "mts-named-export",
            "import",
        ),
        (
            "apps/ext-esm/main.mts",
            MTS_SOURCE,
            "@acme/lib/mts-export",
            "re-export-all",
            "static-import",
            "export * from '@acme/lib/mts-export';",
            "mts-export",
            "import",
        ),
        (
            "apps/ext-esm/main.mts",
            MTS_SOURCE,
            "@acme/lib/mts-namespace-export",
            "namespace",
            "static-import",
            "export * as mtsNamespace from '@acme/lib/mts-namespace-export';",
            "mts-namespace-export",
            "import",
        ),
        (
            "apps/ext-esm/main.mts",
            MTS_SOURCE,
            "@acme/lib/mts-empty-export",
            "side-effect",
            "static-import",
            "export {} from '@acme/lib/mts-empty-export';",
            "mts-empty-export",
            "import",
        ),
        (
            "apps/ext-cjs/main.cts",
            CTS_SOURCE,
            "@acme/lib/cts-static",
            "named",
            "static-import",
            "ctsStatic",
            "cts-static",
            "require",
        ),
        (
            "apps/ext-cjs/main.cts",
            CTS_SOURCE,
            "@acme/lib/cts-export",
            "re-export-all",
            "static-import",
            "export * from '@acme/lib/cts-export';",
            "cts-export",
            "require",
        ),
        (
            "apps/ext-cjs/main.cts",
            CTS_SOURCE,
            "@acme/lib/cts-namespace-export",
            "namespace",
            "static-import",
            "export * as ctsNamespace from '@acme/lib/cts-namespace-export';",
            "cts-namespace-export",
            "require",
        ),
        (
            "apps/ext-cjs/main.cts",
            CTS_SOURCE,
            "@acme/lib/cts-empty-import",
            "side-effect",
            "static-import",
            "import {} from '@acme/lib/cts-empty-import';",
            "cts-empty-import",
            "require",
        ),
        (
            "apps/ext-cjs/main.cts",
            CTS_SOURCE,
            "@acme/lib/cts-side-effect-import",
            "side-effect",
            "static-import",
            "import '@acme/lib/cts-side-effect-import';",
            "cts-side-effect-import",
            "require",
        ),
        (
            "apps/ext-cjs/main.cts",
            CTS_SOURCE,
            "@acme/lib/cts-named-export",
            "re-export-named",
            "static-import",
            "marker as ctsNamed",
            "cts-named-export",
            "require",
        ),
        (
            "apps/ext-cjs/main.cts",
            CTS_SOURCE,
            "@acme/lib/cts-empty-export",
            "side-effect",
            "static-import",
            "export {} from '@acme/lib/cts-empty-export';",
            "cts-empty-export",
            "require",
        ),
        (
            "apps/ext-cjs/main.cts",
            CTS_SOURCE,
            "@acme/lib/cts-import-equals",
            "namespace",
            "require",
            "import equalsLib = require('@acme/lib/cts-import-equals');",
            "cts-import-equals",
            "require",
        ),
        (
            "apps/ext-cjs/main.cjs",
            CJS_SOURCE,
            "@acme/lib/cjs-require",
            "dynamic-broad",
            "require",
            "require('@acme/lib/cjs-require')",
            "cjs-require",
            "require",
        ),
        (
            "apps/ext-cjs/loop.cjs",
            CJS_LOOP_SOURCE,
            "@acme/lib/cjs-loop-rhs",
            "dynamic-broad",
            "require",
            "require('@acme/lib/cjs-loop-rhs')",
            "cjs-loop-rhs",
            "require",
        ),
        (
            "apps/ext-cjs/main.cjs",
            CJS_SOURCE,
            "@acme/lib/cjs-before-write",
            "dynamic-broad",
            "require",
            "require('@acme/lib/cjs-before-write')",
            "cjs-before-write",
            "require",
        ),
        (
            "apps/ext-cjs/main.cjs",
            CJS_SOURCE,
            "@acme/lib/cjs-rhs-write",
            "dynamic-broad",
            "require",
            "require('@acme/lib/cjs-rhs-write')",
            "cjs-rhs-write",
            "require",
        ),
        (
            "root-main.ts",
            ROOT_SOURCE,
            "@acme/lib/root-module",
            "named",
            "static-import",
            "rootStatic",
            "root-module",
            "import",
        ),
        (
            "apps/nearest-commonjs/main.ts",
            NEAREST_COMMONJS_SOURCE,
            "@acme/lib/nearest-commonjs",
            "named",
            "static-import",
            "nearestCommonJs",
            "nearest-commonjs",
            "require",
        ),
        (
            "apps/nearest-commonjs/main.ts",
            NEAREST_COMMONJS_SOURCE,
            "@acme/lib/nearest-bare-var",
            "dynamic-broad",
            "require",
            "require('@acme/lib/nearest-bare-var')",
            "nearest-bare-var",
            "require",
        ),
        (
            "apps/nearest-commonjs/main.ts",
            NEAREST_COMMONJS_SOURCE,
            "@acme/lib/nearest-export-import-equals",
            "namespace",
            "require",
            "import exportedEquals = require('@acme/lib/nearest-export-import-equals');",
            "nearest-export-import-equals",
            "require",
        ),
        (
            "apps/nearest-default/main.ts",
            NEAREST_DEFAULT_SOURCE,
            "@acme/lib/nearest-default",
            "named",
            "static-import",
            "nearestDefault",
            "nearest-default",
            "require",
        ),
        (
            "apps/ext-esm/main.mts",
            MTS_SOURCE,
            "@acme/lib/esm-require",
            "dynamic-broad",
            "require",
            "require('@acme/lib/esm-require')",
            "esm-require",
            "require",
        ),
        (
            "apps/ext-cjs/main.cts",
            CTS_SOURCE,
            "@acme/lib/cjs-dynamic",
            "dynamic-broad",
            "dynamic-import",
            "import('@acme/lib/cjs-dynamic')",
            "cjs-dynamic",
            "import",
        ),
    ];

    let source_paths = expectations
        .iter()
        .map(|expectation| expectation.0)
        .collect::<BTreeSet<_>>();
    let mut sources = BTreeMap::new();
    for path in source_paths {
        let source = file_response(root, &run_id, path)?;
        assert_eq!(
            source
                .pointer("/resolutionProfile/profile")
                .and_then(Value::as_str),
            Some(expected_profile)
        );
        assert_eq!(
            source
                .pointer("/resolutionProfile/source/kind")
                .and_then(Value::as_str),
            Some("invocation")
        );
        let expected_count = expectations
            .iter()
            .filter(|expectation| expectation.0 == path)
            .count();
        assert_eq!(
            source
                .get("resolutions")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(expected_count),
            "unexpected resolution count for {path}"
        );
        sources.insert(path, source);
    }

    let mut targets = BTreeMap::new();
    for expectation in expectations {
        let target_path = target_path(expectation.6, expectation.7);
        if !targets.contains_key(&target_path) {
            targets.insert(target_path.clone(), source_id(root, &run_id, &target_path)?);
        }
        let target = resolution_target(
            sources
                .get(expectation.0)
                .ok_or_else(|| std::io::Error::other("source response is missing"))?,
            expectation.2,
            expectation.3,
            expectation.4,
            expected_span(expectation.1, expectation.5)?,
        )?;
        assert_eq!(
            target,
            *targets
                .get(&target_path)
                .ok_or_else(|| std::io::Error::other("target identity is missing"))?,
            "wrong condition target for {} in {} under {profile}",
            expectation.2,
            expectation.0,
        );
    }
    Ok(())
}

fn verify_exported_static_identities() -> Result<(), Box<dyn std::error::Error>> {
    let root = exported_static_identity_fixture()?;
    let audit = run(
        root.path(),
        &["audit", "--jobs", "1", "--resolution-profile", "nodenext"],
    )?;
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
    assert_dead_export(
        root.path(),
        &run_id,
        "apps/consumer/main.ts",
        "exportedEquals",
    )?;
    assert_dead_export(
        root.path(),
        &run_id,
        "apps/consumer/main.ts",
        "namespaceExport",
    )
}

fn fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"root-app","private":true,"type":"module","workspaces":["apps/*","packages/*"]}"#,
    )?;
    write(
        root.path(),
        "apps/ext-esm/package.json",
        r#"{"name":"@acme/ext-esm","private":true,"type":"commonjs"}"#,
    )?;
    write(
        root.path(),
        "apps/ext-cjs/package.json",
        r#"{"name":"@acme/ext-cjs","private":true,"type":"module"}"#,
    )?;
    write(
        root.path(),
        "apps/nearest-commonjs/package.json",
        r#"{"name":"@acme/nearest-commonjs","private":true,"type":"commonjs"}"#,
    )?;
    write(
        root.path(),
        "apps/nearest-default/package.json",
        r#"{"name":"@acme/nearest-default","private":true}"#,
    )?;

    let mut exports = Map::new();
    for case in TARGET_CASES {
        exports.insert(
            format!("./{case}"),
            json!({
                "import": format!("./targets/{case}-import.js"),
                "require": format!("./targets/{case}-require.js"),
            }),
        );
        for lane in ["import", "require"] {
            write(
                root.path(),
                &target_path(case, lane),
                &format!("export const {}_{} = 1;\n", case.replace('-', "_"), lane),
            )?;
        }
    }
    write(
        root.path(),
        "packages/lib/package.json",
        &json!({
            "name": "@acme/lib",
            "private": true,
            "exports": Value::Object(exports),
        })
        .to_string(),
    )?;

    for (path, source) in [
        ("apps/ext-esm/main.mts", MTS_SOURCE),
        ("apps/ext-esm/main.mjs", MJS_SOURCE),
        ("apps/ext-cjs/main.cts", CTS_SOURCE),
        ("apps/ext-cjs/main.cjs", CJS_SOURCE),
        ("apps/ext-cjs/loop.cjs", CJS_LOOP_SOURCE),
        ("root-main.ts", ROOT_SOURCE),
        ("apps/nearest-commonjs/main.ts", NEAREST_COMMONJS_SOURCE),
        ("apps/nearest-default/main.ts", NEAREST_DEFAULT_SOURCE),
    ] {
        write(root.path(), path, source)?;
    }
    Ok(root)
}

fn exported_static_identity_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"root-app","private":true,"type":"module","workspaces":["apps/*","packages/*"]}"#,
    )?;
    write(
        root.path(),
        "apps/consumer/package.json",
        r#"{"name":"@acme/consumer","private":true,"type":"commonjs"}"#,
    )?;
    write(
        root.path(),
        "apps/consumer/main.ts",
        concat!(
            "export import exportedEquals = require('@acme/lib/identity');\n",
            "export * as namespaceExport from '@acme/lib/namespace';\n",
        ),
    )?;
    write(
        root.path(),
        "packages/lib/package.json",
        &json!({
            "name": "@acme/lib",
            "private": true,
            "exports": {
                "./identity": {
                    "import": "./identity-import.js",
                    "require": "./identity-require.js",
                },
                "./namespace": {
                    "import": "./namespace-import.js",
                    "require": "./namespace-require.js",
                }
            },
        })
        .to_string(),
    )?;
    write(
        root.path(),
        "packages/lib/identity-import.ts",
        "export const imported = 1;\n",
    )?;
    write(
        root.path(),
        "packages/lib/identity-require.ts",
        "export const required = 1;\n",
    )?;
    write(
        root.path(),
        "packages/lib/namespace-import.ts",
        "export const importedNamespace = 1;\n",
    )?;
    write(
        root.path(),
        "packages/lib/namespace-require.ts",
        "export const requiredNamespace = 1;\n",
    )?;
    Ok(root)
}

fn target_path(case: &str, lane: &str) -> String {
    format!("packages/lib/targets/{case}-{lane}.ts")
}

fn limitation_detail_count(limitations: &[Value], detail: &str) -> usize {
    limitations
        .iter()
        .filter(|limitation| limitation.get("detail").and_then(Value::as_str) == Some(detail))
        .count()
}

fn assert_dead_export(
    root: &Path,
    run_id: &str,
    path: &str,
    exported_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let output = run(root, &["findings", "--run", run_id, "--area", "dead-code"])?;
    assert_status(&output, 0);
    let response: Value = serde_json::from_str(&output.stdout)?;
    let items = response
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("dead-code findings are missing"))?;
    assert!(
        items.iter().any(|item| {
            item.get("ruleId").and_then(Value::as_str) == Some("dead-code/zero-exact-fan-in.v1")
                && item.pointer("/path/display").and_then(Value::as_str) == Some(path)
                && item.get("exportedName").and_then(Value::as_str) == Some(exported_name)
                && item.get("namespace").and_then(Value::as_str) == Some("value")
        }),
        "missing grounded dead-export evidence for {path}:{exported_name}: {response}"
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
    Ok((start as u64, (start + syntax.len()) as u64))
}

fn required_str(value: &Value, pointer: &str) -> Result<String, std::io::Error> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other(format!("missing string {pointer}")))
}
