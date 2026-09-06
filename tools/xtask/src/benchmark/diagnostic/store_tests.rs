use super::*;
use lumin_model::audit_store_diagnostic::{AuditStorePhase, AuditStoreTimings};
use lumin_protocol::audit_store_diagnostic::{self, AuditStoreDiagnosticDto};

#[test]
fn audit_store_scripted_children_preserve_every_invalid_raw_prefix() -> Result<(), String> {
    super::tests::scripted_frames(Version::Store)
}

#[test]
fn audit_store_round_comparisons_retain_mixed_signs_and_reject_missing_pairs() -> Result<(), String>
{
    let samples = cells()
        .into_iter()
        .map(|cell| {
            let elapsed = if cell.diagnostic {
                if cell.round == 2 { 80 } else { 110 }
            } else {
                100
            };
            serde_json::json!({
                "round":cell.round, "measured":cell.round != 0,
                "requestedJobs":cell.jobs,
                "binaryKind":if cell.diagnostic { "diagnostic" } else { "control" },
                "process":{"elapsedNanoseconds":elapsed},
            })
        })
        .collect::<Vec<_>>();
    let expected = serde_json::json!([
        {"round":1,"requestedJobs":1,"controlNanoseconds":100,"diagnosticNanoseconds":110,"diagnosticMinusControl":10},
        {"round":1,"requestedJobs":null,"controlNanoseconds":100,"diagnosticNanoseconds":110,"diagnosticMinusControl":10},
        {"round":2,"requestedJobs":1,"controlNanoseconds":100,"diagnosticNanoseconds":80,"diagnosticMinusControl":-20},
        {"round":2,"requestedJobs":null,"controlNanoseconds":100,"diagnosticNanoseconds":80,"diagnosticMinusControl":-20},
        {"round":3,"requestedJobs":1,"controlNanoseconds":100,"diagnosticNanoseconds":110,"diagnosticMinusControl":10},
        {"round":3,"requestedJobs":null,"controlNanoseconds":100,"diagnosticNanoseconds":110,"diagnosticMinusControl":10},
    ]);
    assert_eq!(round_differences(&samples)?, expected);
    for index in 2..samples.len() {
        let mut missing = samples.clone();
        missing.remove(index);
        assert!(round_differences(&missing).is_err());
        let mut duplicate = samples.clone();
        duplicate.push(samples[index].clone());
        assert!(round_differences(&duplicate).is_err());
    }
    Ok(())
}

