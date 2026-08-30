use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::path::Path;
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

use lumin_model::{CapabilityIntentKind, GateId, Limitation, LogicalSourceId, RepoPath};
use serde_json::Value;

mod support;

use support::{assert_status, field, lumin_command, run};

#[test]
fn capability_unavailability_has_one_owner() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(root.path(), "src/main.ts", "console.log('supported');\n")?;
    write(root.path(), "src/lib.rs", "pub fn unavailable() {}\n")?;

    let arguments = [
        "pre-write",
        "--operation-id",
        "op-capability-unavailable",
        "--path",
        "src/main.ts",
        "--path",
        "src/lib.rs",
        "--capability-at",
        "src/main.ts",
        "shape",
        "--capability-at",
        "src/main.ts",
        "clone",
        "--capability-at",
        "src/main.ts",
        "type-escape",
        "--jobs",
        "1",
    ];
    let opened = run(root.path(), &arguments)?;
    assert_status(&opened, 4);
    assert!(opened.stderr.is_empty());
    let opened_json: Value = serde_json::from_str(&opened.stdout)?;
    assert_eq!(
        opened_json.get("decision").and_then(Value::as_str),
        Some("incomplete")
    );
    assert_eq!(
        opened_json.get("lifecycle").and_then(Value::as_str),
        Some("rejected")
    );
    let signals = opened_json
        .get("signals")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("capability gate omitted signals"))?;
    assert_eq!(signals.len(), 1, "unexpected signals: {signals:?}");
    assert_eq!(
        signals[0].get("kind").and_then(Value::as_str),
        Some("required-owner-unavailable")
    );
    assert_eq!(signals[0].get("count").and_then(Value::as_u64), Some(4));

    let replay = run(root.path(), &arguments)?;
    assert_status(&replay, 4);
    assert_eq!(replay.stdout, opened.stdout);

    let gate_id = GateId::from_string(field(&opened.stdout, "gateId")?);
    let shown = run(root.path(), &["gate", "show", gate_id.as_str()])?;
    assert_status(&shown, 0);
    let shown_json: Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(
        shown_json
            .pointer("/baseline/limitationCount")
            .and_then(Value::as_u64),
        Some(4)
    );
    assert_eq!(
        shown_json
            .pointer("/revisions/0/signals/0/kind")
            .and_then(Value::as_str),
        Some("required-owner-unavailable")
    );
    let gate = lumin_engine::load_gate(root.path(), &gate_id)?;
    let baseline = gate
        .baseline
        .as_ref()
        .ok_or_else(|| std::io::Error::other("capability gate omitted its baseline"))?;
    assert_eq!(baseline.snapshot.evidence.capabilities.len(), 5);
    assert_eq!(
        baseline
            .snapshot
            .evidence
            .source_contexts
            .iter()
            .map(|context| context.path.display.as_str())
            .collect::<Vec<_>>(),
        vec!["src/main.ts"]
    );
    assert!(baseline.snapshot.evidence.findings.is_empty());
    assert_eq!(
        baseline.snapshot.scan_invocation.capability_intents.len(),
        4
    );
    assert_eq!(gate.revisions[0].signals.len(), 1);

    let mut availability = BTreeMap::new();
    for limitation in &baseline.snapshot.evidence.limitations {
        let Limitation::CapabilityUnavailable {
            capability,
            targets,
        } = limitation
        else {
            return Err(format!(
                "non-registry limitation entered the availability gate: {limitation:?}"
            )
            .into());
        };
        assert!(availability.insert(*capability, targets.clone()).is_none());
    }
    let main = LogicalSourceId::from_path(&RepoPath::from_portable("src/main.ts")?);
    let rust = LogicalSourceId::from_path(&RepoPath::from_portable("src/lib.rs")?);
    assert_eq!(
        availability,
        BTreeMap::from([
            (CapabilityIntentKind::Rust, vec![rust]),
            (CapabilityIntentKind::Shape, vec![main.clone()]),
            (CapabilityIntentKind::Clone, vec![main.clone()]),
            (CapabilityIntentKind::Discipline, vec![main]),
        ])
    );

    assert_eq!(
        public_capability_states(root.path())?,
        expected_public_capabilities()
    );
    Ok(())
}

