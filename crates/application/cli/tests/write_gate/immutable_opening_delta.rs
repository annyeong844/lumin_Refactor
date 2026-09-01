use super::*;

#[test]
fn immutable_opening_delta_survives_repeated_failed_and_sealed_stale_closes()
-> Result<(), Box<dyn std::error::Error>> {
    repeated_failed_close_uses_the_opening_baseline()?;
    sealed_stale_close_keeps_the_prior_protected_reads()?;
    Ok(())
}

fn repeated_failed_close_uses_the_opening_baseline() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let gate_id = open_gate(root.path(), "op-immutable-open", "src/lib.ts")?;
    let opening = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&opening, 0);
    let opening_json: Value = serde_json::from_str(&opening.stdout)?;
    let opening_baseline = opening_json
        .get("baseline")
        .cloned()
        .ok_or_else(|| std::io::Error::other("opening gate omitted its baseline"))?;
    let model_gate_id = lumin_model::GateId::from_string(gate_id.clone());
    let opening_record = lumin_engine::load_gate(root.path(), &model_gate_id)?;
    let opening_record_baseline = opening_record
        .baseline
        .clone()
        .ok_or_else(|| std::io::Error::other("opening gate record omitted its baseline"))?;

    fs::write(
        root.path().join("src/lib.ts"),
        "export const renamed = 1;\n",
    )?;

    let first_arguments = [
        "post-write",
        gate_id.as_str(),
        "--operation-id",
        "op-immutable-close-1",
    ];
    let first = run(root.path(), &first_arguments)?;
    assert_introduced_close(&first)?;

    let second_arguments = [
        "post-write",
        gate_id.as_str(),
        "--operation-id",
        "op-immutable-close-2",
    ];
    let second = run(root.path(), &second_arguments)?;
    assert_introduced_close(&second)?;

    for operation_id in ["op-immutable-close-1", "op-immutable-close-2"] {
        let operation = run(root.path(), &["operation", "show", operation_id])?;
        assert_status(&operation, 0);
        let operation_json: Value = serde_json::from_str(&operation.stdout)?;
        assert_eq!(
            operation_json
                .pointer("/result/deltas/0/classification/kind")
                .and_then(Value::as_str),
            Some("introduced")
        );
    }

    let shown = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&shown, 0);
    let shown_json: Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(shown_json.get("baseline"), Some(&opening_baseline));
    assert_eq!(
        shown_json
            .get("revisions")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(3)
    );
    for revision in [1, 2] {
        assert_eq!(
            shown_json
                .pointer(&format!(
                    "/revisions/{revision}/deltas/0/classification/kind"
                ))
                .and_then(Value::as_str),
            Some("introduced")
        );
    }
    let persisted = lumin_engine::load_gate(root.path(), &model_gate_id)?;
    assert_eq!(persisted.baseline.as_ref(), Some(&opening_record_baseline));

    let retry = run(root.path(), &second_arguments)?;
    assert_status(&retry, 3);
    assert_eq!(retry.stdout, second.stdout);
    let shown_after_retry = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&shown_after_retry, 0);
    assert_eq!(shown_after_retry.stdout, shown.stdout);
    Ok(())
}

fn assert_introduced_close(
    result: &support::ProcessResult,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_status(result, 3);
    let response: Value = serde_json::from_str(&result.stdout)?;
    assert_eq!(
        response.get("decision").and_then(Value::as_str),
        Some("deny")
    );
    assert_eq!(
        response.get("lifecycle").and_then(Value::as_str),
        Some("active")
    );
    assert_delta(&result.stdout, "dead-export", "introduced")?;
    assert_has_signal(&result.stdout, "adverse-fact-introduced")?;
    assert!(
        !response
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some("pre-existing-adverse-facts")
            }))
    );
    Ok(())
}

fn sealed_stale_close_keeps_the_prior_protected_reads() -> Result<(), Box<dyn std::error::Error>> {
    let root = semantic_read_closure_fixture()?;
    let opened = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-stale-protection-open",
            "--path",
            "src",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);
    let gate_id = field(&opened.stdout, "gateId")?;
    let model_gate_id = lumin_model::GateId::from_string(gate_id.clone());
    let opening = lumin_engine::load_gate(root.path(), &model_gate_id)?;
    let opening_baseline = opening
        .baseline
        .clone()
        .ok_or_else(|| std::io::Error::other("opening gate record omitted its baseline"))?;
    let prior_protected_reads = opening.protected_semantic_inputs.clone();
    assert!(!prior_protected_reads.iter().any(|input| {
        matches!(
            input.path.display.as_str(),
            "config/base.json" | "shared/root.json"
        )
    }));

    fs::write(
        root.path().join("src/tsconfig.json"),
        "{\"extends\":\"../config/base.json\"}\n",
    )?;

    let arguments = [
        "post-write",
        gate_id.as_str(),
        "--operation-id",
        "op-stale-protection-close",
    ];
    let os_arguments = arguments
        .iter()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();
    let effective_arguments = support::determinism::effective_arguments(&os_arguments)?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let mut child = lumin_command(root.path())?
        .args(&effective_arguments)
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
                        "post-write exited before the stale-protection barrier: {status}"
                    ))
                    .into());
                }
                if started.elapsed() >= Duration::from_secs(30) {
                    return Err(std::io::Error::other(
                        "post-write did not reach the stale-protection barrier",
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
    assert_eq!(
        frame.trim_end(),
        format!("close-finalizing op-stale-protection-close {gate_id}")
    );

    fs::write(
        root.path().join("shared/root.json"),
        "{\"compilerOptions\":{\"strict\":true}}\n",
    )?;
    stream.write_all(b"release\n")?;
    stream.flush()?;
    drop(stream);

    let output = child.wait_with_output()?;
    let result = support::finish_process_output(root.path(), &effective_arguments, output)?;
    assert_status(&result, 5);
    let response: Value = serde_json::from_str(&result.stdout)?;
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
    assert_has_signal(&result.stdout, "protected-input-changed")?;

    let persisted = lumin_engine::load_gate(root.path(), &model_gate_id)?;
    assert_eq!(persisted.baseline.as_ref(), Some(&opening_baseline));
    assert_eq!(persisted.protected_semantic_inputs, prior_protected_reads);
    let stale_revision = persisted
        .revisions
        .last()
        .ok_or_else(|| std::io::Error::other("sealed stale revision is missing"))?;
    assert_ne!(
        stale_revision.protected_semantic_inputs,
        persisted.protected_semantic_inputs
    );
    for expected in ["config/base.json", "shared/root.json"] {
        assert!(
            stale_revision
                .protected_semantic_inputs
                .iter()
                .any(|input| input.path.display == expected)
        );
    }

    let shown = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&shown, 0);
    let shown_json: Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(
        shown_json
            .get("protectedSemanticInputCount")
            .and_then(Value::as_u64),
        u64::try_from(persisted.protected_semantic_inputs.len()).ok()
    );
    assert_eq!(
        shown_json
            .pointer("/revisions/1/protectedSemanticInputCount")
            .and_then(Value::as_u64),
        u64::try_from(stale_revision.protected_semantic_inputs.len()).ok()
    );

    let retry = run(root.path(), &arguments)?;
    assert_status(&retry, 5);
    assert_eq!(retry.stdout, result.stdout);
    let shown_after_retry = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&shown_after_retry, 0);
    assert_eq!(shown_after_retry.stdout, shown.stdout);
    Ok(())
}
