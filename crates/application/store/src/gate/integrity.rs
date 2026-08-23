use std::collections::BTreeMap;
use std::ops::Bound;

use lumin_evidence::{
    GATE_RECORD_SCHEMA_VERSION, GateDecision, GateLifecycle, GateOperationKind,
    GateOperationStatus, GateRecord, GateRevision, OperationRecord, WorktreeTransition,
    apply_worktree_transition_for_domain,
};
use lumin_model::{GateId, ObservationBinding, SealedGateObservation};
use redb::{ReadTransaction, ReadableTable, TableError, WriteTransaction};

use crate::{SEQUENCES, StoreError};

use super::receipts::{
    validate_gate_validation_receipts, validate_stored_gate_validation_receipts,
};
use super::records::{read_record, transition_key};
use super::{GATES, OPERATIONS, TRANSITIONS};

pub(super) struct ValidatedGateCatalog {
    pub(super) gates: BTreeMap<String, GateRecord>,
    pub(super) transitions: BTreeMap<u64, WorktreeTransition>,
}

pub(super) fn gate_projection_sha256(
    gate: &GateRecord,
    revision: u64,
) -> Result<String, StoreError> {
    validate_gate_record_shape(gate)?;
    let mut projection = if revision == gate.current_revision {
        gate.clone()
    } else {
        historical_gate_projection(gate, revision)?
    };
    // Transition references are a derived catalog projection that can grow after
    // this gate's own immutable operation commits. They are validated against the
    // authenticated transition catalog instead of being frozen in this digest.
    projection.transition_refs.clear();
    let bytes = serde_json::to_vec(&projection).map_err(crate::serialization_error)?;
    let mut framed = Vec::new();
    lumin_model::append_length_prefixed(&mut framed, b"lumin-gate-record-projection.v1");
    lumin_model::append_length_prefixed(&mut framed, &bytes);
    Ok(crate::digest_hex(&framed))
}

pub(super) fn operation_result_sha256(operation: &OperationRecord) -> Result<String, StoreError> {
    let result = operation.result.as_ref().ok_or_else(|| {
        StoreError::Integrity(format!(
            "committed operation {} omitted its result",
            operation.operation_id.as_str()
        ))
    })?;
    let bytes = serde_json::to_vec(result).map_err(crate::serialization_error)?;
    let mut framed = Vec::new();
    lumin_model::append_length_prefixed(&mut framed, b"lumin-gate-operation-result.v1");
    lumin_model::append_length_prefixed(&mut framed, &bytes);
    Ok(crate::digest_hex(&framed))
}

pub(super) fn operation_projection_sha256(
    operation: &OperationRecord,
) -> Result<String, StoreError> {
    let bytes = serde_json::to_vec(operation).map_err(crate::serialization_error)?;
    let mut framed = Vec::new();
    lumin_model::append_length_prefixed(&mut framed, b"lumin-gate-operation-projection.v1");
    lumin_model::append_length_prefixed(&mut framed, &bytes);
    Ok(crate::digest_hex(&framed))
}

