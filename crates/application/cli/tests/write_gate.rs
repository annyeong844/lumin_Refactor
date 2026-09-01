use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::path::Path;
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

mod support;

#[path = "write_gate/mixed_vue.rs"]
mod mixed_vue;
#[path = "write_gate/reopen_queries.rs"]
mod reopen_queries;
#[path = "write_gate/semantic_demands.rs"]
mod semantic_demands;
#[path = "write_gate/transition_retention.rs"]
mod transition_retention;

use support::{assert_status, field, lumin_command, run};

fn lumin_command_with_args(
    root: &Path,
    arguments: &[&str],
) -> Result<std::process::Command, Box<dyn std::error::Error>> {
    let arguments = arguments
        .iter()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();
    let effective_arguments = support::determinism::effective_arguments(&arguments)?;
    let mut command = lumin_command(root)?;
    command.args(effective_arguments);
    Ok(command)
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
    let pre_json: Value = serde_json::from_str(&pre.stdout)?;
    assert_eq!(
        pre_json.get("schemaVersion").and_then(Value::as_str),
        Some("lumin.gate-mutation.v2")
    );
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
        post_json.get("schemaVersion").and_then(Value::as_str),
        Some("lumin.gate-mutation.v2")
    );
    assert_eq!(
        post_json.get("decision").and_then(Value::as_str),
        Some("allow")
    );
    assert_eq!(
        post_json.get("lifecycle").and_then(Value::as_str),
        Some("closed")
    );
    assert_eq!(
        post_json
            .pointer("/observationBinding/observation/kind")
            .and_then(Value::as_str),
        Some("close")
    );
    let close_observation_id = post_json
        .pointer("/observationBinding/observation/observationId")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("post-write omitted its close observation ID"))?;
    assert!(close_observation_id.starts_with("gate_close_observation_"));
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
        shown_json.get("schemaVersion").and_then(Value::as_str),
        Some("lumin.gate.v2")
    );
    assert_eq!(
        shown_json.get("lifecycle").and_then(Value::as_str),
        Some("closed")
    );
    assert_eq!(
        shown_json.get("currentRevision").and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        shown_json
            .pointer("/baseline/catalogRevision")
            .and_then(Value::as_u64),
        Some(0)
    );
    assert_eq!(
        shown_json
            .pointer("/revisions/0/catalogRevision")
            .and_then(Value::as_u64),
        Some(0)
    );
    assert_eq!(
        shown_json
            .pointer("/revisions/1/catalogRevision")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        shown_json
            .pointer("/revisions/1/observationBinding/observation/observationId")
            .and_then(Value::as_str),
        Some(close_observation_id)
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
            .pointer("/result/schemaVersion")
            .and_then(Value::as_str),
        Some("lumin.gate-mutation.v2")
    );
    assert_eq!(
        operation_json
            .pointer("/result/decision")
            .and_then(Value::as_str),
        Some("allow")
    );
    assert_eq!(
        operation_json
            .pointer("/result/observationBinding/observation/observationId")
            .and_then(Value::as_str),
        Some(close_observation_id)
    );
    Ok(())
}

