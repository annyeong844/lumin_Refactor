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
    let contained_base_url = config_path(&unsupported.path().join("rooted"))?;
    let contained_paths_target = config_path(&unsupported.path().join("outside").join("*"))?;
    let unsupported_config = serde_json::json!({
        "compilerOptions": {
            "baseUrl": contained_base_url,
            "paths": {"@bad/*": [contained_paths_target]}
        }
    });
    write(
        unsupported.path(),
        "tsconfig.json",
        &serde_json::to_string_pretty(&unsupported_config)?,
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
    assert!(limitations.iter().all(|limitation| {
        limitation.get("reason").and_then(Value::as_str) == Some("tsconfig-semantics-unsupported")
            && limitation.get("path").and_then(Value::as_str) == Some("tsconfig.json")
    }));
    let details = limitations
        .iter()
        .filter_map(|limitation| limitation.get("detail").and_then(Value::as_str))
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        details,
        BTreeSet::from([
            "rooted baseUrl syntax is unsupported".to_owned(),
            "paths target uses rooted syntax".to_owned(),
        ])
    );

    let escaping_alias = tempfile::tempdir()?;
    write(
        escaping_alias.path(),
        "tsconfig.json",
        r#"{"compilerOptions":{"paths":{"pkg/*":["src/*"]}}}"#,
    )?;
    write(
        escaping_alias.path(),
        "main.ts",
        "import { value } from 'pkg/../../../outside'; console.log(value);\n",
    )?;
    let (_, overview) = audit_overview(escaping_alias.path(), "incomplete")?;
    let limitations = overview
        .get("limitations")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("limitations are missing"))?;
    assert_eq!(limitations.len(), 1);
    assert_eq!(
        limitations[0].get("reason").and_then(Value::as_str),
        Some("alias-shape-unsupported")
    );
    assert!(
        limitations[0]
            .get("detail")
            .and_then(Value::as_str)
            .is_some_and(|detail| detail.contains("escapes the canonical repository root"))
    );

    let escaping_config = tempfile::tempdir()?;
    let outside_root = escaping_config
        .path()
        .parent()
        .ok_or_else(|| std::io::Error::other("temporary repository has no parent"))?
        .join("lumin-outside-base");
    let escaping_config_body = serde_json::json!({
        "compilerOptions": {"baseUrl": config_path(&outside_root)?}
    });
    write(
        escaping_config.path(),
        "tsconfig.json",
        &serde_json::to_string_pretty(&escaping_config_body)?,
    )?;
    write(
        escaping_config.path(),
        "main.ts",
        "export const value = 1;\n",
    )?;
    let audit = run(escaping_config.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 1);
    assert!(audit.stdout.is_empty());
    assert!(audit.stderr.contains("baseUrl escapes the repository root"));

    let overview = run(escaping_config.path(), &["overview"])?;
    assert_status(&overview, 0);
    let overview: Value = serde_json::from_str(&overview.stdout)?;
    assert_eq!(
        overview.pointer("/scope/kind").and_then(Value::as_str),
        Some("attempt")
    );
    assert_eq!(
        overview
            .pointer("/latestAttempt/status")
            .and_then(Value::as_str),
        Some("failed")
    );
    assert!(
        overview
            .pointer("/latestAttempt/failure")
            .and_then(Value::as_str)
            .is_some_and(|failure| failure.contains("baseUrl escapes the repository root"))
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

fn config_path(path: &Path) -> Result<String, std::io::Error> {
    path.to_str()
        .map(|path| path.replace('\\', "/"))
        .ok_or_else(|| std::io::Error::other("temporary path is not UTF-8"))
}

fn write(root: &Path, relative: &str, source: &str) -> std::io::Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, source)
}