pub(super) fn validate_committed_operation_result(
    operation: &OperationRecord,
    gate: &GateRecord,
    revision: &GateRevision,
) -> Result<(), StoreError> {
    if operation.status != GateOperationStatus::Committed {
        return Err(StoreError::Integrity(format!(
            "operation {} owns a durable revision without a committed result",
            operation.operation_id.as_str()
        )));
    }
    let result = operation.result.as_ref().ok_or_else(|| {
        StoreError::Integrity(format!(
            "committed operation {} omitted its result",
            operation.operation_id.as_str()
        ))
    })?;
    let expected_lifecycle = match operation.kind {
        GateOperationKind::PreWrite => {
            if revision.decision.authorizes() {
                GateLifecycle::Active
            } else {
                GateLifecycle::Rejected
            }
        }
        GateOperationKind::PostWrite => {
            if revision.decision.authorizes() {
                GateLifecycle::Closed
            } else {
                GateLifecycle::Active
            }
        }
        GateOperationKind::GateAbandon => GateLifecycle::Abandoned,
    };
    let expected_leased_write_set = match operation.kind {
        GateOperationKind::PreWrite if !revision.decision.authorizes() => Vec::new(),
        GateOperationKind::PreWrite | GateOperationKind::PostWrite => gate
            .baseline
            .as_ref()
            .map_or_else(Vec::new, |baseline| baseline.leased_write_set.clone()),
        GateOperationKind::GateAbandon => Vec::new(),
    };
    if result.operation_id != operation.operation_id
        || result.request_digest != operation.request_digest
        || result.gate_id != operation.gate_id
        || result.revision != revision.revision
        || revision.operation_id != operation.operation_id
        || revision.decision != result.decision
        || revision.observation_binding != result.observation_binding
        || revision.reason != result.reason
        || revision.signals != result.signals
        || revision.actual_write_set != result.actual_write_set
        || revision.deltas != result.deltas
        || result.lifecycle != expected_lifecycle
        || result.leased_write_set != expected_leased_write_set
    {
        return Err(StoreError::Integrity(format!(
            "operation {} result disagrees with its complete gate revision",
            operation.operation_id.as_str()
        )));
    }
    Ok(())
}

pub(super) fn validate_gate_record_shape(gate: &GateRecord) -> Result<(), StoreError> {
    if gate.schema_version != GATE_RECORD_SCHEMA_VERSION {
        return Err(StoreError::IncompatibleStateSchema(format!(
            "gate {} uses unsupported schema {}; expected {GATE_RECORD_SCHEMA_VERSION}",
            gate.gate_id.as_str(),
            gate.schema_version
        )));
    }
    if gate.revisions.is_empty()
        || gate
            .revisions
            .iter()
            .enumerate()
            .any(|(index, revision)| revision.revision != index as u64)
        || gate.revisions.last().map(|revision| revision.revision) != Some(gate.current_revision)
    {
        return Err(StoreError::Integrity(format!(
            "gate {} has a noncanonical revision history",
            gate.gate_id.as_str()
        )));
    }
    if gate.lifecycle == GateLifecycle::Active && gate.current_revision == u64::MAX {
        return Err(StoreError::Integrity(format!(
            "active gate {} exhausted its revision sequence",
            gate.gate_id.as_str()
        )));
    }
    if gate
        .transition_refs
        .windows(2)
        .any(|pair| pair[0] >= pair[1])
    {
        return Err(StoreError::Integrity(format!(
            "gate {} has noncanonical transition references",
            gate.gate_id.as_str()
        )));
    }
    Ok(())
}

pub(super) fn read_validated_gate(
    write: &WriteTransaction,
    gate_id: &lumin_model::GateId,
) -> Result<Option<GateRecord>, StoreError> {
    let gate = read_record::<GateRecord>(write, GATES, gate_id.as_str())?;
    if let Some(gate) = gate.as_ref() {
        if gate.gate_id != *gate_id {
            return Err(StoreError::Integrity(format!(
                "gate key {} disagrees with its record",
                gate_id.as_str()
            )));
        }
        validate_stored_gate_validation_receipts(write, gate)?;
    }
    Ok(gate)
}

pub(super) fn read_validated_gates(
    write: &WriteTransaction,
) -> Result<Vec<GateRecord>, StoreError> {
    let gates = read_gate_map_from_write(write)?;
    for gate in gates.values() {
        validate_stored_gate_validation_receipts(write, gate)?;
    }
    Ok(gates.into_values().collect())
}

pub(super) fn validate_stored_gate_catalog(
    write: &WriteTransaction,
) -> Result<ValidatedGateCatalog, StoreError> {
    let gates = read_gate_map_from_write(write)?;
    for gate in gates.values() {
        validate_stored_gate_validation_receipts(write, gate)?;
    }
    let transitions = read_transition_map_from_write(write)?;
    validate_transition_catalog(&gates, &transitions)?;
    validate_allocator_floors(
        &gates,
        &transitions,
        &read_operation_gate_ids_from_write(write)?,
        read_sequence_from_write(write, "gate")?,
        read_sequence_from_write(write, "transition")?,
    )?;
    Ok(ValidatedGateCatalog { gates, transitions })
}

