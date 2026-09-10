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

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum DiagnosticVersion {
    Execution,
    Store,
    // The older two external-binary partitions intentionally never select W7.
    #[allow(dead_code)]
    Lifecycle,
    #[allow(dead_code)]
    Boundary,
}
impl From<bool> for DiagnosticVersion {
    fn from(store: bool) -> Self {
        if store { Self::Store } else { Self::Execution }
    }
}

pub fn actual_release_children_report_concrete_pool_and_unchanged_semantics(
    version: impl Into<DiagnosticVersion>,
) -> Result<(), Box<dyn std::error::Error>> {
    let version = version.into();
    let store = version != DiagnosticVersion::Execution;
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
                    match version {
                        DiagnosticVersion::Execution => "lumin.audit-execution-diagnostic.v1",
                        DiagnosticVersion::Store => "lumin.audit-execution-diagnostic.v2",
                        DiagnosticVersion::Lifecycle => "lumin.audit-execution-diagnostic.v3",
                        DiagnosticVersion::Boundary => "lumin.audit-execution-diagnostic.v4",
                    }
                );
                if store {
                    verify_store_frame(&frame, true)?;
                }
                if matches!(
                    version,
                    DiagnosticVersion::Lifecycle | DiagnosticVersion::Boundary
                ) {
                    verify_lifecycle_frame(&frame, LifecycleFixture::Fresh)?;
                    verify_lifecycle_bytes(&frame, &output.stderr, version)?;
                    if version == DiagnosticVersion::Boundary {
                        verify_boundary_frame(&frame)?;
                    }
                    verify_final_lifecycle_state(root.path(), 1)?;
                }
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
            if store && binary_env == "LUMIN_AUDIT_DIAGNOSTIC_BINARY" {
                let mut existing_command = command(binary_env, root.path())?;
                existing_command.args(["audit", "--format", "json"]);
                if let Some(jobs) = jobs {
                    existing_command.args(["--jobs", jobs]);
                }
                let existing_child = existing_command
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()?;
                let existing_pid = existing_child.id();
                let existing = existing_child.wait_with_output()?;
                assert!(
                    existing.status.success(),
                    "{}",
                    String::from_utf8_lossy(&existing.stderr)
                );
                let frame = serde_json::from_slice(&existing.stderr)?;
                verify_store_frame(&frame, false)?;
                if matches!(
                    version,
                    DiagnosticVersion::Lifecycle | DiagnosticVersion::Boundary
                ) {
                    verify_lifecycle_frame(&frame, LifecycleFixture::Seeded)?;
                    verify_lifecycle_bytes(&frame, &existing.stderr, version)?;
                    if version == DiagnosticVersion::Boundary {
                        verify_boundary_frame(&frame)?;
                    }
                    assert_eq!(frame["processId"], existing_pid);
                    let result: Value = serde_json::from_slice(&existing.stdout)?;
                    assert_eq!(frame["attemptId"], result["attemptId"]);
                    assert_eq!(frame["runId"], result["runId"]);
                    verify_final_lifecycle_state(root.path(), 2)?;
                    lifecycle_pending_recovery(jobs, version)?;
                }
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum LifecycleFixture {
    Fresh,
    Seeded,
    Pending,
}

// Independent of the production enums and runner oracle: exact frozen W7 truth.
fn verify_lifecycle_frame(
    frame: &Value,
    fixture: LifecycleFixture,
) -> Result<(), Box<dyn std::error::Error>> {
    let contexts = [
        "open-recovery-latest",
        "attempt-recover-latest",
        "attempt-directory",
        "attempt-latest",
        "staging-create",
        "staging-move",
        "publish-terminal",
        "finalize-latest",
    ];
    let costs = [
        "namespace-validation",
        "store-handle-open",
        "backend-open",
        "store-validation",
        "read-admission",
        "write-admission",
        "backend-commit",
        "backend-abort",
        "database-explicit-drop",
        "database-return-tail",
        "json-write-flush",
        "publication-move",
        "directory-sync",
    ];
    let mut counts = [
        [1, 1, 0, 0, 0, 0, 1, 0, 0, 0],
        [1, 1, 0, 0, 0, 0, 1, 0, 0, 0],
        [2, 0, 0, 0, 0, 2, 0, 0, 0, 1],
        [2, 1, 1, 1, 0, 0, 2, 1, 1, 1],
        [2, 0, 0, 0, 0, 2, 0, 0, 0, 1],
        [2, 0, 0, 0, 0, 2, 0, 0, 1, 1],
        [2, 0, 0, 0, 0, 2, 0, 1, 1, 1],
        [3, 2, 1, 1, 0, 0, 3, 1, 1, 1],
    ];
    if !matches!(fixture, LifecycleFixture::Fresh) {
        // A canonical document excludes the W8 shortcut on both calls.
        counts[0] = [2, 1, 1, 0, 1, 0, 2, 0, 0, 0];
        counts[1] = [2, 1, 1, 0, 1, 0, 2, 0, 0, 0];
        counts[3] = [4, 3, 1, 1, 0, 0, 4, 1, 1, 1];
        counts[7] = [4, 3, 1, 1, 0, 0, 4, 1, 1, 1];
    }
    if matches!(fixture, LifecycleFixture::Pending) {
        counts[0] = [3, 2, 1, 0, 1, 0, 3, 0, 0, 1];
    }
    let rows = frame["lifecycleContexts"]
        .as_array()
        .ok_or("lifecycle contexts")?;
    assert_eq!(rows.len(), 8);
    for (index, (row, name)) in rows.iter().zip(contexts).enumerate() {
        assert_eq!(row.as_object().ok_or("context object")?.len(), 5);
        assert_eq!(row["context"], name);
        assert_eq!(row["calls"], 1);
        let leaves = row["costs"].as_array().ok_or("costs")?;
        assert_eq!(leaves.len(), 13);
        let mut sum = 0_u64;
        for (leaf, cost) in leaves.iter().zip(costs) {
            assert_eq!(leaf.as_object().ok_or("cost object")?.len(), 3);
            assert_eq!(leaf["cost"], cost);
            let calls = leaf["calls"].as_u64().ok_or("cost calls")?;
            if calls == 0 {
                assert!(leaf["elapsedNanoseconds"].is_null());
            } else {
                sum = sum
                    .checked_add(leaf["elapsedNanoseconds"].as_u64().ok_or("cost time")?)
                    .ok_or("sum overflow")?;
            }
        }
        assert!(leaves[0]["calls"].as_u64().is_some_and(|n| n > 0));
        assert!(leaves[3]["calls"].as_u64().is_some_and(|n| n > 0));
        assert_eq!(leaves[1]["calls"], leaves[2]["calls"]);
        let actual = [2, 4, 5, 6, 7, 8, 9, 10, 11, 12].map(|i| leaves[i]["calls"].as_u64());
        assert_eq!(actual, counts[index].map(Some), "{name}");
        let elapsed = row["elapsedNanoseconds"].as_u64().ok_or("context time")?;
        assert_eq!(row["selfNanoseconds"].as_u64(), elapsed.checked_sub(sum));
        let outer = frame["storePhases"]
            .as_array()
            .ok_or("store phases")?
            .iter()
            .find(|phase| phase["phase"] == name)
            .and_then(|phase| phase["elapsedNanoseconds"].as_u64())
            .ok_or("outer time")?;
        assert!(elapsed <= outer, "{name}");
    }
    Ok(())
}

fn verify_boundary_frame(frame: &Value) -> Result<(), Box<dyn std::error::Error>> {
    let contexts = [
        "open-recovery-enter",
        "open-recovery-exit",
        "attempt-enter",
        "attempt-exit",
        "publish-prepare-enter",
        "publish-prepare-exit",
        "publish-finalize-enter",
        "finalize-release",
        "publish-finalize-exit",
    ];
    let costs = [
        "namespace-validation",
        "store-handle-open",
        "backend-open",
        "store-validation",
        "read-admission",
        "write-admission",
        "backend-commit",
        "backend-abort",
        "database-explicit-drop",
        "database-return-tail",
        "guard-prevalidation",
        "lifecycle-lock-acquire",
        "guard-construction",
        "lifecycle-lock-release",
        "native-store-verification",
        "attempt-lock-validation",
        "attempt-lock-drop",
        "attempt-lock-remove",
        "directory-sync",
    ];
    let expected = [
        [1, 1, 1, 1, 0, 0, 0, 0, 1, 0, 1, 1, 1, 0, 2, 0, 0, 0, 0],
        [1, 1, 1, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0],
        [1, 1, 1, 1, 0, 0, 0, 0, 1, 0, 1, 1, 1, 0, 0, 0, 0, 0, 0],
        [1, 1, 1, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0],
        [1, 1, 1, 1, 0, 0, 0, 0, 1, 0, 1, 1, 1, 0, 0, 0, 0, 0, 0],
        [1, 1, 1, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0],
        [1, 1, 1, 1, 0, 0, 0, 0, 1, 0, 1, 1, 1, 0, 0, 0, 0, 0, 0],
        [1, 3, 3, 1, 1, 2, 2, 0, 1, 2, 0, 0, 0, 0, 0, 3, 1, 1, 1],
        [1, 1, 1, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0],
    ];
    let rows = frame["storeBoundaryContexts"]
        .as_array()
        .ok_or("boundary contexts")?;
    assert_eq!(rows.len(), 9);
    let mut total = [0_u64; 7];
    for ((row, name), expected) in rows.iter().zip(contexts).zip(expected) {
        assert_eq!(row.as_object().ok_or("boundary row")?.len(), 5);
        assert_eq!(row["context"], name);
        assert_eq!(row["calls"], 1);
        let leaves = row["costs"].as_array().ok_or("boundary costs")?;
        assert_eq!(leaves.len(), 19);
        let mut sum = 0_u64;
        for (index, ((cost, name), count)) in leaves.iter().zip(costs).zip(expected).enumerate() {
            assert_eq!(cost.as_object().ok_or("boundary cost")?.len(), 3);
            assert_eq!(cost["cost"], name);
            let calls = cost["calls"].as_u64().ok_or("boundary calls")?;
            if index == 0 || index == 3 {
                assert!(calls > 0);
            } else {
                assert_eq!(calls, count, "{} / {name}", row["context"]);
            }
            assert_eq!(cost["elapsedNanoseconds"].is_null(), calls == 0);
            if calls > 0 {
                sum = sum
                    .checked_add(cost["elapsedNanoseconds"].as_u64().ok_or("boundary time")?)
                    .ok_or("cost overflow")?;
            }
        }
        for (target, index) in total.iter_mut().zip([2, 4, 5, 6, 7, 8, 9]) {
            *target += leaves[index]["calls"].as_u64().ok_or("balance count")?;
        }
        let elapsed = row["elapsedNanoseconds"]
            .as_u64()
            .ok_or("boundary elapsed")?;
        assert_eq!(row["selfNanoseconds"].as_u64(), elapsed.checked_sub(sum));
        let enclosing = frame["storePhases"]
            .as_array()
            .ok_or("store phases")?
            .iter()
            .find(|phase| phase["phase"] == name)
            .and_then(|phase| phase["elapsedNanoseconds"].as_u64())
            .ok_or("boundary parent time")?;
        assert!(elapsed <= enclosing, "{name}");
    }
    assert_eq!(total, [11, 1, 2, 2, 0, 9, 2]);
    Ok(())
}

fn lifecycle_pending_recovery(
    jobs: Option<&str>,
    version: DiagnosticVersion,
) -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let control = command("LUMIN_AUDIT_CONTROL_BINARY", root.path())?
        .args(["capabilities"])
        .output()?;
    let setup_build = command("LUMIN_PACKAGE_FIXTURE_BINARY", root.path())?
        .args(["capabilities"])
        .output()?;
    assert!(control.status.success() && setup_build.status.success());
    assert!(control.stderr.is_empty() && setup_build.stderr.is_empty());
    let control: Value = serde_json::from_slice(&control.stdout)?;
    let setup_build: Value = serde_json::from_slice(&setup_build.stdout)?;
    assert_eq!(control["scope"]["buildId"], setup_build["scope"]["buildId"]);
    assert!(!root.path().join(".lumin").exists());
    let death = command("LUMIN_PACKAGE_FIXTURE_BINARY", root.path())?
        .args(["audit", "--jobs", "1"])
        .env("LUMIN_TEST_PUBLICATION_CRASH_POINT", "after-latest-temp")
        .output()?;
    assert_eq!(death.status.code(), Some(95));
    assert!(death.stdout.is_empty() && death.stderr.is_empty());
    let state = root.path().join(".lumin");
    let latest: Value = serde_json::from_slice(&fs::read(state.join("latest.json"))?)?;
    let pending: Value = serde_json::from_slice(&fs::read(state.join("latest.json.pending"))?)?;
    assert_eq!(
        latest["latestAttempt"]["attemptId"],
        "attempt_0000000000000001"
    );
    assert_eq!(latest["latestAttempt"]["status"], "running");
    assert!(latest["latestCompleted"].is_null());
    assert_eq!(pending["latestAttempt"]["status"], "completed");
    assert_eq!(pending["latestCompleted"]["runId"], "run_0000000000000001");
    let before = lumin_engine::complete_logical_store_observation_for_test(root.path())?;
    let before: Value = serde_json::from_slice(&before)?;
    assert_eq!(
        before["records"]["pointers"],
        serde_json::json!({"latest-attempt":b"attempt_0000000000000001".to_vec()})
    );
    assert_eq!(
        before["records"]["attempt_leases"]
            .as_object()
            .ok_or("leases")?
            .len(),
        1
    );
    assert_eq!(
        before["records"]["run_catalog"]
            .as_object()
            .ok_or("catalog")?
            .len(),
        1
    );
    let mut audit = command("LUMIN_AUDIT_DIAGNOSTIC_BINARY", root.path())?;
    audit.args(["audit", "--format", "json"]);
    if let Some(jobs) = jobs {
        audit.args(["--jobs", jobs]);
    }
    let child = audit
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let pid = child.id();
    let output = child.wait_with_output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let frame: Value = serde_json::from_slice(&output.stderr)?;
    verify_store_frame(&frame, false)?;
    verify_lifecycle_frame(&frame, LifecycleFixture::Pending)?;
    verify_lifecycle_bytes(&frame, &output.stderr, version)?;
    if version == DiagnosticVersion::Boundary {
        verify_boundary_frame(&frame)?;
    }
    assert_eq!(frame["processId"], pid);
    assert_eq!(frame["buildId"], control["scope"]["buildId"]);
    let result: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(result["attemptId"], "attempt_0000000000000002");
    assert_eq!(result["runId"], "run_0000000000000002");
    assert_eq!(result["findingCount"], 1);
    assert_eq!(result["limitationCount"], 0);
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
    verify_final_lifecycle_state(root.path(), 2)
}

