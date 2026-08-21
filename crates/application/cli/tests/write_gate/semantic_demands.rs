use super::*;

#[test]
fn pre_write_reserves_semantic_demands_before_capture_and_retries_after_writer_terminal()
-> Result<(), Box<dyn std::error::Error>> {
    let root = semantic_read_closure_fixture()?;
    fs::write(
        root.path().join("src/tsconfig.json"),
        "{\"extends\":\"../config/base.json\"}\n",
    )?;
    let gate_b = open_gate(root.path(), "op-b-open", "config")?;

    fs::write(root.path().join("config/base.json"), "{malformed\n")?;
    let pending_a = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-a-demand-pending",
            "--path",
            "src/new.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&pending_a, 4);
    assert_eq!(field(&pending_a.stdout, "decision")?, "incomplete");
    assert_eq!(field(&pending_a.stdout, "lifecycle")?, "rejected");
    let pending_json: Value = serde_json::from_str(&pending_a.stdout)?;
    let signals = pending_json
        .get("signals")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("pre-write signals are missing"))?;
    let conflict = signals
        .iter()
        .find(|signal| {
            signal.get("kind").and_then(Value::as_str) == Some("semantic-input-conflict")
        })
        .ok_or_else(|| std::io::Error::other("semantic input conflict is missing"))?;
    assert_eq!(
        conflict.pointer("/paths/0/display").and_then(Value::as_str),
        Some("config/base.json")
    );
    assert_eq!(
        conflict.pointer("/gateIds/0").and_then(Value::as_str),
        Some(gate_b.as_str())
    );
    assert!(
        !signals.iter().any(|signal| {
            signal.get("kind").and_then(Value::as_str) == Some("analysis-failed")
        })
    );

    let blocked_operation = run(root.path(), &["operation", "show", "op-a-demand-pending"])?;
    assert_status(&blocked_operation, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&blocked_operation.stdout)?
            .get("semanticReadReservations")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );

    fs::write(
        root.path().join("config/base.json"),
        "{\"extends\":\"../shared/root\",\"compilerOptions\":{}}\n",
    )?;
    let close_b = run(
        root.path(),
        &["post-write", &gate_b, "--operation-id", "op-b-close"],
    )?;
    assert_status(&close_b, 0);
    assert_eq!(field(&close_b.stdout, "decision")?, "allow");

    let opened_a = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-a-open",
            "--path",
            "src/new.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened_a, 0);
    assert_eq!(field(&opened_a.stdout, "decision")?, "allow");

    let operation = run(root.path(), &["operation", "show", "op-a-open"])?;
    assert_status(&operation, 0);
    let operation_json: Value = serde_json::from_str(&operation.stdout)?;
    let reservation_paths = operation_json
        .get("semanticReadReservations")
        .and_then(Value::as_array)
        .and_then(|paths| {
            paths
                .iter()
                .map(|path| path.get("display").and_then(Value::as_str))
                .collect::<Option<Vec<_>>>()
        })
        .ok_or_else(|| std::io::Error::other("semantic read reservations are missing"))?;
    assert!(reservation_paths.is_empty());
    Ok(())
}

