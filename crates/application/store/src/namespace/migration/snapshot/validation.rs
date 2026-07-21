use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use lumin_evidence::{
    GateOperationKind, GateOperationStatus, GateRecord, OperationRecord, WorktreeTransition,
};
use serde::de::DeserializeOwned;

use crate::gate::transition_key;
use crate::{AttemptEnvelope, RunCatalogRecord, StoreError, digest_hex, io_error, read_json};

use super::LogicalStoreSnapshot;

pub(super) fn validate_external_references(
    snapshot: &LogicalStoreSnapshot,
    state_dir: &Path,
) -> Result<(), StoreError> {
    if let Some(attempt_id) = snapshot.pointers.get("latest-attempt") {
        let attempt_id = std::str::from_utf8(attempt_id).map_err(|error| {
            StoreError::Integrity(format!("latest-attempt pointer is not UTF-8: {error}"))
        })?;
        let envelope: AttemptEnvelope = read_json(
            &state_dir
                .join("attempts")
                .join(attempt_id)
                .join("attempt.json"),
        )?;
        if envelope.attempt_id.as_str() != attempt_id {
            return Err(StoreError::Integrity(
                "latest-attempt pointer disagrees with its envelope".to_owned(),
            ));
        }
    }

    for (key, bytes) in &snapshot.run_catalog {
        let record = parse_record::<RunCatalogRecord>("run-catalog", key, bytes)?;
        let run_dir = state_dir.join("runs").join(record.run_id.as_str());
        let envelope = read_json::<RunCatalogRecord>(&run_dir.join("run.json"))?;
        if envelope.run_id != record.run_id
            || envelope.attempt_id != record.attempt_id
            || envelope.sequence != record.sequence
            || envelope.evidence_store_sha256 != record.evidence_store_sha256
            || envelope.evidence_store_size != record.evidence_store_size
        {
            return Err(StoreError::Integrity(format!(
                "run catalog entry {key} disagrees with its durable run envelope"
            )));
        }
        let evidence = fs::read(run_dir.join("evidence.store")).map_err(io_error)?;
        if evidence.len() as u64 != record.evidence_store_size
            || digest_hex(&evidence) != record.evidence_store_sha256
        {
            return Err(StoreError::Integrity(format!(
                "run catalog entry {key} disagrees with its evidence store"
            )));
        }
    }
    Ok(())
}

pub(super) fn validate_referential_closure(
    snapshot: &LogicalStoreSnapshot,
) -> Result<(), StoreError> {
    let (transitions, transition_sequences) = read_transitions(snapshot)?;
    let operations = read_operations(snapshot)?;
    let gates = read_gates(snapshot, &operations, &transition_sequences)?;
    validate_operation_gate_refs(&operations, &gates)?;
    validate_transition_gate_refs(&transitions, &gates)?;
    validate_run_catalog(snapshot)?;
    validate_pointers(snapshot)
}

fn read_transitions(
    snapshot: &LogicalStoreSnapshot,
) -> Result<(BTreeMap<u64, WorktreeTransition>, BTreeSet<u64>), StoreError> {
    let mut transitions = BTreeMap::new();
    let mut sequences = BTreeSet::new();
    for (key, bytes) in &snapshot.transitions {
        let transition = parse_record::<WorktreeTransition>("worktree-transitions", key, bytes)?;
        if transition_key(transition.sequence) != *key {
            return Err(StoreError::Integrity(format!(
                "worktree transition key {key} disagrees with its sequence"
            )));
        }
        sequences.insert(transition.sequence);
        transitions.insert(transition.sequence, transition);
    }
    Ok((transitions, sequences))
}

fn read_operations(
    snapshot: &LogicalStoreSnapshot,
) -> Result<BTreeMap<&str, OperationRecord>, StoreError> {
    let mut operations = BTreeMap::new();
    for (key, bytes) in &snapshot.operations {
        let operation = parse_record::<OperationRecord>("operations", key, bytes)?;
        if operation.operation_id.as_str() != key {
            return Err(StoreError::Integrity(format!(
                "operation key {key} disagrees with its record"
            )));
        }
        validate_operation_result(&operation)?;
        operations.insert(key.as_str(), operation);
    }
    Ok(operations)
}

fn read_gates<'a>(
    snapshot: &'a LogicalStoreSnapshot,
    operations: &BTreeMap<&str, OperationRecord>,
    transition_sequences: &BTreeSet<u64>,
) -> Result<BTreeMap<&'a str, GateRecord>, StoreError> {
    let mut gates = BTreeMap::new();
    for (key, bytes) in &snapshot.gates {
        let gate = parse_record::<GateRecord>("gates", key, bytes)?;
        if gate.gate_id.as_str() != key {
            return Err(StoreError::Integrity(format!(
                "gate key {key} disagrees with its record"
            )));
        }
        validate_gate_history(key, &gate, operations, transition_sequences)?;
        gates.insert(key.as_str(), gate);
    }
    Ok(gates)
}