#[test]
fn directory_write_domain_requires_the_unavailable_rust_owner()
-> Result<(), Box<dyn std::error::Error>> {
    let existing = tempfile::tempdir()?;
    write(
        existing.path(),
        "src/main.ts",
        "export const supported = true;\n",
    )?;
    write(
        existing.path(),
        "src/native/lib.rs",
        "pub fn unavailable() {}\n",
    )?;
    let existing_arguments = [
        "pre-write",
        "--operation-id",
        "op-directory-existing-rust",
        "--path",
        "src",
        "--jobs",
        "1",
    ];
    let rejected = run(existing.path(), &existing_arguments)?;
    assert_status(&rejected, 4);
    assert!(rejected.stderr.is_empty());
    assert_eq!(field(&rejected.stdout, "decision")?, "incomplete");
    assert_eq!(field(&rejected.stdout, "lifecycle")?, "rejected");
    let rejected_json: Value = serde_json::from_str(&rejected.stdout)?;
    assert_eq!(
        rejected_json
            .pointer("/signals/0/kind")
            .and_then(Value::as_str),
        Some("required-owner-unavailable")
    );
    let replay = run(existing.path(), &existing_arguments)?;
    assert_status(&replay, 4);
    assert_eq!(replay.stdout, rejected.stdout);

    let root = tempfile::tempdir()?;
    write(root.path(), "src/main.ts", "console.log('supported');\n")?;
    let arguments = [
        "pre-write",
        "--operation-id",
        "op-directory-rust-unavailable",
        "--path",
        "src",
        "--jobs",
        "1",
    ];
    let opened = run(root.path(), &arguments)?;
    assert_status(&opened, 0);
    assert!(opened.stderr.is_empty());
    let opened_json: Value = serde_json::from_str(&opened.stdout)?;
    assert_eq!(
        opened_json.get("decision").and_then(Value::as_str),
        Some("allow")
    );
    assert_eq!(
        opened_json.get("lifecycle").and_then(Value::as_str),
        Some("active")
    );

    let open_replay = run(root.path(), &arguments)?;
    assert_status(&open_replay, 0);
    assert_eq!(open_replay.stdout, opened.stdout);

    let gate_id = GateId::from_string(field(&opened.stdout, "gateId")?);
    let opened_gate = lumin_engine::load_gate(root.path(), &gate_id)?;
    let baseline = opened_gate
        .baseline
        .as_ref()
        .ok_or_else(|| std::io::Error::other("directory gate omitted its baseline"))?;
    assert_eq!(
        baseline.snapshot.scan_invocation.capability_intents.len(),
        1
    );
    let intent = &baseline.snapshot.scan_invocation.capability_intents[0];
    assert_eq!(intent.capability, CapabilityIntentKind::Rust);
    assert_eq!(intent.path.display, "src");
    assert!(
        !baseline
            .snapshot
            .evidence
            .limitations
            .iter()
            .any(|limitation| {
                matches!(
                    limitation,
                    Limitation::CapabilityUnavailable {
                        capability: CapabilityIntentKind::Rust,
                        ..
                    }
                )
            })
    );

    write(
        root.path(),
        "src/generated/lib.rs",
        "pub fn newly_unavailable() {}\n",
    )?;
    let close_arguments = [
        "post-write",
        gate_id.as_str(),
        "--operation-id",
        "op-directory-new-rust-close",
    ];
    let closed = run(root.path(), &close_arguments)?;
    assert_status(&closed, 4);
    assert!(closed.stderr.is_empty());
    assert_eq!(field(&closed.stdout, "decision")?, "incomplete");
    assert_eq!(field(&closed.stdout, "lifecycle")?, "active");
    let closed_json: Value = serde_json::from_str(&closed.stdout)?;
    assert!(
        closed_json
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some("required-owner-unavailable")
                    && signal.get("count").and_then(Value::as_u64) == Some(1)
            }))
    );
    let close_replay = run(root.path(), &close_arguments)?;
    assert_status(&close_replay, 4);
    assert_eq!(close_replay.stdout, closed.stdout);

    let gate = lumin_engine::load_gate(root.path(), &gate_id)?;
    let close_revision = gate
        .revisions
        .last()
        .ok_or_else(|| std::io::Error::other("failed directory close omitted its revision"))?;
    assert!(close_revision.snapshot.is_none());
    assert!(close_revision.signals.iter().any(|signal| {
        serde_json::to_value(signal).is_ok_and(|signal| {
            signal.get("kind").and_then(Value::as_str) == Some("required-owner-unavailable")
                && signal.get("limitation_count").and_then(Value::as_u64) == Some(1)
        })
    }));
    rust_descendant_created_after_initial_traversal_cannot_seal()?;
    Ok(())
}

