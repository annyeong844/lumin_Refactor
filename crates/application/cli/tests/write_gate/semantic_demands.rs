use super::*;

#[test]
fn warm_cache_replays_owner_semantics() -> Result<(), Box<dyn std::error::Error>> {
    let root = semantic_read_closure_fixture()?;
    fs::create_dir(root.path().join("writers"))?;
    fs::write(
        root.path().join("shared/new.json"),
        "export const blocker = 1;\n",
    )?;
    fs::hard_link(
        root.path().join("shared/new.json"),
        root.path().join("writers/new-config-writer.ts"),
    )?;
    fs::write(
        root.path().join("src/tsconfig.json"),
        "{\"extends\":\"../config/base.json\"}\n",
    )?;
    let seed_gate = open_gate(root.path(), "op-cache-seed", "src/seed.ts")?;
    abandon_gate(
        root.path(),
        &seed_gate,
        "op-cache-seed-abandon",
        "old cache seed complete",
    )?;
    let writer_gate = open_scoped_gate(
        root.path(),
        "op-cache-new-config-writer",
        "writers/new-config-writer.ts",
        "writers/new-config-writer.ts",
    )?;
    fs::write(
        root.path().join("config/base.json"),
        "{\"extends\":\"../shared/new.json\",\"compilerOptions\":{}}\n",
    )?;

    let (blocked, blocked_frames) = run_with_cache_replay_trace(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-cache-stale-demand",
            "--path",
            "src/blocked.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&blocked, 4);
    assert_eq!(field(&blocked.stdout, "decision")?, "incomplete");
    let blocked_json: Value = serde_json::from_str(&blocked.stdout)?;
    let conflict_paths = blocked_json
        .get("signals")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|signal| {
            signal.get("kind").and_then(Value::as_str) == Some("semantic-input-conflict")
        })
        .filter_map(|signal| signal.get("paths").and_then(Value::as_array))
        .flatten()
        .filter_map(|path| path.get("display").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert_eq!(conflict_paths, ["shared/new.json"]);
    let blocked_stages = blocked_frames
        .iter()
        .filter_map(|frame| frame.split_whitespace().next())
        .collect::<Vec<_>>();
    assert_eq!(
        blocked_stages,
        ["cache-demand-hit"],
        "warm execution replayed a demand beyond the changed intermediate config"
    );
    let attempted_domain = blocked_json
        .pointer("/observationBinding/attemptedDomain")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("blocked warm gate omitted its attempted domain"))?;
    let attempted_paths = attempted_domain
        .iter()
        .filter_map(|path| path.get("display").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert!(attempted_paths.contains(&"config/base.json"));
    assert!(attempted_paths.contains(&"shared/new.json"));
    assert!(!attempted_paths.contains(&"shared/root.json"));
    assert!(
        !blocked_json
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some("analysis-failed")
            }))
    );

    abandon_gate(
        root.path(),
        &writer_gate,
        "op-cache-new-config-writer-abandon",
        "release new config writer",
    )?;
    fs::remove_file(root.path().join("writers/new-config-writer.ts"))?;
    let cleanup = run(
        root.path(),
        &[
            "cache",
            "clean",
            "--operation-id",
            "op-cache-reset-before-cold",
        ],
    )?;
    assert_status(&cleanup, 0);

    fs::write(
        root.path().join("shared/new.json"),
        "{\"compilerOptions\":{\"strict\":true}}\n",
    )?;
    let cold = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-cache-cold",
            "--path",
            "src/cold.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&cold, 0);
    let cold_gate_id = field(&cold.stdout, "gateId")?;
    let cold_gate = lumin_engine::load_gate(
        root.path(),
        &lumin_model::GateId::from_string(cold_gate_id.clone()),
    )?;
    abandon_gate(
        root.path(),
        &cold_gate_id,
        "op-cache-cold-abandon",
        "cold capture complete",
    )?;

    let (warm, warm_frames) = run_with_cache_replay_trace(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-cache-warm",
            "--path",
            "src/warm.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&warm, 0);
    assert_eq!(field(&warm.stdout, "decision")?, "allow");
    let stages = warm_frames
        .iter()
        .filter_map(|frame| frame.split_whitespace().next())
        .collect::<Vec<_>>();
    assert!(
        stages
            .iter()
            .filter(|stage| **stage == "cache-demand-hit")
            .count()
            >= 2,
        "warm execution omitted nested cached demand steps: {stages:?}"
    );
    assert_eq!(stages.last().copied(), Some("cache-finished-hit"));
    assert_eq!(
        stages
            .iter()
            .filter(|stage| **stage == "cache-finished-hit")
            .count(),
        1
    );

    let warm_gate_id = field(&warm.stdout, "gateId")?;
    let warm_gate =
        lumin_engine::load_gate(root.path(), &lumin_model::GateId::from_string(warm_gate_id))?;
    let cold_baseline = cold_gate
        .baseline
        .as_ref()
        .ok_or_else(|| std::io::Error::other("cold gate omitted its baseline"))?;
    let warm_baseline = warm_gate
        .baseline
        .as_ref()
        .ok_or_else(|| std::io::Error::other("warm gate omitted its baseline"))?;
    assert_eq!(cold_baseline.snapshot, warm_baseline.snapshot);
    assert_eq!(
        cold_baseline.protected_semantic_inputs,
        warm_baseline.protected_semantic_inputs
    );
    assert_eq!(
        cold_baseline.analysis_contract,
        warm_baseline.analysis_contract
    );
    assert_eq!(
        cold_gate.revisions[0].decision,
        warm_gate.revisions[0].decision
    );
    assert_eq!(
        cold_gate.revisions[0].signals,
        warm_gate.revisions[0].signals
    );
    assert_eq!(cold_gate.revisions[0].deltas, warm_gate.revisions[0].deltas);
    assert!(lumin_engine::gate_observation_binding_matches_owner(
        &cold_gate,
        &cold_gate.revisions[0]
    )?);
    assert!(lumin_engine::gate_observation_binding_matches_owner(
        &warm_gate,
        &warm_gate.revisions[0]
    )?);
    Ok(())
}

