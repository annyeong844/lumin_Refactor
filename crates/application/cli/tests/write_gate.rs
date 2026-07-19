use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

struct ProcessResult {
    status: i32,
    stdout: String,
    stderr: String,
}

#[test]
fn pre_and_post_survive_process_reopen() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let pre = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-open",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&pre, 0);
    let gate_id = field(&pre.stdout, "gateId")?;

    let retry = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-open",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&retry, 0);
    assert_eq!(pre.stdout, retry.stdout);

    fs::write(root.path().join("src/lib.ts"), "export const used = 2;\n")?;
    let post = run(
        root.path(),
        &["post-write", &gate_id, "--operation-id", "op-close"],
    )?;
    assert_status(&post, 0);
    let post_json: Value = serde_json::from_str(&post.stdout)?;
    assert_eq!(
        post_json.get("decision").and_then(Value::as_str),
        Some("allow")
    );
    assert_eq!(
        post_json.get("lifecycle").and_then(Value::as_str),
        Some("closed")
    );
    let post_retry = run(
        root.path(),
        &["post-write", &gate_id, "--operation-id", "op-close"],
    )?;
    assert_status(&post_retry, 0);
    assert_eq!(post.stdout, post_retry.stdout);

    let shown = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&shown, 0);
    let shown_json: Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(
        shown_json.get("lifecycle").and_then(Value::as_str),
        Some("closed")
    );
    assert_eq!(
        shown_json.get("currentRevision").and_then(Value::as_u64),
        Some(1)
    );

    let operation = run(root.path(), &["operation", "show", "op-close"])?;
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
        Some("allow")
    );
    Ok(())
}

#[test]
fn overlapping_gate_is_rejected_and_operation_reuse_is_malformed()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let first = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-first",
            "--path",
            "src/main.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&first, 0);

    let overlap = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-overlap",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&overlap, 4);
    let overlap_json: Value = serde_json::from_str(&overlap.stdout)?;
    assert_eq!(
        overlap_json.get("lifecycle").and_then(Value::as_str),
        Some("rejected")
    );
    assert_eq!(
        overlap_json
            .pointer("/signals/0/kind")
            .and_then(Value::as_str),
        Some("write-conflict")
    );

    let reused = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-first",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&reused, 2);
    assert!(reused.stderr.contains("reused with a different request"));
    Ok(())
}

#[test]
fn introduced_grounded_finding_denies_and_records_its_delta()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let gate_id = open_gate(root.path(), "op-open", "src/lib.ts")?;
    fs::write(
        root.path().join("src/lib.ts"),
        "export const renamed = 1;\n",
    )?;

    let post = assert_active_close(root.path(), &gate_id, 3, "deny", "adverse-fact-introduced")?;
    assert_delta(&post, "dead-export", "introduced")?;
    Ok(())
}

#[test]
fn resolved_grounded_finding_authorizes_and_remains_queryable()
-> Result<(), Box<dyn std::error::Error>> {
    let root = dead_finding_fixture()?;
    let gate_id = open_gate(root.path(), "op-resolve-open", "src/lib.ts")?;
    fs::write(root.path().join("src/lib.ts"), "console.log('resolved');\n")?;

    let post = run(
        root.path(),
        &["post-write", &gate_id, "--operation-id", "op-resolve-close"],
    )?;
    assert_status(&post, 0);
    assert_eq!(field(&post.stdout, "decision")?, "allow");
    assert_eq!(field(&post.stdout, "lifecycle")?, "closed");
    assert_delta(&post.stdout, "dead-export", "resolved")?;

    let shown = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&shown, 0);
    let shown_json: Value = serde_json::from_str(&shown.stdout)?;
    let persisted = shown_json
        .pointer("/revisions/1/deltas/0")
        .ok_or_else(|| std::io::Error::other("resolved delta was not persisted"))?;
    assert_eq!(
        persisted.pointer("/key/family").and_then(Value::as_str),
        Some("dead-export")
    );
    assert_eq!(
        persisted
            .pointer("/classification/kind")
            .and_then(Value::as_str),
        Some("resolved")
    );
    Ok(())
}

#[test]
fn unchanged_grounded_finding_remains_an_advisory_warning() -> Result<(), Box<dyn std::error::Error>>
{
    let root = dead_finding_fixture()?;
    let gate_id = open_gate(root.path(), "op-unchanged-open", "src/lib.ts")?;

    let post = run(
        root.path(),
        &[
            "post-write",
            &gate_id,
            "--operation-id",
            "op-unchanged-close",
        ],
    )?;
    assert_status(&post, 0);
    assert_eq!(field(&post.stdout, "decision")?, "allow-with-warnings");
    assert_eq!(field(&post.stdout, "lifecycle")?, "closed");
    assert_delta(&post.stdout, "dead-export", "unchanged")?;
    Ok(())
}