fn rust_descendant_created_after_initial_traversal_cannot_seal()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "src/main.ts",
        "export const supported = true;\n",
    )?;
    fs::create_dir_all(root.path().join("src/native"))?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let arguments = [
        "pre-write",
        "--operation-id",
        "op-directory-late-rust",
        "--path",
        "src",
        "--jobs",
        "1",
    ];
    let os_arguments = arguments
        .iter()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();
    let effective_arguments = support::determinism::effective_arguments(&os_arguments)?;
    let mut child = lumin_command(root.path())?
        .args(effective_arguments)
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
                        "pre-write exited before the final capability barrier: {status}"
                    ))
                    .into());
                }
                if started.elapsed() >= Duration::from_secs(30) {
                    return Err(std::io::Error::other(
                        "pre-write did not reach the final capability barrier",
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
    assert_eq!(fields[1], "op-directory-late-rust");

    write(
        root.path(),
        "src/native/lib.rs",
        "pub fn appeared_during_pre_write() {}\n",
    )?;
    stream.write_all(b"release\n")?;
    stream.flush()?;
    drop(stream);

    let output = child.wait_with_output()?;
    assert_eq!(
        output.status.code(),
        Some(4),
        "unexpected final capability result: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout)?;
    let response: Value = serde_json::from_str(&stdout)?;
    assert_eq!(
        response.get("decision").and_then(Value::as_str),
        Some("incomplete")
    );
    assert_eq!(
        response.get("lifecycle").and_then(Value::as_str),
        Some("rejected")
    );
    assert!(
        response
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some("analysis-failed")
                    && signal
                        .get("detail")
                        .and_then(Value::as_str)
                        .is_some_and(|detail| {
                            detail.contains(
                                "semantic input link or mount topology changed after capture",
                            )
                        })
            }))
    );

    let replay = run(root.path(), &arguments)?;
    assert_status(&replay, 4);
    assert_eq!(replay.stdout, stdout);
    Ok(())
}

#[test]
fn capability_intent_syntax_is_closed_before_state_initialization()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(root.path(), "src/main.ts", "console.log('supported');\n")?;
    let rejected = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-invalid-capability",
            "--path",
            "src/main.ts",
            "--capability-at",
            "src/main.ts",
            "rust",
        ],
    )?;
    assert_status(&rejected, 2);
    assert!(rejected.stdout.is_empty());
    assert_eq!(rejected.stderr, "lumin: unknown capability intent: rust\n");
    assert!(!root.path().join(".lumin").exists());

    let outside = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-outside-capability",
            "--path",
            "src/main.ts",
            "--capability-at",
            "src/other.ts",
            "shape",
        ],
    )?;
    assert_status(&outside, 2);
    assert!(outside.stdout.is_empty());
    assert_eq!(
        outside.stderr,
        "lumin: capability intent is outside the declared write set: src/other.ts\n"
    );
    assert!(!root.path().join(".lumin").exists());
    Ok(())
}

fn public_capability_states(
    root: &Path,
) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let mut states = BTreeMap::new();
    let mut cursor = None;
    loop {
        let mut arguments = vec!["capabilities"];
        if let Some(value) = cursor.as_deref() {
            arguments.extend(["--cursor", value]);
        }
        let page = run(root, &arguments)?;
        assert_status(&page, 0);
        let response: Value = serde_json::from_str(&page.stdout)?;
        for row in response
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| std::io::Error::other("capabilities page omitted items"))?
        {
            let capability_id = row
                .get("capabilityId")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("capability row omitted its ID"))?;
            let state = row
                .get("state")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("capability row omitted its state"))?;
            assert!(
                states
                    .insert(capability_id.to_owned(), state.to_owned())
                    .is_none(),
                "duplicate public capability {capability_id}"
            );
        }
        cursor = response
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    Ok(states)
}

fn expected_public_capabilities() -> BTreeMap<String, String> {
    [
        ("dead-code.v1", "complete"),
        ("inventory/dependency-ownership.v1", "complete"),
        ("sfc/astro.v1", "unavailable"),
        ("sfc/svelte.v1", "unavailable"),
        ("sfc/vue.v1", "complete"),
    ]
    .into_iter()
    .map(|(capability, state)| (capability.to_owned(), state.to_owned()))
    .collect()
}

fn write(root: &Path, path: &str, contents: &str) -> std::io::Result<()> {
    let path = root.join(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}