#[test]
fn pre_write_observation_binds_promotion_and_interrupted_admission_leaves_no_active_lease()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let arguments = [
        "pre-write",
        "--operation-id",
        "op-observation",
        "--path",
        "src/lib.ts",
        "--jobs",
        "1",
    ];
    let mut child = lumin_command_with_args(root.path(), &arguments)?
        .env(
            "LUMIN_TEST_GATE_ADMISSION_BARRIER",
            listener.local_addr()?.to_string(),
        )
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
                        "pre-write exited before admission barrier: {status}"
                    ))
                    .into());
                }
                if started.elapsed() >= Duration::from_secs(30) {
                    return Err(std::io::Error::other(
                        "pre-write did not reach the admission barrier",
                    )
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
    let mut frame = String::new();
    BufReader::new(stream.try_clone()?).read_line(&mut frame)?;
    let fields = frame.split_whitespace().collect::<Vec<_>>();
    assert_eq!(fields.len(), 3, "unexpected admission frame: {frame:?}");
    assert_eq!(fields[0], "reserved");
    assert_eq!(fields[1], "op-observation");
    let gate_id = fields[2].to_owned();

    let provisional = run(root.path(), &["gate", "list", "--active"])?;
    assert_status(&provisional, 0);
    let provisional_json: Value = serde_json::from_str(&provisional.stdout)?;
    assert_eq!(
        provisional_json.get("total").and_then(Value::as_u64),
        Some(0)
    );

    let competing = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-observation-conflict",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&competing, 4);
    let competing_json: Value = serde_json::from_str(&competing.stdout)?;
    assert_eq!(
        competing_json.get("lifecycle").and_then(Value::as_str),
        Some("rejected")
    );
    assert_eq!(
        competing_json
            .pointer("/signals/0/kind")
            .and_then(Value::as_str),
        Some("write-conflict")
    );
    assert_eq!(
        competing_json
            .pointer("/observationBinding/state")
            .and_then(Value::as_str),
        Some("unsealed")
    );
    assert_eq!(
        competing_json
            .pointer("/observationBinding/reason")
            .and_then(Value::as_str),
        Some("admission-conflict")
    );
    assert_eq!(
        competing_json
            .pointer("/observationBinding/conflictingOrUnboundedInputs/0/display")
            .and_then(Value::as_str),
        Some("src/lib.ts")
    );

    child.kill()?;
    let interrupted_output = child.wait_with_output()?;
    assert!(!interrupted_output.status.success());
    drop(stream);

    let interrupted = run(root.path(), &["operation", "show", "op-observation"])?;
    assert_status(&interrupted, 0);
    let interrupted_json: Value = serde_json::from_str(&interrupted.stdout)?;
    assert_eq!(
        interrupted_json.get("status").and_then(Value::as_str),
        Some("interrupted")
    );
    assert_eq!(
        interrupted_json
            .get("interruptionCount")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        interrupted_json
            .get("leasedWriteSet")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );
    let after_death = run(root.path(), &["gate", "list", "--active"])?;
    assert_status(&after_death, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&after_death.stdout)?
            .get("total")
            .and_then(Value::as_u64),
        Some(0)
    );

    let retry = run(root.path(), &arguments)?;
    assert_status(&retry, 0);
    let retry_json: Value = serde_json::from_str(&retry.stdout)?;
    assert_eq!(
        retry_json.get("decision").and_then(Value::as_str),
        Some("allow")
    );
    assert_eq!(
        retry_json.get("lifecycle").and_then(Value::as_str),
        Some("active")
    );
    assert_eq!(
        retry_json.get("gateId").and_then(Value::as_str),
        Some(gate_id.as_str())
    );
    assert_eq!(
        retry_json
            .pointer("/observationBinding/state")
            .and_then(Value::as_str),
        Some("sealed")
    );
    assert_eq!(
        retry_json
            .pointer("/observationBinding/observation/kind")
            .and_then(Value::as_str),
        Some("baseline")
    );
    let observation_id = retry_json
        .pointer("/observationBinding/observation/observationId")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("pre-write omitted its baseline observation ID"))?;
    assert!(observation_id.starts_with("gate_baseline_observation_"));

    let shown = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&shown, 0);
    let shown_json: Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(
        shown_json
            .pointer("/baseline/observationId")
            .and_then(Value::as_str),
        Some(observation_id)
    );
    assert_eq!(
        shown_json
            .pointer("/revisions/0/observationBinding/observation/observationId")
            .and_then(Value::as_str),
        Some(observation_id)
    );

    let committed = run(root.path(), &["operation", "show", "op-observation"])?;
    assert_status(&committed, 0);
    let committed_json: Value = serde_json::from_str(&committed.stdout)?;
    assert_eq!(
        committed_json
            .pointer("/result/observationBinding/observation/observationId")
            .and_then(Value::as_str),
        Some(observation_id)
    );
    Ok(())
}

#[test]
fn final_observation_rechecks_current_domain_before_sealing()
-> Result<(), Box<dyn std::error::Error>> {
    final_promotion_reobserves_the_complete_write_domain()?;
    final_promotion_reenumerates_new_directory_source_aliases()?;
    final_promotion_rejects_a_new_source_outside_the_captured_alias_domain()?;
    final_promotion_rejects_a_new_configuration_input()?;
    final_promotion_preserves_a_sealed_stale_observation_when_an_alias_seed_disappears()?;
    final_close_reobserves_the_complete_write_domain()?;
    Ok(())
}

