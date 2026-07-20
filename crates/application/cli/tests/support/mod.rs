use std::path::Path;
use std::process::Command;

use serde_json::Value;

pub struct ProcessResult {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

pub fn run(root: &Path, arguments: &[&str]) -> Result<ProcessResult, Box<dyn std::error::Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_lumin"))
        .current_dir(root)
        .args(arguments)
        .output()?;
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