#[test]
fn bounded_unresolved_edge_is_advisory_and_comparable() -> Result<(), Box<dyn std::error::Error>> {
    let root = unresolved_edge_fixture()?;
    let pre = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-unresolved-open",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&pre, 0);
    assert_eq!(field(&pre.stdout, "decision")?, "allow-with-warnings");
    assert_has_signal(&pre.stdout, "pre-existing-adverse-facts")?;
    let gate_id = field(&pre.stdout, "gateId")?;

    let post = run(
        root.path(),
        &[
            "post-write",
            &gate_id,
            "--operation-id",
            "op-unresolved-close",
        ],
    )?;
    assert_status(&post, 0);
    assert_eq!(field(&post.stdout, "decision")?, "allow-with-warnings");
    assert_has_signal(&post.stdout, "pre-existing-adverse-facts")?;
    assert_delta(&post.stdout, "unresolved-internal-edge", "unchanged")?;
    Ok(())
}

#[test]
fn unsupported_config_remains_a_required_evidence_gap() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    fs::write(
        root.path().join("tsconfig.json"),
        "{\"compilerOptions\":{\"unknownLuminOption\":true}}\n",
    )?;
    let pre = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-gap-open",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&pre, 4);
    assert_eq!(field(&pre.stdout, "decision")?, "incomplete");
    assert_has_signal(&pre.stdout, "required-evidence-incomplete")?;
    assert_empty_deltas(&pre.stdout)?;
    Ok(())
}

#[test]
fn unexpected_new_source_denies_and_keeps_the_gate_active() -> Result<(), Box<dyn std::error::Error>>
{
    let root = fixture()?;
    let gate_id = open_gate(root.path(), "op-open", "src/lib.ts")?;
    fs::write(
        root.path().join("src/extra.ts"),
        "export const extra = 1;\n",
    )?;

    assert_active_close(root.path(), &gate_id, 3, "deny", "unplanned-write")?;
    Ok(())
}

#[test]
fn protected_input_drift_is_stale() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let pre = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-open",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&pre, 0);
    let gate_id = field(&pre.stdout, "gateId")?;
    fs::write(
        root.path().join("src/main.ts"),
        "import { used } from './lib';\nconsole.log(used);\n",
    )?;

    let post = run(
        root.path(),
        &["post-write", &gate_id, "--operation-id", "op-close"],
    )?;
    assert_status(&post, 5);
    let post_json: Value = serde_json::from_str(&post.stdout)?;
    assert_eq!(
        post_json.get("decision").and_then(Value::as_str),
        Some("stale")
    );
    assert_eq!(
        post_json.get("lifecycle").and_then(Value::as_str),
        Some("active")
    );
    Ok(())
}

#[test]
fn unsupported_non_source_path_is_queryable_incomplete() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    fs::write(root.path().join("notes.md"), "not a source input\n")?;
    let pre = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-notes",
            "--path",
            "notes.md",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&pre, 4);
    let pre_json: Value = serde_json::from_str(&pre.stdout)?;
    assert_eq!(
        pre_json.get("decision").and_then(Value::as_str),
        Some("incomplete")
    );
    assert_eq!(
        pre_json.get("lifecycle").and_then(Value::as_str),
        Some("rejected")
    );
    assert_eq!(
        pre_json
            .pointer("/signals/0/reason")
            .and_then(Value::as_str),
        Some("not-analyzed-source")
    );

    let operation = run(root.path(), &["operation", "show", "op-notes"])?;
    assert_status(&operation, 0);
    let operation_json: Value = serde_json::from_str(&operation.stdout)?;
    assert_eq!(
        operation_json
            .pointer("/result/decision")
            .and_then(Value::as_str),
        Some("incomplete")
    );
    Ok(())
}

#[test]
fn missing_operation_is_a_typed_hard_stop() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let operation = run(root.path(), &["operation", "show", "op-missing"])?;

    assert_status(&operation, 2);
    assert!(operation.stdout.is_empty());
    assert_eq!(
        operation.stderr,
        "lumin: operation does not exist: op-missing\n"
    );

    let gate = run(root.path(), &["gate", "show", "gate_missing"])?;
    assert_status(&gate, 2);
    assert!(gate.stdout.is_empty());
    assert_eq!(gate.stderr, "lumin: gate does not exist: gate_missing\n");
    Ok(())
}

