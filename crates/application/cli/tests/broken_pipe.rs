#![cfg(unix)]

use std::fs;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

#[test]
fn closed_stdout_consumer_does_not_abort_the_public_cli() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    let output = run_with_closed_stdout(root.path(), &["capabilities"])?;

    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    Ok(())
}

#[test]
fn closed_mutation_result_pipe_requires_operation_recovery()
-> Result<(), Box<dyn std::error::Error>> {
    const OPERATION_ID: &str = "closed-pipe-prewrite";

    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(root.path().join("src/lib.ts"), "export const value = 1;\n")?;

    let output = run_with_closed_stdout(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            OPERATION_ID,
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_eq!(
        output.status.code(),
        Some(1),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());

    let recovered = run_with_captured_stdout(root.path(), &["operation", "show", OPERATION_ID])?;
    assert_eq!(
        recovered.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    assert!(recovered.stderr.is_empty());
    let recovered: Value = serde_json::from_slice(&recovered.stdout)?;
    assert_eq!(
        recovered.get("kind").and_then(Value::as_str),
        Some("gate-open")
    );
    assert_eq!(
        recovered.get("status").and_then(Value::as_str),
        Some("committed")
    );
    assert_eq!(
        recovered
            .pointer("/result/operationId")
            .and_then(Value::as_str),
        Some(OPERATION_ID)
    );
    assert!(
        recovered
            .pointer("/result/gateId")
            .and_then(Value::as_str)
            .is_some()
    );
    Ok(())
}

fn run_with_closed_stdout(root: &std::path::Path, arguments: &[&str]) -> std::io::Result<Output> {
    let (consumer, producer) = UnixStream::pair()?;
    drop(consumer);
    let producer: OwnedFd = producer.into();

    Command::new(env!("CARGO_BIN_EXE_lumin"))
        .current_dir(root)
        .args(arguments)
        .stdout(Stdio::from(producer))
        .stderr(Stdio::piped())
        .output()
}

fn run_with_captured_stdout(root: &std::path::Path, arguments: &[&str]) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_lumin"))
        .current_dir(root)
        .args(arguments)
        .output()
}
