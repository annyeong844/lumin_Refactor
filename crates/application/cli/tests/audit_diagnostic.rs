//! Explicit external-binary partition: the probe itself never enables diagnostic/fault
//! features together. Both release payload paths are mandatory, never a skipped test.
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

fn command(binary_env: &str, root: &Path) -> Result<Command, Box<dyn std::error::Error>> {
    let binary = std::env::var_os(binary_env)
        .ok_or_else(|| format!("{binary_env} is required for --test audit_diagnostic"))?;
    let mut command = Command::new(binary);
    command.current_dir(root).env_clear().stdin(Stdio::null());
    #[cfg(windows)]
    command.env(
        "SystemRoot",
        std::env::var_os("SystemRoot").ok_or("SystemRoot is required")?,
    );
    Ok(command)
}

fn fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(
        root.path().join("src/unused.ts"),
        "export const unused = 1;\n",
    )?;
    Ok(root)
}

#[test]
fn actual_release_children_report_concrete_pool_and_unchanged_semantics()
-> Result<(), Box<dyn std::error::Error>> {
    let mut finding_id = None;
    for binary_env in [
        "LUMIN_AUDIT_CONTROL_BINARY",
        "LUMIN_AUDIT_DIAGNOSTIC_BINARY",
    ] {
        for jobs in [None, Some("1")] {
            let root = fixture()?;
            let mut child = command(binary_env, root.path())?;
            child.args(["audit", "--format", "json"]);
            if let Some(jobs) = jobs {
                child.args(["--jobs", jobs]);
            }
            child.stdout(Stdio::piped()).stderr(Stdio::piped());
            let child = child.spawn()?;
            let process_id = child.id();
            let output = child.wait_with_output()?;
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let result: Value = serde_json::from_slice(&output.stdout)?;
            assert_eq!(result["schemaVersion"], "lumin.audit.v2");
            assert_eq!(result["findingCount"], 1);
            assert_eq!(result["limitationCount"], 0);
            if binary_env == "LUMIN_AUDIT_CONTROL_BINARY" {
                assert!(output.stderr.is_empty());
            } else {
                let frame: Value = serde_json::from_slice(&output.stderr)?;
                assert_eq!(
                    frame["schemaVersion"],
                    "lumin.audit-execution-diagnostic.v1"
                );
                assert_eq!(frame["processId"], process_id);
                assert_eq!(frame["attemptId"], result["attemptId"]);
                assert_eq!(frame["runId"], result["runId"]);
                assert_eq!(
                    frame["requestedJobs"],
                    if jobs.is_some() {
                        serde_json::json!(1)
                    } else {
                        Value::Null
                    }
                );
                let observed = frame["observedAvailableParallelism"]
                    .as_u64()
                    .ok_or("missing child parallelism")?;
                assert_eq!(
                    frame["actualJobs"],
                    if jobs.is_some() { 1 } else { observed.min(8) }
                );
                assert_eq!(frame["configuredWorkerStackBytes"], 4_194_304);
                assert!(frame["parallelismObservationError"].is_null());
                assert_eq!(
                    frame["phases"].as_array().ok_or("missing phases")?.len(),
                    23
                );
                let capabilities = command(binary_env, root.path())?
                    .args(["capabilities", "--format", "json"])
                    .output()?;
                assert!(capabilities.status.success());
                assert!(capabilities.stderr.is_empty());
                let capabilities: Value = serde_json::from_slice(&capabilities.stdout)?;
                assert_eq!(frame["buildId"], capabilities["scope"]["buildId"]);
            }
            let run_id = result["runId"].as_str().ok_or("missing run")?;
            let findings = command(binary_env, root.path())?
                .args(["findings", "--run", run_id, "--area", "dead-code"])
                .output()?;
            assert!(findings.status.success());
            assert!(findings.stderr.is_empty());
            let findings: Value = serde_json::from_slice(&findings.stdout)?;
            let item = &findings["items"][0];
            assert_eq!(item["exportedName"], "unused");
            assert_eq!(item["path"]["display"], "src/unused.ts");
            if let Some(expected) = &finding_id {
                assert_eq!(&item["findingId"], expected);
            }
            finding_id = Some(item["findingId"].clone());
        }
    }
    Ok(())
}

#[test]
fn diagnostic_transport_failure_preserves_exactly_one_committed_run()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let (reader, writer) = std::io::pipe()?;
    // Deterministically fail the first diagnostic write after normal stdout flush.
    drop(reader);
    let output = command("LUMIN_AUDIT_DIAGNOSTIC_BINARY", root.path())?
        .args(["audit", "--jobs", "1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::from(writer))
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    let result: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(result["findingCount"], 1);
    let run_id = result["runId"].as_str().ok_or("missing committed run")?;
    let before = command("LUMIN_AUDIT_DIAGNOSTIC_BINARY", root.path())?
        .args(["overview", "--run", run_id])
        .output()?;
    assert!(before.status.success());
    assert!(before.stderr.is_empty());
    let overview: Value = serde_json::from_slice(&before.stdout)?;
    assert_eq!(overview["attemptId"], result["attemptId"]);
    assert_eq!(overview["scope"]["id"], result["runId"]);
    let replay = command("LUMIN_AUDIT_DIAGNOSTIC_BINARY", root.path())?
        .args(["overview", "--run", run_id])
        .output()?;
    assert!(replay.status.success());
    assert!(replay.stderr.is_empty());
    assert_eq!(replay.stdout, before.stdout);
    for (parent, prefix) in [("runs", "run_"), ("attempts", "attempt_")] {
        let names = fs::read_dir(root.path().join(".lumin").join(parent))?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            names
                .iter()
                .filter(|name| name.to_str().is_some_and(|name| name.starts_with(prefix)))
                .count(),
            1
        );
    }
    Ok(())
}

#[test]
fn original_audit_failure_has_no_completed_diagnostic_frame()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let output = command("LUMIN_AUDIT_DIAGNOSTIC_BINARY", root.path())?
        .args(["audit", "--jobs", "0"])
        .output()?;
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.starts_with(b"lumin: "));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("lumin.audit-execution-diagnostic"));
    assert!(!root.path().join(".lumin").exists());
    Ok(())
}
