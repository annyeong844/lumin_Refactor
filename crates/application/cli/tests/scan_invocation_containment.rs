use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

#[test]
fn scan_flags_and_containment_round_trip_through_public_gate()
-> Result<(), Box<dyn std::error::Error>> {
    assert_scan_tier_round_trip()?;
    assert_excluded_entry_limitation()?;
    assert_root_escapes_fail_closed()?;
    assert_alias_drift_fails_closed()?;
    Ok(())
}

fn assert_scan_tier_round_trip() -> Result<(), Box<dyn std::error::Error>> {
    let root = source_fixture()?;
    fs::write(
        root.path().join("src/unused.ts"),
        "export const unusedGenerated = 1;\n",
    )?;
    fs::write(
        root.path().join("tsconfig.json"),
        r#"{"compilerOptions":{"moduleResolution":"bundler","module":"node16"}}"#,
    )?;
    let open_args = [
        "pre-write",
        "--operation-id",
        "op-open",
        "--path",
        "src/main.ts",
        "--include",
        "src/**",
        "--exclude",
        "src/excluded.ts",
        "--role-at",
        "src/unused.ts",
        "generated",
        "--entry",
        "src/main.ts",
        "--resolution-profile",
        "node16",
        "--jobs",
        "1",
    ];
    let opened = run(root.path(), &open_args)?;
    assert_status(&opened, 0);
    assert_eq!(field(&opened.stdout, "decision")?, "allow-with-warnings");
    let gate_id = field(&opened.stdout, "gateId")?;
    let request_digest = field(&opened.stdout, "requestDigest")?;
    assert!(!request_digest.is_empty());

    let exact_retry = run(root.path(), &open_args)?;
    assert_status(&exact_retry, 0);
    assert_eq!(opened.stdout, exact_retry.stdout);

    let changed_retry = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-open",
            "--path",
            "src/main.ts",
            "--include",
            "src/*.ts",
            "--exclude",
            "src/excluded.ts",
            "--role-at",
            "src/unused.ts",
            "generated",
            "--entry",
            "src/main.ts",
            "--resolution-profile",
            "node16",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&changed_retry, 2);

    let open_operation = run(root.path(), &["operation", "show", "op-open"])?;
    assert_status(&open_operation, 0);
    assert_eq!(
        field(&open_operation.stdout, "requestDigest")?,
        request_digest
    );

    let opening_gate = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&opening_gate, 0);
    let opening_json: Value = serde_json::from_str(&opening_gate.stdout)?;
    let baseline_input = required_string(&opening_json, "/baseline/analysisInputId")?.to_owned();
    assert!(!baseline_input.is_empty());
    let baseline_entries = opening_json
        .pointer("/baseline/entrySelections")
        .cloned()
        .ok_or_else(|| std::io::Error::other("baseline entry selections are missing"))?;
    let baseline_entry_items = baseline_entries
        .as_array()
        .ok_or_else(|| std::io::Error::other("baseline entry selections are not an array"))?;
    assert_eq!(baseline_entry_items.len(), 1);
    assert_eq!(
        baseline_entry_items[0]
            .pointer("/path/display")
            .and_then(Value::as_str),
        Some("src/main.ts")
    );
    assert_eq!(
        baseline_entry_items[0]
            .get("source")
            .and_then(Value::as_str),
        Some("invocation")
    );
    assert_eq!(
        opening_json
            .pointer("/baseline/limitationCount")
            .and_then(Value::as_u64),
        Some(0)
    );
    assert_generated_unused_finding(root.path(), &gate_id, "0")?;

    let model_gate_id = lumin_model::GateId::from_string(gate_id.clone());
    let model_open_operation_id = lumin_model::OperationId::from_string("op-open".to_owned());
    let persisted_opening = lumin_engine::load_gate(root.path(), &model_gate_id)?;
    let persisted_open_operation =
        lumin_engine::load_operation(root.path(), &model_open_operation_id)?;
    let persisted_open_options = persisted_open_operation
        .analysis_options
        .as_ref()
        .ok_or_else(|| std::io::Error::other("opening operation omitted analysis options"))?;
    assert_eq!(&persisted_opening.analysis_options, persisted_open_options);
    let opening_tier = persisted_opening.analysis_options.scan_invocation.clone();
    assert_eq!(opening_tier.includes, vec!["src/**".to_owned()]);
    assert_eq!(opening_tier.excludes, vec!["src/excluded.ts".to_owned()]);
    assert_eq!(
        opening_tier.role_overrides,
        vec![lumin_model::RoleOverride {
            pattern: "src/unused.ts".to_owned(),
            role: lumin_model::ScanRole::Generated,
        }]
    );
    assert_eq!(
        opening_tier
            .entries
            .iter()
            .map(|entry| entry.display.as_str())
            .collect::<Vec<_>>(),
        ["src/main.ts"]
    );
    assert_eq!(
        opening_tier.resolution_profile,
        Some(lumin_model::ResolutionProfile::Node16)
    );
    let persisted_baseline = persisted_opening
        .baseline
        .as_ref()
        .ok_or_else(|| std::io::Error::other("opening gate omitted its sealed baseline"))?;
    assert_eq!(persisted_baseline.snapshot.scan_invocation, opening_tier);
    let opening_entry_selections = persisted_baseline.snapshot.entry_selections.clone();

    let rejected_replacement = run(
        root.path(),
        &[
            "post-write",
            &gate_id,
            "--operation-id",
            "op-replacement-close",
            "--entry",
            "src/main.ts",
        ],
    )?;
    assert_status(&rejected_replacement, 2);
    let missing_operation = run(root.path(), &["operation", "show", "op-replacement-close"])?;
    assert_status(&missing_operation, 2);
    assert!(missing_operation.stdout.is_empty());
    assert_gate_active_at_revision_zero(root.path(), &gate_id)?;
    assert_eq!(
        lumin_engine::load_gate(root.path(), &model_gate_id)?,
        persisted_opening
    );
    assert_eq!(
        lumin_engine::load_operation(root.path(), &model_open_operation_id)?,
        persisted_open_operation
    );

    fs::write(
        root.path().join("src/main.ts"),
        "console.log('main changed');\n",
    )?;
    let closed = run(
        root.path(),
        &["post-write", &gate_id, "--operation-id", "op-close"],
    )?;
    assert_status(&closed, 0);
    assert_eq!(field(&closed.stdout, "decision")?, "allow-with-warnings");
    assert_eq!(field(&closed.stdout, "lifecycle")?, "closed");
    let close_digest = field(&closed.stdout, "requestDigest")?;
    let closed_result: Value = serde_json::from_str(&closed.stdout)?;
    assert_eq!(
        closed_result
            .pointer("/actualWriteSet/paths/0/display")
            .and_then(Value::as_str),
        Some("src/main.ts")
    );
    assert_generated_unused_finding(root.path(), &gate_id, "1")?;

    let closed_gate = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&closed_gate, 0);
    let closed_json: Value = serde_json::from_str(&closed_gate.stdout)?;
    assert_eq!(
        required_string(&closed_json, "/baseline/analysisInputId")?,
        baseline_input
    );
    let close_input = required_string(&closed_json, "/revisions/1/analysisInputId")?.to_owned();
    assert_ne!(close_input, baseline_input);
    assert_eq!(
        closed_json.pointer("/baseline/entrySelections"),
        Some(&baseline_entries)
    );
    assert_eq!(
        closed_json
            .pointer("/revisions/1/observationBinding/state")
            .and_then(Value::as_str),
        Some("sealed")
    );
    assert_eq!(
        closed_json
            .pointer("/revisions/1/observationBinding/observation/kind")
            .and_then(Value::as_str),
        Some("close")
    );
    let close_operation = run(root.path(), &["operation", "show", "op-close"])?;
    assert_status(&close_operation, 0);
    let close_operation_json: Value = serde_json::from_str(&close_operation.stdout)?;
    assert_eq!(
        field(&close_operation.stdout, "requestDigest")?,
        close_digest
    );
    assert_eq!(
        close_operation_json.pointer("/result/observationBinding"),
        closed_json.pointer("/revisions/1/observationBinding")
    );

    let persisted_closed = lumin_engine::load_gate(root.path(), &model_gate_id)?;
    assert_eq!(
        persisted_closed.analysis_options.scan_invocation,
        opening_tier
    );
    let persisted_close = persisted_closed
        .revisions
        .iter()
        .find(|revision| revision.revision == 1)
        .ok_or_else(|| std::io::Error::other("closed gate omitted revision 1"))?;
    let close_snapshot = persisted_close
        .snapshot
        .as_ref()
        .ok_or_else(|| std::io::Error::other("sealed close omitted its snapshot"))?;
    assert_eq!(close_snapshot.scan_invocation, opening_tier);
    assert_eq!(close_snapshot.entry_selections, opening_entry_selections);
    assert_eq!(close_snapshot.analysis_input_id.as_str(), close_input);
    assert!(lumin_engine::gate_observation_binding_matches_owner(
        &persisted_closed,
        persisted_close
    )?);
    Ok(())
}