fn validate_gate_history(
    key: &str,
    gate: &GateRecord,
    operations: &BTreeMap<&str, OperationRecord>,
    transition_sequences: &BTreeSet<u64>,
) -> Result<(), StoreError> {
    if gate.revisions.last().map(|revision| revision.revision) != Some(gate.current_revision) {
        return Err(StoreError::Integrity(format!(
            "gate {key} current revision is not its durable tail"
        )));
    }
    for (index, revision) in gate.revisions.iter().enumerate() {
        if revision.revision != index as u64
            || !operations.contains_key(revision.operation_id.as_str())
        {
            return Err(StoreError::Integrity(format!(
                "gate {key} revision history is not referentially closed"
            )));
        }
        if revision
            .reconciled_transition_sequences
            .iter()
            .any(|sequence| !transition_sequences.contains(sequence))
        {
            return Err(StoreError::Integrity(format!(
                "gate {key} reconciles a missing transition"
            )));
        }
    }
    if gate
        .transition_refs
        .iter()
        .any(|sequence| !transition_sequences.contains(sequence))
    {
        return Err(StoreError::Integrity(format!(
            "gate {key} retains a missing transition"
        )));
    }
    Ok(())
}

fn validate_operation_gate_refs(
    operations: &BTreeMap<&str, OperationRecord>,
    gates: &BTreeMap<&str, GateRecord>,
) -> Result<(), StoreError> {
    for operation in operations.values() {
        let gate_required = operation.kind != GateOperationKind::PreWrite
            || operation.status == GateOperationStatus::Committed;
        if gate_required && !gates.contains_key(operation.gate_id.as_str()) {
            return Err(StoreError::Integrity(format!(
                "operation {} references a missing gate",
                operation.operation_id.as_str()
            )));
        }
    }
    Ok(())
}

fn validate_transition_gate_refs(
    transitions: &BTreeMap<u64, WorktreeTransition>,
    gates: &BTreeMap<&str, GateRecord>,
) -> Result<(), StoreError> {
    for transition in transitions.values() {
        let Some(gate) = gates.get(transition.capsule.gate_id.as_str()) else {
            return Err(StoreError::Integrity(format!(
                "transition {} references a missing gate",
                transition.sequence
            )));
        };
        if !gate
            .revisions
            .iter()
            .any(|revision| revision.revision == transition.capsule.revision)
        {
            return Err(StoreError::Integrity(format!(
                "transition {} references a missing gate revision",
                transition.sequence
            )));
        }
    }
    Ok(())
}

fn validate_run_catalog(snapshot: &LogicalStoreSnapshot) -> Result<(), StoreError> {
    for (key, bytes) in &snapshot.run_catalog {
        let record = parse_record::<RunCatalogRecord>("run-catalog", key, bytes)?;
        if record.run_id.as_str() != key {
            return Err(StoreError::Integrity(format!(
                "run catalog key {key} disagrees with its record"
            )));
        }
    }
    Ok(())
}

fn validate_operation_result(operation: &OperationRecord) -> Result<(), StoreError> {
    match (&operation.status, &operation.result) {
        (GateOperationStatus::Committed, Some(result))
            if result.operation_id == operation.operation_id
                && result.request_digest == operation.request_digest
                && result.gate_id == operation.gate_id =>
        {
            Ok(())
        }
        (GateOperationStatus::Pending | GateOperationStatus::Interrupted, None) => Ok(()),
        _ => Err(StoreError::Integrity(format!(
            "operation {} has an incoherent terminal result",
            operation.operation_id.as_str()
        ))),
    }
}

fn validate_pointers(snapshot: &LogicalStoreSnapshot) -> Result<(), StoreError> {
    for key in snapshot.pointers.keys() {
        if key != "latest-attempt" && key != "latest-completed" {
            return Err(StoreError::Integrity(format!(
                "lifecycle store contains unknown pointer {key}"
            )));
        }
    }
    if let Some(run_id) = snapshot.pointers.get("latest-completed") {
        let run_id = std::str::from_utf8(run_id).map_err(|error| {
            StoreError::Integrity(format!("latest-completed pointer is not UTF-8: {error}"))
        })?;
        if !snapshot.run_catalog.contains_key(run_id) {
            return Err(StoreError::Integrity(
                "latest-completed pointer references a missing run".to_owned(),
            ));
        }
    }
    Ok(())
}

fn parse_record<T: DeserializeOwned>(
    table: &str,
    key: &str,
    bytes: &[u8],
) -> Result<T, StoreError> {
    serde_json::from_slice(bytes).map_err(|error| {
        StoreError::Integrity(format!("{table} record {key} is malformed: {error}"))
    })
}
