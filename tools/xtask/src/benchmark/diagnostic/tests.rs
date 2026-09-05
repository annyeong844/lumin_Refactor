use super::*;

const PHASES: &[&str] = &[
    "command",
    "pool-create",
    "audit-work",
    "pool-release",
    "admission",
    "store-open",
    "entry-identities",
    "attempt-begin",
    "capture",
    "inventory",
    "profiles",
    "extraction",
    "resolution",
    "demand-capture",
    "finish",
    "graph",
    "dead-code",
    "publication",
    "evidence-prepare",
    "store-publish",
    "final-inputs",
    "response",
    "stdout",
];

fn frame() -> Value {
    serde_json::json!({
        "schemaVersion":"lumin.audit-execution-diagnostic.v1", "diagnosticOnly":true,
        "buildId":"build_fixture", "processId":1, "attemptId":"attempt_fixture", "runId":"run_fixture",
        "requestedJobs":1, "observedAvailableParallelism":4, "parallelismObservationError":null,
        "actualJobs":1, "configuredWorkerStackBytes":4_194_304,
        "phases":PHASES.iter().map(|phase| {
            let absent = *phase == "demand-capture";
            serde_json::json!({"phase":phase, "calls":if absent { 0 } else { 1 },
                "elapsedNanoseconds":if absent { None } else { Some(0) },
                "selfNanoseconds":if absent { None } else { Some(0) }})
        }).collect::<Vec<_>>(),
    })
}

fn canonical(value: Value) -> Result<Vec<u8>, String> {
    // DTO encoding supplies the reviewed field order; the values/phase oracle above
    // are independently authored, not copied from a product run.
    let dto: AuditDiagnosticDto =
        serde_json::from_value(value).map_err(|error| error.to_string())?;
    let mut bytes = serde_json::to_vec(&dto).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[test]
fn schedule_is_exactly_two_conditioning_and_three_counterbalanced_rounds() {
    let names = cells()
        .into_iter()
        .map(|cell| cell.name)
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "conditioning-control",
            "conditioning-diagnostic",
            "round-1-control-1",
            "round-1-control-default",
            "round-1-diagnostic-1",
            "round-1-diagnostic-default",
            "round-2-diagnostic-default",
            "round-2-diagnostic-1",
            "round-2-control-default",
            "round-2-control-1",
            "round-3-control-1",
            "round-3-control-default",
            "round-3-diagnostic-1",
            "round-3-diagnostic-default",
        ]
    );
}

#[test]
fn closed_decoder_rejects_missing_duplicate_unknown_and_inconsistent_phases() -> Result<(), String>
{
    let valid = canonical(frame())?;
    assert!(decode(&valid).is_ok());
    let mut repeated = valid.clone();
    repeated.extend_from_slice(&valid);
    assert!(decode(&repeated).is_err());
    assert!(decode(&[]).is_err());
    assert!(decode(&valid[..valid.len() - 2]).is_err());
    let text = std::str::from_utf8(&valid).map_err(|error| error.to_string())?;
    for altered in [
        text.replace(
            "\"diagnosticOnly\":true",
            "\"diagnosticOnly\":true,\"diagnosticOnly\":true",
        ),
        text.replace("\"calls\":1", "\"calls\":1,\"calls\":1"),
        text.replace(
            "\"diagnosticOnly\":true",
            "\"diagnosticOnly\":true,\"opaque\":1",
        ),
        format!("{text}warning\n"),
    ] {
        assert!(decode(altered.as_bytes()).is_err());
    }
    for (pointer, replacement) in [
        ("/actualJobs", serde_json::json!(2)),
        ("/actualJobs", Value::Null),
        ("/observedAvailableParallelism", Value::Null),
        ("/configuredWorkerStackBytes", serde_json::json!(0)),
        (
            "/parallelismObservationError",
            serde_json::json!("unavailable"),
        ),
        ("/phases/16/elapsedNanoseconds", serde_json::json!(1)),
        ("/phases/2/calls", serde_json::json!(0)),
        ("/phases/1/phase", serde_json::json!("audit-work")),
    ] {
        let mut value = frame();
        *value
            .pointer_mut(pointer)
            .ok_or("fixture pointer missing")? = replacement;
        match canonical(value) {
            Ok(bytes) => assert!(decode(&bytes).is_err(), "{pointer}"),
            Err(_) => assert_eq!(pointer, "/actualJobs"),
        }
    }
    Ok(())
}