fn assert_excluded_entry_limitation() -> Result<(), Box<dyn std::error::Error>> {
    let root = source_fixture()?;
    let audit = run(
        root.path(),
        &[
            "audit",
            "--entry",
            "src/excluded.ts",
            "--exclude",
            "src/excluded.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&audit, 0);
    let run_id = field(&audit.stdout, "runId")?;
    let overview = run(root.path(), &["overview", "--run", &run_id])?;
    assert_status(&overview, 0);
    let overview_json: Value = serde_json::from_str(&overview.stdout)?;
    let limitations = overview_json
        .get("limitations")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("overview limitations are missing"))?;
    assert_eq!(limitations.len(), 1);
    let limitation = &limitations[0];
    assert_eq!(
        limitation.get("reason").and_then(Value::as_str),
        Some("explicit-entry-unavailable")
    );
    assert_eq!(
        limitation.get("path").and_then(Value::as_str),
        Some("src/excluded.ts")
    );
    assert_eq!(
        limitation.get("source").and_then(Value::as_str),
        Some("invocation")
    );
    assert_eq!(
        limitation.get("unavailable_reason").and_then(Value::as_str),
        Some("excluded")
    );
    Ok(())
}

fn assert_root_escapes_fail_closed() -> Result<(), Box<dyn std::error::Error>> {
    let caller_root = source_fixture()?;
    let caller_audit = run(
        caller_root.path(),
        &["audit", "--entry", "../outside.ts", "--jobs", "1"],
    )?;
    assert_status(&caller_audit, 2);
    let caller_pre = run(
        caller_root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-escape",
            "--path",
            "src/main.ts",
            "--entry",
            "../outside.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&caller_pre, 2);
    let missing_operation = run(caller_root.path(), &["operation", "show", "op-escape"])?;
    assert_status(&missing_operation, 2);
    assert!(missing_operation.stdout.is_empty());
    let active = run(
        caller_root.path(),
        &["gate", "list", "--active", "--format", "json"],
    )?;
    assert_status(&active, 0);
    let active_json: Value = serde_json::from_str(&active.stdout)?;
    assert_eq!(active_json.get("total").and_then(Value::as_u64), Some(0));

    let config_root = source_fixture()?;
    fs::write(
        config_root.path().join("lumin.json"),
        r#"{"schemaVersion":"lumin-config.v1","entries":["../outside.ts"]}"#,
    )?;
    let config_audit = run(config_root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&config_audit, 1);
    assert!(config_audit.stdout.is_empty());
    let attempt = run(config_root.path(), &["overview"])?;
    assert_status(&attempt, 0);
    let attempt_json: Value = serde_json::from_str(&attempt.stdout)?;
    assert_eq!(
        attempt_json.get("schemaVersion").and_then(Value::as_str),
        Some("lumin.attempt-overview.v1")
    );
    assert_eq!(
        attempt_json
            .pointer("/latestAttempt/status")
            .and_then(Value::as_str),
        Some("failed")
    );
    assert_eq!(
        attempt_json.pointer("/scope/kind").and_then(Value::as_str),
        Some("attempt")
    );
    Ok(())
}

fn assert_alias_drift_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let root = source_fixture()?;
    fs::create_dir(root.path().join("inside"))?;
    fs::write(
        root.path().join("inside/entry.ts"),
        "console.log('inside target');\n",
    )?;
    let alias = root.path().join("alias");
    create_directory_alias(&root.path().join("inside"), &alias)?;

    let opened = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-alias-open",
            "--path",
            "src/main.ts",
            "--entry",
            "alias/entry.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);
    let gate_id = field(&opened.stdout, "gateId")?;

    let outside = tempfile::tempdir()?;
    fs::write(outside.path().join("entry.ts"), "console.log('outside');\n")?;
    remove_directory_alias(&alias)?;
    create_directory_alias(outside.path(), &alias)?;

    let close = run(
        root.path(),
        &["post-write", &gate_id, "--operation-id", "op-alias-close"],
    )?;
    assert_status(&close, 5);
    assert_eq!(field(&close.stdout, "decision")?, "stale");
    assert_eq!(field(&close.stdout, "lifecycle")?, "active");
    let close_json: Value = serde_json::from_str(&close.stdout)?;
    assert!(
        close_json
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some("protected-input-changed")
            }))
    );

    let operation = run(root.path(), &["operation", "show", "op-alias-close"])?;
    assert_status(&operation, 0);
    let operation_json: Value = serde_json::from_str(&operation.stdout)?;
    assert_eq!(
        operation_json.get("status").and_then(Value::as_str),
        Some("committed")
    );
    assert_eq!(
        operation_json
            .pointer("/result/decision")
            .and_then(Value::as_str),
        Some("stale")
    );

    let shown = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&shown, 0);
    let shown_json: Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(
        shown_json.get("lifecycle").and_then(Value::as_str),
        Some("active")
    );
    assert_eq!(
        shown_json.get("currentRevision").and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        shown_json
            .pointer("/revisions/1/decision")
            .and_then(Value::as_str),
        Some("stale")
    );
    Ok(())
}