pub(super) fn validate_loaded_gate_catalog(
    read: &ReadTransaction,
) -> Result<ValidatedGateCatalog, StoreError> {
    let gates = read_gate_map_from_read(read)?;
    for gate in gates.values() {
        validate_gate_validation_receipts(read, gate)?;
    }
    let transitions = read_transition_map_from_read(read)?;
    validate_transition_catalog(&gates, &transitions)?;
    validate_allocator_floors(
        &gates,
        &transitions,
        &read_operation_gate_ids_from_read(read)?,
        read_sequence_from_read(read, "gate")?,
        read_sequence_from_read(read, "transition")?,
    )?;
    Ok(ValidatedGateCatalog { gates, transitions })
}

fn validate_allocator_floors(
    gates: &BTreeMap<String, GateRecord>,
    transitions: &BTreeMap<u64, WorktreeTransition>,
    operation_gate_ids: &[GateId],
    gate_sequence: u64,
    transition_sequence: u64,
) -> Result<(), StoreError> {
    let mut minimum_gate_sequence = 0;
    for gate_id in gates
        .values()
        .map(|gate| &gate.gate_id)
        .chain(operation_gate_ids)
    {
        minimum_gate_sequence = minimum_gate_sequence.max(canonical_gate_sequence(gate_id)?);
    }
    if gate_sequence < minimum_gate_sequence {
        return Err(StoreError::Integrity(format!(
            "gate allocator sequence {gate_sequence} trails retained allocation {minimum_gate_sequence}"
        )));
    }
    if gate_sequence == u64::MAX {
        return Err(StoreError::Integrity(
            "gate allocator sequence is exhausted".to_owned(),
        ));
    }

    let minimum_transition_sequence = transitions.keys().next_back().copied().unwrap_or(0);
    if transition_sequence < minimum_transition_sequence {
        return Err(StoreError::Integrity(format!(
            "transition allocator sequence {transition_sequence} trails authenticated catalog {minimum_transition_sequence}"
        )));
    }
    if transition_sequence == u64::MAX {
        return Err(StoreError::Integrity(
            "transition allocator sequence is exhausted".to_owned(),
        ));
    }
    Ok(())
}

fn canonical_gate_sequence(gate_id: &GateId) -> Result<u64, StoreError> {
    let value = gate_id.as_str();
    let Some(sequence) = value.strip_prefix("gate_") else {
        return Err(StoreError::Integrity(format!(
            "gate allocator owner has a noncanonical ID: {value}"
        )));
    };
    if sequence.len() != 16
        || !sequence
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(StoreError::Integrity(format!(
            "gate allocator owner has a noncanonical ID: {value}"
        )));
    }
    u64::from_str_radix(sequence, 16).map_err(|error| {
        StoreError::Integrity(format!(
            "gate allocator owner ID {value} cannot be decoded: {error}"
        ))
    })
}

fn read_operation_gate_ids_from_write(write: &WriteTransaction) -> Result<Vec<GateId>, StoreError> {
    let table = write.open_table(OPERATIONS).map_err(crate::backend_error)?;
    let mut gate_ids = Vec::new();
    for row in table.iter().map_err(crate::backend_error)? {
        let (_, value) = row.map_err(crate::backend_error)?;
        let operation = serde_json::from_slice::<OperationRecord>(value.value())
            .map_err(crate::serialization_error)?;
        gate_ids.push(operation.gate_id);
    }
    Ok(gate_ids)
}