#[test]
fn cache_projection_is_gate_contextual() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(
        root.path().join("package.json"),
        r#"{"name":"gate-context-cache","private":true,"type":"module"}"#,
    )?;
    fs::write(
        root.path().join("src/broken.ts"),
        concat!(
            "import { used } from './target.js';\n",
            "console.log(used);\n",
            "export const visible = 1;\n",
            "export const hiddenLocal;\n",
        ),
    )?;
    fs::write(
        root.path().join("src/consumer.ts"),
        "import { visible } from './broken.js'; console.log(visible);\n",
    )?;
    fs::write(
        root.path().join("src/target.ts"),
        "export const used = 1;\n",
    )?;
    fs::write(root.path().join("src/safe.ts"), "console.log('safe');\n")?;

    let (intersecting, cold_frames) = run_with_cache_replay_trace(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-cache-context-intersecting",
            "--path",
            "src/broken.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&intersecting, 4);
    assert!(
        cold_frames.is_empty(),
        "cold gate unexpectedly replayed cached analysis: {cold_frames:?}"
    );
    let intersecting_json: Value = serde_json::from_str(&intersecting.stdout)?;
    assert_eq!(
        intersecting_json.get("decision").and_then(Value::as_str),
        Some("incomplete")
    );
    assert_eq!(
        intersecting_json.get("lifecycle").and_then(Value::as_str),
        Some("rejected")
    );
    let intersecting_signals = intersecting_json
        .get("signals")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("intersecting gate omitted signals"))?;
    assert_eq!(intersecting_signals.len(), 1);
    assert_eq!(
        intersecting_signals[0].get("kind").and_then(Value::as_str),
        Some("required-evidence-incomplete")
    );
    assert_eq!(
        intersecting_signals[0].get("count").and_then(Value::as_u64),
        Some(1)
    );

    let (replayed_intersecting, replayed_intersecting_frames) = run_with_cache_replay_trace(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-cache-context-intersecting-replay",
            "--path",
            "src/broken.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&replayed_intersecting, 4);
    let replayed_intersecting_json: Value = serde_json::from_str(&replayed_intersecting.stdout)?;
    assert_eq!(
        replayed_intersecting_json.get("decision"),
        intersecting_json.get("decision")
    );
    assert_eq!(
        replayed_intersecting_json.get("lifecycle"),
        intersecting_json.get("lifecycle")
    );
    assert_eq!(
        replayed_intersecting_json.get("signals"),
        intersecting_json.get("signals"),
        "finished-cache replay dropped the intersecting gate's required signal"
    );
    let replayed_intersecting_stages = replayed_intersecting_frames
        .iter()
        .filter_map(|frame| frame.split_whitespace().next())
        .collect::<Vec<_>>();
    assert_eq!(
        replayed_intersecting_stages.last().copied(),
        Some("cache-finished-hit")
    );
    assert_eq!(
        replayed_intersecting_stages
            .iter()
            .filter(|stage| **stage == "cache-finished-hit")
            .count(),
        1
    );

    let (disjoint, warm_frames) = run_with_cache_replay_trace(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-cache-context-disjoint",
            "--path",
            "src/safe.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&disjoint, 0);
    let disjoint_json: Value = serde_json::from_str(&disjoint.stdout)?;
    assert_eq!(
        disjoint_json.get("decision").and_then(Value::as_str),
        Some("allow")
    );
    assert_eq!(
        disjoint_json.get("lifecycle").and_then(Value::as_str),
        Some("active")
    );
    assert_eq!(
        disjoint_json
            .get("signals")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "warm disjoint gate replayed the prior gate's required effect: {disjoint_json:#?}"
    );
    let warm_stages = warm_frames
        .iter()
        .filter_map(|frame| frame.split_whitespace().next())
        .collect::<Vec<_>>();
    assert_eq!(warm_stages.last().copied(), Some("cache-finished-hit"));
    assert_eq!(
        warm_stages
            .iter()
            .filter(|stage| **stage == "cache-finished-hit")
            .count(),
        1
    );

    let intersecting_gate_id = field(&intersecting.stdout, "gateId")?;
    let replayed_intersecting_gate_id = field(&replayed_intersecting.stdout, "gateId")?;
    let disjoint_gate_id = field(&disjoint.stdout, "gateId")?;
    let intersecting_gate = lumin_engine::load_gate(
        root.path(),
        &lumin_model::GateId::from_string(intersecting_gate_id),
    )?;
    let replayed_intersecting_gate = lumin_engine::load_gate(
        root.path(),
        &lumin_model::GateId::from_string(replayed_intersecting_gate_id),
    )?;
    let disjoint_gate = lumin_engine::load_gate(
        root.path(),
        &lumin_model::GateId::from_string(disjoint_gate_id),
    )?;
    let intersecting_baseline = intersecting_gate
        .baseline
        .as_ref()
        .ok_or_else(|| std::io::Error::other("intersecting gate omitted its baseline"))?;
    let disjoint_baseline = disjoint_gate
        .baseline
        .as_ref()
        .ok_or_else(|| std::io::Error::other("disjoint gate omitted its baseline"))?;
    let replayed_intersecting_baseline = replayed_intersecting_gate
        .baseline
        .as_ref()
        .ok_or_else(|| std::io::Error::other("replayed intersecting gate omitted its baseline"))?;
    assert_eq!(
        intersecting_baseline.snapshot, replayed_intersecting_baseline.snapshot,
        "intersecting warm replay changed the cached owner output"
    );
    assert_eq!(
        intersecting_baseline.snapshot, disjoint_baseline.snapshot,
        "warm replay changed the cached owner output"
    );
    assert_eq!(
        intersecting_gate.revisions[0].signals, replayed_intersecting_gate.revisions[0].signals,
        "intersecting warm replay did not recompute the required signal"
    );
    assert_ne!(
        intersecting_gate.revisions[0].signals, disjoint_gate.revisions[0].signals,
        "gate-context projection was cached with the repository analysis"
    );
    assert!(lumin_engine::gate_observation_binding_matches_owner(
        &intersecting_gate,
        &intersecting_gate.revisions[0]
    )?);
    assert!(lumin_engine::gate_observation_binding_matches_owner(
        &replayed_intersecting_gate,
        &replayed_intersecting_gate.revisions[0]
    )?);
    assert!(lumin_engine::gate_observation_binding_matches_owner(
        &disjoint_gate,
        &disjoint_gate.revisions[0]
    )?);
    Ok(())
}

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
    let pending_binding = pending_json
        .get("observationBinding")
        .ok_or_else(|| std::io::Error::other("pre-write unsealed binding is missing"))?;
    assert_eq!(
        pending_binding.get("state").and_then(Value::as_str),
        Some("unsealed")
    );
    assert_eq!(
        pending_binding.get("reason").and_then(Value::as_str),
        Some("semantic-read-conflict")
    );
    assert!(pending_binding.get("observation").is_none());
    assert_eq!(
        pending_binding
            .pointer("/conflictingOrUnboundedInputs/0/display")
            .and_then(Value::as_str),
        Some("config/base.json")
    );
    let rejected_gate_id = pending_json
        .get("gateId")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("rejected pre-write gate ID is missing"))?;
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
    let blocked_operation_json: Value = serde_json::from_str(&blocked_operation.stdout)?;
    assert_eq!(
        blocked_operation_json
            .get("semanticReadReservations")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );
    assert_eq!(
        blocked_operation_json.get("status").and_then(Value::as_str),
        Some("committed")
    );
    assert_eq!(
        blocked_operation_json.pointer("/result/observationBinding"),
        Some(pending_binding)
    );

    let rejected_gate = run(root.path(), &["gate", "show", rejected_gate_id])?;
    assert_status(&rejected_gate, 0);
    let rejected_gate_json: Value = serde_json::from_str(&rejected_gate.stdout)?;
    assert_eq!(
        rejected_gate_json.get("lifecycle").and_then(Value::as_str),
        Some("rejected")
    );
    assert!(
        rejected_gate_json
            .get("baseline")
            .is_some_and(Value::is_null)
    );
    assert_eq!(
        rejected_gate_json.pointer("/revisions/0/observationBinding"),
        Some(pending_binding)
    );
    assert!(
        rejected_gate_json
            .pointer("/revisions/0/analysisInputId")
            .is_some_and(Value::is_null)
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

    let pending_retry = run(
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
    assert_status(&pending_retry, 4);
    assert_eq!(pending_retry.stdout, pending_a.stdout);

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
    let blocked_json: Value = serde_json::from_str(&blocked.stdout)?;
    let blocked_binding = blocked_json
        .get("observationBinding")
        .ok_or_else(|| std::io::Error::other("post-write conflict binding is missing"))?;
    assert_eq!(
        blocked_binding.get("state").and_then(Value::as_str),
        Some("unsealed")
    );
    assert_eq!(
        blocked_binding.get("reason").and_then(Value::as_str),
        Some("semantic-read-conflict")
    );
    assert!(blocked_binding.get("observation").is_none());
    assert_eq!(
        blocked_binding
            .pointer("/conflictingOrUnboundedInputs/0/display")
            .and_then(Value::as_str),
        Some("config/base.json")
    );

    let blocked_operation = run(
        root.path(),
        &["operation", "show", "op-demand-blocked-close"],
    )?;
    assert_status(&blocked_operation, 0);
    let blocked_operation_json: Value = serde_json::from_str(&blocked_operation.stdout)?;
    assert_eq!(
        blocked_operation_json.pointer("/result/observationBinding"),
        Some(blocked_binding)
    );

    let blocked_gate = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&blocked_gate, 0);
    let blocked_gate_json: Value = serde_json::from_str(&blocked_gate.stdout)?;
    assert_eq!(
        blocked_gate_json.pointer("/revisions/1/observationBinding"),
        Some(blocked_binding)
    );
    assert!(
        blocked_gate_json
            .pointer("/revisions/1/analysisInputId")
            .is_some_and(Value::is_null)
    );

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

    let blocked_retry = run(
        root.path(),
        &[
            "post-write",
            &gate_id,
            "--operation-id",
            "op-demand-blocked-close",
        ],
    )?;
    assert_status(&blocked_retry, 4);
    assert_eq!(blocked_retry.stdout, blocked.stdout);

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
    let first_json: Value = serde_json::from_str(&first.stdout)?;
    let first_binding = first_json
        .get("observationBinding")
        .ok_or_else(|| std::io::Error::other("sealed deny binding is missing"))?;
    assert_eq!(
        first_binding.get("state").and_then(Value::as_str),
        Some("sealed")
    );
    assert_eq!(
        first_binding
            .pointer("/observation/kind")
            .and_then(Value::as_str),
        Some("close")
    );
    assert!(
        first_binding
            .pointer("/observation/observationId")
            .and_then(Value::as_str)
            .is_some_and(|id| id.starts_with("gate_close_observation_"))
    );
    let first_operation = run(root.path(), &["operation", "show", "op-demand-first-close"])?;
    assert_status(&first_operation, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&first_operation.stdout)?
            .pointer("/result/observationBinding")
            .cloned(),
        Some(first_binding.clone())
    );
    let after_first = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&after_first, 0);
    let after_first_json: Value = serde_json::from_str(&after_first.stdout)?;
    let protected_after_first = after_first_json
        .get("protectedSemanticInputCount")
        .and_then(Value::as_u64)
        .ok_or_else(|| std::io::Error::other("first close protected input count is missing"))?;
    let first_revision = after_first_json
        .get("revisions")
        .and_then(Value::as_array)
        .and_then(|revisions| revisions.last())
        .ok_or_else(|| std::io::Error::other("sealed deny revision is missing"))?;
    assert_eq!(
        first_revision.get("observationBinding"),
        Some(first_binding)
    );
    assert!(
        first_revision
            .get("analysisInputId")
            .and_then(Value::as_str)
            .is_some()
    );

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
    let retry_json: Value = serde_json::from_str(&retry.stdout)?;
    let retry_binding = retry_json
        .get("observationBinding")
        .ok_or_else(|| std::io::Error::other("unsealed retry binding is missing"))?;
    assert_eq!(
        retry_binding.get("state").and_then(Value::as_str),
        Some("unsealed")
    );
    assert!(retry_binding.get("observation").is_none());

    let operation = run(root.path(), &["operation", "show", "op-demand-retry-close"])?;
    assert_status(&operation, 0);
    let operation_json: Value = serde_json::from_str(&operation.stdout)?;
    assert_eq!(
        operation_json.pointer("/result/observationBinding"),
        Some(retry_binding)
    );
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
    let retry_revision = shown_json
        .get("revisions")
        .and_then(Value::as_array)
        .and_then(|revisions| revisions.last())
        .ok_or_else(|| std::io::Error::other("unsealed retry revision is missing"))?;
    assert_eq!(
        retry_revision.get("observationBinding"),
        Some(retry_binding)
    );
    assert!(
        retry_revision
            .get("analysisInputId")
            .is_some_and(Value::is_null)
    );
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
    let arguments = [
        "post-write",
        gate_id.as_str(),
        "--operation-id",
        "op-conflict-recheck-close",
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
    stream.set_nonblocking(false)?;
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
    assert_status(&abandoned, 3);
    assert_eq!(field(&abandoned.stdout, "decision")?, "deny");

    stream.write_all(b"release\n")?;
    stream.flush()?;
    drop(stream);
    let output = child.wait_with_output()?;
    let effective_arguments = support::determinism::effective_arguments(
        &arguments
            .iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>(),
    )?;
    let output = support::finish_process_output(root.path(), &effective_arguments, output)?;
    assert_status(&output, 4);
    let response: Value = serde_json::from_str(&output.stdout)?;
    assert_eq!(
        response.get("decision").and_then(Value::as_str),
        Some("incomplete")
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
                signal.get("kind").and_then(Value::as_str)
                    == Some("semantic-read-closure-incomplete")
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
            .pointer("/observationBinding/reason")
            .and_then(Value::as_str)
            == Some("semantic-read-closure-incomplete")
    );
    assert!(
        response
            .pointer("/observationBinding/attemptedDomain")
            .and_then(Value::as_array)
            .is_some_and(|paths| paths.iter().any(|path| {
                path.get("display").and_then(Value::as_str) == Some("config/base.json")
            }))
    );
    assert!(
        response
            .pointer("/observationBinding/observation")
            .is_none()
    );

    let operation = run(
        root.path(),
        &["operation", "show", "op-conflict-recheck-close"],
    )?;
    assert_status(&operation, 0);
    let operation_json: Value = serde_json::from_str(&operation.stdout)?;
    assert_eq!(
        operation_json.pointer("/result/observationBinding"),
        response.get("observationBinding")
    );

    let shown = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&shown, 0);
    let shown_json: Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(
        shown_json.pointer("/revisions/1/observationBinding"),
        response.get("observationBinding")
    );
    assert!(
        shown_json
            .pointer("/revisions/1/analysisInputId")
            .is_some_and(Value::is_null)
    );
    assert_eq!(
        shown_json
            .pointer("/baseline/protectedSemanticInputCount")
            .and_then(Value::as_u64),
        shown_json
            .get("protectedSemanticInputCount")
            .and_then(Value::as_u64)
    );
    Ok(())
}