fn verify_final_lifecycle_state(
    root: &Path,
    count: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let state = root.join(".lumin");
    assert!(!state.join("latest.json.pending").exists());
    let before = lumin_engine::complete_logical_store_observation_for_test(root)?;
    let snapshot: Value = serde_json::from_slice(&before)?;
    assert_eq!(snapshot["records"]["attempt_leases"], serde_json::json!({}));
    assert_eq!(
        snapshot["records"]["run_catalog"]
            .as_object()
            .ok_or("catalog")?
            .len(),
        count
    );
    assert_eq!(
        snapshot["records"]["pointers"],
        serde_json::json!({
            "latest-attempt": format!("attempt_{count:016x}").as_bytes(),
            "latest-completed": format!("run_{count:016x}").as_bytes(),
        })
    );
    let first = command("LUMIN_AUDIT_DIAGNOSTIC_BINARY", root)?
        .args(["overview"])
        .output()?;
    let second = command("LUMIN_AUDIT_DIAGNOSTIC_BINARY", root)?
        .args(["overview"])
        .output()?;
    assert!(first.status.success() && second.status.success());
    assert!(first.stderr.is_empty() && second.stderr.is_empty());
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(
        lumin_engine::complete_logical_store_observation_for_test(root)?,
        before
    );
    for parent in ["runs", "attempts"] {
        let mut names = fs::read_dir(state.join(parent))?
            .map(|entry| entry.map(|e| e.file_name()))
            .collect::<Result<Vec<_>, _>>()?;
        names.sort();
        let prefix = if parent == "runs" { "run" } else { "attempt" };
        let mut expected = vec![std::ffi::OsString::from("namespace.anchor")];
        expected.extend((1..=count).map(|n| format!("{prefix}_{n:016x}").into()));
        expected.sort();
        assert_eq!(names, expected);
    }
    for entry in fs::read_dir(&state)? {
        assert!(
            !entry?
                .file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("attempt-liveness-"))
        );
    }
    Ok(())
}

