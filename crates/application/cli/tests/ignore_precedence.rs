use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
mod support;
use support::{assert_status, field, run};

#[test]
fn ignore_precedence_follows_section_3_1_scan_admission() -> Result<(), Box<dyn std::error::Error>>
{
    authored_directories_and_non_gitignore_excludes()?;
    nested_gitignore_negation()?;
    explicit_include_readmits_gitignored()?;
    exclusion_wins_over_inclusion()?;
    Ok(())
}

fn authored_directories_and_non_gitignore_excludes() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    for (path, name) in [
        ("target/a.ts", "targetDead"),
        ("build/b.ts", "buildDead"),
        ("coverage/c.ts", "coverageDead"),
        ("src/global.ts", "globalDead"),
        ("node_modules/dep/index.ts", "dependencyDead"),
    ] {
        write(root.path(), path, &format!("export const {name} = 1;\n"))?;
    }
    write(root.path(), "main.ts", "console.log('main');\n")?;
    write(root.path(), ".git/info/exclude", "src/global.ts\n")?;
    assert_eq!(
        finding_paths(root.path(), &[])?,
        BTreeSet::from([
            "build/b.ts".to_owned(),
            "coverage/c.ts".to_owned(),
            "src/global.ts".to_owned(),
            "target/a.ts".to_owned(),
        ])
    );
    Ok(())
}

fn nested_gitignore_negation() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(root.path(), ".gitignore", "src/*.ts\n")?;
    write(root.path(), "src/.gitignore", "!keep.ts\n")?;
    write(root.path(), "src/keep.ts", "export const keep = 1;\n")?;
    write(root.path(), "src/dropped.ts", "export const dropped = 1;\n")?;
    write(root.path(), "main.ts", "console.log('main');\n")?;
    assert_eq!(
        finding_paths(root.path(), &[])?,
        BTreeSet::from(["src/keep.ts".to_owned()])
    );
    Ok(())
}

fn explicit_include_readmits_gitignored() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(root.path(), ".gitignore", "src/reincluded.ts\n")?;
    write(root.path(), "src/main.ts", "console.log('main');\n")?;
    write(
        root.path(),
        "src/reincluded.ts",
        "export const readmitted = 1;\n",
    )?;
    assert_eq!(
        finding_paths(root.path(), &["--include", "src/**"])?,
        BTreeSet::from(["src/reincluded.ts".to_owned()])
    );
    Ok(())
}

fn exclusion_wins_over_inclusion() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "src/included.ts",
        "export const included = 1;\n",
    )?;
    write(
        root.path(),
        "src/excluded.ts",
        "export const excluded = 1;\n",
    )?;
    assert_eq!(
        finding_paths(
            root.path(),
            &["--include", "src/**", "--exclude", "src/excluded.ts"]
        )?,
        BTreeSet::from(["src/included.ts".to_owned()])
    );
    Ok(())
}

fn finding_paths(
    root: &Path,
    extra: &[&str],
) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
    let mut args = vec!["audit", "--jobs", "1"];
    args.extend_from_slice(extra);
    let audit = run(root, &args)?;
    assert_status(&audit, 0);
    let audit_json: Value = serde_json::from_str(&audit.stdout)?;
    assert_eq!(
        audit_json.get("limitationCount").and_then(Value::as_u64),
        Some(0)
    );
    let run_id = field(&audit.stdout, "runId")?;
    let output = run(root, &["findings", "--run", &run_id, "--area", "dead-code"])?;
    assert_status(&output, 0);
    let value: Value = serde_json::from_str(&output.stdout)?;
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("items missing"))?;
    let paths = items
        .iter()
        .map(|item| {
            item.pointer("/path/display")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| std::io::Error::other("finding path missing"))
        })
        .collect::<Result<BTreeSet<_>, std::io::Error>>()?;
    Ok(paths)
}

fn write(root: &Path, relative: &str, content: &str) -> std::io::Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)
}