#[test]
fn new_source_path_is_admitted_before_it_exists() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let pre = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-new-open",
            "--path",
            "src/generated/deep/new.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&pre, 0);
    let gate_id = field(&pre.stdout, "gateId")?;
    let pre_json: Value = serde_json::from_str(&pre.stdout)?;
    assert_eq!(
        pre_json
            .pointer("/leasedWriteSet/0/kind")
            .and_then(Value::as_str),
        Some("new-file")
    );

    fs::create_dir_all(root.path().join("src/generated/deep"))?;
    fs::write(
        root.path().join("src/generated/deep/new.ts"),
        "console.log('new');\n",
    )?;
    let post = run(
        root.path(),
        &["post-write", &gate_id, "--operation-id", "op-new-close"],
    )?;
    assert_status(&post, 0);
    assert_eq!(field(&post.stdout, "decision")?, "allow");
    assert_eq!(field(&post.stdout, "lifecycle")?, "closed");
    assert_empty_deltas(&post.stdout)?;
    Ok(())
}

#[test]
fn directory_lease_covers_new_descendants_and_conflicts_with_them()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    fs::create_dir(root.path().join("src/feature"))?;
    fs::write(
        root.path().join("src/feature/existing.ts"),
        "console.log('existing');\n",
    )?;
    let pre = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-dir-open",
            "--path",
            "src/feature",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&pre, 0);
    let gate_id = field(&pre.stdout, "gateId")?;

    let overlap = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-dir-overlap",
            "--path",
            "src/feature/new.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&overlap, 4);
    assert_eq!(field(&overlap.stdout, "lifecycle")?, "rejected");

    fs::write(
        root.path().join("src/feature/new.ts"),
        "console.log('new');\n",
    )?;
    let post = run(
        root.path(),
        &["post-write", &gate_id, "--operation-id", "op-dir-close"],
    )?;
    assert_status(&post, 0);
    assert_eq!(field(&post.stdout, "decision")?, "allow");
    assert_eq!(field(&post.stdout, "lifecycle")?, "closed");
    assert_empty_deltas(&post.stdout)?;
    Ok(())
}

#[test]
fn physical_alias_closure_is_visible_and_rejects_a_late_unleased_alias()
-> Result<(), Box<dyn std::error::Error>> {
    let root = alias_fixture()?;
    let pre = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-alias-open",
            "--path",
            "src/original.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&pre, 0);
    let gate_id = field(&pre.stdout, "gateId")?;
    let pre_json: Value = serde_json::from_str(&pre.stdout)?;
    let leased_paths = pre_json
        .get("leasedWriteSet")
        .and_then(Value::as_array)
        .ok_or("leasedWriteSet is missing")?
        .iter()
        .filter_map(|lease| lease.pointer("/path/display").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert!(leased_paths.contains(&"src/original.ts"));
    assert!(leased_paths.contains(&"src/alias.ts"));

    fs::write(
        root.path().join("src/original.ts"),
        "console.log('updated');\n",
    )?;
    fs::hard_link(
        root.path().join("src/original.ts"),
        root.path().join("src/late-alias.ts"),
    )?;
    let post = run(
        root.path(),
        &["post-write", &gate_id, "--operation-id", "op-alias-close"],
    )?;
    assert_status(&post, 3);
    assert_eq!(field(&post.stdout, "decision")?, "deny");
    assert!(
        serde_json::from_str::<Value>(&post.stdout)?
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some("unplanned-write")
            }))
    );
    Ok(())
}

#[test]
fn physical_alias_members_are_reanalyzed_as_one_leased_payload()
-> Result<(), Box<dyn std::error::Error>> {
    let root = alias_fixture()?;
    let gate_id = open_gate(root.path(), "op-alias-positive-open", "src/original.ts")?;

    fs::write(
        root.path().join("src/alias.ts"),
        "console.log('updated');\n",
    )?;
    let post = run(
        root.path(),
        &[
            "post-write",
            &gate_id,
            "--operation-id",
            "op-alias-positive-close",
        ],
    )?;
    assert_status(&post, 0);
    assert_eq!(field(&post.stdout, "decision")?, "allow");

    let shown = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&shown, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&shown.stdout)?
            .pointer("/revisions/1/aliasGroupCount")
            .and_then(Value::as_u64),
        Some(1)
    );
    Ok(())
}

