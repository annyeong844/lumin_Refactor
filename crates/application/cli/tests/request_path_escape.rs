use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

#[test]
fn request_path_escape_distinguishes_malformed_stale_and_blocked_containment()
-> Result<(), Box<dyn std::error::Error>> {
    assert_caller_escape_allocates_no_state()?;
    assert_admitted_entry_escape_is_stale()?;
    assert_planned_new_path_escape_is_denied()?;
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
