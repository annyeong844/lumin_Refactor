use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

#[cfg(feature = "publication-test-crash")]
pub mod publication;

pub struct ProcessResult {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

pub fn run(root: &Path, arguments: &[&str]) -> Result<ProcessResult, Box<dyn std::error::Error>> {
    run_with_env(root, arguments, &[])
}

pub fn run_with_env(
    root: &Path,
    arguments: &[&str],
    environment: &[(&str, &str)],
) -> Result<ProcessResult, Box<dyn std::error::Error>> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lumin"));
    command.current_dir(root).args(arguments);
    for (name, value) in environment {
        command.env(name, value);
    }
    let output = command.output()?;

    // Corpus child marker: after Command::output returns, if both env vars are
    // set, append the row ID + newline to the marker file.
    let corpus_row = std::env::var("LUMIN_CORPUS_ROW");
    let corpus_marker = std::env::var("LUMIN_CORPUS_CHILD_MARKER");
    match (corpus_row.as_deref(), corpus_marker.as_deref()) {
        (Ok(row), Ok(marker_path)) => {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(marker_path)?;
            writeln!(file, "{row}")?;
        }
        (Ok(_), Err(_)) | (Err(_), Ok(_)) => {
            return Err(std::io::Error::other(
                "LUMIN_CORPUS_ROW and LUMIN_CORPUS_CHILD_MARKER must both be set or both unset",
            )
            .into());
        }
        (Err(_), Err(_)) => {}
    }

    Ok(ProcessResult {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8(output.stdout)?,
        stderr: String::from_utf8(output.stderr)?,
    })
}

pub fn assert_status(result: &ProcessResult, expected: i32) {
    assert_eq!(
        result.status, expected,
        "stdout={}\nstderr={}",
        result.stdout, result.stderr
    );
}

pub fn field(json: &str, name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let value: Value = serde_json::from_str(json)?;
    value
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other(format!("missing string field {name}")).into())
}
