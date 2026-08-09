#![cfg(unix)]

use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};

#[test]
fn closed_stdout_consumer_does_not_abort_the_public_cli() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    let (consumer, producer) = UnixStream::pair()?;
    drop(consumer);
    let producer: OwnedFd = producer.into();

    let output = Command::new(env!("CARGO_BIN_EXE_lumin"))
        .current_dir(root.path())
        .arg("capabilities")
        .stdout(Stdio::from(producer))
        .stderr(Stdio::piped())
        .output()?;

    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    Ok(())
}
