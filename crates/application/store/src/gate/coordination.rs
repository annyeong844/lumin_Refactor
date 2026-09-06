use lumin_evidence::{
    GateLifecycle, GateOperationKind, GateOperationStatus, GateRecord,
    PreWriteAdmissionConflictOwner, PreWriteAdmissionEvidence, RepoPathProjection,
    SemanticInputRecord, SemanticReadReservationBinding, WorktreeTransition, WriteLease,
    derive_pre_write_admission_signals,
};
use lumin_model::{GateId, OperationId};
use redb::WriteTransaction;

use crate::StoreError;

use super::integrity::ValidatedGateCatalog;
use super::records::write_record;
use super::{ActiveGateLease, ConflictSet, GATES, validate_reservation_binding_set};

pub(super) fn pre_write_admission_evidence(
    catalog: &ValidatedGateCatalog,
    own_operation_id: &OperationId,
    attempted_leased_write_set: &[WriteLease],
    catalog_revision: u64,
) -> Result<PreWriteAdmissionEvidence, StoreError> {
    let mut conflict_owners = Vec::new();
    for operation in catalog.operations.values() {
        if operation.operation_id == *own_operation_id
            || operation.status != GateOperationStatus::Pending
        {
            continue;
        }
        validate_reservation_binding_set(operation)?;
        let owner = PreWriteAdmissionConflictOwner::PendingOperation {
            operation_id: operation.operation_id.clone(),
            gate_id: operation.gate_id.clone(),
            leased_write_set: if operation.kind == GateOperationKind::PreWrite {
                operation.leased_write_set.clone()
            } else {
                Vec::new()
            },
            semantic_read_reservation_bindings: operation
                .semantic_read_reservation_bindings
                .clone(),
        };
        if admission_owner_conflicts(attempted_leased_write_set, &owner) {
            conflict_owners.push(owner);
        }
    }
    for gate in catalog.gates.values() {
        if gate.lifecycle != GateLifecycle::Active {
            continue;
        }
        let owner = PreWriteAdmissionConflictOwner::ActiveGate {
            gate_id: gate.gate_id.clone(),
            revision: gate.current_revision,
            leased_write_set: gate.leased_write_set.clone(),
            protected_semantic_inputs: gate.protected_semantic_inputs.clone(),
        };
        if admission_owner_conflicts(attempted_leased_write_set, &owner) {
            conflict_owners.push(owner);
        }
    }
    conflict_owners.sort();
    conflict_owners.dedup();
    let mut attempted_leased_write_set = attempted_leased_write_set.to_vec();
    attempted_leased_write_set.sort();
    attempted_leased_write_set.dedup();
    Ok(PreWriteAdmissionEvidence {
        catalog_revision,
        attempted_leased_write_set,
        conflict_owners,
    })
}

fn admission_owner_conflicts(
    attempted_leased_write_set: &[WriteLease],
    owner: &PreWriteAdmissionConflictOwner,
) -> bool {
    !derive_pre_write_admission_signals(&PreWriteAdmissionEvidence {
        catalog_revision: 0,
        attempted_leased_write_set: attempted_leased_write_set.to_vec(),
        conflict_owners: vec![owner.clone()],
    })
    .is_empty()
}

pub(super) fn conflicts(
    catalog: &ValidatedGateCatalog,
    own_operation_id: &OperationId,
    leased_write_set: &[WriteLease],
    semantic_inputs: &[SemanticInputRecord],
    own_gate_id: Option<&GateId>,
) -> Result<(Vec<RepoPathProjection>, Vec<GateId>), StoreError> {
    let mut paths = Vec::new();
    let mut gate_ids = Vec::new();
    for operation in catalog.operations.values() {
        if operation.operation_id == *own_operation_id
            || operation.status != GateOperationStatus::Pending
        {
            continue;
        }
        validate_reservation_binding_set(operation)?;
        if operation.kind == GateOperationKind::PreWrite {
            collect_conflicts(
                leased_write_set,
                semantic_inputs,
                &operation.leased_write_set,
                &[],
                &operation.gate_id,
                &mut paths,
                &mut gate_ids,
            );
        }
        collect_reservation_conflicts(
            &operation.semantic_read_reservation_bindings,
            leased_write_set,
            &operation.gate_id,
            &mut paths,
            &mut gate_ids,
        );
    }
    for gate in catalog.gates.values() {
        if gate.lifecycle != GateLifecycle::Active
            || own_gate_id.is_some_and(|gate_id| gate.gate_id == *gate_id)
        {
            continue;
        }
        collect_conflicts(
            leased_write_set,
            semantic_inputs,
            &gate.leased_write_set,
            &gate.protected_semantic_inputs,
            &gate.gate_id,
            &mut paths,
            &mut gate_ids,
        );
    }
    paths.sort();
    paths.dedup();
    gate_ids.sort();
    gate_ids.dedup();
    Ok((paths, gate_ids))
}

