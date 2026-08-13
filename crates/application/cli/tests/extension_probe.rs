use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

type FindingTuple = (String, String, String);

#[test]
fn relative_extension_and_directory_probes_follow_frozen_precedence()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "src/main.ts",
        concat!(
            "import { exactUsed } from './exact.ts';\n",
            "import { runtimeUsed } from './runtime.js';\n",
            "import { fileUsed } from './choice';\n",
            "import { packageUsed } from './pkg';\n",
            "import { indexUsed } from './fallback';\n",
            "import ExactVue from './Exact.vue';\n",
            "import type { DeclUsed } from './decl.d.ts';\n",
            "const declared: DeclUsed = 'ok';\n",
            "console.log(exactUsed, runtimeUsed, fileUsed, packageUsed, indexUsed, ExactVue, declared);\n",
        ),
    )?;
    write(
        root.path(),
        "src/exact.ts",
        "export const exactUsed = 1; export const exactSelectedDead = 2;\n",
    )?;
    write(
        root.path(),
        "src/exact.tsx",
        "export const exactUsed = 3;\n",
    )?;
    write(
        root.path(),
        "src/runtime.ts",
        "export const runtimeUsed = 1; export const runtimeSelectedDead = 2;\n",
    )?;
    write(
        root.path(),
        "src/runtime.js",
        "export const runtimeUsed = 3;\n",
    )?;
    write(
        root.path(),
        "src/choice.ts",
        "export const fileUsed = 1; export const fileSelectedDead = 2;\n",
    )?;
    write(
        root.path(),
        "src/choice/index.ts",
        "export const fileUsed = 3;\n",
    )?;
    write(
        root.path(),
        "src/pkg/package.json",
        r#"{"private":true,"module":"./module.js","main":"./main.js"}"#,
    )?;
    write(
        root.path(),
        "src/pkg/module.ts",
        "export const packageUsed = 1; export const packageSelectedDead = 2;\n",
    )?;
    write(
        root.path(),
        "src/pkg/main.ts",
        "export const packageUsed = 3;\n",
    )?;
    write(
        root.path(),
        "src/pkg/index.ts",
        "export const packageUsed = 4;\n",
    )?;
    write(
        root.path(),
        "src/fallback/index.ts",
        "export const indexUsed = 1; export const indexSelectedDead = 2;\n",
    )?;
    write(
        root.path(),
        "src/Exact.vue",
        "<template><article>Exact</article></template>\n",
    )?;
    write(
        root.path(),
        "src/Exact.vue.ts",
        "export const vueSubstitutionMustNotWin = 1;\n",
    )?;
    write(
        root.path(),
        "src/decl.d.ts",
        "export type DeclUsed = string; export declare const declarationValueOnly: number;\n",
    )?;
    write(
        root.path(),
        "src/decl.ts",
        "export type DeclUsed = number;\n",
    )?;

    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    assert_eq!(field(&audit.stdout, "status")?, "complete");
    let run_id = field(&audit.stdout, "runId")?;

    let overview = run(root.path(), &["overview", "--run", &run_id])?;
    assert_status(&overview, 0);
    let overview: Value = serde_json::from_str(&overview.stdout)?;
    assert_eq!(
        overview.pointer("/analysisMetrics/jsParseProductCount"),
        Some(&Value::from(14)),
    );

    assert_eq!(
        finding_set(root.path(), &run_id)?,
        BTreeSet::from([
            finding("src/choice.ts", "fileSelectedDead"),
            finding("src/choice/index.ts", "fileUsed"),
            (
                "src/decl.ts".to_owned(),
                "DeclUsed".to_owned(),
                "type".to_owned(),
            ),
            finding("src/exact.ts", "exactSelectedDead"),
            finding("src/exact.tsx", "exactUsed"),
            finding("src/Exact.vue.ts", "vueSubstitutionMustNotWin"),
            finding("src/fallback/index.ts", "indexSelectedDead"),
            finding("src/pkg/index.ts", "packageUsed"),
            finding("src/pkg/main.ts", "packageUsed"),
            finding("src/pkg/module.ts", "packageSelectedDead"),
            finding("src/runtime.js", "runtimeUsed"),
            finding("src/runtime.ts", "runtimeSelectedDead"),
        ])
    );
    Ok(())
}

fn finding(path: &str, name: &str) -> FindingTuple {
    (path.to_owned(), name.to_owned(), "value".to_owned())
}

fn finding_set(
    root: &Path,
    run_id: &str,
) -> Result<BTreeSet<FindingTuple>, Box<dyn std::error::Error>> {
    let output = run(root, &["findings", "--run", run_id, "--area", "dead-code"])?;
    assert_status(&output, 0);
    let response: Value = serde_json::from_str(&output.stdout)?;
    let items = response
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("finding items are missing"))?;
    items
        .iter()
        .map(|item| {
            let path = item
                .pointer("/path/display")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("finding path is missing"))?;
            let name = item
                .get("exportedName")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("finding name is missing"))?;
            let namespace = item
                .get("namespace")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("finding namespace is missing"))?;
            Ok((path.to_owned(), name.to_owned(), namespace.to_owned()))
        })
        .collect()
}

fn write(root: &Path, relative: &str, source: &str) -> std::io::Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, source)
}