#[test]
fn audit_store_feature_closure_is_exact_and_rejects_an_extra_implication() -> Result<(), String> {
    let workspace = crate::metadata::find_workspace_root()?;
    let mut policy: Value = serde_json::from_slice(
        &fs::read(workspace.join("tools/xtask/dependency-surface-policy.v2.json"))
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    assert_eq!(
        store_feature_closure(&policy)?,
        expected_store_feature_closure()
    );
    let cli = policy["members"]
        .as_array_mut()
        .ok_or("members")?
        .iter_mut()
        .find(|member| member["name"] == "lumin-cli")
        .ok_or("cli")?;
    cli["features"]["audit-store-test-profile"]
        .as_array_mut()
        .ok_or("features")?
        .push("lifecycle-test-fault".into());
    assert_ne!(
        store_feature_closure(&policy)?,
        expected_store_feature_closure()
    );
    Ok(())
}

#[test]
fn audit_store_encoding_retains_parallelism_errors_without_accepting_the_frame()
-> Result<(), String> {
    use lumin_model::audit_diagnostic::{
        AuditExecutionDiagnostic, AuditPhase, AuditPoolObservation,
    };
    let mut pool = AuditPoolObservation {
        actual_jobs: Some(1),
        configured_worker_stack_bytes: Some(4_194_304),
        ..AuditPoolObservation::default()
    };
    for phase in AuditPhase::ALL {
        if phase != AuditPhase::DemandCapture {
            pool.timings.record(phase, 0);
        }
    }
    for root in AuditStorePhase::ROOTS {
        let mut packet = AuditStoreTimings::default();
        for phase in AuditStorePhase::ALL
            .into_iter()
            .filter(|phase| phase.root() == root)
        {
            packet.record(phase, 0);
        }
        pool.store_timings.merge_root(root, packet);
    }
    let evidence = AuditExecutionDiagnostic {
        build_id: "build_fixture".to_owned(),
        process_id: 1,
        attempt_id: "attempt_fixture".to_owned(),
        run_id: "run_fixture".to_owned(),
        requested_jobs: None,
        observed_available_parallelism: None,
        parallelism_observation_error: Some("owned parallelism failure".to_owned()),
        pool,
    };
    let bytes = audit_store_diagnostic::encode(&evidence)?;
    let frame: Value = serde_json::from_str(&bytes).map_err(|e| e.to_string())?;
    assert!(frame["observedAvailableParallelism"].is_null());
    assert_eq!(
        frame["parallelismObservationError"],
        "owned parallelism failure"
    );
    assert_eq!(
        frame["storePhases"]
            .as_array()
            .ok_or("missing store phases")?
            .len(),
        52
    );
    assert!(audit_store_diagnostic::decode(bytes.as_bytes()).is_err());
    Ok(())
}

// Independently authored W3 truth: do not derive this from product phase constants.
const STORE_PHASES: &[(&str, Option<&str>)] = &[
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

pub(super) fn value() -> Value {
    let mut value = super::tests::frame();
    value["schemaVersion"] = "lumin.audit-execution-diagnostic.v2".into();
    value["storePhases"] = STORE_PHASES
        .iter()
        .map(|(name, _)| {
            serde_json::json!({
                "phase":name, "calls":1, "elapsedNanoseconds":0, "selfNanoseconds":0
            })
        })
        .collect();
    value
}
pub(super) fn canonical(value: Value) -> Result<Vec<u8>, String> {
    let dto: AuditStoreDiagnosticDto = serde_json::from_value(value).map_err(|e| e.to_string())?;
    let mut bytes = serde_json::to_vec(&dto).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    Ok(bytes)
}
#[test]
fn audit_store_exact_owner_inventory_and_root_merge() -> Result<(), String> {
    assert_eq!(
        AuditStorePhase::ALL
            .map(|p| (p.name(), p.parent().map(|p| p.name())))
            .as_slice(),
        STORE_PHASES
    );
    let mut combined = AuditStoreTimings::default();
    for root in AuditStorePhase::ROOTS {
        let mut packet = AuditStoreTimings::default();
        for phase in AuditStorePhase::ALL
            .into_iter()
            .filter(|phase| phase.root() == root)
        {
            packet.record(phase, 0);
        }
        combined.merge_root(root, packet);
    }
    assert_eq!(combined.observations()?.len(), 52);
    let valid = combined.clone();
    combined.merge_root(AuditStorePhase::StoreOpen, AuditStoreTimings::default());
    assert!(combined.observations().is_err());
    let mut wrong = AuditStoreTimings::default();
    wrong.merge_root(AuditStorePhase::AttemptBegin, AuditStoreTimings::default());
    assert!(wrong.observations().is_err());
    let mut foreign = AuditStoreTimings::default();
    foreign.record(AuditStorePhase::AttemptReserve, 0);
    let mut wrong = AuditStoreTimings::default();
    wrong.merge_root(AuditStorePhase::StoreOpen, foreign);
    wrong.merge_root(AuditStorePhase::AttemptBegin, valid.clone());
    wrong.merge_root(AuditStorePhase::StorePublish, valid);
    assert!(wrong.observations().is_err());
    assert!(AuditStoreTimings::default().observations().is_err());
    Ok(())
}
#[test]
fn audit_store_closed_frames_reject_crossover_inventory_arithmetic_and_binding()
-> Result<(), String> {
    let valid = canonical(value())?;
    assert!(audit_store_diagnostic::decode(&valid).is_ok());
    assert!(decode(&valid).is_err());
    let v1 = super::tests::canonical(super::tests::frame())?;
    assert!(audit_store_diagnostic::decode(&v1).is_err());
    for bytes in [
        Vec::new(),
        valid[..valid.len() - 2].to_vec(),
        [valid.clone(), valid.clone()].concat(),
        [valid.clone(), b"\n".to_vec()].concat(),
        String::from_utf8(valid.clone())
            .map_err(|e| e.to_string())?
            .replace("\"storePhases\":", "\"actualJobs\":1,\"storePhases\":")
            .into_bytes(),
    ] {
        assert!(audit_store_diagnostic::decode(&bytes).is_err());
    }
    for index in 0..52 {
        let mut missing = value();
        missing["storePhases"]
            .as_array_mut()
            .ok_or("rows")?
            .remove(index);
        assert!(audit_store_diagnostic::decode(&canonical(missing)?).is_err());
        let mut reordered = value();
        reordered["storePhases"]
            .as_array_mut()
            .ok_or("rows")?
            .swap(index, (index + 1) % 52);
        assert!(audit_store_diagnostic::decode(&canonical(reordered)?).is_err());
        for field in ["elapsedNanoseconds", "selfNanoseconds", "calls"] {
            let mut forged = value();
            forged["storePhases"][index][field] = u64::MAX.into();
            assert!(audit_store_diagnostic::decode(&canonical(forged)?).is_err());
        }
    }
    let observer = serde_json::json!({"schemaVersion":"lumin.phase1-process-measurement.v2",
        "exitCode":0, "analysisChildPids":[], "processId":1});
    let stdout = serde_json::json!({"schemaVersion":"lumin.audit.v2", "attemptId":"attempt_fixture", "runId":"run_fixture"});
    assert!(validate_store_frame(&valid, &observer, &stdout, "build_fixture", Some(1)).is_ok());
    for (key, forged_value) in [
        ("processId", 2.into()),
        ("buildId", "other".into()),
        ("runId", "other".into()),
        ("actualJobs", 8.into()),
        ("requestedJobs", 2.into()),
    ] {
        let mut forged = value();
        forged[key] = forged_value;
        assert!(
            validate_store_frame(
                &canonical(forged)?,
                &observer,
                &stdout,
                "build_fixture",
                Some(1)
            )
            .is_err()
        );
    }
    let mut existing = value();
    for index in 2..7 {
        existing["storePhases"][index]["calls"] = 0.into();
        existing["storePhases"][index]["elapsedNanoseconds"] = Value::Null;
        existing["storePhases"][index]["selfNanoseconds"] = Value::Null;
    }
    let existing = canonical(existing)?;
    assert!(audit_store_diagnostic::decode(&existing).is_ok());
    assert!(validate_store_frame(&existing, &observer, &stdout, "build_fixture", Some(1)).is_err());
    Ok(())
}