fn collect_reservation_conflicts(
    reservations: &[SemanticReadReservationBinding],
    leases: &[WriteLease],
    existing_gate_id: &GateId,
    paths: &mut Vec<RepoPathProjection>,
    gate_ids: &mut Vec<GateId>,
) {
    for reservation in reservations {
        if leases.iter().any(|lease| {
            lease.conflicts_with_semantic_read(
                &reservation.path,
                reservation.physical_identity.as_ref(),
                reservation.absence_parent.as_ref(),
            )
        }) {
            paths.push(reservation.path.clone());
            gate_ids.push(existing_gate_id.clone());
        }
    }
}

fn collect_conflicts(
    candidate_leases: &[WriteLease],
    candidate_inputs: &[SemanticInputRecord],
    existing_leases: &[WriteLease],
    existing_inputs: &[SemanticInputRecord],
    existing_gate_id: &GateId,
    paths: &mut Vec<RepoPathProjection>,
    gate_ids: &mut Vec<GateId>,
) {
    for lease in candidate_leases {
        if existing_leases
            .iter()
            .any(|existing| lease.conflicts_with(existing))
            || existing_inputs.iter().any(|input| {
                lease.conflicts_with_semantic_read(
                    &input.path,
                    input.physical_identity.as_ref(),
                    input.absence_parent.as_ref(),
                )
            })
        {
            paths.push(lease.path.clone());
            gate_ids.push(existing_gate_id.clone());
        }
    }
    for input in candidate_inputs {
        if existing_leases.iter().any(|lease| {
            lease.conflicts_with_semantic_read(
                &input.path,
                input.physical_identity.as_ref(),
                input.absence_parent.as_ref(),
            )
        }) {
            paths.push(input.path.clone());
            gate_ids.push(existing_gate_id.clone());
        }
    }
}

pub(super) fn post_write_analysis_context(
    catalog: &ValidatedGateCatalog,
    gate: &GateRecord,
    transition_sequence: u64,
) -> Result<(Vec<WorktreeTransition>, Vec<ActiveGateLease>), StoreError> {
    let sequences =
        transition_sequences_from_catalog(gate, transition_sequence, &catalog.transitions)?;
    let mut transitions = Vec::with_capacity(sequences.len());
    for sequence in sequences {
        transitions.push(catalog.transitions.get(&sequence).cloned().ok_or_else(|| {
            StoreError::Integrity(format!(
                "referenced worktree transition is missing: {sequence}"
            ))
        })?);
    }
    let mut active_gates = catalog
        .gates
        .values()
        .filter(|other| other.lifecycle == GateLifecycle::Active && other.gate_id != gate.gate_id)
        .map(|other| ActiveGateLease {
            gate_id: other.gate_id.clone(),
            leased_write_set: other.leased_write_set.clone(),
        })
        .collect::<Vec<_>>();
    active_gates.sort_by(|left, right| left.gate_id.cmp(&right.gate_id));
    Ok((transitions, active_gates))
}

pub(super) fn transition_sequences_for_gate(
    catalog: &ValidatedGateCatalog,
    gate: &GateRecord,
    ceiling: u64,
) -> Result<Vec<u64>, StoreError> {
    transition_sequences_from_catalog(gate, ceiling, &catalog.transitions)
}