fn assert_gate_active_at_revision_zero(
    root: &Path,
    gate_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let shown = run(root, &["gate", "show", gate_id])?;
    assert_status(&shown, 0);
    let value: Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(
        value.get("lifecycle").and_then(Value::as_str),
        Some("active")
    );
    assert_eq!(
        value.get("currentRevision").and_then(Value::as_u64),
        Some(0)
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

fn assert_generated_unused_finding(
    root: &Path,
    gate_id: &str,
    revision: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let findings = run(root, &["gate", "findings", gate_id, "--revision", revision])?;
    assert_status(&findings, 0);
    let findings: Value = serde_json::from_str(&findings.stdout)?;
    let items = findings
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("gate findings items are missing"))?;
    assert_eq!(items.len(), 1);
    let finding = &items[0];
    assert_eq!(
        finding.pointer("/path/display").and_then(Value::as_str),
        Some("src/unused.ts")
    );
    assert_eq!(
        finding.get("exportedName").and_then(Value::as_str),
        Some("unusedGenerated")
    );
    assert_eq!(
        finding.pointer("/disposition/kind").and_then(Value::as_str),
        Some("review-only")
    );
    assert_eq!(
        finding
            .pointer("/disposition/reason")
            .and_then(Value::as_str),
        Some("generated-source")
    );
    Ok(())
}

fn source_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(root.path().join("src/main.ts"), "console.log('main');\n")?;
    fs::write(
        root.path().join("src/excluded.ts"),
        "export const excluded = 1;\n",
    )?;
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
