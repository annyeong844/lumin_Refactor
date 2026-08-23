use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

mod support;

use support::{assert_status, field, lumin_command, run};

#[test]
fn request_path_escape_distinguishes_malformed_stale_and_blocked_containment()
-> Result<(), Box<dyn std::error::Error>> {
    assert_caller_escape_allocates_no_state()?;
    assert_admitted_entry_escape_is_stale()?;
    assert_planned_new_path_escape_is_denied()?;
    assert_capture_escape_uses_opening_lease_kind()?;
    assert_final_escape_preserves_new_file_kind()?;
    Ok(())
}

fn assert_caller_escape_allocates_no_state() -> Result<(), Box<dyn std::error::Error>> {
    let root = source_fixture()?;
    let lexical = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-lexical-escape",
            "--path",
            "../outside.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&lexical, 2);
    assert!(lexical.stdout.is_empty());
    assert!(!root.path().join(".lumin").exists());

    let outside = tempfile::tempdir()?;
    let alias = root.path().join("outside-alias");
    create_directory_alias(outside.path(), &alias)?;
    let physical = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-physical-escape",
            "--path",
            "outside-alias/new.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&physical, 2);
    assert!(physical.stdout.is_empty());
    assert!(physical.stderr.contains("resolves outside repository root"));
    assert!(!root.path().join(".lumin").exists());
    remove_directory_alias(&alias)?;
    Ok(())
}

fn assert_admitted_entry_escape_is_stale() -> Result<(), Box<dyn std::error::Error>> {
    let root = source_fixture()?;
    fs::create_dir(root.path().join("inside"))?;
    fs::write(
        root.path().join("inside/entry.ts"),
        "export const location = 'inside';\n",
    )?;
    let alias = root.path().join("entry-alias");
    create_directory_alias(&root.path().join("inside"), &alias)?;

    let opened = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-entry-open",
            "--path",
            "src/main.ts",
            "--entry",
            "entry-alias/entry.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);
    let gate_id = field(&opened.stdout, "gateId")?;

    let outside = tempfile::tempdir()?;
    fs::write(
        outside.path().join("entry.ts"),
        "export const location = 'outside';\n",
    )?;
    remove_directory_alias(&alias)?;
    create_directory_alias(outside.path(), &alias)?;

    let closed = run(
        root.path(),
        &["post-write", &gate_id, "--operation-id", "op-entry-close"],
    )?;
    assert_status(&closed, 5);
    let closed_json: Value = serde_json::from_str(&closed.stdout)?;
    assert_eq!(required_string(&closed_json, "/decision")?, "stale");
    assert_eq!(required_string(&closed_json, "/lifecycle")?, "active");
    assert_eq!(
        required_string(&closed_json, "/observationBinding/state")?,
        "unsealed"
    );
    assert_eq!(
        required_string(&closed_json, "/observationBinding/reason")?,
        "protected-input-changed"
    );
    assert_signal_path(
        &closed_json,
        "protected-input-changed",
        "entry-alias/entry.ts",
    )?;
    assert!(closed_json.get("actualWriteSet").is_none());

    assert_committed_result(
        root.path(),
        "op-entry-close",
        "stale",
        "protected-input-changed",
    )?;
    assert_active_revision(root.path(), &gate_id, 1, "stale")?;
    remove_directory_alias(&alias)?;
    Ok(())
}