fn verify_lifecycle_bytes(
    frame: &Value,
    bytes: &[u8],
    version: DiagnosticVersion,
) -> Result<(), Box<dyn std::error::Error>> {
    // Authored transport field order. Value parsing alone would accept duplicate
    // keys, reordering and extra transport whitespace.
    fn object(
        value: &Value,
        keys: &[&str],
        children: &[(&str, String)],
    ) -> Result<String, Box<dyn std::error::Error>> {
        assert_eq!(value.as_object().ok_or("object")?.len(), keys.len());
        let fields = keys
            .iter()
            .map(|key| {
                let value = value.get(key).ok_or("missing field")?;
                let encoded = match children.iter().find(|(name, _)| name == key) {
                    Some((_, encoded)) => encoded.clone(),
                    None => serde_json::to_string(value)?,
                };
                Ok(format!("\"{key}\":{encoded}"))
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        Ok(format!("{{{}}}", fields.join(",")))
    }
    let mut children = Vec::new();
    for field in ["phases", "storePhases"] {
        let rows = frame[field]
            .as_array()
            .ok_or("phases")?
            .iter()
            .map(|row| {
                object(
                    row,
                    &["phase", "calls", "elapsedNanoseconds", "selfNanoseconds"],
                    &[],
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        children.push((field, format!("[{}]", rows.join(","))));
    }
    let mut context_fields = vec!["lifecycleContexts"];
    if version == DiagnosticVersion::Boundary {
        context_fields.push("storeBoundaryContexts");
    }
    for field in context_fields {
        let contexts = frame[field]
            .as_array()
            .ok_or("contexts")?
            .iter()
            .map(|row| {
                let costs = row["costs"]
                    .as_array()
                    .ok_or("costs")?
                    .iter()
                    .map(|cost| object(cost, &["cost", "calls", "elapsedNanoseconds"], &[]))
                    .collect::<Result<Vec<_>, _>>()?;
                object(
                    row,
                    &[
                        "context",
                        "calls",
                        "elapsedNanoseconds",
                        "selfNanoseconds",
                        "costs",
                    ],
                    &[("costs", format!("[{}]", costs.join(",")))],
                )
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        children.push((field, format!("[{}]", contexts.join(","))));
    }
    let mut keys = vec![
        "schemaVersion",
        "diagnosticOnly",
        "buildId",
        "processId",
        "attemptId",
        "runId",
        "requestedJobs",
        "observedAvailableParallelism",
        "parallelismObservationError",
        "actualJobs",
        "configuredWorkerStackBytes",
        "phases",
        "storePhases",
        "lifecycleContexts",
    ];
    if version == DiagnosticVersion::Boundary {
        keys.push("storeBoundaryContexts");
    }
    let canonical = object(frame, &keys, &children)?;
    assert_eq!(bytes, format!("{canonical}\n").as_bytes());
    Ok(())
}

pub fn diagnostic_transport_failure_preserves_exactly_one_committed_run()
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
    verify_final_lifecycle_state(root.path(), 1)
}

pub fn original_audit_failure_has_no_completed_diagnostic_frame()
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
    // A real store-admission failure, not only malformed arguments, must also
    // preserve its owned failure and the foreign object without a diagnostic.
    let state = root.path().join(".lumin");
    fs::write(&state, b"foreign namespace")?;
    let output = command("LUMIN_AUDIT_DIAGNOSTIC_BINARY", root.path())?
        .args(["audit", "--jobs", "1"])
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.starts_with(b"lumin: "));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("lumin.audit-execution-diagnostic"));
    assert_eq!(fs::read(&state)?, b"foreign namespace");
    Ok(())
}

// Independently authored W3 truth: do not derive this from product phase constants.
pub const STORE_PHASES: &[(&str, Option<&str>)] = &[
    ("store-open", None),
    ("namespace-open", Some("store-open")),
    ("bootstrap-setup", Some("namespace-open")),
    ("bootstrap-parents", Some("namespace-open")),
    ("bootstrap-marker", Some("namespace-open")),
    ("bootstrap-store", Some("namespace-open")),
    ("bootstrap-validation", Some("namespace-open")),
    ("open-recovery", Some("store-open")),
    ("open-recovery-enter", Some("open-recovery")),
    ("open-recovery-latest", Some("open-recovery")),
    ("open-recovery-leases", Some("open-recovery")),
    ("open-recovery-exit", Some("open-recovery")),
    ("attempt-begin", None),
    ("attempt-enter", Some("attempt-begin")),
    ("attempt-recover-latest", Some("attempt-begin")),
    ("attempt-recover-leases", Some("attempt-begin")),
    ("attempt-reserve", Some("attempt-begin")),
    ("attempt-lock", Some("attempt-begin")),
    ("attempt-activate", Some("attempt-begin")),
    ("attempt-directory", Some("attempt-begin")),
    ("attempt-envelope", Some("attempt-begin")),
    ("attempt-latest", Some("attempt-begin")),
    ("attempt-exit", Some("attempt-begin")),
    ("store-publish", None),
    ("publish-prepare", Some("store-publish")),
    ("publish-prepare-enter", Some("publish-prepare")),
    ("publish-session", Some("publish-prepare")),
    ("publish-envelope", Some("publish-prepare")),
    ("publish-identities", Some("publish-prepare")),
    ("publish-preflight", Some("publish-prepare")),
    ("publish-directory", Some("publish-prepare")),
    ("staging-create", Some("publish-directory")),
    ("evidence-write", Some("publish-directory")),
    ("evidence-create", Some("evidence-write")),
    ("evidence-begin-write", Some("evidence-write")),
    ("evidence-rows", Some("evidence-write")),
    ("evidence-commit", Some("evidence-write")),
    ("evidence-close", Some("evidence-write")),
    ("evidence-bind-flush-hash", Some("publish-directory")),
    ("run-envelope", Some("publish-directory")),
    ("staging-flush", Some("publish-directory")),
    ("staging-move", Some("publish-directory")),
    ("published-validation", Some("publish-directory")),
    ("publish-terminal", Some("publish-prepare")),
    ("publish-prepare-exit", Some("publish-prepare")),
    ("publish-finalize", Some("store-publish")),
    ("publish-finalize-enter", Some("publish-finalize")),
    ("finalize-candidate", Some("publish-finalize")),
    ("finalize-catalog", Some("publish-finalize")),
    ("finalize-latest", Some("publish-finalize")),
    ("finalize-release", Some("publish-finalize")),
    ("publish-finalize-exit", Some("publish-finalize")),
];

fn verify_store_frame(frame: &Value, fresh: bool) -> Result<(), Box<dyn std::error::Error>> {
    let rows = frame["storePhases"]
        .as_array()
        .ok_or("missing store phases")?;
    assert_eq!(rows.len(), 52);
    let outer = frame["phases"]
        .as_array()
        .ok_or("missing execution phases")?;
    for (index, (name, parent)) in STORE_PHASES.iter().enumerate() {
        let row = &rows[index];
        assert_eq!(row.as_object().ok_or("store phase object")?.len(), 4);
        assert_eq!(row["phase"], *name);
        let absent = !fresh && name.starts_with("bootstrap-");
        assert_eq!(row["calls"], if absent { 0 } else { 1 }, "{name}");
        if absent {
            assert!(row["elapsedNanoseconds"].is_null());
            assert!(row["selfNanoseconds"].is_null());
            continue;
        }
        let elapsed = row["elapsedNanoseconds"]
            .as_u64()
            .ok_or("missing store elapsed")?;
        let mut children = 0_u64;
        for (child_index, (_, owner)) in STORE_PHASES.iter().enumerate() {
            if *owner == Some(*name) {
                children = children
                    .checked_add(
                        rows[child_index]["elapsedNanoseconds"]
                            .as_u64()
                            .unwrap_or(0),
                    )
                    .ok_or("child timing overflow")?;
            }
        }
        assert_eq!(
            row["selfNanoseconds"].as_u64(),
            elapsed.checked_sub(children),
            "{name}"
        );
        if parent.is_none() {
            let enclosing = outer
                .iter()
                .find(|row| row["phase"] == *name)
                .and_then(|row| row["elapsedNanoseconds"].as_u64())
                .ok_or("missing outer phase")?;
            assert!(elapsed <= enclosing, "{name}");
        }
        if *name == "publish-preflight" {
            let inputs = outer
                .iter()
                .find(|row| row["phase"] == "final-inputs")
                .and_then(|row| row["elapsedNanoseconds"].as_u64())
                .ok_or("missing final inputs")?;
            assert!(inputs <= elapsed);
        }
    }
    Ok(())
}