#[test]
fn disjoint_gates_reconcile_a_terminal_transition_on_retry()
-> Result<(), Box<dyn std::error::Error>> {
    let root = disjoint_fixture()?;
    let gate_a = open_gate(root.path(), "op-a-open", "src/a.ts")?;
    let gate_b = open_gate(root.path(), "op-b-open", "src/b.ts")?;

    fs::write(root.path().join("src/b.ts"), "console.log('b2');\n")?;
    let pending_a = run(
        root.path(),
        &["post-write", &gate_a, "--operation-id", "op-a-pending"],
    )?;
    assert_status(&pending_a, 4);
    assert_eq!(field(&pending_a.stdout, "decision")?, "incomplete");
    assert!(
        serde_json::from_str::<Value>(&pending_a.stdout)?
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some("active-transition-pending")
            }))
    );

    let close_b = run(
        root.path(),
        &["post-write", &gate_b, "--operation-id", "op-b-close"],
    )?;
    assert_status(&close_b, 0);
    assert_eq!(field(&close_b.stdout, "decision")?, "allow");

    fs::write(root.path().join("src/a.ts"), "console.log('a2');\n")?;
    let close_a = run(
        root.path(),
        &["post-write", &gate_a, "--operation-id", "op-a-close"],
    )?;
    assert_status(&close_a, 0);
    assert_eq!(field(&close_a.stdout, "decision")?, "allow");

    let shown = run(root.path(), &["gate", "show", &gate_a])?;
    assert_status(&shown, 0);
    let shown_json: Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(
        shown_json
            .pointer("/revisions/2/reconciledTransitionSequences/0")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        shown_json
            .get("transitionRefs")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );
    Ok(())
}

fn open_gate(
    root: &Path,
    operation_id: &str,
    path: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let pre = run(
        root,
        &[
            "pre-write",
            "--operation-id",
            operation_id,
            "--path",
            path,
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&pre, 0);
    field(&pre.stdout, "gateId")
}

fn assert_delta(
    stdout: &str,
    expected_family: &str,
    expected_classification: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let value: Value = serde_json::from_str(stdout)?;
    let deltas = value
        .get("deltas")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("deltas are missing"))?;
    assert_eq!(deltas.len(), 1);
    assert_eq!(
        deltas[0].pointer("/key/family").and_then(Value::as_str),
        Some(expected_family)
    );
    assert_eq!(
        deltas[0]
            .pointer("/classification/kind")
            .and_then(Value::as_str),
        Some(expected_classification)
    );
    Ok(())
}

fn assert_empty_deltas(stdout: &str) -> Result<(), Box<dyn std::error::Error>> {
    let value: Value = serde_json::from_str(stdout)?;
    assert_eq!(
        value.get("deltas").and_then(Value::as_array).map(Vec::len),
        Some(0)
    );
    Ok(())
}

fn assert_has_signal(stdout: &str, expected_kind: &str) -> Result<(), Box<dyn std::error::Error>> {
    let value: Value = serde_json::from_str(stdout)?;
    assert!(
        value
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some(expected_kind)
            }))
    );
    Ok(())
}

fn assert_active_close(
    root: &Path,
    gate_id: &str,
    expected_status: i32,
    expected_decision: &str,
    expected_signal: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let post = run(root, &["post-write", gate_id, "--operation-id", "op-close"])?;
    assert_status(&post, expected_status);
    let value: Value = serde_json::from_str(&post.stdout)?;
    assert_eq!(
        value.get("decision").and_then(Value::as_str),
        Some(expected_decision)
    );
    assert_eq!(
        value.get("lifecycle").and_then(Value::as_str),
        Some("active")
    );
    assert!(
        value
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some(expected_signal)
            }))
    );
    Ok(post.stdout)
}

fn alias_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(
        root.path().join("src/original.ts"),
        "console.log('original');\n",
    )?;
    fs::hard_link(
        root.path().join("src/original.ts"),
        root.path().join("src/alias.ts"),
    )?;
    Ok(root)
}

fn disjoint_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(root.path().join("src/a.ts"), "console.log('a');\n")?;
    fs::write(root.path().join("src/b.ts"), "console.log('b');\n")?;
    Ok(root)
}

fn fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    source_fixture(
        "export const used = 1;\n",
        "import { used } from './lib'; console.log(used);\n",
    )
}

fn dead_finding_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    source_fixture("export const unused = 1;\n", "console.log('main');\n")
}

fn unresolved_edge_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    source_fixture(
        "console.log('lib');\n",
        "import { missing } from './missing'; console.log(missing);\n",
    )
}

fn source_fixture(
    lib_source: &str,
    main_source: &str,
) -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(root.path().join("src/lib.ts"), lib_source)?;
    fs::write(root.path().join("src/main.ts"), main_source)?;
    Ok(root)
}

fn run(root: &Path, arguments: &[&str]) -> Result<ProcessResult, Box<dyn std::error::Error>> {
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

fn assert_status(result: &ProcessResult, expected: i32) {
    assert_eq!(
        result.status, expected,
        "stdout={}\nstderr={}",
        result.stdout, result.stderr
    );
}

fn field(json: &str, name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let value: Value = serde_json::from_str(json)?;
    value
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other(format!("missing string field {name}")).into())
}