fn final_promotion_reobserves_the_complete_write_domain() -> Result<(), Box<dyn std::error::Error>>
{
    let root = fixture()?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let arguments = [
        "pre-write",
        "--operation-id",
        "op-final-topology",
        "--path",
        "src/new.ts",
        "--jobs",
        "1",
    ];
    let mut child = lumin_command_with_args(root.path(), &arguments)?
        .env(
            "LUMIN_TEST_GATE_PREWRITE_FINAL_BARRIER",
            listener.local_addr()?.to_string(),
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let started = Instant::now();
    let (mut stream, peer) = loop {
        match listener.accept() {
            Ok(accepted) => break accepted,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if let Some(status) = child.try_wait()? {
                    return Err(std::io::Error::other(format!(
                        "pre-write exited before final topology barrier: {status}"
                    ))
                    .into());
                }
                if started.elapsed() >= Duration::from_secs(30) {
                    return Err(std::io::Error::other(
                        "pre-write did not reach the final topology barrier",
                    )
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
    let fields = frame.split_whitespace().collect::<Vec<_>>();
    assert_eq!(fields.len(), 3, "unexpected final barrier frame: {frame:?}");
    assert_eq!(fields[0], "finalizing");
    assert_eq!(fields[1], "op-final-topology");

    fs::hard_link(
        root.path().join("src/main.ts"),
        root.path().join("src/new.ts"),
    )?;
    stream.write_all(b"release\n")?;
    stream.flush()?;
    drop(stream);
    let output = child.wait_with_output()?;
    assert_eq!(
        output.status.code(),
        Some(5),
        "unexpected final-topology pre-write result: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_replayed_mutation(root.path(), &arguments, &output)?;
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response.get("decision").and_then(Value::as_str),
        Some("stale")
    );
    assert_eq!(
        response.get("lifecycle").and_then(Value::as_str),
        Some("rejected")
    );
    assert_eq!(
        response
            .pointer("/observationBinding/state")
            .and_then(Value::as_str),
        Some("sealed"),
        "unexpected final-topology response: {response:#?}"
    );
    assert_eq!(
        response
            .pointer("/observationBinding/observation/kind")
            .and_then(Value::as_str),
        Some("baseline")
    );
    assert!(
        response
            .pointer("/observationBinding/observation/observationId")
            .and_then(Value::as_str)
            .is_some_and(|id| id.starts_with("gate_baseline_observation_"))
    );
    assert!(
        response
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some("protected-input-changed")
            }))
    );
    let changed = response
        .get("signals")
        .and_then(Value::as_array)
        .ok_or("final topology result omitted its signals")?
        .iter()
        .filter(|signal| {
            signal.get("kind").and_then(Value::as_str) == Some("protected-input-changed")
        })
        .filter_map(|signal| signal.get("paths").and_then(Value::as_array))
        .flatten()
        .collect::<Vec<_>>();
    for expected in ["src/main.ts", "src/new.ts"] {
        assert!(
            changed
                .iter()
                .any(|path| { path.get("display").and_then(Value::as_str) == Some(expected) })
        );
    }
    let active = run(root.path(), &["gate", "list", "--active"])?;
    assert_status(&active, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&active.stdout)?
            .get("total")
            .and_then(Value::as_u64),
        Some(0)
    );
    Ok(())
}

fn final_promotion_reenumerates_new_directory_source_aliases()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    fs::create_dir(root.path().join("src/feature"))?;
    fs::write(
        root.path().join("src/feature/existing.ts"),
        "export const existing = 1;\n",
    )?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let arguments = [
        "pre-write",
        "--operation-id",
        "op-final-directory-alias",
        "--path",
        "src/feature",
        "--jobs",
        "1",
    ];
    let mut child = lumin_command_with_args(root.path(), &arguments)?
        .env(
            "LUMIN_TEST_GATE_PREWRITE_FINAL_BARRIER",
            listener.local_addr()?.to_string(),
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let started = Instant::now();
    let (mut stream, peer) = loop {
        match listener.accept() {
            Ok(accepted) => break accepted,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if let Some(status) = child.try_wait()? {
                    return Err(std::io::Error::other(format!(
                        "pre-write exited before directory-alias barrier: {status}"
                    ))
                    .into());
                }
                if started.elapsed() >= Duration::from_secs(30) {
                    return Err(std::io::Error::other(
                        "pre-write did not reach the directory-alias barrier",
                    )
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
    let fields = frame.split_whitespace().collect::<Vec<_>>();
    assert_eq!(
        fields.len(),
        3,
        "unexpected directory-alias frame: {frame:?}"
    );
    assert_eq!(fields[0], "finalizing");
    assert_eq!(fields[1], "op-final-directory-alias");

    fs::write(
        root.path().join("src/feature/late.ts"),
        "export const late = 1;\n",
    )?;
    fs::hard_link(
        root.path().join("src/feature/late.ts"),
        root.path().join("src/late-alias.ts"),
    )?;
    stream.write_all(b"release\n")?;
    stream.flush()?;
    drop(stream);

    let output = child.wait_with_output()?;
    assert_eq!(
        output.status.code(),
        Some(5),
        "unexpected directory-alias result: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_replayed_mutation(root.path(), &arguments, &output)?;
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response.get("decision").and_then(Value::as_str),
        Some("stale")
    );
    assert_eq!(
        response.get("lifecycle").and_then(Value::as_str),
        Some("rejected")
    );
    assert_eq!(
        response
            .pointer("/observationBinding/state")
            .and_then(Value::as_str),
        Some("sealed")
    );
    let changed = response
        .get("signals")
        .and_then(Value::as_array)
        .ok_or("directory-alias result omitted its signals")?
        .iter()
        .filter(|signal| {
            signal.get("kind").and_then(Value::as_str) == Some("protected-input-changed")
        })
        .filter_map(|signal| signal.get("paths").and_then(Value::as_array))
        .flatten()
        .collect::<Vec<_>>();
    for expected in ["src/feature/late.ts", "src/late-alias.ts"] {
        assert!(
            changed
                .iter()
                .any(|path| path.get("display").and_then(Value::as_str) == Some(expected))
        );
    }
    let active = run(root.path(), &["gate", "list", "--active"])?;
    assert_status(&active, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&active.stdout)?
            .get("total")
            .and_then(Value::as_u64),
        Some(0)
    );
    Ok(())
}

fn final_promotion_rejects_a_new_source_outside_the_captured_alias_domain()
-> Result<(), Box<dyn std::error::Error>> {
    final_promotion_rejects_a_late_semantic_input(
        "op-final-source-set",
        "src/late-unrelated.ts",
        "export const lateUnrelated = 1;\n",
    )
}

fn final_promotion_rejects_a_new_configuration_input() -> Result<(), Box<dyn std::error::Error>> {
    final_promotion_rejects_a_late_semantic_input(
        "op-final-config-set",
        ".gitignore",
        "# late semantic policy input\n",
    )
}

fn final_promotion_rejects_a_late_semantic_input(
    operation_id: &str,
    late_path: &str,
    contents: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let arguments = [
        "pre-write",
        "--operation-id",
        operation_id,
        "--path",
        "src/main.ts",
        "--jobs",
        "1",
    ];
    let mut child = lumin_command_with_args(root.path(), &arguments)?
        .env(
            "LUMIN_TEST_GATE_PREWRITE_FINAL_BARRIER",
            listener.local_addr()?.to_string(),
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let started = Instant::now();
    let (mut stream, peer) = loop {
        match listener.accept() {
            Ok(accepted) => break accepted,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if let Some(status) = child.try_wait()? {
                    return Err(std::io::Error::other(format!(
                        "pre-write exited before semantic-input barrier: {status}"
                    ))
                    .into());
                }
                if started.elapsed() >= Duration::from_secs(30) {
                    return Err(std::io::Error::other(
                        "pre-write did not reach the semantic-input barrier",
                    )
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
    let fields = frame.split_whitespace().collect::<Vec<_>>();
    assert_eq!(
        fields.len(),
        3,
        "unexpected semantic-input barrier frame: {frame:?}"
    );
    assert_eq!(fields[0], "finalizing");
    assert_eq!(fields[1], operation_id);

    fs::write(root.path().join(late_path), contents)?;
    stream.write_all(b"release\n")?;
    stream.flush()?;
    drop(stream);

    let output = child.wait_with_output()?;
    assert_eq!(
        output.status.code(),
        Some(5),
        "unexpected semantic-input result: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_replayed_mutation(root.path(), &arguments, &output)?;
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response.get("decision").and_then(Value::as_str),
        Some("stale")
    );
    assert_eq!(
        response.get("lifecycle").and_then(Value::as_str),
        Some("rejected")
    );
    assert_eq!(
        response
            .pointer("/observationBinding/state")
            .and_then(Value::as_str),
        Some("sealed")
    );
    assert!(
        response
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some("protected-input-changed")
                    && signal
                        .get("paths")
                        .and_then(Value::as_array)
                        .is_some_and(|paths| {
                            paths.iter().any(|path| {
                                path.get("display").and_then(Value::as_str) == Some(late_path)
                            })
                        })
            }))
    );
    let active = run(root.path(), &["gate", "list", "--active"])?;
    assert_status(&active, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&active.stdout)?
            .get("total")
            .and_then(Value::as_u64),
        Some(0)
    );
    Ok(())
}

fn final_promotion_preserves_a_sealed_stale_observation_when_an_alias_seed_disappears()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    fs::create_dir(root.path().join("src/feature"))?;
    fs::write(
        root.path().join("src/feature/aaa-retained.ts"),
        "export const retained = 1;\n",
    )?;
    let captured = root.path().join("src/feature/captured.ts");
    fs::write(&captured, "export const captured = 1;\n")?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let arguments = [
        "pre-write",
        "--operation-id",
        "op-final-directory-disappearance",
        "--path",
        "src/feature",
        "--jobs",
        "1",
    ];
    let mut child = lumin_command_with_args(root.path(), &arguments)?
        .env(
            "LUMIN_TEST_GATE_PREWRITE_FINAL_BARRIER",
            listener.local_addr()?.to_string(),
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let started = Instant::now();
    let (mut stream, peer) = loop {
        match listener.accept() {
            Ok(accepted) => break accepted,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if let Some(status) = child.try_wait()? {
                    return Err(std::io::Error::other(format!(
                        "pre-write exited before directory-disappearance barrier: {status}"
                    ))
                    .into());
                }
                if started.elapsed() >= Duration::from_secs(30) {
                    return Err(std::io::Error::other(
                        "pre-write did not reach the directory-disappearance barrier",
                    )
                    .into());
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    };
    assert!(peer.ip().is_loopback());
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let mut frame = String::new();
    BufReader::new(stream.try_clone()?).read_line(&mut frame)?;
    let fields = frame.split_whitespace().collect::<Vec<_>>();
    assert_eq!(fields.first().copied(), Some("finalizing"));
    assert_eq!(
        fields.get(1).copied(),
        Some("op-final-directory-disappearance")
    );

    fs::remove_file(&captured)?;
    stream.write_all(b"release\n")?;
    stream.flush()?;
    drop(stream);

    let output = child.wait_with_output()?;
    assert_eq!(
        output.status.code(),
        Some(5),
        "unexpected directory-disappearance result: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_replayed_mutation(root.path(), &arguments, &output)?;
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response.get("decision").and_then(Value::as_str),
        Some("stale")
    );
    assert_eq!(
        response
            .pointer("/observationBinding/state")
            .and_then(Value::as_str),
        Some("sealed")
    );
    let signals = response
        .get("signals")
        .and_then(Value::as_array)
        .ok_or("directory-disappearance result omitted its signals")?;
    assert!(signals.iter().any(|signal| {
        signal.get("kind").and_then(Value::as_str) == Some("protected-input-changed")
            && signal
                .get("paths")
                .and_then(Value::as_array)
                .is_some_and(|paths| {
                    paths.iter().any(|path| {
                        path.get("display").and_then(Value::as_str)
                            == Some("src/feature/captured.ts")
                    })
                })
    }));
    assert!(
        !signals.iter().any(|signal| {
            signal.get("kind").and_then(Value::as_str) == Some("analysis-failed")
        })
    );
    Ok(())
}

fn final_close_reobserves_the_complete_write_domain() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let opened = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-close-topology-open",
            "--path",
            "src/new.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);
    let gate_id = field(&opened.stdout, "gateId")?;
    let before = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&before, 0);
    let before_json: Value = serde_json::from_str(&before.stdout)?;
    let protected_count = before_json
        .get("protectedSemanticInputCount")
        .and_then(Value::as_u64)
        .ok_or("opening gate omitted its protected input count")?;

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let arguments = [
        "post-write",
        gate_id.as_str(),
        "--operation-id",
        "op-close-topology",
    ];
    let mut child = lumin_command_with_args(root.path(), &arguments)?
        .env(
            "LUMIN_TEST_GATE_POSTWRITE_FINAL_BARRIER",
            listener.local_addr()?.to_string(),
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let started = Instant::now();
    let (mut stream, peer) = loop {
        match listener.accept() {
            Ok(accepted) => break accepted,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if let Some(status) = child.try_wait()? {
                    return Err(std::io::Error::other(format!(
                        "post-write exited before final topology barrier: {status}"
                    ))
                    .into());
                }
                if started.elapsed() >= Duration::from_secs(30) {
                    return Err(std::io::Error::other(
                        "post-write did not reach the final topology barrier",
                    )
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
    let fields = frame.split_whitespace().collect::<Vec<_>>();
    assert_eq!(fields.len(), 3, "unexpected close barrier frame: {frame:?}");
    assert_eq!(fields[0], "close-finalizing");
    assert_eq!(fields[1], "op-close-topology");
    assert_eq!(fields[2], gate_id);

    fs::hard_link(
        root.path().join("src/main.ts"),
        root.path().join("src/new.ts"),
    )?;
    stream.write_all(b"release\n")?;
    stream.flush()?;
    drop(stream);
    let output = child.wait_with_output()?;
    assert_eq!(
        output.status.code(),
        Some(5),
        "unexpected final-topology post-write result: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_replayed_mutation(root.path(), &arguments, &output)?;
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response.get("decision").and_then(Value::as_str),
        Some("stale")
    );
    assert_eq!(
        response.get("lifecycle").and_then(Value::as_str),
        Some("active")
    );
    assert_eq!(
        response
            .pointer("/observationBinding/state")
            .and_then(Value::as_str),
        Some("sealed")
    );
    assert_eq!(
        response
            .pointer("/observationBinding/observation/kind")
            .and_then(Value::as_str),
        Some("close")
    );
    assert!(
        response
            .pointer("/observationBinding/observation/observationId")
            .and_then(Value::as_str)
            .is_some_and(|id| id.starts_with("gate_close_observation_"))
    );
    assert!(response.get("actualWriteSet").is_some());
    assert!(
        response
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some("protected-input-changed")
            }))
    );

    let shown = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&shown, 0);
    let shown_json: Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(
        shown_json.get("lifecycle").and_then(Value::as_str),
        Some("active")
    );
    assert_eq!(
        shown_json
            .get("protectedSemanticInputCount")
            .and_then(Value::as_u64),
        Some(protected_count)
    );
    assert!(
        shown_json
            .pointer("/revisions/1/analysisInputId")
            .is_some_and(|value| !value.is_null())
    );
    assert!(shown_json.pointer("/revisions/1/actualWriteSet").is_some());
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
    assert!(post_json.get("actualWriteSet").is_none());
    assert_eq!(
        post_json
            .pointer("/observationBinding/state")
            .and_then(Value::as_str),
        Some("unsealed")
    );
    assert_eq!(
        post_json
            .pointer("/observationBinding/reason")
            .and_then(Value::as_str),
        Some("protected-input-changed")
    );
    assert!(
        post_json
            .pointer("/observationBinding/observation")
            .is_none()
    );

    let shown = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&shown, 0);
    let shown_json: Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(
        shown_json.pointer("/revisions/1/observationBinding"),
        post_json.get("observationBinding")
    );
    assert!(
        shown_json
            .pointer("/revisions/1/analysisInputId")
            .is_some_and(Value::is_null)
    );
    let operation = run(root.path(), &["operation", "show", "op-close"])?;
    assert_status(&operation, 0);
    let operation_json: Value = serde_json::from_str(&operation.stdout)?;
    assert_eq!(
        operation_json.pointer("/result/observationBinding"),
        post_json.get("observationBinding")
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn escaping_existing_write_target_is_stale_and_withholds_actual_write_attribution()
-> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::symlink;

    let root = source_fixture("console.log('lib');\n", "console.log('main');\n")?;
    let gate_id = open_gate(root.path(), "op-incomplete-open", "src/lib.ts")?;
    let outside = tempfile::tempdir()?;
    let outside_source = outside.path().join("outside.ts");
    fs::write(&outside_source, "export const outside = true;\n")?;
    fs::remove_file(root.path().join("src/lib.ts"))?;
    symlink(&outside_source, root.path().join("src/lib.ts"))?;

    let post = run(
        root.path(),
        &[
            "post-write",
            &gate_id,
            "--operation-id",
            "op-incomplete-close",
        ],
    )?;
    assert_status(&post, 5);
    let post_json: Value = serde_json::from_str(&post.stdout)?;
    assert_eq!(
        post_json.get("decision").and_then(Value::as_str),
        Some("stale")
    );
    assert_has_signal(&post.stdout, "protected-input-changed")?;
    assert!(post_json.get("actualWriteSet").is_none());

    let shown = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&shown, 0);
    let shown_json: Value = serde_json::from_str(&shown.stdout)?;
    assert!(shown_json.pointer("/revisions/1/actualWriteSet").is_none());

    let operation = run(root.path(), &["operation", "show", "op-incomplete-close"])?;
    assert_status(&operation, 0);
    let operation_json: Value = serde_json::from_str(&operation.stdout)?;
    assert!(operation_json.pointer("/result/actualWriteSet").is_none());
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
    assert_eq!(
        pre_json
            .pointer("/observationBinding/state")
            .and_then(Value::as_str),
        Some("unsealed")
    );
    assert_eq!(
        pre_json
            .pointer("/observationBinding/reason")
            .and_then(Value::as_str),
        Some("declared-path-unsupported")
    );
    assert!(
        !pre_json
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some("protected-input-changed")
            }))
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
fn planned_semantic_config_write_is_recaptured_and_attributed()
-> Result<(), Box<dyn std::error::Error>> {
    let root = profile_reconciliation_fixture()?;
    fs::write(
        root.path().join("tsconfig.json"),
        r#"{"compilerOptions":{"moduleResolution":"node16","module":"node16"}}"#,
    )?;
    let gate_id = open_gate(root.path(), "op-config-open", "tsconfig.json")?;
    assert_public_used_findings(root.path(), &gate_id, "0", &["packages/lib/default.ts"])?;
    let opening = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&opening, 0);
    let opening_json: Value = serde_json::from_str(&opening.stdout)?;
    let opening_analysis_input_id = opening_json
        .pointer("/baseline/analysisInputId")
        .and_then(Value::as_str)
        .ok_or("opening gate omitted its analysis input ID")?
        .to_owned();
    let model_gate_id = lumin_model::GateId::from_string(gate_id.clone());
    let persisted_opening = lumin_engine::load_gate(root.path(), &model_gate_id)?;
    let persisted_baseline = persisted_opening
        .baseline
        .as_ref()
        .ok_or_else(|| std::io::Error::other("opening gate omitted its baseline"))?;
    let opening_tier = persisted_baseline.snapshot.scan_invocation.clone();
    let opening_entry_selections = persisted_baseline.snapshot.entry_selections.clone();
    let opening_config_input = persisted_baseline
        .snapshot
        .inputs
        .iter()
        .find(|input| input.path.display == "tsconfig.json")
        .cloned()
        .ok_or_else(|| std::io::Error::other("baseline omitted tsconfig.json input"))?;
    let opening_profiles = &persisted_baseline.snapshot.evidence.resolution_profiles;
    assert_eq!(opening_profiles.len(), 3);
    assert!(opening_profiles.iter().all(|selected| {
        selected.profile == lumin_model::ResolutionProfile::Node16
            && matches!(
                &selected.source,
                lumin_model::ResolutionProfileSource::Config { path_display, .. }
                    if path_display == "tsconfig.json"
            )
    }));
    let opening_profile_sources = opening_profiles
        .iter()
        .map(|selected| selected.source_id.clone())
        .collect::<Vec<_>>();

    fs::write(
        root.path().join("tsconfig.json"),
        r#"{"compilerOptions":{"moduleResolution":"bundler","module":"esnext"}}"#,
    )?;
    let post = run(
        root.path(),
        &["post-write", &gate_id, "--operation-id", "op-config-close"],
    )?;
    assert_status(&post, 0);
    assert_eq!(field(&post.stdout, "decision")?, "allow");
    assert_public_used_findings(root.path(), &gate_id, "1", &[])?;
    let post_json: Value = serde_json::from_str(&post.stdout)?;
    assert_eq!(
        display_paths(&post_json, "/actualWriteSet/paths")?,
        vec!["tsconfig.json"]
    );
    assert_alias_group_members(
        &post_json,
        "/actualWriteSet/baselineAliasClosures",
        &["tsconfig.json"],
    )?;
    assert_alias_group_members(
        &post_json,
        "/actualWriteSet/currentAliasClosures",
        &["tsconfig.json"],
    )?;

    let shown = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&shown, 0);
    let shown_json: Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(
        display_paths(&shown_json, "/revisions/1/actualWriteSet/paths")?,
        vec!["tsconfig.json"]
    );
    let close_analysis_input_id = shown_json
        .pointer("/revisions/1/analysisInputId")
        .and_then(Value::as_str)
        .ok_or("sealed close omitted its current analysis input ID")?;
    assert_ne!(opening_analysis_input_id, close_analysis_input_id);
    assert_eq!(
        shown_json
            .pointer("/revisions/1/observationBinding/observation/kind")
            .and_then(Value::as_str),
        Some("close")
    );
    assert_eq!(
        shown_json
            .pointer("/revisions/1/observationBinding/state")
            .and_then(Value::as_str),
        Some("sealed")
    );

    let operation = run(root.path(), &["operation", "show", "op-config-close"])?;
    assert_status(&operation, 0);
    let operation_json: Value = serde_json::from_str(&operation.stdout)?;
    assert_eq!(
        operation_json.pointer("/result/observationBinding"),
        shown_json.pointer("/revisions/1/observationBinding")
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
    assert_eq!(
        close_snapshot.analysis_input_id.as_str(),
        close_analysis_input_id
    );
    let close_config_input = close_snapshot
        .inputs
        .iter()
        .find(|input| input.path.display == "tsconfig.json")
        .ok_or_else(|| std::io::Error::other("sealed close omitted tsconfig.json input"))?;
    assert_ne!(close_config_input, &opening_config_input);
    assert_ne!(
        close_config_input.payload_sha256,
        opening_config_input.payload_sha256
    );
    let close_profiles = &close_snapshot.evidence.resolution_profiles;
    assert_eq!(close_profiles.len(), 3);
    assert_eq!(
        close_profiles
            .iter()
            .map(|selected| selected.source_id.clone())
            .collect::<Vec<_>>(),
        opening_profile_sources
    );
    assert!(close_profiles.iter().all(|selected| {
        selected.profile == lumin_model::ResolutionProfile::Bundler
            && matches!(
                &selected.source,
                lumin_model::ResolutionProfileSource::Config { path_display, .. }
                    if path_display == "tsconfig.json"
            )
    }));
    assert!(lumin_engine::gate_observation_binding_matches_owner(
        &persisted_closed,
        persisted_close
    )?);
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
fn empty_directory_gate_protects_all_opening_sources() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    fs::create_dir(root.path().join("src/empty-feature"))?;
    let gate_id = open_gate(root.path(), "op-dir-protected-open", "src/empty-feature")?;

    fs::write(
        root.path().join("src/lib.ts"),
        "export const used = 2; export const dead = 2;\n",
    )?;
    let post = run(
        root.path(),
        &[
            "post-write",
            &gate_id,
            "--operation-id",
            "op-dir-protected-close",
        ],
    )?;
    assert_status(&post, 5);
    assert_eq!(field(&post.stdout, "decision")?, "stale");
    assert_has_signal(&post.stdout, "protected-input-changed")?;
    Ok(())
}

#[test]
fn nonempty_directory_gate_protects_sources_outside_the_directory()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    fs::create_dir(root.path().join("src/feature"))?;
    fs::write(
        root.path().join("src/feature/existing.ts"),
        "console.log('existing');\n",
    )?;
    let gate_id = open_gate(root.path(), "op-nonempty-dir-protected-open", "src/feature")?;

    fs::write(root.path().join("src/lib.ts"), "export const used = 2;\n")?;
    let post = run(
        root.path(),
        &[
            "post-write",
            &gate_id,
            "--operation-id",
            "op-nonempty-dir-protected-close",
        ],
    )?;
    assert_status(&post, 5);
    assert_eq!(field(&post.stdout, "decision")?, "stale");
    assert_has_signal(&post.stdout, "protected-input-changed")?;
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
    let post_json: Value = serde_json::from_str(&post.stdout)?;
    assert!(
        post_json
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some("unplanned-write")
            }))
    );
    assert_display_path_set(
        &post_json,
        "/actualWriteSet/paths",
        &["src/alias.ts", "src/late-alias.ts", "src/original.ts"],
    )?;
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
    let post_json: Value = serde_json::from_str(&post.stdout)?;
    assert_display_path_set(
        &post_json,
        "/actualWriteSet/paths",
        &["src/alias.ts", "src/original.ts"],
    )?;
    assert_alias_group_members(
        &post_json,
        "/actualWriteSet/baselineAliasClosures",
        &["src/alias.ts", "src/original.ts"],
    )?;
    assert_alias_group_members(
        &post_json,
        "/actualWriteSet/currentAliasClosures",
        &["src/alias.ts", "src/original.ts"],
    )?;

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

