use super::*;
use lumin_protocol::audit_lifecycle_diagnostic::{self, AuditLifecycleDiagnosticDto};

const CONTEXTS: [&str; 8] = [
    "open-recovery-latest",
    "attempt-recover-latest",
    "attempt-directory",
    "attempt-latest",
    "staging-create",
    "staging-move",
    "publish-terminal",
    "finalize-latest",
];
const COSTS: [&str; 13] = [
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
// Positive validation counters are fixture-only. W8 amends only the two fresh
// empty-index rows of the frozen W7 cost contract.
const COUNTS: [[u64; 13]; 8] = [
    [1, 1, 1, 1, 1, 0, 0, 0, 0, 1, 0, 0, 0],
    [1, 1, 1, 1, 1, 0, 0, 0, 0, 1, 0, 0, 0],
    [1, 2, 2, 1, 0, 0, 0, 0, 2, 0, 0, 0, 1],
    [1, 2, 2, 1, 1, 1, 1, 0, 0, 2, 1, 1, 1],
    [1, 2, 2, 1, 0, 0, 0, 0, 2, 0, 0, 0, 1],
    [1, 2, 2, 1, 0, 0, 0, 0, 2, 0, 0, 1, 1],
    [1, 2, 2, 1, 0, 0, 0, 0, 2, 0, 1, 1, 1],
    [1, 3, 3, 1, 2, 1, 1, 0, 0, 3, 1, 1, 1],
];
pub(super) fn value() -> Value {
    let mut value = super::store_tests::value();
    value["schemaVersion"] = "lumin.audit-execution-diagnostic.v3".into();
    value["lifecycleContexts"] = CONTEXTS.into_iter().zip(COUNTS).map(|(context, counts)| {
        serde_json::json!({"context":context, "calls":1, "elapsedNanoseconds":0, "selfNanoseconds":0,
            "costs":COSTS.into_iter().zip(counts).map(|(cost,calls)| serde_json::json!({
                "cost":cost, "calls":calls, "elapsedNanoseconds":(calls != 0).then_some(0)
            })).collect::<Vec<_>>()})
    }).collect();
    value
}
pub(super) fn canonical(value: Value) -> Result<Vec<u8>, String> {
    let dto: AuditLifecycleDiagnosticDto =
        serde_json::from_value(value).map_err(|e| e.to_string())?;
    let mut bytes = serde_json::to_vec(&dto).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    Ok(bytes)
}
fn validate_fresh(bytes: &[u8]) -> Result<AuditLifecycleDiagnosticDto, String> {
    validate_lifecycle_frame(
        bytes,
        &serde_json::json!({"schemaVersion":"lumin.phase1-process-measurement.v2",
            "exitCode":0, "analysisChildPids":[], "processId":1}),
        &serde_json::json!({"schemaVersion":"lumin.audit.v2", "attemptId":"attempt_fixture", "runId":"run_fixture"}),
        "build_fixture",
        Some(1),
    )
}
#[test]
fn audit_lifecycle_closed_versions_inventory_and_raw_transport() -> Result<(), String> {
    let valid = canonical(value())?;
    validate_fresh(&valid)?;
    assert!(decode(&valid).is_err());
    assert!(lumin_protocol::audit_store_diagnostic::decode(&valid).is_err());
    for bytes in [
        super::tests::canonical(super::tests::frame())?,
        super::store_tests::canonical(super::store_tests::value())?,
        Vec::new(),
        valid[..valid.len() - 1].to_vec(),
        valid[..valid.len() - 2].to_vec(),
        [valid.clone(), valid.clone()].concat(),
        [valid.clone(), b"\n".to_vec()].concat(),
        String::from_utf8(valid.clone())
            .map_err(|e| e.to_string())?
            .replace(
                "\"lifecycleContexts\":",
                "\"actualJobs\":1,\"lifecycleContexts\":",
            )
            .into_bytes(),
        String::from_utf8(valid.clone())
            .map_err(|e| e.to_string())?
            .replace("\"cost\":", "\"opaque\":1,\"cost\":")
            .into_bytes(),
    ] {
        assert!(audit_lifecycle_diagnostic::decode(&bytes).is_err());
    }
    for index in 0..8 {
        let mut missing = value();
        missing["lifecycleContexts"]
            .as_array_mut()
            .ok_or("contexts")?
            .remove(index);
        assert!(validate_fresh(&canonical(missing)?).is_err());
        let mut reordered = value();
        reordered["lifecycleContexts"]
            .as_array_mut()
            .ok_or("contexts")?
            .swap(index, (index + 1) % 8);
        assert!(validate_fresh(&canonical(reordered)?).is_err());
        for cost in 0..13 {
            let mut missing = value();
            missing["lifecycleContexts"][index]["costs"]
                .as_array_mut()
                .ok_or("costs")?
                .remove(cost);
            assert!(validate_fresh(&canonical(missing)?).is_err());
            let mut reordered = value();
            reordered["lifecycleContexts"][index]["costs"]
                .as_array_mut()
                .ok_or("costs")?
                .swap(cost, (cost + 1) % 13);
            assert!(validate_fresh(&canonical(reordered)?).is_err());
            let mut forged = value();
            forged["lifecycleContexts"][index]["costs"][cost]["elapsedNanoseconds"] =
                u64::MAX.into();
            assert!(validate_fresh(&canonical(forged)?).is_err());
        }
        for field in ["calls", "selfNanoseconds", "elapsedNanoseconds"] {
            let mut forged = value();
            forged["lifecycleContexts"][index][field] = 1.into();
            if field == "calls" {
                forged["lifecycleContexts"][index][field] = 2.into();
            }
            assert!(validate_fresh(&canonical(forged)?).is_err());
        }
    }
    Ok(())
}
#[test]
fn audit_lifecycle_fresh_binding_and_zero_absent_counts_are_not_inferred() -> Result<(), String> {
    for (key, replacement) in [
        ("processId", 2.into()),
        ("buildId", "other".into()),
        ("runId", "other".into()),
        ("requestedJobs", 2.into()),
        ("actualJobs", 8.into()),
        ("parallelismObservationError", "unavailable".into()),
    ] {
        let mut forged = value();
        forged[key] = replacement;
        assert!(validate_fresh(&canonical(forged)?).is_err());
    }
    for index in 0..8 {
        for cost in 4..13 {
            let mut forged = value();
            let row = &mut forged["lifecycleContexts"][index]["costs"][cost];
            row["calls"] = (COUNTS[index][cost] + 1).into();
            row["elapsedNanoseconds"] = 0.into();
            assert!(validate_fresh(&canonical(forged)?).is_err());
        }
    }
    Ok(())
}

#[test]
fn audit_lifecycle_candidate_rejects_the_previous_fresh_index_vector() -> Result<(), String> {
    let mut old = value();
    for index in 0..2 {
        for (cost, calls) in [1, 2, 2, 1, 1, 1, 0, 1, 0, 2, 0, 0, 0]
            .into_iter()
            .enumerate()
        {
            old["lifecycleContexts"][index]["costs"][cost]["calls"] = calls.into();
            old["lifecycleContexts"][index]["costs"][cost]["elapsedNanoseconds"] =
                if calls == 0 { Value::Null } else { 0.into() };
        }
    }
    let bytes = canonical(old)?;
    // v3 still transports the old shape, but this build's fresh oracle must not
    // guess its source version from those counts or accept it as candidate truth.
    audit_lifecycle_diagnostic::decode(&bytes)?;
    assert!(validate_fresh(&bytes).is_err());
    validate_fresh(&canonical(value())?)?;
    Ok(())
}
#[test]
fn audit_lifecycle_feature_closure_is_exact() -> Result<(), String> {
    let mut policy: Value = serde_json::from_slice(
        &fs::read(
            crate::metadata::find_workspace_root()?
                .join("tools/xtask/dependency-surface-policy.v2.json"),
        )
        .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    assert_eq!(
        diagnostic_feature_closure(&policy, Version::Lifecycle)?,
        serde_json::json!({
            "lumin-cli":["audit-execution-test-profile","audit-lifecycle-test-profile","audit-store-test-profile"],
            "lumin-engine":["audit-execution-test-profile","audit-lifecycle-test-profile","audit-store-test-profile"],
            "lumin-model":["audit-execution-test-profile","audit-lifecycle-test-profile","audit-store-test-profile"],
            "lumin-protocol":["audit-execution-test-profile","audit-lifecycle-test-profile","audit-store-test-profile"],
            "lumin-store":["audit-lifecycle-test-profile","audit-store-test-profile"],
        })
    );
    let cli = policy["members"]
        .as_array_mut()
        .ok_or("members")?
        .iter_mut()
        .find(|member| member["name"] == "lumin-cli")
        .ok_or("cli")?;
    cli["features"]["audit-lifecycle-test-profile"]
        .as_array_mut()
        .ok_or("features")?
        .push("lifecycle-test-fault".into());
    assert_ne!(
        diagnostic_feature_closure(&policy, Version::Lifecycle)?,
        expected_feature_closure(Version::Lifecycle)
    );
    Ok(())
}
#[test]
fn audit_lifecycle_scripted_children_preserve_every_invalid_raw_prefix() -> Result<(), String> {
    super::tests::scripted_frames(Version::Lifecycle)
}