#[test]
fn public_child_frame_is_bound_to_observed_pid_and_preserves_bad_raw_bytes() -> Result<(), String> {
    let python = measurement::require_python()?;
    let workspace = crate::metadata::find_workspace_root().map_err(|error| error.to_string())?;
    let helper = workspace.join("tools/xtask/benchmark/measure-process.py");
    let fixture = tempfile::tempdir().map_err(|error| error.to_string())?;
    let template = String::from_utf8(canonical(frame())?).map_err(|error| error.to_string())?;
    let payload_before = hash_file(&python)?;
    for mode in [
        "valid",
        "missing",
        "malformed",
        "duplicate-key",
        "duplicate-frame",
        "stale-run",
        "stale-attempt",
        "wrong-pid",
        "wrong-build",
        "extra-stderr",
        "truncated",
        "bad-stdout",
        "failed",
    ] {
        let capture = fixture.path().join(mode);
        let code = r#"import json,os,sys
frame=json.loads(sys.argv[1]); mode=sys.argv[2]
if mode=='capabilities':
    sys.stdout.buffer.write(b'{"scope":{"buildId":"build_fixture"}}\n'); sys.exit(0)
frame['processId']=os.getpid()
if mode=='stale-run': frame['runId']='run_stale'
if mode=='stale-attempt': frame['attemptId']='attempt_stale'
if mode=='wrong-pid': frame['processId']+=1
if mode=='wrong-build': frame['buildId']='build_other'
wire=json.dumps(frame,separators=(',',':'))+'\n'
if mode=='missing': wire=''
if mode=='malformed': wire='not json\n'
if mode=='duplicate-key': wire=wire.replace('"diagnosticOnly":true','"diagnosticOnly":true,"diagnosticOnly":true')
if mode=='duplicate-frame': wire+=wire
if mode=='extra-stderr': wire+='warning\n'
if mode=='truncated': wire=wire[:-3]
stdout={'schemaVersion':'lumin.audit.v2','attemptId':'attempt_fixture','runId':'run_fixture'}
sys.stdout.buffer.write(('not json\n' if mode=='bad-stdout' else json.dumps(stdout,separators=(',',':'))+'\n').encode())
sys.stderr.buffer.write(wire.encode())
sys.exit(1 if mode=='failed' else 0)
"#;
        let capabilities = measurement::run_query(
            &python,
            fixture.path(),
            &[
                "-I".into(),
                "-S".into(),
                "-c".into(),
                code.into(),
                template.clone().into(),
                "capabilities".into(),
            ],
            &fixture.path().join(format!("capabilities-{mode}")),
        )?;
        let build = capabilities
            .pointer("/scope/buildId")
            .and_then(Value::as_str)
            .ok_or("fake binary omitted build scope")?;
        let args = [
            "-I".into(),
            "-S".into(),
            "-c".into(),
            code.into(),
            template.clone().into(),
            mode.into(),
        ];
        let measured = measurement::measure_diagnostic(
            &python,
            &helper,
            &python,
            fixture.path(),
            &args,
            &capture,
        );
        let valid = measured
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|measured| {
                let bytes = fs::read(capture.join("stderr")).map_err(|error| error.to_string())?;
                validate_frame(&bytes, &measured.raw, &measured.response, build, Some(1))
            });
        assert_eq!(valid.is_ok(), mode == "valid", "{mode}: {valid:?}");
        assert!(capture.join("stdout").is_file());
        assert!(capture.join("stderr").is_file());
        assert!(capture.join("measurement.json").is_file());
        if mode == "valid" {
            let measured = measured?;
            let mut child_observation = measured.raw.clone();
            child_observation["analysisChildPids"] = serde_json::json!([27]);
            assert!(
                validate_frame(
                    &fs::read(capture.join("stderr")).map_err(|error| error.to_string())?,
                    &child_observation,
                    &measured.response,
                    build,
                    Some(1)
                )
                .is_err()
            );
        }
    }
    assert_eq!(hash_file(&python)?, payload_before);
    Ok(())
}

#[test]
fn descriptive_summary_never_substitutes_a_performance_pass() -> Result<(), String> {
    let samples = cells().iter().map(|cell| serde_json::json!({
        "measured":cell.round != 0, "binaryKind":if cell.diagnostic {"diagnostic"} else {"control"},
        "requestedJobs":cell.jobs, "process":{"elapsedNanoseconds":if cell.jobs.is_some() {100} else {90}},
        "engineObservations":if cell.diagnostic {serde_json::json!({"actualJobs":1})} else {Value::Null},
    })).collect::<Vec<_>>();
    let summary = summarize(&samples)?;
    assert_eq!(summary["controlDefaultOverOne"], 0.9);
    assert_eq!(summary["scalingAuthority"], false);
    assert!(summary.get("passed").is_none());
    assert!(summarize(&samples[..13]).is_err());
    Ok(())
}
