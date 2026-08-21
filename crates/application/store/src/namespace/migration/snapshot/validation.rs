mod cache;
mod external;
mod retention;

use std::collections::{BTreeMap, BTreeSet};

use lumin_evidence::{
    GATE_RECORD_SCHEMA_VERSION, GateLifecycle, GateOperationKind, GateOperationStatus, GateRecord,
    OperationRecord, WorktreeTransition,
};
use lumin_model::{ObservationBinding, SealedGateObservation};
use serde::de::DeserializeOwned;

use crate::gate::transition_key;
use crate::{RunCatalogRecord, StoreError};

use super::super::super::NamespaceGuard;
use super::LogicalStoreSnapshot;

pub(super) fn validate_external_references(
    snapshot: &LogicalStoreSnapshot,
    guard: &NamespaceGuard,
) -> Result<(), StoreError> {
    external::validate_external_references(snapshot, guard)?;
    crate::publication::validate_attempt_lease_locks(&snapshot.attempt_leases, guard)
}

pub(super) fn validate_referential_closure(
    snapshot: &LogicalStoreSnapshot,
) -> Result<(), StoreError> {
    let (transitions, transition_sequences) = read_transitions(snapshot)?;
    let operations = read_operations(snapshot)?;
    let gates = read_gates(snapshot, &operations, &transition_sequences)?;
    validate_operation_gate_refs(&operations, &gates)?;
    validate_transition_gate_refs(&transitions, &gates)?;
    crate::publication::validate_attempt_leases(&snapshot.attempt_leases)?;
    validate_run_catalog(snapshot)?;
    cache::validate_cache(snapshot, &operations)?;
    retention::validate_retention(snapshot, &operations)?;
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
    if gate.schema_version != GATE_RECORD_SCHEMA_VERSION {
        return Err(StoreError::IncompatibleStateSchema(format!(
            "gate {key} uses unsupported schema {}; expected {GATE_RECORD_SCHEMA_VERSION}",
            gate.schema_version
        )));
    }
    if gate.revisions.last().map(|revision| revision.revision) != Some(gate.current_revision) {
        return Err(StoreError::Integrity(format!(
            "gate {key} current revision is not its durable tail"
        )));
    }
    validate_gate_observations(key, gate, operations)?;
    for (index, revision) in gate.revisions.iter().enumerate() {
        let operation = operations.get(revision.operation_id.as_str());
        if revision.revision != index as u64
            || operation.is_none_or(|operation| operation.gate_id != gate.gate_id)
        {
            return Err(StoreError::Integrity(format!(
                "gate {key} revision history is not owned by that gate"
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

fn validate_gate_observations(
    key: &str,
    gate: &GateRecord,
    operations: &BTreeMap<&str, OperationRecord>,
) -> Result<(), StoreError> {
    let opening = gate
        .revisions
        .first()
        .ok_or_else(|| StoreError::Integrity(format!("gate {key} omitted its opening revision")))?;
    match &gate.baseline {
        Some(baseline) => match &opening.observation_binding {
            Some(ObservationBinding::Sealed {
                observation: SealedGateObservation::Baseline { observation_id },
            }) if observation_id == &baseline.observation_id => {}
            _ => {
                return Err(StoreError::Integrity(format!(
                    "gate {key} baseline observation disagrees with its opening revision"
                )));
            }
        },
        None if gate.lifecycle == GateLifecycle::Active
            || opening.decision.authorizes()
            || !matches!(
                opening.observation_binding.as_ref(),
                Some(ObservationBinding::Unsealed { .. })
            ) =>
        {
            return Err(StoreError::Integrity(format!(
                "gate {key} opening omitted its matching baseline observation"
            )));
        }
        None => {}
    }

    for revision in &gate.revisions {
        let is_abandon = operations
            .get(revision.operation_id.as_str())
            .is_some_and(|operation| operation.kind == GateOperationKind::GateAbandon);
        if !is_abandon && revision.observation_binding.is_none() {
            return Err(StoreError::Integrity(format!(
                "gate {key} revision {} omitted its observation binding",
                revision.revision
            )));
        }
        if revision.decision.authorizes()
            && !is_abandon
            && !matches!(
                revision.observation_binding.as_ref(),
                Some(ObservationBinding::Sealed { .. })
            )
        {
            return Err(StoreError::Integrity(format!(
                "gate {key} authorizing revision {} is not observation-sealed",
                revision.revision
            )));
        }
        let wrong_observation_kind = if revision.revision == 0 {
            matches!(
                revision.observation_binding.as_ref(),
                Some(ObservationBinding::Sealed {
                    observation: SealedGateObservation::Close { .. }
                })
            )
        } else {
            matches!(
                revision.observation_binding.as_ref(),
                Some(ObservationBinding::Sealed {
                    observation: SealedGateObservation::Baseline { .. }
                })
            )
        };
        if wrong_observation_kind {
            return Err(StoreError::Integrity(format!(
                "gate {key} revision {} carries the wrong observation kind",
                revision.revision
            )));
        }
    }

    if gate.lifecycle == GateLifecycle::Closed
        && !matches!(
            gate.revisions
                .last()
                .and_then(|revision| revision.observation_binding.as_ref()),
            Some(ObservationBinding::Sealed {
                observation: SealedGateObservation::Close { .. }
            })
        )
    {
        return Err(StoreError::Integrity(format!(
            "closed gate {key} omitted its sealed close observation"
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
        if let Some(result) = &operation.result {
            let gate = gates.get(operation.gate_id.as_str()).ok_or_else(|| {
                StoreError::Integrity(format!(
                    "operation {} result references a missing gate",
                    operation.operation_id.as_str()
                ))
            })?;
            let revision = gate
                .revisions
                .iter()
                .find(|revision| {
                    revision.revision == result.revision
                        && revision.operation_id == operation.operation_id
                })
                .ok_or_else(|| {
                    StoreError::Integrity(format!(
                        "operation {} result references a missing gate revision",
                        operation.operation_id.as_str()
                    ))
                })?;
            if revision.decision != result.decision
                || revision.observation_binding != result.observation_binding
            {
                return Err(StoreError::Integrity(format!(
                    "operation {} result disagrees with its gate observation",
                    operation.operation_id.as_str()
                )));
            }
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
        let revision = gate
            .revisions
            .iter()
            .find(|revision| revision.revision == transition.capsule.revision)
            .ok_or_else(|| {
                StoreError::Integrity(format!(
                    "transition {} references a missing gate revision",
                    transition.sequence
                ))
            })?;
        let baseline_matches = gate.baseline.as_ref().is_some_and(|baseline| {
            baseline.observation_id == transition.capsule.baseline_observation_id
        });
        let close_matches = matches!(
            revision.observation_binding.as_ref(),
            Some(ObservationBinding::Sealed {
                observation: SealedGateObservation::Close { observation_id }
            }) if observation_id == &transition.capsule.close_observation_id
        );
        if !baseline_matches || !close_matches {
            return Err(StoreError::Integrity(format!(
                "transition {} observation binding disagrees with its gate revision",
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
            validate_operation_observation(operation, result)
        }
        (GateOperationStatus::Pending | GateOperationStatus::Interrupted, None) => Ok(()),
        _ => Err(StoreError::Integrity(format!(
            "operation {} has an incoherent terminal result",
            operation.operation_id.as_str()
        ))),
    }
}

fn validate_operation_observation(
    operation: &OperationRecord,
    result: &lumin_evidence::GateOperationResult,
) -> Result<(), StoreError> {
    if operation.kind != GateOperationKind::GateAbandon && result.observation_binding.is_none() {
        return Err(StoreError::Integrity(format!(
            "operation {} omitted its observation binding",
            operation.operation_id.as_str()
        )));
    }
    if result.decision.authorizes() {
        let correct = match operation.kind {
            GateOperationKind::PreWrite => matches!(
                result.observation_binding.as_ref(),
                Some(ObservationBinding::Sealed {
                    observation: SealedGateObservation::Baseline { .. }
                })
            ),
            GateOperationKind::PostWrite => matches!(
                result.observation_binding.as_ref(),
                Some(ObservationBinding::Sealed {
                    observation: SealedGateObservation::Close { .. }
                })
            ),
            GateOperationKind::GateAbandon => true,
        };
        if !correct {
            return Err(StoreError::Integrity(format!(
                "authorizing operation {} omitted its sealed observation",
                operation.operation_id.as_str()
            )));
        }
    }
    let wrong_kind = matches!(
        (&operation.kind, result.observation_binding.as_ref()),
        (
            GateOperationKind::PreWrite,
            Some(ObservationBinding::Sealed {
                observation: SealedGateObservation::Close { .. }
            })
        ) | (
            GateOperationKind::PostWrite,
            Some(ObservationBinding::Sealed {
                observation: SealedGateObservation::Baseline { .. }
            })
        ) | (GateOperationKind::GateAbandon, Some(_))
    );
    if wrong_kind {
        return Err(StoreError::Integrity(format!(
            "operation {} carries the wrong observation kind",
            operation.operation_id.as_str()
        )));
    }
    Ok(())
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