#[test]
fn gate_unsealed_observation_public_contract() -> Result<(), Box<dyn std::error::Error>> {
    pre_write_reserves_semantic_demands_before_capture_and_retries_after_writer_terminal()?;
    close_time_new_semantic_demand_outside_lease_stays_unplanned_on_retry()?;
    failed_close_rechecks_a_semantic_conflict_at_the_final_barrier()?;
    Ok(())
}

#[test]
fn failed_pre_write_rechecks_a_semantic_conflict_and_retains_prior_reservations()
-> Result<(), Box<dyn std::error::Error>> {
    let root = semantic_read_closure_fixture()?;
    fs::write(
        root.path().join("src/tsconfig.json"),
        "{\"extends\":\"../config/base.json\"}\n",
    )?;
    let writer_gate = open_gate(root.path(), "op-pre-conflict-recheck-writer-open", "shared")?;

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let mut child = lumin_command(root.path())?
        .args([
            "pre-write",
            "--operation-id",
            "op-pre-conflict-recheck-open",
            "--path",
            "src/new.ts",
            "--jobs",
            "1",
        ])
        .env(
            "LUMIN_TEST_GATE_PREWRITE_FINAL_BARRIER",
            listener.local_addr()?.to_string(),
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let (mut stream, frame) =
        wait_for_gate_barrier(&listener, &mut child, "pre-write conflict recheck")?;
    let frame = frame.split_whitespace().collect::<Vec<_>>();
    assert_eq!(frame.first().copied(), Some("finalizing"));
    assert_eq!(frame.get(1).copied(), Some("op-pre-conflict-recheck-open"));
    assert!(frame.get(2).is_some());

    let abandoned = run(
        root.path(),
        &[
            "gate",
            "abandon",
            &writer_gate,
            "--operation-id",
            "op-pre-conflict-recheck-abandon",
            "--reason",
            "release the semantic input",
        ],
    )?;
    assert_status(&abandoned, 3);
    assert_eq!(field(&abandoned.stdout, "decision")?, "deny");

    release_gate_barrier(&mut stream)?;
    drop(stream);
    let output = child.wait_with_output()?;
    assert_eq!(
        output.status.code(),
        Some(4),
        "unexpected pre-write conflict-recheck result: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let response: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        response.get("decision").and_then(Value::as_str),
        Some("incomplete")
    );
    let signals = response
        .get("signals")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("pre-write conflict signals are missing"))?;
    assert!(signals.iter().any(|signal| {
        signal.get("kind").and_then(Value::as_str) == Some("semantic-read-closure-incomplete")
    }));
    assert!(!signals.iter().any(|signal| {
        signal.get("kind").and_then(Value::as_str) == Some("semantic-input-conflict")
    }));
    assert_eq!(
        response
            .pointer("/observationBinding/reason")
            .and_then(Value::as_str),
        Some("semantic-read-closure-incomplete")
    );
    let attempted = response
        .pointer("/observationBinding/attemptedDomain")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("pre-write attempted domain is missing"))?;
    for expected in ["config/base.json", "shared/root"] {
        assert!(
            attempted
                .iter()
                .any(|path| { path.get("display").and_then(Value::as_str) == Some(expected) }),
            "pre-write attempted domain omitted {expected}: {attempted:?}"
        );
    }
    Ok(())
}

