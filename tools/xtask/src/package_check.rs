use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

mod platform;
mod skills;

const BINARY_ENVIRONMENT: &str = "LUMIN_PACKAGE_BINARY";
const MIGRATION_RESPONSE: &str = concat!(
    "{\"schemaVersion\":\"lumin.lifecycle-store-migration.v1\",",
    "\"storeSchema\":\"lumin-lifecycle-store-header.v13\",",
    "\"status\":\"ready\"}",
);

pub(crate) fn run(arguments: &[String]) -> ExitCode {
    let result = match arguments {
        [target] if target == "skills" => skills::check(),
        [target] if target == "windows-x64" || target == "linux-x64" => platform::check(target),
        _ => {
            eprintln!("[TOOL ERROR] usage: lumin-xtask package-check windows-x64|linux-x64|skills");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => {
            println!("package-check {}: PASS", arguments[0]);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("[FAIL] {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_binary(
    binary: &Path,
    root: &Path,
    arguments: &[&str],
) -> Result<std::process::Output, String> {
    let mut command = Command::new(binary);
    command.env_clear().current_dir(root).args(arguments);
    #[cfg(windows)]
    command.env(
        "SystemRoot",
        std::env::var_os("SystemRoot")
            .ok_or_else(|| "SystemRoot is required to launch lumin on Windows".to_owned())?,
    );
    command.output().map_err(|error| {
        format!(
            "cannot execute packaged lumin {}: {error}",
            binary.display()
        )
    })
}

fn expect_success(
    output: Result<std::process::Output, String>,
    command: &str,
) -> Result<std::process::Output, String> {
    let output = output?;
    expect_status(&output, Some(0), command)?;
    if !output.stderr.is_empty() {
        return Err(format!(
            "{command} wrote stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(output)
}

fn expect_status(
    output: &std::process::Output,
    expected: Option<i32>,
    command: &str,
) -> Result<(), String> {
    if output.status.code() != expected {
        return Err(format!(
            "{command} exited {:?}; stdout={} stderr={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

fn parse_json(label: &str, bytes: &[u8]) -> Result<Value, String> {
    serde_json::from_slice(bytes).map_err(|error| format!("{label} returned invalid JSON: {error}"))
}

fn expect_string(value: &Value, pointer: &str, expected: &str) -> Result<(), String> {
    let observed = value
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("response omitted string field {pointer}"))?;
    if observed != expected {
        return Err(format!(
            "response field {pointer} was {observed:?}; expected {expected:?}"
        ));
    }
    Ok(())
}

fn validate_help_output(bytes: &[u8]) -> Result<(), String> {
    let stdout = String::from_utf8(bytes.to_vec())
        .map_err(|error| format!("packaged lumin help-agent returned non-UTF-8 output: {error}"))?;
    for required in [
        "Lumin agent workflow",
        "lumin operation show <operation-id> --format json",
        "lumin store migrate --format json",
        MIGRATION_RESPONSE,
        "Never read or modify .lumin.",
    ] {
        if !stdout.contains(required) {
            return Err(format!(
                "packaged lumin help-agent omitted required contract text: {required}"
            ));
        }
    }
    Ok(())
}

fn locate_binary(workspace: &Path) -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os(BINARY_ENVIRONMENT) {
        return canonical_binary(PathBuf::from(path));
    }
    let suffix = std::env::consts::EXE_SUFFIX;
    let mut candidates = Vec::new();
    if let Some(target) = std::env::var_os("CARGO_TARGET_DIR") {
        let target = PathBuf::from(target);
        candidates.push(target.join("release").join(format!("lumin{suffix}")));
        candidates.push(target.join("debug").join(format!("lumin{suffix}")));
    }
    candidates.push(
        workspace
            .join("target")
            .join("release")
            .join(format!("lumin{suffix}")),
    );
    candidates.push(
        workspace
            .join("target")
            .join("debug")
            .join(format!("lumin{suffix}")),
    );
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .map(canonical_binary)
        .transpose()?
        .ok_or_else(|| {
            format!("a built lumin binary is required; set {BINARY_ENVIRONMENT} to its exact path")
        })
}

fn canonical_binary(path: PathBuf) -> Result<PathBuf, String> {
    path.canonicalize().map_err(|error| {
        format!(
            "cannot open packaged lumin binary {}: {error}",
            path.display()
        )
    })
}

fn scratch_directory_for(kind: &str) -> Result<PathBuf, String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock precedes Unix epoch: {error}"))?
        .as_nanos();
    Ok(std::env::temp_dir().join(format!(
        "lumin-package-check-{kind}-{}-{nonce}",
        std::process::id()
    )))
}

#[cfg(test)]
mod tests;
