use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

type FindingTuple = (String, String, String);

#[test]
fn tsconfig_aliases_follow_exact_wildcard_base_url_and_extends_precedence()
-> Result<(), Box<dyn std::error::Error>> {
    let supported = tempfile::tempdir()?;
    write(
        supported.path(),
        "configs/base.json",
        r#"{
            "compilerOptions": {
                "baseUrl": "../shared",
                "paths": {"@parent/*": ["mapped/*"]}
            }
        }"#,
    )?;
    write(
        supported.path(),
        "parent/tsconfig.json",
        r#"{"extends":"../configs/base.json"}"#,
    )?;
    write(
        supported.path(),
        "parent/main.ts",
        "import { parentUsed } from '@parent/value'; console.log(parentUsed);\n",
    )?;
    write(
        supported.path(),
        "shared/mapped/value.ts",
        concat!(
            "export const parentUsed = 1;\n",
            "export const childParentUsed = 2;\n",
            "export const parentSelectedDead = 3;\n",
        ),
    )?;

    write(
        supported.path(),
        "child/tsconfig.json",
        r#"{
            "extends": "../configs/base.json",
            "compilerOptions": {
                "paths": {
                    "@pick/exact": ["targets/missing", "targets/exact"],
                    "@pick/*": ["targets/general/*"],
                    "@pick/special/*": ["targets/specific/*"],
                    "@tie/*": ["targets/tie-first/*"],
                    "@tie/*tail": ["targets/tie-second/*"],
                    "@fall/*": ["missing/*"]
                }
            }
        }"#,
    )?;
    write(
        supported.path(),
        "child/main.ts",
        concat!(
            "import { exactUsed } from '@pick/exact';\n",
            "import { specificUsed } from '@pick/special/value';\n",
            "import { tieUsed } from '@tie/valuetail';\n",
            "import { fallbackUsed } from '@fall/value';\n",
            "import { baseUsed } from 'base-only';\n",
            "import { childParentUsed } from '@parent/value';\n",
            "console.log(exactUsed, specificUsed, tieUsed, fallbackUsed, baseUsed, childParentUsed);\n",
        ),
    )?;
    module(
        supported.path(),
        "child/targets/exact.ts",
        "exactUsed",
        "exactSelectedDead",
    )?;
    module(
        supported.path(),
        "child/targets/general/exact.ts",
        "exactUsed",
        "exactWildcardDecoyDead",
    )?;
    module(
        supported.path(),
        "child/targets/specific/value.ts",
        "specificUsed",
        "specificSelectedDead",
    )?;
    module(
        supported.path(),
        "child/targets/general/special/value.ts",
        "specificUsed",
        "generalWildcardDecoyDead",
    )?;
    module(
        supported.path(),
        "child/targets/tie-first/valuetail.ts",
        "tieUsed",
        "tieSelectedDead",
    )?;
    module(
        supported.path(),
        "child/targets/tie-second/value.ts",
        "tieUsed",
        "laterTieDecoyDead",
    )?;
    module(
        supported.path(),
        "shared/@fall/value.ts",
        "fallbackUsed",
        "fallbackSelectedDead",
    )?;
    module(
        supported.path(),
        "shared/base-only.ts",
        "baseUsed",
        "baseSelectedDead",
    )?;
    module(
        supported.path(),
        "shared/@parent/value.ts",
        "childParentUsed",
        "childParentSelectedDead",
    )?;

    let (run_id, overview) = audit_overview(supported.path(), "complete")?;
    assert_eq!(
        overview.get("limitationCount").and_then(Value::as_u64),
        Some(0)
    );
    assert_eq!(
        finding_set(supported.path(), &run_id)?,
        BTreeSet::from([
            finding("child/targets/exact.ts", "exactSelectedDead"),
            finding("child/targets/general/exact.ts", "exactUsed"),
            finding("child/targets/general/exact.ts", "exactWildcardDecoyDead"),
            finding(
                "child/targets/general/special/value.ts",
                "generalWildcardDecoyDead"
            ),
            finding("child/targets/general/special/value.ts", "specificUsed"),
            finding("child/targets/specific/value.ts", "specificSelectedDead"),
            finding("child/targets/tie-first/valuetail.ts", "tieSelectedDead"),
            finding("child/targets/tie-second/value.ts", "laterTieDecoyDead"),
            finding("child/targets/tie-second/value.ts", "tieUsed"),
            finding("shared/@fall/value.ts", "fallbackSelectedDead"),
            finding("shared/@parent/value.ts", "childParentSelectedDead"),
            finding("shared/base-only.ts", "baseSelectedDead"),
            finding("shared/mapped/value.ts", "childParentUsed"),
            finding("shared/mapped/value.ts", "parentSelectedDead"),
        ])
    );

    let unsupported = tempfile::tempdir()?;
    write(
        unsupported.path(),
        "tsconfig.json",
        r#"{
            "compilerOptions": {
                "baseUrl": "/rooted",
                "paths": {"@bad/*": ["/outside/*"]}
            }
        }"#,
    )?;
    write(
        unsupported.path(),
        "main.ts",
        "import { value } from '@bad/value'; console.log(value);\n",
    )?;

    let (_, overview) = audit_overview(unsupported.path(), "incomplete")?;
    let limitations = overview
        .get("limitations")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("limitations are missing"))?;
    assert_eq!(limitations.len(), 2);
    assert_eq!(
        limitations
            .iter()
            .map(|limitation| {
                (
                    limitation.get("reason").and_then(Value::as_str),
                    limitation.get("path").and_then(Value::as_str),
                    limitation.get("detail").and_then(Value::as_str),
                )
            })
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            (
                Some("tsconfig-semantics-unsupported"),
                Some("tsconfig.json"),
                Some("paths target /outside/* uses rooted syntax"),
            ),
            (
                Some("tsconfig-semantics-unsupported"),
                Some("tsconfig.json"),
                Some("rooted baseUrl syntax is unsupported"),
            ),
        ])
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

fn module(root: &Path, relative: &str, used: &str, dead: &str) -> std::io::Result<()> {
    write(
        root,
        relative,
        &format!("export const {used} = 1; export const {dead} = 2;\n"),
    )
}

fn write(root: &Path, relative: &str, source: &str) -> std::io::Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, source)
}