#[test]
fn stale_pre_write_capture_retains_and_rechecks_its_semantic_bindings()
-> Result<(), Box<dyn std::error::Error>> {
    let root = semantic_read_closure_fixture()?;
    fs::write(
        root.path().join("src/tsconfig.json"),
        "{\"extends\":\"../config/base.json\"}\n",
    )?;
    let response = run_stale_capture_binding_case(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-stale-capture-open",
            "--path",
            "src/new.ts",
            "--jobs",
            "1",
        ],
        "op-stale-capture-open",
        "LUMIN_TEST_GATE_PREWRITE_FINAL_BARRIER",
        "finalizing",
        CaptureBarrierAction::MakeStale,
    )?;
    assert_stale_capture_binding(&response)?;
    assert_operation_retains_binding(root.path(), "op-stale-capture-open", &response)?;
    Ok(())
}

#[test]
fn stale_post_write_capture_retains_and_rechecks_its_semantic_bindings()
-> Result<(), Box<dyn std::error::Error>> {
    let root = semantic_read_closure_fixture()?;
    fs::write(
        root.path().join("src/tsconfig.json"),
        "{\"extends\":\"../config/base.json\"}\n",
    )?;
    let gate_id = open_gate(root.path(), "op-stale-capture-baseline", "src/a.ts")?;
    let response = run_stale_capture_binding_case(
        root.path(),
        &[
            "post-write",
            &gate_id,
            "--operation-id",
            "op-stale-capture-close",
        ],
        "op-stale-capture-close",
        "LUMIN_TEST_GATE_POSTWRITE_FINAL_BARRIER",
        "close-finalizing",
        CaptureBarrierAction::MakeStale,
    )?;
    assert_stale_capture_binding(&response)?;
    assert_operation_retains_binding(root.path(), "op-stale-capture-close", &response)?;
    Ok(())
}