#[test]
fn close_time_new_semantic_demand_outside_lease_stays_unplanned_on_retry()
-> Result<(), Box<dyn std::error::Error>> {
    let root = semantic_read_closure_fixture()?;
    fs::write(root.path().join("config/base.json"), "{}\n")?;
    fs::hard_link(
        root.path().join("config/base.json"),
        root.path().join("src/config-writer.ts"),
    )?;
    let gate_id = open_gate(root.path(), "op-demand-open", "src/a.ts")?;
    let writer_gate = open_gate(root.path(), "op-demand-writer-open", "src/config-writer.ts")?;

    fs::write(
        root.path().join("src/tsconfig.json"),
        "{\"extends\":\"../config/base.json\"}\n",
    )?;
    let blocked = run(
        root.path(),
        &[
            "post-write",
            &gate_id,
            "--operation-id",
            "op-demand-blocked-close",
        ],
    )?;
    assert_status(&blocked, 4);
    assert_eq!(field(&blocked.stdout, "decision")?, "incomplete");
    assert_has_signal(&blocked.stdout, "semantic-input-conflict")?;

    fs::remove_file(root.path().join("src/tsconfig.json"))?;
    let writer_close = run(
        root.path(),
        &[
            "post-write",
            &writer_gate,
            "--operation-id",
            "op-demand-writer-close",
        ],
    )?;
    assert_status(&writer_close, 0);
    assert_eq!(field(&writer_close.stdout, "decision")?, "allow");

    fs::write(
        root.path().join("src/tsconfig.json"),
        "{\"extends\":\"../config/base.json\"}\n",
    )?;
    let first = run(
        root.path(),
        &[
            "post-write",
            &gate_id,
            "--operation-id",
            "op-demand-first-close",
        ],
    )?;
    assert_status(&first, 3);
    assert_eq!(field(&first.stdout, "decision")?, "deny");
    assert_has_signal(&first.stdout, "unplanned-write")?;
    assert_eq!(
        serde_json::from_str::<Value>(&first.stdout)?
            .pointer("/observationBinding/state")
            .and_then(Value::as_str),
        Some("sealed")
    );
    let after_first = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&after_first, 0);
    let protected_after_first = serde_json::from_str::<Value>(&after_first.stdout)?
        .get("protectedSemanticInputCount")
        .and_then(Value::as_u64)
        .ok_or_else(|| std::io::Error::other("first close protected input count is missing"))?;

    fs::write(
        root.path().join("config/base.json"),
        "{\"extends\":\"../shared/root\",\"compilerOptions\":{\"strict\":true}}\n",
    )?;
    let retry = run(
        root.path(),
        &[
            "post-write",
            &gate_id,
            "--operation-id",
            "op-demand-retry-close",
        ],
    )?;
    assert_status(&retry, 5);
    assert_eq!(field(&retry.stdout, "decision")?, "stale");
    assert_has_signal(&retry.stdout, "unplanned-write")?;
    assert_has_signal(&retry.stdout, "protected-input-changed")?;
    assert_eq!(
        serde_json::from_str::<Value>(&retry.stdout)?
            .pointer("/observationBinding/state")
            .and_then(Value::as_str),
        Some("unsealed")
    );

    let operation = run(root.path(), &["operation", "show", "op-demand-retry-close"])?;
    assert_status(&operation, 0);
    let operation_json: Value = serde_json::from_str(&operation.stdout)?;
    let reservation_paths = operation_json
        .get("semanticReadReservations")
        .and_then(Value::as_array)
        .and_then(|paths| {
            paths
                .iter()
                .map(|path| path.get("display").and_then(Value::as_str))
                .collect::<Option<Vec<_>>>()
        })
        .ok_or_else(|| std::io::Error::other("semantic read reservations are missing"))?;
    assert!(reservation_paths.is_empty());

    let shown = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&shown, 0);
    let shown_json: Value = serde_json::from_str(&shown.stdout)?;
    let baseline_count = shown_json
        .pointer("/baseline/protectedSemanticInputCount")
        .and_then(Value::as_u64)
        .ok_or_else(|| std::io::Error::other("baseline protected input count is missing"))?;
    let current_count = shown_json
        .get("protectedSemanticInputCount")
        .and_then(Value::as_u64)
        .ok_or_else(|| std::io::Error::other("current protected input count is missing"))?;
    assert!(protected_after_first > baseline_count);
    assert_eq!(current_count, protected_after_first);
    assert_eq!(field(&shown.stdout, "lifecycle")?, "active");
    Ok(())
}

#[test]
fn failed_close_rechecks_a_semantic_conflict_at_the_final_barrier()
-> Result<(), Box<dyn std::error::Error>> {
    let root = semantic_read_closure_fixture()?;
    fs::write(root.path().join("config/base.json"), "{}\n")?;
    fs::hard_link(
        root.path().join("config/base.json"),
        root.path().join("src/config-writer.ts"),
    )?;
    let gate_id = open_gate(root.path(), "op-conflict-recheck-open", "src/a.ts")?;
    let writer_gate = open_gate(
        root.path(),
        "op-conflict-recheck-writer-open",
        "src/config-writer.ts",
    )?;
    fs::write(
        root.path().join("src/tsconfig.json"),
        "{\"extends\":\"../config/base.json\"}\n",
    )?;

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let mut child = lumin_command(root.path())?
        .args([
            "post-write",
            &gate_id,
            "--operation-id",
            "op-conflict-recheck-close",
        ])
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
                        "post-write exited before conflict recheck barrier: {status}"
                    ))
                    .into());
                }
                if started.elapsed() >= Duration::from_secs(30) {
                    return Err(std::io::Error::other(
                        "post-write did not reach the conflict recheck barrier",
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
    assert_eq!(
        frame.trim_end(),
        format!("close-finalizing op-conflict-recheck-close {gate_id}")
    );

    let abandoned = run(
        root.path(),
        &[
            "gate",
            "abandon",
            &writer_gate,
            "--operation-id",
            "op-conflict-recheck-abandon",
            "--reason",
            "release the semantic input",
        ],
    )?;
    assert_status(&abandoned, 0);
    assert_eq!(field(&abandoned.stdout, "decision")?, "deny");

    stream.write_all(b"release\n")?;
    stream.flush()?;
    drop(stream);
    let output = child.wait_with_output()?;
    assert_eq!(
        output.status.code(),
        Some(5),
        "unexpected conflict-recheck result: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response.get("decision").and_then(Value::as_str),
        Some("stale")
    );
    assert_eq!(
        response
            .pointer("/observationBinding/state")
            .and_then(Value::as_str),
        Some("unsealed")
    );
    assert!(
        response
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some("transition-catalog-changed")
            }))
    );
    assert!(
        !response
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some("semantic-input-conflict")
            }))
    );
    assert!(
        response
            .pointer("/observationBinding/attemptedDomain")
            .and_then(Value::as_array)
            .is_some_and(|paths| paths.iter().any(|path| {
                path.get("display").and_then(Value::as_str) == Some("config/base.json")
            }))
    );
    Ok(())
}
