use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::process::{Output, Stdio};

use serde_json::Value;

mod command;
pub(crate) mod determinism;

pub use command::lumin_command;

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
    let arguments = arguments.iter().map(OsString::from).collect::<Vec<_>>();
    if environment.is_empty() {
        run_os_with_stdin(root, &arguments, &[])
    } else {
        run_os_with_stdin_and_env(root, &arguments, None, environment)
    }
}

pub fn run_os_with_stdin(
    root: &Path,
    arguments: &[OsString],
    stdin: &[u8],
) -> Result<ProcessResult, Box<dyn std::error::Error>> {
    run_os_with_stdin_and_env(root, arguments, Some(stdin), &[])
}

fn run_os_with_stdin_and_env(
    root: &Path,
    arguments: &[OsString],
    stdin: Option<&[u8]>,
    environment: &[(&str, &str)],
) -> Result<ProcessResult, Box<dyn std::error::Error>> {
    let effective_arguments = determinism::effective_arguments(arguments)?;
    let mut command = lumin_command(root)?;
    command.args(&effective_arguments);
    for (name, value) in environment {
        command.env(name, value);
    }
    let output = if let Some(stdin) = stdin {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("child stdin pipe is missing"))?
            .write_all(stdin)?;
        child.wait_with_output()?
    } else {
        command.output()?
    };
    finish_process_output(root, &effective_arguments, output)
}

pub(crate) fn finish_process_output(
    root: &Path,
    effective_arguments: &[OsString],
    output: Output,
) -> Result<ProcessResult, Box<dyn std::error::Error>> {
    let command_succeeded = output.status.success();
    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;

    determinism::record_semantic_evidence(root, effective_arguments, command_succeeded, &stdout)?;

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
        stdout,
        stderr,
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