#[test]
fn failed_pre_write_capture_retains_and_rechecks_its_semantic_bindings()
-> Result<(), Box<dyn std::error::Error>> {
    let root = semantic_read_closure_fixture()?;
    fs::write(
        root.path().join("src/tsconfig.json"),
        "{\"extends\":\"../config/base.json\"}\n",
    )?;
    let response = run_stale_capture_binding_case(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-failed-capture-open",
            "--path",
            "src/new.ts",
            "--jobs",
            "1",
        ],
        "op-failed-capture-open",
        "LUMIN_TEST_GATE_PREWRITE_FINAL_BARRIER",
        "finalizing",
        CaptureBarrierAction::FailAnalysis,
    )?;
    assert_stale_capture_binding(&response)?;
    assert_capture_failed(&response)?;
    assert_operation_retains_binding(root.path(), "op-failed-capture-open", &response)?;
    Ok(())
}

#[test]
fn failed_post_write_capture_retains_and_rechecks_its_semantic_bindings()
-> Result<(), Box<dyn std::error::Error>> {
    let root = semantic_read_closure_fixture()?;
    fs::write(
        root.path().join("src/tsconfig.json"),
        "{\"extends\":\"../config/base.json\"}\n",
    )?;
    let gate_id = open_gate(root.path(), "op-failed-capture-baseline", "src/a.ts")?;
    let response = run_stale_capture_binding_case(
        root.path(),
        &[
            "post-write",
            &gate_id,
            "--operation-id",
            "op-failed-capture-close",
        ],
        "op-failed-capture-close",
        "LUMIN_TEST_GATE_POSTWRITE_FINAL_BARRIER",
        "close-finalizing",
        CaptureBarrierAction::FailAnalysis,
    )?;
    assert_stale_capture_binding(&response)?;
    assert_capture_failed(&response)?;
    assert_operation_retains_binding(root.path(), "op-failed-capture-close", &response)?;
    Ok(())
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum CaptureBarrierAction {
    MakeStale,
    FailAnalysis,
}

fn run_stale_capture_binding_case(
    root: &Path,
    arguments: &[&str],
    operation_id: &str,
    final_barrier_environment: &str,
    final_stage: &str,
    capture_action: CaptureBarrierAction,
) -> Result<Value, Box<dyn std::error::Error>> {
    let capture_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    capture_listener.set_nonblocking(true)?;
    let final_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    final_listener.set_nonblocking(true)?;
    let mut child = lumin_command(root)?
        .args(arguments)
        .env(
            "LUMIN_TEST_GATE_CAPTURE_FRESHNESS_BARRIER",
            capture_listener.local_addr()?.to_string(),
        )
        .env(
            final_barrier_environment,
            final_listener.local_addr()?.to_string(),
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let (mut capture_stream, capture_frame) =
        wait_for_gate_barrier(&capture_listener, &mut child, "capture freshness")?;
    let capture_parts = capture_frame.split_whitespace().collect::<Vec<_>>();
    assert_eq!(capture_parts.first().copied(), Some("capture-freshness"));
    assert_eq!(capture_parts.get(1).copied(), Some(operation_id));
    let gate_id = capture_parts
        .get(2)
        .ok_or_else(|| std::io::Error::other("capture barrier omitted the gate ID"))?
        .to_string();
    match capture_action {
        CaptureBarrierAction::MakeStale => {
            fs::write(
                root.join("config/base.json"),
                "{\"extends\":\"../shared/root\",\"compilerOptions\":{\"strict\":true}}\n",
            )?;
            release_gate_barrier(&mut capture_stream)?;
        }
        CaptureBarrierAction::FailAnalysis => {
            capture_stream.write_all(b"fail-analysis\n")?;
            capture_stream.flush()?;
        }
    }
    drop(capture_stream);

    let (mut final_stream, final_frame) =
        wait_for_gate_barrier(&final_listener, &mut child, "final validation")?;
    assert_eq!(
        final_frame.trim_end(),
        format!("{final_stage} {operation_id} {gate_id}")
    );
    if capture_action == CaptureBarrierAction::FailAnalysis {
        let replacement = root.join("config/base.replacement.json");
        fs::write(
            &replacement,
            "{\"extends\":\"../shared/root\",\"compilerOptions\":{\"strict\":true}}\n",
        )?;
        fs::remove_file(root.join("config/base.json"))?;
        fs::rename(replacement, root.join("config/base.json"))?;
    }
    let replacement = root.join("shared/root.replacement.json");
    fs::write(&replacement, "{\"compilerOptions\":{\"strict\":true}}\n")?;
    fs::remove_file(root.join("shared/root.json"))?;
    fs::rename(replacement, root.join("shared/root.json"))?;
    release_gate_barrier(&mut final_stream)?;

    let output = child.wait_with_output()?;
    assert_eq!(
        output.status.code(),
        Some(5),
        "unexpected stale-capture result: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn wait_for_gate_barrier(
    listener: &TcpListener,
    child: &mut std::process::Child,
    label: &str,
) -> Result<(std::net::TcpStream, String), Box<dyn std::error::Error>> {
    let started = Instant::now();
    let (stream, peer) = loop {
        match listener.accept() {
            Ok(accepted) => break accepted,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if let Some(status) = child.try_wait()? {
                    return Err(std::io::Error::other(format!(
                        "gate command exited before the {label} barrier: {status}"
                    ))
                    .into());
                }
                if started.elapsed() >= Duration::from_secs(30) {
                    return Err(std::io::Error::other(format!(
                        "gate command did not reach the {label} barrier"
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
    Ok((stream, frame))
}

fn release_gate_barrier(
    stream: &mut std::net::TcpStream,
) -> Result<(), Box<dyn std::error::Error>> {
    stream.write_all(b"release\n")?;
    stream.flush()?;
    Ok(())
}

fn run_with_cache_replay_trace(
    root: &Path,
    arguments: &[&str],
) -> Result<(support::ProcessResult, Vec<String>), Box<dyn std::error::Error>> {
    let arguments = arguments
        .iter()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();
    let effective_arguments = support::determinism::effective_arguments(&arguments)?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let mut child = lumin_command(root)?
        .args(&effective_arguments)
        .env(
            "LUMIN_TEST_GATE_CACHE_REPLAY_BARRIER",
            listener.local_addr()?.to_string(),
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let started = Instant::now();
    let mut frames = Vec::new();
    loop {
        match listener.accept() {
            Ok((mut stream, peer)) => {
                assert!(peer.ip().is_loopback());
                stream.set_nonblocking(false)?;
                stream.set_read_timeout(Some(Duration::from_secs(30)))?;
                stream.set_write_timeout(Some(Duration::from_secs(30)))?;
                let mut frame = String::new();
                BufReader::new(stream.try_clone()?).read_line(&mut frame)?;
                frames.push(frame.trim_end().to_owned());
                release_gate_barrier(&mut stream)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if child.try_wait()?.is_some() {
                    break;
                }
                if started.elapsed() >= Duration::from_secs(30) {
                    return Err(std::io::Error::other(
                        "gate command did not finish its cache replay trace",
                    )
                    .into());
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    }
    let output = child.wait_with_output()?;
    let result = support::finish_process_output(root, &effective_arguments, output)?;
    Ok((result, frames))
}

fn abandon_gate(
    root: &Path,
    gate_id: &str,
    operation_id: &str,
    reason: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let abandoned = run(
        root,
        &[
            "gate",
            "abandon",
            gate_id,
            "--operation-id",
            operation_id,
            "--reason",
            reason,
        ],
    )?;
    assert_status(&abandoned, 3);
    assert_eq!(field(&abandoned.stdout, "lifecycle")?, "abandoned");
    Ok(())
}

fn open_scoped_gate(
    root: &Path,
    operation_id: &str,
    path: &str,
    include: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let opened = run(
        root,
        &[
            "pre-write",
            "--operation-id",
            operation_id,
            "--path",
            path,
            "--include",
            include,
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);
    field(&opened.stdout, "gateId")
}

fn assert_stale_capture_binding(response: &Value) -> Result<(), Box<dyn std::error::Error>> {
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
    let attempted = response
        .pointer("/observationBinding/attemptedDomain")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("stale capture omitted its attempted domain"))?;
    for expected in ["config/base.json", "shared/root.json"] {
        assert!(
            attempted
                .iter()
                .any(|path| { path.get("display").and_then(Value::as_str) == Some(expected) })
        );
    }
    let signals = response
        .get("signals")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("stale capture omitted its signals"))?;
    let changed_paths = signals
        .iter()
        .filter(|signal| {
            signal.get("kind").and_then(Value::as_str) == Some("protected-input-changed")
        })
        .filter_map(|signal| signal.get("paths").and_then(Value::as_array))
        .flatten()
        .filter_map(|path| path.get("display").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert!(changed_paths.contains(&"config/base.json"));
    assert!(changed_paths.contains(&"shared/root.json"));
    assert!(!signals.iter().any(|signal| {
        signal.get("kind").and_then(Value::as_str) == Some("transition-catalog-changed")
    }));
    Ok(())
}

fn assert_capture_failed(response: &Value) -> Result<(), Box<dyn std::error::Error>> {
    let signals = response
        .get("signals")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("failed capture omitted its signals"))?;
    assert!(
        signals.iter().any(|signal| {
            signal.get("kind").and_then(Value::as_str) == Some("analysis-failed")
        })
    );
    Ok(())
}

fn assert_operation_retains_binding(
    root: &Path,
    operation_id: &str,
    response: &Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let shown = run(root, &["operation", "show", operation_id])?;
    assert_status(&shown, 0);
    let shown: Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(
        shown.pointer("/result/observationBinding"),
        response.get("observationBinding")
    );
    Ok(())
}