fn transition_sequences_from_catalog(
    gate: &GateRecord,
    ceiling: u64,
    catalog: &std::collections::BTreeMap<u64, WorktreeTransition>,
) -> Result<Vec<u64>, StoreError> {
    let baseline_sequence = gate
        .baseline
        .as_ref()
        .ok_or_else(|| {
            StoreError::Integrity(format!(
                "active gate omitted its baseline: {}",
                gate.gate_id.as_str()
            ))
        })?
        .transition_sequence;
    let mut references = gate
        .transition_refs
        .iter()
        .copied()
        .filter(|sequence| *sequence > baseline_sequence && *sequence <= ceiling)
        .collect::<Vec<_>>();
    let reference_count = references.len();
    references.sort_unstable();
    references.dedup();
    if references.len() != reference_count {
        return Err(StoreError::Integrity(format!(
            "active gate contains duplicate transition references: {}",
            gate.gate_id.as_str()
        )));
    }

    let catalog = catalog
        .keys()
        .copied()
        .filter(|sequence| *sequence > baseline_sequence && *sequence <= ceiling)
        .collect::<Vec<_>>();
    if references != catalog {
        return Err(StoreError::Integrity(format!(
            "active gate transition references disagree with the catalog: {}",
            gate.gate_id.as_str()
        )));
    }
    Ok(references)
}

pub(super) fn active_write_conflicts(
    catalog: &ValidatedGateCatalog,
    own_gate_id: &GateId,
    changed_paths: &[RepoPathProjection],
) -> Result<Option<ConflictSet>, StoreError> {
    let mut paths = Vec::new();
    let mut gate_ids = Vec::new();
    for gate in catalog.gates.values() {
        if gate.lifecycle != GateLifecycle::Active || gate.gate_id == *own_gate_id {
            continue;
        }
        for path in changed_paths {
            if gate.leased_write_set.iter().any(|lease| lease.covers(path)) {
                paths.push(path.clone());
                gate_ids.push(gate.gate_id.clone());
            }
        }
    }
    paths.sort();
    paths.dedup();
    gate_ids.sort();
    gate_ids.dedup();
    if paths.is_empty() {
        Ok(None)
    } else {
        Ok(Some(ConflictSet { paths, gate_ids }))
    }
}

pub(super) fn semantic_read_conflicts(
    catalog: &ValidatedGateCatalog,
    own_operation_id: &OperationId,
    own_gate_id: &GateId,
    demanded_inputs: &[SemanticReadReservationBinding],
) -> Result<ConflictSet, StoreError> {
    let mut paths = Vec::new();
    let mut gate_ids = Vec::new();
    for operation in catalog.operations.values() {
        if operation.operation_id == *own_operation_id
            || operation.status != GateOperationStatus::Pending
            || operation.kind != GateOperationKind::PreWrite
        {
            continue;
        }
        validate_reservation_binding_set(operation)?;
        collect_reservation_conflicts(
            demanded_inputs,
            &operation.leased_write_set,
            &operation.gate_id,
            &mut paths,
            &mut gate_ids,
        );
    }
    for gate in catalog.gates.values() {
        if gate.lifecycle != GateLifecycle::Active || gate.gate_id == *own_gate_id {
            continue;
        }
        collect_reservation_conflicts(
            demanded_inputs,
            &gate.leased_write_set,
            &gate.gate_id,
            &mut paths,
            &mut gate_ids,
        );
    }
    paths.sort();
    paths.dedup();
    gate_ids.sort();
    gate_ids.dedup();
    Ok(ConflictSet { paths, gate_ids })
}

pub(super) fn attach_transition_references(
    write: &WriteTransaction,
    gates: &mut std::collections::BTreeMap<String, GateRecord>,
    originating_gate_id: &GateId,
    sequence: u64,
) -> Result<(), StoreError> {
    for gate in gates.values_mut() {
        if gate.lifecycle != GateLifecycle::Active || gate.gate_id == *originating_gate_id {
            continue;
        }
        let baseline_sequence = gate
            .baseline
            .as_ref()
            .ok_or_else(|| {
                StoreError::Integrity(format!(
                    "active gate omitted its baseline: {}",
                    gate.gate_id.as_str()
                ))
            })?
            .transition_sequence;
        if baseline_sequence < sequence {
            gate.transition_refs.push(sequence);
            gate.transition_refs.sort_unstable();
            gate.transition_refs.dedup();
            write_record(write, GATES, gate.gate_id.as_str(), &gate)?;
        }
    }
    Ok(())
}