fn display_paths<'a>(
    value: &'a Value,
    pointer: &str,
) -> Result<Vec<&'a str>, Box<dyn std::error::Error>> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other(format!("missing path array at {pointer}")))?
        .iter()
        .map(|path| {
            path.get("display").and_then(Value::as_str).ok_or_else(|| {
                std::io::Error::other(format!("path at {pointer} omitted display")).into()
            })
        })
        .collect()
}

fn assert_alias_group_members(
    value: &Value,
    pointer: &str,
    expected: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    let groups = value
        .pointer(pointer)
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other(format!("missing alias groups at {pointer}")))?;
    let mut expected = expected.to_vec();
    expected.sort_unstable();
    assert!(groups.iter().any(|group| {
        group
            .get("members")
            .and_then(Value::as_array)
            .and_then(|members| {
                members
                    .iter()
                    .map(|member| member.get("display").and_then(Value::as_str))
                    .collect::<Option<Vec<_>>>()
            })
            .is_some_and(|mut members| {
                members.sort_unstable();
                members == expected
            })
    }));
    Ok(())
}

fn assert_display_path_set(
    value: &Value,
    pointer: &str,
    expected: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut actual = display_paths(value, pointer)?;
    actual.sort_unstable();
    let mut expected = expected.to_vec();
    expected.sort_unstable();
    assert_eq!(actual, expected);
    Ok(())
}