fn assert_planned_new_path_escape_is_denied() -> Result<(), Box<dyn std::error::Error>> {
    let root = source_fixture()?;
    fs::create_dir(root.path().join("planned"))?;
    let opened = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-planned-open",
            "--path",
            "planned/new.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);
    let gate_id = field(&opened.stdout, "gateId")?;

    fs::remove_dir(root.path().join("planned"))?;
    let outside = tempfile::tempdir()?;
    fs::write(
        outside.path().join("new.ts"),
        "export const escaped = true;\n",
    )?;
    let alias = root.path().join("planned");
    create_directory_alias(outside.path(), &alias)?;

    let closed = run(
        root.path(),
        &["post-write", &gate_id, "--operation-id", "op-planned-close"],
    )?;
    assert_status(&closed, 3);
    let closed_json: Value = serde_json::from_str(&closed.stdout)?;
    assert_eq!(required_string(&closed_json, "/decision")?, "deny");
    assert_eq!(required_string(&closed_json, "/lifecycle")?, "active");
    assert_eq!(
        required_string(&closed_json, "/observationBinding/state")?,
        "unsealed"
    );
    assert_eq!(
        required_string(&closed_json, "/observationBinding/reason")?,
        "planned-path-containment-violation"
    );
    assert_signal_path(
        &closed_json,
        "planned-path-containment-violation",
        "planned/new.ts",
    )?;
    assert!(closed_json.get("actualWriteSet").is_none());

    assert_committed_result(
        root.path(),
        "op-planned-close",
        "deny",
        "planned-path-containment-violation",
    )?;
    assert_active_revision(root.path(), &gate_id, 1, "deny")?;
    remove_directory_alias(&alias)?;
    Ok(())
}

fn assert_capture_escape_uses_opening_lease_kind() -> Result<(), Box<dyn std::error::Error>> {
    let root = source_fixture()?;
    let opened = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-capture-escape-open",
            "--path",
            "src/main.ts",
            "--entry",
            "src/main.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);
    let gate_id = field(&opened.stdout, "gateId")?;

    let (child, mut barrier) = post_write_at_barrier(
        root.path(),
        &gate_id,
        "op-capture-escape-close",
        "LUMIN_TEST_GATE_POSTWRITE_CAPTURE_BARRIER",
        "close-capturing",
    )?;
    let outside = tempfile::tempdir()?;
    fs::write(
        outside.path().join("main.ts"),
        "export const escaped = true;\n",
    )?;
    fs::remove_dir_all(root.path().join("src"))?;
    let alias = root.path().join("src");
    create_directory_alias(outside.path(), &alias)?;
    release_barrier(&mut barrier)?;

    let output = child.wait_with_output()?;
    assert_eq!(
        output.status.code(),
        Some(5),
        "unexpected capture-escape result: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(required_string(&response, "/decision")?, "stale");
    assert_eq!(required_string(&response, "/lifecycle")?, "active");
    assert_eq!(
        required_string(&response, "/observationBinding/reason")?,
        "protected-input-changed"
    );
    assert_signal_path(&response, "protected-input-changed", "src/main.ts")?;
    assert!(
        response
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| !signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some("analysis-failed")
            }))
    );
    assert_committed_result(
        root.path(),
        "op-capture-escape-close",
        "stale",
        "protected-input-changed",
    )?;
    assert_active_revision(root.path(), &gate_id, 1, "stale")?;
    remove_directory_alias(&alias)?;
    Ok(())
}

