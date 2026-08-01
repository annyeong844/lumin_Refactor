use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

type FindingTuple = (String, String, String);

#[test]
fn declaration_facts_satisfy_type_space_only() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "src/types.d.ts",
        "export type TypeOnly = string; export declare const RuntimeOnly: number;\n",
    )?;
    write(
        root.path(),
        "src/type-user.ts",
        "import type { TypeOnly } from './types.js'; const x: TypeOnly = 'x'; console.log(x);\n",
    )?;
    write(
        root.path(),
        "src/value-user.ts",
        "import { RuntimeOnly } from './types.js'; console.log(RuntimeOnly);\n",
    )?;
    write(
        root.path(),
        "src/main.ts",
        "import './type-user.js'; import './value-user.js';\n",
    )?;
    write(root.path(), "src/unrelated.ts", "export const dead = 1;\n")?;

    let (run_id, overview) = audit_overview(root.path(), "incomplete")?;
    let limitations = overview
        .get("limitations")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("limitations are missing"))?;
    assert_eq!(limitations.len(), 1);
    assert_eq!(
        limitations[0].get("reason").and_then(Value::as_str),
        Some("internal-specifier-unresolved")
    );
    assert_eq!(
        limitations[0].get("specifier").and_then(Value::as_str),
        Some("./types.js")
    );
    let candidates = limitations[0]
        .get("candidates")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("unresolved candidates are missing"))?;
    assert!(candidates.iter().all(|candidate| {
        candidate
            .as_str()
            .is_some_and(|path| !path.ends_with("types.d.ts"))
    }));

    let findings = finding_set(root.path(), &run_id)?;
    assert_eq!(
        findings,
        BTreeSet::from([(
            "src/unrelated.ts".to_owned(),
            "dead".to_owned(),
            "value".to_owned(),
        )])
    );
    assert!(findings.iter().all(|(path, _, _)| !path.ends_with(".d.ts")));
    Ok(())
}

#[test]
fn next_route_group_characters_are_ordinary_path_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "app/(doc)/layout.tsx",
        "export const used = 1; export const deadSibling = 2;\n",
    )?;
    write(
        root.path(),
        "main.ts",
        "import { used } from './app/(doc)/layout.js'; console.log(used);\n",
    )?;

    let (run_id, overview) = audit_overview(root.path(), "complete")?;
    assert_eq!(
        overview.get("limitationCount").and_then(Value::as_u64),
        Some(0)
    );
    assert_eq!(
        finding_set(root.path(), &run_id)?,
        BTreeSet::from([(
            "app/(doc)/layout.tsx".to_owned(),
            "deadSibling".to_owned(),
            "value".to_owned(),
        )])
    );
    Ok(())
}

fn audit_overview(
    root: &Path,
    expected_status: &str,
) -> Result<(String, Value), Box<dyn std::error::Error>> {
    let audit = run(root, &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    assert_eq!(field(&audit.stdout, "status")?, expected_status);
    let run_id = field(&audit.stdout, "runId")?;
    let overview = run(root, &["overview", "--run", &run_id])?;
    assert_status(&overview, 0);
    Ok((run_id, serde_json::from_str(&overview.stdout)?))
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