fn assert_replayed_mutation(
    root: &Path,
    arguments: &[&str],
    output: &std::process::Output,
) -> Result<(), Box<dyn std::error::Error>> {
    let replay = run(root, arguments)?;
    assert_eq!(replay.status, output.status.code().unwrap_or(-1));
    assert_eq!(replay.stdout.as_bytes(), output.stdout.as_slice());
    assert_eq!(replay.stderr.as_bytes(), output.stderr.as_slice());
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

fn semantic_read_closure_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::create_dir(root.path().join("config"))?;
    fs::create_dir(root.path().join("shared"))?;
    fs::write(root.path().join("src/a.ts"), "console.log('a');\n")?;
    fs::write(
        root.path().join("config/helper.ts"),
        "console.log('helper');\n",
    )?;
    fs::write(
        root.path().join("config/base.json"),
        "{\"extends\":\"../shared/root\",\"compilerOptions\":{}}\n",
    )?;
    fs::write(
        root.path().join("shared/root.json"),
        "{\"compilerOptions\":{}}\n",
    )?;
    Ok(root)
}

fn fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    source_fixture(
        "export const used = 1;\n",
        "import { used } from './lib'; console.log(used);\n",
    )
}

fn profile_reconciliation_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::create_dir_all(root.path().join("packages/lib"))?;
    fs::write(
        root.path().join("package.json"),
        r#"{"name":"app","private":true,"type":"module","workspaces":["packages/*"]}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/package.json"),
        r#"{"name":"@acme/lib","private":true,"exports":{"node":"./node.js","default":"./default.js"}}"#,
    )?;
    fs::write(
        root.path().join("packages/lib/node.ts"),
        "console.log('node branch');\n",
    )?;
    fs::write(
        root.path().join("packages/lib/default.ts"),
        "export const used = 1;\n",
    )?;
    fs::write(
        root.path().join("src/main.ts"),
        "import { used } from '@acme/lib'; console.log(used);\n",
    )?;
    Ok(root)
}

fn assert_public_used_findings(
    root: &Path,
    gate_id: &str,
    revision: &str,
    expected_paths: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    let response = run(root, &["gate", "findings", gate_id, "--revision", revision])?;
    assert_status(&response, 0);
    let response: Value = serde_json::from_str(&response.stdout)?;
    let used_paths = response
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("gate findings items are missing"))?
        .iter()
        .filter(|finding| finding.get("exportedName").and_then(Value::as_str) == Some("used"))
        .filter_map(|finding| {
            finding
                .pointer("/path/display")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect::<BTreeSet<_>>();
    let expected_paths = expected_paths
        .iter()
        .map(|path| (*path).to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(used_paths, expected_paths, "stdout={}", response);
    Ok(())
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