fn assert_final_escape_preserves_new_file_kind() -> Result<(), Box<dyn std::error::Error>> {
    let root = source_fixture()?;
    fs::create_dir(root.path().join("planned"))?;
    let opened = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-final-escape-open",
            "--path",
            "planned/new.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);
    let gate_id = field(&opened.stdout, "gateId")?;
    fs::write(
        root.path().join("planned/new.ts"),
        "export const created = true;\n",
    )?;

    let (child, mut barrier) = post_write_at_barrier(
        root.path(),
        &gate_id,
        "op-final-escape-close",
        "LUMIN_TEST_GATE_POSTWRITE_FINAL_BARRIER",
        "close-finalizing",
    )?;
    let outside = tempfile::tempdir()?;
    fs::write(
        outside.path().join("new.ts"),
        "export const escaped = true;\n",
    )?;
    fs::remove_file(root.path().join("planned/new.ts"))?;
    fs::remove_dir(root.path().join("planned"))?;
    let alias = root.path().join("planned");
    create_directory_alias(outside.path(), &alias)?;
    release_barrier(&mut barrier)?;

    let output = child.wait_with_output()?;
    assert_eq!(
        output.status.code(),
        Some(3),
        "unexpected final-escape result: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(required_string(&response, "/decision")?, "deny");
    assert_eq!(required_string(&response, "/lifecycle")?, "active");
    assert_eq!(
        required_string(&response, "/observationBinding/reason")?,
        "planned-path-containment-violation"
    );
    assert_signal_path(
        &response,
        "planned-path-containment-violation",
        "planned/new.ts",
    )?;
    assert_committed_result(
        root.path(),
        "op-final-escape-close",
        "deny",
        "planned-path-containment-violation",
    )?;
    assert_active_revision(root.path(), &gate_id, 1, "deny")?;
    remove_directory_alias(&alias)?;
    Ok(())
}

fn post_write_at_barrier(
    root: &Path,
    gate_id: &str,
    operation_id: &str,
    environment: &str,
    expected_stage: &str,
) -> Result<(Child, TcpStream), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let mut child = lumin_command(root)?
        .args(["post-write", gate_id, "--operation-id", operation_id])
        .env(environment, listener.local_addr()?.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let started = Instant::now();
    let (stream, peer) = loop {
        match listener.accept() {
            Ok(accepted) => break accepted,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if let Some(status) = child.try_wait()? {
                    return Err(std::io::Error::other(format!(
                        "post-write exited before {expected_stage}: {status}"
                    ))
                    .into());
                }
                if started.elapsed() >= Duration::from_secs(30) {
                    return Err(std::io::Error::other(format!(
                        "post-write did not reach {expected_stage}"
                    ))
                    .into());
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    };
    assert!(peer.ip().is_loopback());
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let mut frame = String::new();
    BufReader::new(stream.try_clone()?).read_line(&mut frame)?;
    assert_eq!(
        frame.trim_end(),
        format!("{expected_stage} {operation_id} {gate_id}")
    );
    Ok((child, stream))
}

fn release_barrier(stream: &mut TcpStream) -> Result<(), Box<dyn std::error::Error>> {
    stream.write_all(b"release\n")?;
    stream.flush()?;
    Ok(())
}

fn assert_committed_result(
    root: &Path,
    operation_id: &str,
    decision: &str,
    signal_kind: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let shown = run(root, &["operation", "show", operation_id])?;
    assert_status(&shown, 0);
    let shown: Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(required_string(&shown, "/status")?, "committed");
    assert_eq!(required_string(&shown, "/result/decision")?, decision);
    assert!(
        shown
            .pointer("/result/signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals
                .iter()
                .any(|signal| { signal.get("kind").and_then(Value::as_str) == Some(signal_kind) }))
    );
    Ok(())
}

fn assert_active_revision(
    root: &Path,
    gate_id: &str,
    revision: u64,
    decision: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let shown = run(root, &["gate", "show", gate_id])?;
    assert_status(&shown, 0);
    let shown: Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(required_string(&shown, "/lifecycle")?, "active");
    assert_eq!(
        shown.get("currentRevision").and_then(Value::as_u64),
        Some(revision)
    );
    assert_eq!(
        required_string(&shown, &format!("/revisions/{revision}/decision"))?,
        decision
    );
    Ok(())
}

fn assert_signal_path(
    value: &Value,
    kind: &str,
    expected_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let signals = value
        .get("signals")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("gate result omitted signals"))?;
    let signal = signals
        .iter()
        .find(|signal| signal.get("kind").and_then(Value::as_str) == Some(kind))
        .ok_or_else(|| std::io::Error::other(format!("missing {kind} signal")))?;
    assert!(
        signal
            .get("paths")
            .and_then(Value::as_array)
            .is_some_and(|paths| paths.iter().any(|path| {
                path.get("display").and_then(Value::as_str) == Some(expected_path)
            }))
    );
    Ok(())
}

fn required_string<'a>(
    value: &'a Value,
    pointer: &str,
) -> Result<&'a str, Box<dyn std::error::Error>> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other(format!("missing string at {pointer}")).into())
}

fn source_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(root.path().join("src/main.ts"), "export const value = 1;\n")?;
    Ok(root)
}

#[cfg(unix)]
fn create_directory_alias(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(unix)]
fn remove_directory_alias(link: &Path) -> std::io::Result<()> {
    fs::remove_file(link)
}

#[cfg(windows)]
fn create_directory_alias(target: &Path, link: &Path) -> std::io::Result<()> {
    let status = std::process::Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "mklink /J failed with {status}"
        )))
    }
}

#[cfg(windows)]
fn remove_directory_alias(link: &Path) -> std::io::Result<()> {
    fs::remove_dir(link)
}
