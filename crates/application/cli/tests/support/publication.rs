use std::fs;
use std::path::Path;

use serde_json::Value;

use super::{ProcessResult, assert_status, run};

pub fn baseline_repository()
-> Result<(tempfile::TempDir, ProcessResult), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(
        root.path().join("src/lib.ts"),
        "export const visible = 1;\n",
    )?;
    let baseline = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&baseline, 0);
    Ok((root, baseline))
}

pub fn json(output: &str) -> Result<Value, Box<dyn std::error::Error>> {
    serde_json::from_str(output).map_err(Into::into)
}

pub fn number(output: &str, field: &str) -> Result<u64, Box<dyn std::error::Error>> {
    json(output)?
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| std::io::Error::other(format!("missing numeric field {field}")).into())
}

pub fn assert_no_attempt_liveness_files(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut names = fs::read_dir(root.join(".lumin"))?
        .map(|entry| {
            entry?
                .file_name()
                .into_string()
                .map_err(|_| std::io::Error::other("state entry name is not UTF-8"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    names.retain(|name| name.starts_with("attempt-liveness-") && name.ends_with(".lock"));
    assert!(names.is_empty(), "attempt liveness files leaked: {names:?}");
    Ok(())
}