fn read_operation_gate_ids_from_read(read: &ReadTransaction) -> Result<Vec<GateId>, StoreError> {
    let table = match read.open_table(OPERATIONS) {
        Ok(table) => table,
        Err(TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
        Err(error) => return Err(crate::backend_error(error)),
    };
    let mut gate_ids = Vec::new();
    for row in table.iter().map_err(crate::backend_error)? {
        let (_, value) = row.map_err(crate::backend_error)?;
        let operation = serde_json::from_slice::<OperationRecord>(value.value())
            .map_err(crate::serialization_error)?;
        gate_ids.push(operation.gate_id);
    }
    Ok(gate_ids)
}

fn read_sequence_from_write(write: &WriteTransaction, key: &str) -> Result<u64, StoreError> {
    let table = write.open_table(SEQUENCES).map_err(crate::backend_error)?;
    let value = table.get(key).map_err(crate::backend_error)?;
    Ok(value.map_or(0, |value| value.value()))
}

fn read_sequence_from_read(read: &ReadTransaction, key: &str) -> Result<u64, StoreError> {
    let table = match read.open_table(SEQUENCES) {
        Ok(table) => table,
        Err(TableError::TableDoesNotExist(_)) => return Ok(0),
        Err(error) => return Err(crate::backend_error(error)),
    };
    let value = table.get(key).map_err(crate::backend_error)?;
    Ok(value.map_or(0, |value| value.value()))
}

fn historical_gate_projection(
    gate: &GateRecord,
    revision_number: u64,
) -> Result<GateRecord, StoreError> {
    let index = gate
        .revisions
        .iter()
        .position(|revision| revision.revision == revision_number)
        .ok_or_else(|| {
            StoreError::Integrity(format!(
                "gate {} omitted historical revision {revision_number}",
                gate.gate_id.as_str()
            ))
        })?;
    let revisions = gate.revisions[..=index].to_vec();
    let opening = &revisions[0];
    let mut lifecycle = if opening.decision.authorizes() {
        GateLifecycle::Active
    } else {
        GateLifecycle::Rejected
    };
    let mut leased_write_set = Vec::new();
    let mut alias_closures = Vec::new();
    let mut protected_semantic_inputs = Vec::new();
    if lifecycle == GateLifecycle::Active {
        let baseline = gate.baseline.as_ref().ok_or_else(|| {
            StoreError::Integrity(format!(
                "active gate {} omitted its baseline",
                gate.gate_id.as_str()
            ))
        })?;
        leased_write_set = baseline.leased_write_set.clone();
        alias_closures = baseline.alias_closures.clone();
        protected_semantic_inputs = baseline.protected_semantic_inputs.clone();
    }
    for revision in revisions.iter().skip(1) {
        let administrative_abandon = revision.reason.is_some()
            && revision.observation_binding.is_none()
            && revision.snapshot.is_none();
        if administrative_abandon {
            lifecycle = GateLifecycle::Abandoned;
            leased_write_set.clear();
            alias_closures.clear();
            protected_semantic_inputs.clear();
            continue;
        }
        let sealed_current_close = revision.snapshot.is_some()
            && revision.decision != GateDecision::Stale
            && matches!(
                revision.observation_binding.as_ref(),
                Some(ObservationBinding::Sealed {
                    observation: SealedGateObservation::Close { .. }
                })
            );
        if sealed_current_close {
            protected_semantic_inputs = revision.protected_semantic_inputs.clone();
        }
        if revision.decision.authorizes() {
            lifecycle = GateLifecycle::Closed;
            alias_closures = revision.alias_closures.clone();
        } else {
            lifecycle = GateLifecycle::Active;
        }
    }
    Ok(GateRecord {
        schema_version: gate.schema_version.clone(),
        gate_id: gate.gate_id.clone(),
        lifecycle,
        current_revision: revision_number,
        declared_write_set: gate.declared_write_set.clone(),
        leased_write_set,
        alias_closures,
        transition_refs: Vec::new(),
        analysis_options: gate.analysis_options.clone(),
        baseline: gate.baseline.clone(),
        protected_semantic_inputs,
        revisions,
    })
}

fn validate_transition_catalog(
    gates: &BTreeMap<String, GateRecord>,
    transitions: &BTreeMap<u64, WorktreeTransition>,
) -> Result<(), StoreError> {
    for (sequence, transition) in transitions {
        let gate = gates
            .get(transition.capsule.gate_id.as_str())
            .ok_or_else(|| {
                StoreError::Integrity(format!("transition {sequence} references a missing gate"))
            })?;
        let revision = gate
            .revisions
            .iter()
            .find(|revision| revision.revision == transition.capsule.revision)
            .ok_or_else(|| {
                StoreError::Integrity(format!(
                    "transition {sequence} references a missing gate revision"
                ))
            })?;
        let baseline = gate.baseline.as_ref().ok_or_else(|| {
            StoreError::Integrity(format!("transition {sequence} owner omitted its baseline"))
        })?;
        let close_matches = matches!(
            revision.observation_binding.as_ref(),
            Some(ObservationBinding::Sealed {
                observation: SealedGateObservation::Close { observation_id }
            }) if observation_id == &transition.capsule.close_observation_id
        );
        let payload_matches = baseline.observation_id == transition.capsule.baseline_observation_id
            && revision.decision.authorizes()
            && gate.lifecycle == GateLifecycle::Closed
            && gate.current_revision == revision.revision
            && revision.snapshot.as_ref() == Some(&transition.capsule.after_snapshot)
            && revision.changed_paths == transition.capsule.changed_paths
            && baseline.leased_write_set == transition.capsule.leased_write_set;
        let before_snapshot =
            reconstruct_transition_before(transitions, baseline, revision, *sequence)?;
        if !close_matches
            || !payload_matches
            || before_snapshot != transition.capsule.before_snapshot
        {
            return Err(StoreError::Integrity(format!(
                "transition {sequence} payload disagrees with its authenticated gate revision"
            )));
        }
    }

    for gate in gates.values() {
        let expected_refs = if gate.lifecycle == GateLifecycle::Active {
            let baseline = gate.baseline.as_ref().ok_or_else(|| {
                StoreError::Integrity(format!(
                    "active gate {} omitted its baseline",
                    gate.gate_id.as_str()
                ))
            })?;
            transitions
                .range((
                    Bound::Excluded(baseline.transition_sequence),
                    Bound::Unbounded,
                ))
                .map(|(sequence, _)| *sequence)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        if gate.transition_refs != expected_refs {
            return Err(StoreError::Integrity(format!(
                "gate {} transition references disagree with the authenticated catalog",
                gate.gate_id.as_str()
            )));
        }
        if gate.lifecycle == GateLifecycle::Closed {
            let matching = transitions
                .values()
                .filter(|transition| {
                    transition.capsule.gate_id == gate.gate_id
                        && transition.capsule.revision == gate.current_revision
                })
                .count();
            if matching != 1 {
                return Err(StoreError::Integrity(format!(
                    "closed gate {} requires exactly one authenticated terminal transition",
                    gate.gate_id.as_str()
                )));
            }
        }
    }
    Ok(())
}

fn reconstruct_transition_before(
    transitions: &BTreeMap<u64, WorktreeTransition>,
    baseline: &lumin_evidence::GateBaseline,
    revision: &GateRevision,
    transition_sequence: u64,
) -> Result<lumin_evidence::AnalysisSnapshot, StoreError> {
    if baseline.transition_sequence >= transition_sequence {
        return Err(StoreError::Integrity(format!(
            "transition {transition_sequence} does not follow its owner's baseline"
        )));
    }
    let expected_sequences = transitions
        .range((
            Bound::Excluded(baseline.transition_sequence),
            Bound::Excluded(transition_sequence),
        ))
        .map(|(sequence, _)| *sequence)
        .collect::<Vec<_>>();
    if revision.reconciled_transition_sequences != expected_sequences {
        return Err(StoreError::Integrity(format!(
            "transition {transition_sequence} owner omitted its exact predecessor chain"
        )));
    }
    let mut reconstructed = baseline.snapshot.clone();
    for sequence in expected_sequences {
        let transition = transitions.get(&sequence).ok_or_else(|| {
            StoreError::Integrity(format!(
                "transition {transition_sequence} predecessor {sequence} is missing"
            ))
        })?;
        if !apply_worktree_transition_for_domain(
            &mut reconstructed,
            transition,
            &baseline.leased_write_set,
            &baseline.protected_semantic_inputs,
        ) {
            return Err(StoreError::Integrity(format!(
                "transition {transition_sequence} predecessor chain cannot be replayed"
            )));
        }
    }
    Ok(reconstructed)
}

fn read_gate_map_from_write(
    write: &WriteTransaction,
) -> Result<BTreeMap<String, GateRecord>, StoreError> {
    let table = write.open_table(GATES).map_err(crate::backend_error)?;
    let mut gates = BTreeMap::new();
    for row in table.iter().map_err(crate::backend_error)? {
        let (key, value) = row.map_err(crate::backend_error)?;
        let key = key.value().to_owned();
        let gate = serde_json::from_slice::<GateRecord>(value.value())
            .map_err(crate::serialization_error)?;
        insert_gate(&mut gates, key, gate)?;
    }
    Ok(gates)
}

fn read_gate_map_from_read(
    read: &ReadTransaction,
) -> Result<BTreeMap<String, GateRecord>, StoreError> {
    let table = match read.open_table(GATES) {
        Ok(table) => table,
        Err(TableError::TableDoesNotExist(_)) => return Ok(BTreeMap::new()),
        Err(error) => return Err(crate::backend_error(error)),
    };
    let mut gates = BTreeMap::new();
    for row in table.iter().map_err(crate::backend_error)? {
        let (key, value) = row.map_err(crate::backend_error)?;
        let key = key.value().to_owned();
        let gate = serde_json::from_slice::<GateRecord>(value.value())
            .map_err(crate::serialization_error)?;
        insert_gate(&mut gates, key, gate)?;
    }
    Ok(gates)
}

fn insert_gate(
    gates: &mut BTreeMap<String, GateRecord>,
    key: String,
    gate: GateRecord,
) -> Result<(), StoreError> {
    if gate.gate_id.as_str() != key {
        return Err(StoreError::Integrity(format!(
            "gate key {key} disagrees with gate_id {}",
            gate.gate_id.as_str()
        )));
    }
    if gates.insert(key.clone(), gate).is_some() {
        return Err(StoreError::Integrity(format!(
            "gate catalog contains duplicate key {key}"
        )));
    }
    Ok(())
}

fn read_transition_map_from_write(
    write: &WriteTransaction,
) -> Result<BTreeMap<u64, WorktreeTransition>, StoreError> {
    let table = write
        .open_table(TRANSITIONS)
        .map_err(crate::backend_error)?;
    let mut transitions = BTreeMap::new();
    for row in table.iter().map_err(crate::backend_error)? {
        let (key, value) = row.map_err(crate::backend_error)?;
        let key = key.value().to_owned();
        let transition = serde_json::from_slice::<WorktreeTransition>(value.value())
            .map_err(crate::serialization_error)?;
        insert_transition(&mut transitions, key, transition)?;
    }
    Ok(transitions)
}

fn read_transition_map_from_read(
    read: &ReadTransaction,
) -> Result<BTreeMap<u64, WorktreeTransition>, StoreError> {
    let table = match read.open_table(TRANSITIONS) {
        Ok(table) => table,
        Err(TableError::TableDoesNotExist(_)) => return Ok(BTreeMap::new()),
        Err(error) => return Err(crate::backend_error(error)),
    };
    let mut transitions = BTreeMap::new();
    for row in table.iter().map_err(crate::backend_error)? {
        let (key, value) = row.map_err(crate::backend_error)?;
        let key = key.value().to_owned();
        let transition = serde_json::from_slice::<WorktreeTransition>(value.value())
            .map_err(crate::serialization_error)?;
        insert_transition(&mut transitions, key, transition)?;
    }
    Ok(transitions)
}

fn insert_transition(
    transitions: &mut BTreeMap<u64, WorktreeTransition>,
    key: String,
    transition: WorktreeTransition,
) -> Result<(), StoreError> {
    if key != transition_key(transition.sequence) {
        return Err(StoreError::Integrity(format!(
            "transition key {key} disagrees with sequence {}",
            transition.sequence
        )));
    }
    let sequence = transition.sequence;
    if transitions.insert(sequence, transition).is_some() {
        return Err(StoreError::Integrity(format!(
            "transition catalog contains duplicate sequence {sequence}"
        )));
    }
    Ok(())
}
