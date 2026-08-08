use std::path::Path;

use serde_json::Value;

use crate::support::{assert_status, field, run};

pub fn assert_incomplete_prewrite_retry(
    root: &Path,
    operation_id: &str,
    path: &str,
    additional_arguments: &[&str],
) -> Result<Value, Box<dyn std::error::Error>> {
    let mut arguments = vec!["pre-write", "--operation-id", operation_id, "--path", path];
    arguments.extend_from_slice(additional_arguments);
    arguments.extend(["--jobs", "1"]);

    let first = run(root, &arguments)?;
    assert_status(&first, 4);
    assert_eq!(field(&first.stdout, "decision")?, "incomplete");
    let response: Value = serde_json::from_str(&first.stdout)?;
    let signals = response
        .get("signals")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("signals are missing"))?;
    assert!(signals.iter().any(|signal| {
        signal.get("kind").and_then(Value::as_str) == Some("required-evidence-incomplete")
    }));
    assert!(!signals.iter().any(|signal| {
        signal.get("kind").and_then(Value::as_str) == Some("semantic-input-conflict")
    }));
    assert!(
        !signals
            .iter()
            .any(|signal| signal.get("kind").and_then(Value::as_str) == Some("write-conflict"))
    );
    assert!(
        response
            .get("deltas")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
    );
    let gate_id = field(&first.stdout, "gateId")?;

    let operation_before = run(root, &["operation", "show", operation_id])?;
    assert_status(&operation_before, 0);
    let operation_before: Value = serde_json::from_str(&operation_before.stdout)?;
    assert_eq!(
        operation_before.get("kind").and_then(Value::as_str),
        Some("pre-write")
    );
    assert_eq!(
        operation_before.get("status").and_then(Value::as_str),
        Some("committed")
    );
    assert_eq!(
        operation_before.get("gateId").and_then(Value::as_str),
        Some(gate_id.as_str())
    );
    assert_eq!(
        operation_before
            .get("semanticReadReservations")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );
    assert_eq!(operation_before.get("result"), Some(&response));

    let gate_before = run(root, &["gate", "show", &gate_id])?;
    assert_status(&gate_before, 0);
    let gate_before: Value = serde_json::from_str(&gate_before.stdout)?;
    assert_eq!(
        gate_before.get("lifecycle").and_then(Value::as_str),
        Some("rejected")
    );
    assert_eq!(
        gate_before.get("currentRevision").and_then(Value::as_u64),
        Some(0)
    );
    assert_eq!(
        gate_before
            .get("revisions")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        gate_before
            .pointer("/revisions/0/operationId")
            .and_then(Value::as_str),
        Some(operation_id)
    );

    let active_before = run(root, &["gate", "list", "--active"])?;
    assert_status(&active_before, 0);
    let active_before: Value = serde_json::from_str(&active_before.stdout)?;
    assert!(
        !active_before
            .get("items")
            .and_then(Value::as_array)
            .is_some_and(|items| items.iter().any(|item| {
                item.get("gateId").and_then(Value::as_str) == Some(gate_id.as_str())
            }))
    );

    let retry = run(root, &arguments)?;
    assert_status(&retry, 4);
    assert_eq!(retry.stdout, first.stdout);

    let operation_after = run(root, &["operation", "show", operation_id])?;
    assert_status(&operation_after, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&operation_after.stdout)?,
        operation_before,
        "retry mutated the durable operation snapshot",
    );
    let gate_after = run(root, &["gate", "show", &gate_id])?;
    assert_status(&gate_after, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&gate_after.stdout)?,
        gate_before,
        "retry mutated the rejected gate revision or lifecycle",
    );
    let active_after = run(root, &["gate", "list", "--active"])?;
    assert_status(&active_after, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&active_after.stdout)?,
        active_before,
        "retry mutated the active gate catalog",
    );
    Ok(gate_before)
}

pub fn assert_probe_candidates_excluded(
    rejected_gate: &Value,
    expected_excluded: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let semantic_input_count = rejected_gate
        .pointer("/baseline/semanticInputCount")
        .and_then(Value::as_u64)
        .ok_or_else(|| std::io::Error::other("baseline semantic input count is missing"))?;
    let protected_input_count = rejected_gate
        .pointer("/baseline/protectedSemanticInputCount")
        .and_then(Value::as_u64)
        .ok_or_else(|| std::io::Error::other("baseline protected input count is missing"))?;
    assert_eq!(
        semantic_input_count.checked_sub(protected_input_count),
        Some(expected_excluded),
        "probe candidates entered the protected read closure",
    );
    assert_eq!(
        rejected_gate
            .get("protectedSemanticInputCount")
            .and_then(Value::as_u64),
        Some(protected_input_count),
    );
    assert_eq!(
        rejected_gate
            .pointer("/revisions/0/protectedSemanticInputCount")
            .and_then(Value::as_u64),
        Some(protected_input_count),
    );
    Ok(())
}
