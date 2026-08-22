mod cache;
mod external;
mod retention;

use std::collections::{BTreeMap, BTreeSet};

use lumin_evidence::{
    AnalysisSnapshot, GATE_OPERATION_SCHEMA_VERSION, GATE_RECORD_SCHEMA_VERSION, GateBaseline,
    GateBaselineObservationInput, GateCloseObservationInput, GateDecision, GateLifecycle,
    GateOperationKind, GateOperationStatus, GateRecord, GateRevision, GateSignal, OperationRecord,
    PhysicalAliasClosureRecord, RUN_EVIDENCE_SCHEMA_VERSION,
    SUPPORTED_ACTIVE_GATE_ANALYSIS_CONTRACT_ID, WorktreeTransition, WriteLeaseKind,
    apply_worktree_transition_for_domain, derive_gate_baseline_observation_id,
    derive_gate_close_observation_id, derive_protected_semantic_inputs,
    derive_unsealed_gate_observation_binding, gate_abandon_request_digest, gate_policy,
    post_write_request_digest, pre_write_request_digest, seal_analysis_snapshot,
};
use lumin_model::{ObservationBinding, RepoPath, SealedGateObservation};
use serde::de::DeserializeOwned;

use crate::gate::{
    records::ACTIVE_GATE_CATALOG_SEQUENCE_KEY, transition_key, validate_reservation_binding_set,
};
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
    let gates = read_gates(snapshot, &operations, &transitions, &transition_sequences)?;
    validate_baseline_transition_boundaries(&transitions, &gates)?;
    validate_gate_id_sequence(snapshot, &gates, &operations)?;
    validate_transition_catalog_sequence(snapshot, &transition_sequences, &gates, &operations)?;
    validate_active_gate_catalog(snapshot, &gates, &operations)?;
    validate_operation_gate_refs(&operations, &gates)?;
    validate_transition_gate_refs(&transitions, &gates)?;
    crate::publication::validate_attempt_leases(&snapshot.attempt_leases)?;
    validate_run_catalog(snapshot)?;
    cache::validate_cache(snapshot, &operations)?;
    retention::validate_retention(snapshot, &operations)?;
    validate_pointers(snapshot)
}

fn validate_gate_id_sequence(
    snapshot: &LogicalStoreSnapshot,
    gates: &BTreeMap<&str, GateRecord>,
    operations: &BTreeMap<&str, OperationRecord>,
) -> Result<(), StoreError> {
    let observed = snapshot.sequences.get("gate").copied().unwrap_or(0);
    let minimum = gates
        .values()
        .map(|gate| canonical_gate_sequence(gate.gate_id.as_str()))
        .chain(
            operations
                .values()
                .map(|operation| canonical_gate_sequence(operation.gate_id.as_str())),
        )
        .try_fold(0_u64, |maximum, sequence| {
            sequence.map(|sequence| maximum.max(sequence))
        })?;
    if observed < minimum {
        return Err(StoreError::Integrity(format!(
            "gate sequence regressed below retained gate allocation: observed {observed}, minimum {minimum}"
        )));
    }
    Ok(())
}

fn canonical_gate_sequence(value: &str) -> Result<u64, StoreError> {
    let suffix = value.strip_prefix("gate_").ok_or_else(|| {
        StoreError::Integrity("gate ID is outside its canonical grammar".to_owned())
    })?;
    if suffix.len() != 16
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(StoreError::Integrity(
            "gate ID is outside its canonical grammar".to_owned(),
        ));
    }
    let sequence = u64::from_str_radix(suffix, 16).map_err(|error| {
        StoreError::Integrity(format!("gate ID sequence is malformed: {error}"))
    })?;
    if sequence == 0 {
        return Err(StoreError::Integrity(
            "gate ID sequence must be nonzero".to_owned(),
        ));
    }
    Ok(sequence)
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
        if operation.schema_version != GATE_OPERATION_SCHEMA_VERSION {
            return Err(StoreError::IncompatibleStateSchema(format!(
                "operation {key} uses unsupported schema {}; expected {GATE_OPERATION_SCHEMA_VERSION}",
                operation.schema_version
            )));
        }
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

fn validate_baseline_transition_boundaries(
    transitions: &BTreeMap<u64, WorktreeTransition>,
    gates: &BTreeMap<&str, GateRecord>,
) -> Result<(), StoreError> {
    for (key, gate) in gates {
        let Some(baseline) = gate.baseline.as_ref() else {
            continue;
        };
        let opening_authorized = gate
            .revisions
            .first()
            .is_some_and(|revision| revision.decision.authorizes());
        if !opening_authorized {
            continue;
        }
        for transition in transitions.values() {
            let owner = gates
                .get(transition.capsule.gate_id.as_str())
                .ok_or_else(|| {
                    StoreError::Integrity(format!(
                        "transition {} references a missing gate",
                        transition.sequence
                    ))
                })?;
            let close_revision = owner
                .revisions
                .iter()
                .find(|revision| revision.revision == transition.capsule.revision)
                .ok_or_else(|| {
                    StoreError::Integrity(format!(
                        "transition {} references a missing gate revision",
                        transition.sequence
                    ))
                })?;
            let close_catalog_revision = close_revision.catalog_revision.ok_or_else(|| {
                StoreError::Integrity(format!(
                    "transition {} omitted its authorizing catalog epoch",
                    transition.sequence
                ))
            })?;
            let included = transition.sequence <= baseline.transition_sequence;
            let predates_opening = close_catalog_revision < baseline.catalog_revision;
            let follows_opening = close_catalog_revision > baseline.catalog_revision;
            if included != predates_opening || (!included && !follows_opening) {
                return Err(StoreError::Integrity(format!(
                    "gate {key} baseline transition boundary disagrees with its catalog epoch"
                )));
            }
        }
    }
    Ok(())
}

fn read_gates<'a>(
    snapshot: &'a LogicalStoreSnapshot,
    operations: &BTreeMap<&str, OperationRecord>,
    transitions: &BTreeMap<u64, WorktreeTransition>,
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
        validate_gate_history(key, &gate, operations, transitions, transition_sequences)?;
        gates.insert(key.as_str(), gate);
    }
    Ok(gates)
}

fn validate_transition_catalog_sequence(
    snapshot: &LogicalStoreSnapshot,
    transition_sequences: &BTreeSet<u64>,
    gates: &BTreeMap<&str, GateRecord>,
    operations: &BTreeMap<&str, OperationRecord>,
) -> Result<(), StoreError> {
    let observed = snapshot.sequences.get("transition").copied().unwrap_or(0);
    let mut minimum = transition_sequences
        .iter()
        .next_back()
        .copied()
        .unwrap_or(0);
    for gate in gates.values() {
        if let Some(baseline) = &gate.baseline {
            minimum = minimum.max(baseline.transition_sequence);
        }
        if let Some(sequence) = gate.transition_refs.iter().max() {
            minimum = minimum.max(*sequence);
        }
        for revision in &gate.revisions {
            if let Some(sequence) = revision.reconciled_transition_sequences.iter().max() {
                minimum = minimum.max(*sequence);
            }
        }
    }
    for operation in operations.values() {
        minimum = minimum.max(operation.transition_sequence);
    }
    if observed < minimum {
        return Err(StoreError::Integrity(format!(
            "transition sequence regressed below durable transition history: observed {observed}, minimum {minimum}"
        )));
    }
    Ok(())
}

fn validate_active_gate_catalog(
    snapshot: &LogicalStoreSnapshot,
    gates: &BTreeMap<&str, GateRecord>,
    operations: &BTreeMap<&str, OperationRecord>,
) -> Result<(), StoreError> {
    let observed = snapshot
        .sequences
        .get(ACTIVE_GATE_CATALOG_SEQUENCE_KEY)
        .copied()
        .unwrap_or(0);
    let mut minimum = 0_u64;
    let mut retained_mutation_count = 0_u64;
    for (key, gate) in gates {
        let mut gate_minimum = 0_u64;
        let mut protected_semantic_inputs =
            gate.baseline.as_ref().map_or_else(Vec::new, |baseline| {
                baseline.protected_semantic_inputs.clone()
            });
        for revision in &gate.revisions {
            if let Some(catalog_revision) = revision.catalog_revision {
                if catalog_revision > observed {
                    return Err(StoreError::Integrity(format!(
                        "gate {key} observation catalog revision exceeds the active-gate catalog"
                    )));
                }
                gate_minimum = gate_minimum.max(catalog_revision);
            }
            let kind = operations
                .get(revision.operation_id.as_str())
                .map(|operation| operation.kind)
                .ok_or_else(|| {
                    StoreError::Integrity(format!(
                        "gate {key} catalog history references a missing operation"
                    ))
                })?;
            let sealed_current_close = kind == GateOperationKind::PostWrite
                && revision.decision != GateDecision::Stale
                && matches!(
                    revision.observation_binding.as_ref(),
                    Some(ObservationBinding::Sealed {
                        observation: SealedGateObservation::Close { .. }
                    })
                );
            let replaces_protected_reads = sealed_current_close
                && revision.protected_semantic_inputs != protected_semantic_inputs;
            if sealed_current_close {
                protected_semantic_inputs = revision.protected_semantic_inputs.clone();
            }
            let advances_catalog = match kind {
                GateOperationKind::PreWrite | GateOperationKind::PostWrite => {
                    revision.decision.authorizes() || replaces_protected_reads
                }
                GateOperationKind::GateAbandon => true,
            };
            if advances_catalog {
                retained_mutation_count =
                    retained_mutation_count.checked_add(1).ok_or_else(|| {
                        StoreError::Integrity(
                            "retained active-catalog mutation history overflowed".to_owned(),
                        )
                    })?;
                gate_minimum = gate_minimum.checked_add(1).ok_or_else(|| {
                    StoreError::Integrity(format!("gate {key} active-catalog history overflowed"))
                })?;
            }
            minimum = minimum.max(gate_minimum);
        }
    }
    minimum = minimum.max(retained_mutation_count);
    if observed < minimum {
        return Err(StoreError::Integrity(format!(
            "active-gate catalog sequence regressed below durable gate history: observed {observed}, minimum {minimum}"
        )));
    }
    Ok(())
}

fn validate_gate_history(
    key: &str,
    gate: &GateRecord,
    operations: &BTreeMap<&str, OperationRecord>,
    transitions: &BTreeMap<u64, WorktreeTransition>,
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
    for (index, revision) in gate.revisions.iter().enumerate() {
        let operation = operations
            .get(revision.operation_id.as_str())
            .ok_or_else(|| {
                StoreError::Integrity(format!(
                    "gate {key} revision history references a missing operation"
                ))
            })?;
        if revision.revision != index as u64
            || operation.gate_id != gate.gate_id
            || operation.status != GateOperationStatus::Committed
            || operation.result.as_ref().is_none_or(|result| {
                result.revision != revision.revision || result.operation_id != revision.operation_id
            })
            || if revision.revision == 0 {
                operation.kind != GateOperationKind::PreWrite || operation.target_revision != 0
            } else {
                !matches!(
                    operation.kind,
                    GateOperationKind::PostWrite | GateOperationKind::GateAbandon
                ) || operation.target_revision != revision.revision.saturating_sub(1)
            }
        {
            return Err(StoreError::Integrity(format!(
                "gate {key} revision history is not owned by committed gate operations"
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
        if operation.kind == GateOperationKind::PostWrite
            && revision.decision.authorizes()
            && revision.revision != gate.current_revision
        {
            return Err(StoreError::Integrity(format!(
                "gate {key} authorizing close revision {} is not the durable tail",
                revision.revision
            )));
        }
        if matches!(
            revision.observation_binding.as_ref(),
            Some(ObservationBinding::Sealed {
                observation: SealedGateObservation::Close { .. }
            })
        ) {
            let baseline = gate.baseline.as_ref().ok_or_else(|| {
                StoreError::Integrity(format!(
                    "gate {key} sealed close omitted its opening baseline"
                ))
            })?;
            let expected = transition_sequences
                .iter()
                .copied()
                .filter(|sequence| {
                    *sequence > baseline.transition_sequence
                        && *sequence <= operation.transition_sequence
                })
                .collect::<Vec<_>>();
            if revision.reconciled_transition_sequences != expected {
                return Err(StoreError::Integrity(format!(
                    "gate {key} sealed close revision {} omitted or reordered its transition chain",
                    revision.revision
                )));
            }
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
    if gate.lifecycle == GateLifecycle::Active {
        let baseline = gate.baseline.as_ref().ok_or_else(|| {
            StoreError::Integrity(format!("active gate {key} omitted its sealed baseline"))
        })?;
        let expected = transition_sequences
            .iter()
            .copied()
            .filter(|sequence| *sequence > baseline.transition_sequence)
            .collect::<Vec<_>>();
        if gate.transition_refs != expected {
            return Err(StoreError::Integrity(format!(
                "active gate {key} transition reference set disagrees with the complete catalog"
            )));
        }
    }
    validate_gate_observations(key, gate, operations, transitions)?;
    Ok(())
}

fn validate_gate_observations(
    key: &str,
    gate: &GateRecord,
    operations: &BTreeMap<&str, OperationRecord>,
    transitions: &BTreeMap<u64, WorktreeTransition>,
) -> Result<(), StoreError> {
    let opening = gate
        .revisions
        .first()
        .ok_or_else(|| StoreError::Integrity(format!("gate {key} omitted its opening revision")))?;
    let opening_operation = operations
        .get(opening.operation_id.as_str())
        .ok_or_else(|| StoreError::Integrity(format!("gate {key} opening operation is missing")))?;
    let opening_analysis_options =
        opening_operation.analysis_options.as_ref().ok_or_else(|| {
            StoreError::Integrity(format!(
                "gate {key} opening operation omitted its analysis options"
            ))
        })?;
    let opening_final_validation = opening_operation
        .pre_write_final_validation
        .as_ref()
        .ok_or_else(|| {
            StoreError::Integrity(format!(
                "gate {key} opening operation omitted its final validation record"
            ))
        })?;
    if opening.catalog_revision != Some(opening_final_validation.catalog_revision)
        || opening_final_validation.signals != opening.signals
    {
        return Err(StoreError::Integrity(format!(
            "gate {key} opening revision disagrees with its operation-owned final validation"
        )));
    }
    if opening_analysis_options != &gate.analysis_options {
        return Err(StoreError::Integrity(format!(
            "gate {key} analysis options disagree with its opening operation"
        )));
    }
    if opening_operation.declared_write_set != gate.declared_write_set {
        return Err(StoreError::Integrity(format!(
            "gate {key} declared write set disagrees with its opening operation"
        )));
    }
    if gate.analysis_options.jobs == 0 {
        return Err(StoreError::Integrity(format!(
            "gate {key} analysis options have an invalid zero worker count"
        )));
    }
    if gate.analysis_options.resolution_profile
        != gate.analysis_options.scan_invocation.resolution_profile
    {
        return Err(StoreError::Integrity(format!(
            "gate {key} analysis options have inconsistent resolution profiles"
        )));
    }
    validate_scan_invocation_patterns(
        &format!("gate {key}"),
        &gate.analysis_options.scan_invocation,
    )?;
    if opening.snapshot.is_some()
        || opening.actual_write_set.is_some()
        || !opening.changed_paths.is_empty()
        || !opening.reconciled_transition_sequences.is_empty()
        || !opening.deltas.is_empty()
    {
        return Err(StoreError::Integrity(format!(
            "gate {key} opening revision retained close-only payloads"
        )));
    }
    match &gate.baseline {
        Some(baseline) => match &opening.observation_binding {
            Some(ObservationBinding::Sealed {
                observation: SealedGateObservation::Baseline { observation_id },
            }) if observation_id == &baseline.observation_id
                && opening.catalog_revision == Some(baseline.catalog_revision) =>
            {
                if gate.lifecycle == GateLifecycle::Active
                    && baseline.analysis_contract != SUPPORTED_ACTIVE_GATE_ANALYSIS_CONTRACT_ID
                {
                    return Err(StoreError::IncompatibleStateSchema(format!(
                        "active gate {key} uses unsupported analysis contract {}",
                        baseline.analysis_contract
                    )));
                }
                if baseline.transition_sequence != opening_operation.transition_sequence {
                    return Err(StoreError::Integrity(format!(
                        "gate {key} baseline transition boundary disagrees with its opening operation"
                    )));
                }
                if opening.protected_semantic_inputs != baseline.protected_semantic_inputs
                    || opening.alias_closures != baseline.alias_closures
                {
                    return Err(StoreError::Integrity(format!(
                        "gate {key} opening revision payload disagrees with its sealed baseline"
                    )));
                }
                if gate.analysis_options.scan_invocation != baseline.snapshot.scan_invocation {
                    return Err(StoreError::Integrity(format!(
                        "gate {key} analysis invocation disagrees with its sealed baseline"
                    )));
                }
                let evidence_payload_sha256 =
                    validate_analysis_snapshot(key, "baseline", &baseline.snapshot)?;
                validate_baseline_write_domain(key, gate, baseline)?;
                if baseline.protected_semantic_inputs
                    != derive_protected_semantic_inputs(
                        &baseline.snapshot,
                        &baseline.leased_write_set,
                    )
                {
                    return Err(StoreError::Integrity(format!(
                        "gate {key} baseline protected reads cannot be derived from its sealed snapshot"
                    )));
                }
                if gate.lifecycle == GateLifecycle::Active
                    && (gate.leased_write_set.iter().collect::<BTreeSet<_>>()
                        != baseline.leased_write_set.iter().collect::<BTreeSet<_>>()
                        || gate.alias_closures.iter().collect::<BTreeSet<_>>()
                            != baseline.alias_closures.iter().collect::<BTreeSet<_>>())
                {
                    return Err(StoreError::Integrity(format!(
                        "gate {key} retained lease/alias domain disagrees with its sealed baseline"
                    )));
                }
                let derived = derive_gate_baseline_observation_id(GateBaselineObservationInput {
                    catalog_revision: baseline.catalog_revision,
                    transition_sequence: baseline.transition_sequence,
                    analysis_contract: &baseline.analysis_contract,
                    analysis_input_id: &baseline.snapshot.analysis_input_id,
                    evidence_payload_sha256: &evidence_payload_sha256,
                    signals: &opening.signals,
                    declared_write_set: &gate.declared_write_set,
                    leased_write_set: &baseline.leased_write_set,
                    alias_closures: &baseline.alias_closures,
                    protected_semantic_inputs: &baseline.protected_semantic_inputs,
                });
                if derived != baseline.observation_id {
                    return Err(StoreError::Integrity(format!(
                        "gate {key} baseline observation cannot be reconstructed"
                    )));
                }
                validate_opening_snapshot_policy(key, opening, baseline)?;
            }
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

    let mut expected_protected_semantic_inputs =
        gate.baseline.as_ref().map_or_else(Vec::new, |baseline| {
            baseline.protected_semantic_inputs.clone()
        });
    for revision in &gate.revisions {
        let is_abandon = operations
            .get(revision.operation_id.as_str())
            .is_some_and(|operation| operation.kind == GateOperationKind::GateAbandon);
        if !is_abandon && revision.reason.is_some() {
            return Err(StoreError::Integrity(format!(
                "gate {key} non-administrative revision {} retained a reason",
                revision.revision
            )));
        }
        if !is_abandon && revision.decision != gate_policy::decision(&revision.signals) {
            return Err(StoreError::Integrity(format!(
                "gate {key} revision {} decision disagrees with canonical signal policy",
                revision.revision
            )));
        }
        if !is_abandon && revision.observation_binding.is_none() {
            return Err(StoreError::Integrity(format!(
                "gate {key} revision {} omitted its observation binding",
                revision.revision
            )));
        }
        if is_abandon {
            if revision.decision != GateDecision::Deny {
                return Err(StoreError::Integrity(format!(
                    "gate {key} administrative abandon revision {} must deny",
                    revision.revision
                )));
            }
            if revision.catalog_revision.is_some()
                || revision.observation_binding.is_some()
                || revision.unsealed_observation_inputs.is_some()
                || !revision.signals.is_empty()
                || !revision.changed_paths.is_empty()
                || revision.actual_write_set.is_some()
                || revision.snapshot.is_some()
                || !revision.protected_semantic_inputs.is_empty()
                || !revision.alias_closures.is_empty()
                || !revision.reconciled_transition_sequences.is_empty()
                || !revision.deltas.is_empty()
            {
                return Err(StoreError::Integrity(format!(
                    "gate {key} administrative abandon revision {} retained evidence payloads",
                    revision.revision
                )));
            }
        } else if revision.catalog_revision.is_none() {
            return Err(StoreError::Integrity(format!(
                "gate {key} revision {} omitted its observation catalog revision",
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
        if matches!(
            revision.observation_binding.as_ref(),
            Some(ObservationBinding::Unsealed { .. })
        ) {
            if revision.snapshot.is_some()
                || revision.actual_write_set.is_some()
                || !revision.changed_paths.is_empty()
                || !revision.protected_semantic_inputs.is_empty()
                || !revision.alias_closures.is_empty()
                || !revision.reconciled_transition_sequences.is_empty()
                || !revision.deltas.is_empty()
            {
                return Err(StoreError::Integrity(format!(
                    "gate {key} unsealed revision {} retained complete-observation payloads",
                    revision.revision
                )));
            }
            let inputs = revision
                .unsealed_observation_inputs
                .as_ref()
                .ok_or_else(|| {
                    StoreError::Integrity(format!(
                        "gate {key} unsealed revision {} omitted its typed derivation inputs",
                        revision.revision
                    ))
                })?;
            if !inputs.is_canonical() {
                return Err(StoreError::Integrity(format!(
                    "gate {key} unsealed revision {} has noncanonical derivation inputs",
                    revision.revision
                )));
            }
            if revision.revision > 0 {
                let baseline = gate.baseline.as_ref().ok_or_else(|| {
                    StoreError::Integrity(format!(
                        "gate {key} unsealed close omitted its opening baseline"
                    ))
                })?;
                if inputs
                    .attempted_write_leases
                    .iter()
                    .collect::<BTreeSet<_>>()
                    != baseline.leased_write_set.iter().collect::<BTreeSet<_>>()
                {
                    return Err(StoreError::Integrity(format!(
                        "gate {key} unsealed close revision {} changed its attempted write domain",
                        revision.revision
                    )));
                }
                let mut expected_last_complete = expected_protected_semantic_inputs
                    .iter()
                    .map(|input| input.path.clone())
                    .collect::<Vec<_>>();
                expected_last_complete.sort();
                expected_last_complete.dedup();
                if inputs.last_complete_read_set != expected_last_complete {
                    return Err(StoreError::Integrity(format!(
                        "gate {key} unsealed close revision {} changed its last complete read set",
                        revision.revision
                    )));
                }
            }
            let primary_paths = if revision.revision == 0 {
                gate.declared_write_set.as_slice()
            } else {
                &[]
            };
            let derived =
                derive_unsealed_gate_observation_binding(primary_paths, inputs, &revision.signals);
            if revision.observation_binding.as_ref() != Some(&derived) {
                return Err(StoreError::Integrity(format!(
                    "gate {key} unsealed revision {} cannot be reconstructed",
                    revision.revision
                )));
            }
        } else if revision.unsealed_observation_inputs.is_some() {
            return Err(StoreError::Integrity(format!(
                "gate {key} sealed or administrative revision {} retained unsealed derivation inputs",
                revision.revision
            )));
        }
        if let Some(ObservationBinding::Sealed {
            observation: SealedGateObservation::Close { observation_id },
        }) = revision.observation_binding.as_ref()
        {
            let baseline = gate.baseline.as_ref().ok_or_else(|| {
                StoreError::Integrity(format!(
                    "gate {key} sealed close omitted its opening baseline"
                ))
            })?;
            let snapshot = revision.snapshot.as_ref().ok_or_else(|| {
                StoreError::Integrity(format!(
                    "gate {key} sealed close revision {} omitted its snapshot",
                    revision.revision
                ))
            })?;
            let evidence_payload_sha256 = validate_analysis_snapshot(
                key,
                &format!("close revision {}", revision.revision),
                snapshot,
            )?;
            if snapshot.scan_invocation != gate.analysis_options.scan_invocation {
                return Err(StoreError::Integrity(format!(
                    "gate {key} sealed close revision {} changed its opening analysis invocation",
                    revision.revision
                )));
            }
            if revision.protected_semantic_inputs
                != derive_protected_semantic_inputs(snapshot, &baseline.leased_write_set)
            {
                return Err(StoreError::Integrity(format!(
                    "gate {key} sealed close revision {} protected reads cannot be derived from its snapshot",
                    revision.revision
                )));
            }
            let actual_write_set = revision.actual_write_set.as_ref().ok_or_else(|| {
                StoreError::Integrity(format!(
                    "gate {key} sealed close revision {} omitted its actual-write set",
                    revision.revision
                ))
            })?;
            if revision.changed_paths != actual_write_set.paths {
                return Err(StoreError::Integrity(format!(
                    "gate {key} sealed close revision {} changed paths disagree with its actual-write set",
                    revision.revision
                )));
            }
            let reconciled_baseline =
                reconstruct_close_baseline(key, baseline, revision, transitions)?;
            validate_close_snapshot_policy(
                key,
                revision,
                &reconciled_baseline,
                snapshot,
                &expected_protected_semantic_inputs,
                &baseline.leased_write_set,
                &baseline.alias_closures,
            )?;
            let derived = derive_gate_close_observation_id(GateCloseObservationInput {
                gate_id: &gate.gate_id,
                opening_observation_id: &baseline.observation_id,
                opening_analysis_contract: &baseline.analysis_contract,
                prior_revision: revision.revision.saturating_sub(1),
                catalog_revision: revision.catalog_revision.ok_or_else(|| {
                    StoreError::Integrity(format!(
                        "gate {key} sealed close revision {} omitted its catalog revision",
                        revision.revision
                    ))
                })?,
                analysis_input_id: &snapshot.analysis_input_id,
                evidence_payload_sha256: &evidence_payload_sha256,
                signals: &revision.signals,
                leased_write_set: &baseline.leased_write_set,
                protected_semantic_inputs: &revision.protected_semantic_inputs,
                changed_paths: &revision.changed_paths,
                actual_write_set,
                alias_closures: &revision.alias_closures,
                reconciled_transition_sequences: &revision.reconciled_transition_sequences,
            });
            if &derived != observation_id {
                return Err(StoreError::Integrity(format!(
                    "gate {key} close observation revision {} cannot be reconstructed",
                    revision.revision
                )));
            }
            if revision.decision != lumin_evidence::GateDecision::Stale {
                expected_protected_semantic_inputs = revision.protected_semantic_inputs.clone();
            }
        }
    }

    let expected_top_level_protected = if gate.lifecycle == GateLifecycle::Rejected {
        &[][..]
    } else {
        expected_protected_semantic_inputs.as_slice()
    };
    if matches!(
        gate.lifecycle,
        GateLifecycle::Active | GateLifecycle::Rejected | GateLifecycle::Closed
    ) && gate
        .protected_semantic_inputs
        .iter()
        .collect::<BTreeSet<_>>()
        != expected_top_level_protected.iter().collect::<BTreeSet<_>>()
    {
        return Err(StoreError::Integrity(format!(
            "gate {key} protected read set disagrees with its latest sealed observation"
        )));
    }
    validate_gate_lifecycle(key, gate, opening, operations)
}

fn validate_baseline_write_domain(
    key: &str,
    gate: &GateRecord,
    baseline: &GateBaseline,
) -> Result<(), StoreError> {
    let mut normalized_leases = baseline.leased_write_set.clone();
    normalized_leases.sort();
    normalized_leases.dedup();
    if normalized_leases != baseline.leased_write_set {
        return Err(StoreError::Integrity(format!(
            "gate {key} sealed lease domain is not canonical"
        )));
    }
    for lease in &baseline.leased_write_set {
        if lease.kind == WriteLeaseKind::NewFile {
            validate_new_file_lease_prefixes(key, lease)?;
        }
    }

    let mut captured_by_identity = BTreeMap::<
        lumin_model::PhysicalFileIdentity,
        BTreeSet<lumin_evidence::RepoPathProjection>,
    >::new();
    for input in &baseline.snapshot.inputs {
        if let Some(identity) = &input.physical_identity {
            captured_by_identity
                .entry(identity.clone())
                .or_default()
                .insert(input.path.clone());
        }
    }

    let mut seeded_identities = BTreeSet::new();
    for declared in &gate.declared_write_set {
        let direct = baseline
            .leased_write_set
            .iter()
            .filter(|lease| lease.path == *declared)
            .collect::<Vec<_>>();
        if direct.len() != 1 {
            return Err(StoreError::Integrity(format!(
                "gate {key} declared path {} does not have exactly one sealed direct lease",
                declared.display
            )));
        }
        let direct = direct[0];
        if let Some(identity) = baseline
            .snapshot
            .inputs
            .iter()
            .find(|input| input.path == *declared)
            .and_then(|input| input.physical_identity.as_ref())
        {
            if direct.kind != WriteLeaseKind::ExistingFile
                || direct.physical_identity.as_ref() != Some(identity)
            {
                return Err(StoreError::Integrity(format!(
                    "gate {key} declared existing path {} disagrees with its captured physical identity",
                    declared.display
                )));
            }
            seeded_identities.insert(identity.clone());
        }
        let descendant_identities = baseline
            .snapshot
            .inputs
            .iter()
            .filter(|input| {
                input.path != *declared
                    && input.path.components.starts_with(&declared.components)
                    && input.physical_identity.is_some()
            })
            .filter_map(|input| input.physical_identity.clone())
            .collect::<Vec<_>>();
        if !descendant_identities.is_empty() && direct.kind != WriteLeaseKind::Directory {
            return Err(StoreError::Integrity(format!(
                "gate {key} declared directory {} lost its sealed directory lease",
                declared.display
            )));
        }
        if direct.kind == WriteLeaseKind::Directory {
            seeded_identities.extend(descendant_identities);
        }
    }

    for lease in &baseline.leased_write_set {
        if lease.kind != WriteLeaseKind::ExistingFile {
            continue;
        }
        let Some(identity) = &lease.physical_identity else {
            continue;
        };
        let captured = baseline.snapshot.inputs.iter().any(|input| {
            input.path == lease.path && input.physical_identity.as_ref() == Some(identity)
        });
        if !captured {
            return Err(StoreError::Integrity(format!(
                "gate {key} existing lease {} has no matching captured physical identity",
                lease.path.display
            )));
        }
        seeded_identities.insert(identity.clone());
    }

    let mut expected_alias_closures = Vec::new();
    for identity in seeded_identities {
        let members = captured_by_identity.get(&identity).ok_or_else(|| {
            StoreError::Integrity(format!(
                "gate {key} sealed physical lease has no captured alias domain"
            ))
        })?;
        for member in members {
            let matching_leases = baseline
                .leased_write_set
                .iter()
                .filter(|lease| {
                    lease.kind == WriteLeaseKind::ExistingFile
                        && lease.path == *member
                        && lease.physical_identity.as_ref() == Some(&identity)
                })
                .count();
            if matching_leases != 1 {
                return Err(StoreError::Integrity(format!(
                    "gate {key} captured alias {} does not have exactly one physical lease",
                    member.display
                )));
            }
        }
        expected_alias_closures.push(PhysicalAliasClosureRecord {
            physical_identity: identity,
            members: members.iter().cloned().collect(),
        });
    }

    let mut normalized_alias_closures = baseline.alias_closures.clone();
    for closure in &mut normalized_alias_closures {
        closure.members.sort();
        closure.members.dedup();
    }
    normalized_alias_closures.sort();
    normalized_alias_closures.dedup();
    if normalized_alias_closures != baseline.alias_closures
        || baseline.alias_closures != expected_alias_closures
    {
        return Err(StoreError::Integrity(format!(
            "gate {key} sealed physical-alias closure cannot be reconstructed"
        )));
    }
    Ok(())
}

fn validate_new_file_lease_prefixes(
    key: &str,
    lease: &lumin_evidence::WriteLease,
) -> Result<(), StoreError> {
    let nearest = lease.nearest_existing_parent.as_ref().ok_or_else(|| {
        StoreError::Integrity(format!(
            "gate {key} new-file lease {} omitted its nearest existing parent",
            lease.path.display
        ))
    })?;
    if lease.physical_identity.is_some()
        || nearest.components.len() >= lease.path.components.len()
        || !lease.path.components.starts_with(&nearest.components)
    {
        return Err(StoreError::Integrity(format!(
            "gate {key} new-file lease {} has an invalid parent binding",
            lease.path.display
        )));
    }
    let nearest = RepoPath::from_canonical_bytes(&nearest.canonical).map_err(|error| {
        StoreError::Integrity(format!(
            "gate {key} new-file lease {} has a noncanonical parent: {error}",
            lease.path.display
        ))
    })?;
    validate_lease_prefixes(key, lease, nearest)
}

fn validate_existing_lease_prefixes(
    key: &str,
    lease: &lumin_evidence::WriteLease,
) -> Result<(), StoreError> {
    let path = RepoPath::from_canonical_bytes(&lease.path.canonical).map_err(|error| {
        StoreError::Integrity(format!(
            "gate {key} existing lease {} is not canonical: {error}",
            lease.path.display
        ))
    })?;
    let parent = path.parent().ok_or_else(|| {
        StoreError::Integrity(format!(
            "gate {key} existing lease {} has no parent",
            lease.path.display
        ))
    })?;
    validate_lease_prefixes(key, lease, parent)
}

fn validate_lease_prefixes(
    key: &str,
    lease: &lumin_evidence::WriteLease,
    nearest: RepoPath,
) -> Result<(), StoreError> {
    let mut cursor = Some(nearest);
    let mut expected_prefixes = Vec::new();
    while let Some(prefix) = cursor {
        cursor = prefix.parent();
        expected_prefixes.push(lumin_evidence::RepoPathProjection::from(&prefix));
    }
    expected_prefixes.reverse();
    let observed_prefixes = lease
        .prefix_identities
        .iter()
        .map(|prefix| &prefix.path)
        .collect::<Vec<_>>();
    if observed_prefixes != expected_prefixes.iter().collect::<Vec<_>>() {
        return Err(StoreError::Integrity(format!(
            "gate {key} lease {} has an incomplete physical-prefix chain",
            lease.path.display
        )));
    }
    Ok(())
}

fn validate_opening_snapshot_policy(
    key: &str,
    opening: &GateRevision,
    baseline: &GateBaseline,
) -> Result<(), StoreError> {
    let expected = gate_policy::opening_signals(&baseline.snapshot, &baseline.leased_write_set);
    let observed = opening
        .signals
        .iter()
        .filter(|signal| is_opening_snapshot_signal(signal))
        .cloned()
        .collect::<Vec<_>>();
    let impossible = opening.signals.iter().any(|signal| {
        !is_opening_snapshot_signal(signal)
            && !matches!(signal, GateSignal::ProtectedInputChanged { .. })
    });
    if observed != expected || impossible {
        return Err(StoreError::Integrity(format!(
            "gate {key} opening signals disagree with its sealed analysis snapshot"
        )));
    }
    Ok(())
}

fn is_opening_snapshot_signal(signal: &GateSignal) -> bool {
    matches!(
        signal,
        GateSignal::FindingWarnings { .. }
            | GateSignal::PreExistingAdverseFacts { .. }
            | GateSignal::RequiredEvidenceIncomplete { .. }
    )
}

fn reconstruct_close_baseline(
    key: &str,
    baseline: &GateBaseline,
    revision: &GateRevision,
    transitions: &BTreeMap<u64, WorktreeTransition>,
) -> Result<AnalysisSnapshot, StoreError> {
    let mut reconciled = baseline.snapshot.clone();
    for sequence in &revision.reconciled_transition_sequences {
        let transition = transitions.get(sequence).ok_or_else(|| {
            StoreError::Integrity(format!(
                "gate {key} sealed close revision {} references a missing transition",
                revision.revision
            ))
        })?;
        if !apply_worktree_transition_for_domain(
            &mut reconciled,
            transition,
            &baseline.leased_write_set,
            &baseline.protected_semantic_inputs,
        ) {
            return Err(StoreError::Integrity(format!(
                "gate {key} sealed close revision {} cannot replay transition {sequence}",
                revision.revision
            )));
        }
    }
    Ok(reconciled)
}

fn validate_close_snapshot_policy(
    key: &str,
    revision: &GateRevision,
    reconciled_baseline: &AnalysisSnapshot,
    snapshot: &AnalysisSnapshot,
    prior_protected_semantic_inputs: &[lumin_evidence::SemanticInputRecord],
    leased_write_set: &[lumin_evidence::WriteLease],
    baseline_alias_closures: &[lumin_evidence::PhysicalAliasClosureRecord],
) -> Result<(), StoreError> {
    validate_close_alias_closures(key, revision, snapshot, leased_write_set)?;
    let (expected_signals, expected_changed_paths, expected_deltas) = gate_policy::closing_signals(
        reconciled_baseline,
        snapshot,
        prior_protected_semantic_inputs,
        leased_write_set,
    );
    let expected_actual_write_set = gate_policy::closure_expanded_actual_write_set(
        &expected_changed_paths,
        baseline_alias_closures,
        &revision.alias_closures,
    );
    if revision.actual_write_set.as_ref() != Some(&expected_actual_write_set) {
        return Err(StoreError::Integrity(format!(
            "gate {key} close revision {} actual-write set cannot be derived from its sealed snapshots",
            revision.revision
        )));
    }
    if revision.deltas != expected_deltas {
        return Err(StoreError::Integrity(format!(
            "gate {key} close revision {} deltas disagree with its sealed analysis snapshots",
            revision.revision
        )));
    }

    let expected_owned = expected_signals
        .iter()
        .filter(|signal| is_strict_close_snapshot_signal(signal))
        .cloned()
        .collect::<Vec<_>>();
    let observed_owned = revision
        .signals
        .iter()
        .filter(|signal| is_strict_close_snapshot_signal(signal))
        .cloned()
        .collect::<Vec<_>>();
    let contextual_signals_present = expected_signals
        .iter()
        .filter(|signal| {
            matches!(
                signal,
                GateSignal::ProtectedInputChanged { .. } | GateSignal::UnplannedWrite { .. }
            )
        })
        .all(|signal| revision.signals.contains(signal));
    let impossible = revision.signals.iter().any(|signal| {
        !is_strict_close_snapshot_signal(signal)
            && !matches!(
                signal,
                GateSignal::ProtectedInputChanged { .. } | GateSignal::UnplannedWrite { .. }
            )
    });
    if observed_owned != expected_owned || !contextual_signals_present || impossible {
        return Err(StoreError::Integrity(format!(
            "gate {key} close revision {} signals disagree with its sealed analysis snapshots",
            revision.revision
        )));
    }
    Ok(())
}

fn validate_close_alias_closures(
    key: &str,
    revision: &GateRevision,
    snapshot: &AnalysisSnapshot,
    leased_write_set: &[lumin_evidence::WriteLease],
) -> Result<(), StoreError> {
    let mut paths_by_identity = BTreeMap::<
        lumin_model::PhysicalFileIdentity,
        BTreeSet<lumin_evidence::RepoPathProjection>,
    >::new();
    for input in &snapshot.inputs {
        if let Some(identity) = &input.physical_identity {
            paths_by_identity
                .entry(identity.clone())
                .or_default()
                .insert(input.path.clone());
        }
    }

    let mut seeded_identities = BTreeSet::new();
    for lease in leased_write_set {
        seeded_identities.extend(snapshot.inputs.iter().filter_map(|input| {
            (input.path.components.starts_with(&lease.path.components))
                .then(|| input.physical_identity.clone())
                .flatten()
        }));
    }
    let expected = seeded_identities
        .into_iter()
        .map(|physical_identity| PhysicalAliasClosureRecord {
            members: paths_by_identity
                .get(&physical_identity)
                .into_iter()
                .flatten()
                .cloned()
                .collect(),
            physical_identity,
        })
        .collect::<Vec<_>>();

    let mut observed = revision.alias_closures.clone();
    for closure in &mut observed {
        closure.members.sort();
        closure.members.dedup();
    }
    observed.sort();
    observed.dedup();
    if observed != revision.alias_closures || observed != expected {
        return Err(StoreError::Integrity(format!(
            "gate {key} close revision {} physical-alias closure cannot be reconstructed from its sealed snapshot",
            revision.revision
        )));
    }
    Ok(())
}

fn is_strict_close_snapshot_signal(signal: &GateSignal) -> bool {
    matches!(
        signal,
        GateSignal::RequiredEvidenceIncomplete { .. }
            | GateSignal::AdverseFactIntroduced { .. }
            | GateSignal::AdverseFactRegressed { .. }
            | GateSignal::OpacityIntroduced { .. }
            | GateSignal::OpacityRegressed { .. }
            | GateSignal::LifecycleEvidenceRegressed { .. }
            | GateSignal::LifecycleDeltaIncomparable { .. }
            | GateSignal::LifecycleBaselineUnavailable { .. }
    )
}

fn validate_analysis_snapshot(
    gate_key: &str,
    role: &str,
    snapshot: &AnalysisSnapshot,
) -> Result<String, StoreError> {
    if snapshot.evidence.schema_version != RUN_EVIDENCE_SCHEMA_VERSION {
        return Err(StoreError::IncompatibleStateSchema(format!(
            "gate {gate_key} {role} uses unsupported evidence schema {}; expected {RUN_EVIDENCE_SCHEMA_VERSION}",
            snapshot.evidence.schema_version
        )));
    }
    let mut input_paths = BTreeSet::new();
    if snapshot
        .inputs
        .iter()
        .any(|input| !input_paths.insert(input.path.canonical.clone()))
    {
        return Err(StoreError::Integrity(format!(
            "gate {gate_key} {role} contains more than one semantic input for one canonical path"
        )));
    }
    let evidence_payload_sha256 = crate::evidence_payload_sha256(&snapshot.evidence)?;
    let resealed = seal_analysis_snapshot(
        snapshot.inputs.clone(),
        snapshot.evidence.clone(),
        snapshot.scan_invocation.clone(),
        snapshot.entry_selections.clone(),
    );
    if resealed != *snapshot {
        return Err(StoreError::Integrity(format!(
            "gate {gate_key} {role} analysis input identity cannot be reconstructed"
        )));
    }
    Ok(evidence_payload_sha256)
}

fn validate_gate_lifecycle(
    key: &str,
    gate: &GateRecord,
    opening: &lumin_evidence::GateRevision,
    operations: &BTreeMap<&str, OperationRecord>,
) -> Result<(), StoreError> {
    let tail = gate
        .revisions
        .last()
        .ok_or_else(|| StoreError::Integrity(format!("gate {key} omitted its durable tail")))?;
    let tail_kind = operations
        .get(tail.operation_id.as_str())
        .map(|operation| operation.kind);
    let sealed_authorizing_close = tail.decision.authorizes()
        && matches!(
            tail.observation_binding.as_ref(),
            Some(ObservationBinding::Sealed {
                observation: SealedGateObservation::Close { .. }
            })
        );
    let coherent = match gate.lifecycle {
        GateLifecycle::Active => {
            opening.decision.authorizes()
                && (tail.revision == 0 || !tail.decision.authorizes())
                && tail_kind != Some(GateOperationKind::GateAbandon)
        }
        GateLifecycle::Rejected => {
            !opening.decision.authorizes()
                && gate.revisions.len() == 1
                && tail_kind == Some(GateOperationKind::PreWrite)
                && gate.leased_write_set.is_empty()
                && gate.alias_closures.is_empty()
                && gate.protected_semantic_inputs.is_empty()
                && gate.transition_refs.is_empty()
        }
        GateLifecycle::Closed => {
            sealed_authorizing_close
                && tail_kind == Some(GateOperationKind::PostWrite)
                && gate.transition_refs.is_empty()
        }
        GateLifecycle::Abandoned => {
            tail_kind == Some(GateOperationKind::GateAbandon)
                && gate.leased_write_set.is_empty()
                && gate.alias_closures.is_empty()
                && gate.protected_semantic_inputs.is_empty()
                && gate.transition_refs.is_empty()
        }
    };
    if !coherent {
        return Err(StoreError::Integrity(format!(
            "gate {key} lifecycle disagrees with its authorizing revision tail"
        )));
    }
    Ok(())
}

fn validate_operation_gate_refs(
    operations: &BTreeMap<&str, OperationRecord>,
    gates: &BTreeMap<&str, GateRecord>,
) -> Result<(), StoreError> {
    let mut allocated_gate_ids = gates.keys().copied().collect::<BTreeSet<_>>();
    for operation in operations.values() {
        if operation.kind == GateOperationKind::PreWrite
            && operation.status != GateOperationStatus::Committed
            && !allocated_gate_ids.insert(operation.gate_id.as_str())
        {
            return Err(StoreError::Integrity(format!(
                "unfinished pre-write operation {} reuses an allocated gate ID",
                operation.operation_id.as_str()
            )));
        }
        let gate_required = operation.kind != GateOperationKind::PreWrite
            || operation.status == GateOperationStatus::Committed;
        if gate_required && !gates.contains_key(operation.gate_id.as_str()) {
            return Err(StoreError::Integrity(format!(
                "operation {} references a missing gate",
                operation.operation_id.as_str()
            )));
        }
        if operation.status != GateOperationStatus::Committed
            && matches!(
                operation.kind,
                GateOperationKind::PostWrite | GateOperationKind::GateAbandon
            )
        {
            let gate = gates.get(operation.gate_id.as_str()).ok_or_else(|| {
                StoreError::Integrity(format!(
                    "unfinished operation {} references a missing gate",
                    operation.operation_id.as_str()
                ))
            })?;
            let resumable_target = match (operation.kind, operation.status) {
                (GateOperationKind::PostWrite, GateOperationStatus::Interrupted) => {
                    gate.lifecycle == GateLifecycle::Active
                        && gate
                            .revisions
                            .iter()
                            .any(|revision| revision.revision == operation.target_revision)
                }
                _ => {
                    gate.lifecycle == GateLifecycle::Active
                        && gate.current_revision == operation.target_revision
                }
            };
            if !resumable_target {
                return Err(StoreError::Integrity(format!(
                    "unfinished operation {} cannot resume against its target gate revision",
                    operation.operation_id.as_str()
                )));
            }
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
                GateOperationKind::PreWrite if result.lifecycle != GateLifecycle::Active => {
                    Vec::new()
                }
                GateOperationKind::PreWrite | GateOperationKind::PostWrite => gate
                    .baseline
                    .as_ref()
                    .map_or_else(Vec::new, |baseline| baseline.leased_write_set.clone()),
                GateOperationKind::GateAbandon => Vec::new(),
            };
            if revision.decision != result.decision
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
        let baseline = gate.baseline.as_ref();
        let baseline_matches = baseline.is_some_and(|baseline| {
            baseline.observation_id == transition.capsule.baseline_observation_id
        });
        let close_matches = matches!(
            revision.observation_binding.as_ref(),
            Some(ObservationBinding::Sealed {
                observation: SealedGateObservation::Close { observation_id }
            }) if observation_id == &transition.capsule.close_observation_id
                && revision.decision.authorizes()
                && gate.lifecycle == GateLifecycle::Closed
                && gate.current_revision == revision.revision
        );
        let payload_matches = baseline.is_some_and(|baseline| {
            revision.snapshot.as_ref() == Some(&transition.capsule.after_snapshot)
                && revision.changed_paths == transition.capsule.changed_paths
                && baseline.leased_write_set == transition.capsule.leased_write_set
        });
        let before_matches = baseline.is_some_and(|baseline| {
            reconstruct_transition_before(transitions, baseline, revision, transition.sequence)
                .as_ref()
                == Some(&transition.capsule.before_snapshot)
        });
        if !baseline_matches || !close_matches || !payload_matches || !before_matches {
            return Err(StoreError::Integrity(format!(
                "transition {} payload or observation binding disagrees with its gate revision",
                transition.sequence
            )));
        }
    }
    for (key, gate) in gates {
        if gate.lifecycle != GateLifecycle::Closed {
            continue;
        }
        let terminal_revision = gate.current_revision;
        let matching = transitions
            .values()
            .filter(|transition| {
                transition.capsule.gate_id == gate.gate_id
                    && transition.capsule.revision == terminal_revision
            })
            .count();
        if matching != 1 {
            return Err(StoreError::Integrity(format!(
                "closed gate {key} requires exactly one terminal worktree transition"
            )));
        }
    }
    Ok(())
}

fn reconstruct_transition_before(
    transitions: &BTreeMap<u64, WorktreeTransition>,
    baseline: &GateBaseline,
    revision: &GateRevision,
    transition_sequence: u64,
) -> Option<AnalysisSnapshot> {
    if baseline.transition_sequence >= transition_sequence {
        return None;
    }
    let expected_sequences = transitions
        .range((
            std::ops::Bound::Excluded(baseline.transition_sequence),
            std::ops::Bound::Excluded(transition_sequence),
        ))
        .map(|(sequence, _)| *sequence)
        .collect::<Vec<_>>();
    if revision.reconciled_transition_sequences != expected_sequences {
        return None;
    }

    let mut reconstructed = baseline.snapshot.clone();
    for sequence in expected_sequences {
        apply_worktree_transition_for_domain(
            &mut reconstructed,
            transitions.get(&sequence)?,
            &baseline.leased_write_set,
            &baseline.protected_semantic_inputs,
        )
        .then_some(())?;
    }
    Some(reconstructed)
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
    if operation.kind != GateOperationKind::GateAbandon && operation.reason.is_some() {
        return Err(StoreError::Integrity(format!(
            "non-administrative operation {} retained a reason",
            operation.operation_id.as_str()
        )));
    }
    if operation.kind == GateOperationKind::PreWrite {
        let options = operation.analysis_options.as_ref().ok_or_else(|| {
            StoreError::Integrity(format!(
                "pre-write operation {} omitted its analysis options",
                operation.operation_id.as_str()
            ))
        })?;
        validate_pre_write_analysis_options(operation, options)?;
        let expected =
            pre_write_request_digest(&operation.declared_write_set, &options.scan_invocation);
        if operation.request_digest != expected {
            return Err(StoreError::Integrity(format!(
                "pre-write operation {} disagrees with its authenticated request",
                operation.operation_id.as_str()
            )));
        }
    } else if operation.analysis_options.is_some() || !operation.declared_write_set.is_empty() {
        return Err(StoreError::Integrity(format!(
            "non-pre-write operation {} retained pre-write request fields",
            operation.operation_id.as_str()
        )));
    }
    if operation.kind != GateOperationKind::PreWrite
        && operation.pre_write_final_validation.is_some()
    {
        return Err(StoreError::Integrity(format!(
            "non-pre-write operation {} retained a pre-write final validation record",
            operation.operation_id.as_str()
        )));
    }
    if operation.kind == GateOperationKind::PostWrite {
        let expected = post_write_request_digest(&operation.gate_id);
        if operation.request_digest != expected {
            return Err(StoreError::Integrity(format!(
                "post-write operation {} disagrees with its authenticated request",
                operation.operation_id.as_str()
            )));
        }
    }
    match (&operation.status, &operation.result) {
        (GateOperationStatus::Committed, Some(result))
            if result.operation_id == operation.operation_id
                && result.request_digest == operation.request_digest
                && result.gate_id == operation.gate_id =>
        {
            if operation.operation_liveness.is_some() {
                return Err(StoreError::Integrity(format!(
                    "committed operation retained a liveness binding: {}",
                    operation.operation_id.as_str()
                )));
            }
            if !operation.semantic_read_reservations.is_empty()
                || !operation.semantic_read_reservation_bindings.is_empty()
            {
                return Err(StoreError::Integrity(format!(
                    "committed operation retained semantic-read reservations: {}",
                    operation.operation_id.as_str()
                )));
            }
            if operation.kind == GateOperationKind::PreWrite {
                let final_validation = operation.pre_write_final_validation.as_ref().ok_or_else(
                    || {
                        StoreError::Integrity(format!(
                            "committed pre-write operation {} omitted its final validation record",
                            operation.operation_id.as_str()
                        ))
                    },
                )?;
                if final_validation.signals != result.signals {
                    return Err(StoreError::Integrity(format!(
                        "pre-write operation {} result disagrees with its final validation record",
                        operation.operation_id.as_str()
                    )));
                }
            }
            validate_operation_observation(operation, result)
        }
        (GateOperationStatus::Pending, None) => {
            reject_unfinished_pre_write_final_validation(operation)?;
            validate_pending_operation_state(operation)
        }
        (GateOperationStatus::Interrupted, None) => {
            reject_unfinished_pre_write_final_validation(operation)?;
            validate_interrupted_operation_state(operation)
        }
        _ => Err(StoreError::Integrity(format!(
            "operation {} has an incoherent terminal result",
            operation.operation_id.as_str()
        ))),
    }
}

fn reject_unfinished_pre_write_final_validation(
    operation: &OperationRecord,
) -> Result<(), StoreError> {
    if operation.pre_write_final_validation.is_some() {
        return Err(StoreError::Integrity(format!(
            "unfinished operation {} retained a final validation record",
            operation.operation_id.as_str()
        )));
    }
    Ok(())
}

fn validate_pending_operation_state(operation: &OperationRecord) -> Result<(), StoreError> {
    validate_reservation_binding_set(operation)?;
    if operation.kind == GateOperationKind::PreWrite {
        validate_pending_pre_write_write_domain(operation)?;
    }
    let liveness = operation.operation_liveness.as_ref().ok_or_else(|| {
        StoreError::Integrity(format!(
            "pending operation omitted its liveness binding: {}",
            operation.operation_id.as_str()
        ))
    })?;
    let nonce_is_canonical = liveness.lease_nonce.len() == 32
        && liveness
            .lease_nonce
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !nonce_is_canonical
        || liveness.owner_process_id == 0
        || liveness.lock_physical_identity.is_none()
    {
        return Err(StoreError::Integrity(format!(
            "pending operation has an invalid liveness binding: {}",
            operation.operation_id.as_str()
        )));
    }
    Ok(())
}

fn validate_pre_write_analysis_options(
    operation: &OperationRecord,
    options: &lumin_evidence::GateAnalysisOptions,
) -> Result<(), StoreError> {
    if options.jobs == 0 {
        return Err(StoreError::Integrity(format!(
            "pre-write operation {} has an invalid zero worker count",
            operation.operation_id.as_str()
        )));
    }
    if options.resolution_profile != options.scan_invocation.resolution_profile {
        return Err(StoreError::Integrity(format!(
            "pre-write operation {} has inconsistent resolution profiles",
            operation.operation_id.as_str()
        )));
    }
    validate_scan_invocation_patterns(
        &format!("pre-write operation {}", operation.operation_id.as_str()),
        &options.scan_invocation,
    )?;
    Ok(())
}

fn validate_scan_invocation_patterns(
    owner: &str,
    invocation: &lumin_evidence::ScanInvocationTier,
) -> Result<(), StoreError> {
    invocation.validate_patterns().map_err(|error| {
        StoreError::Integrity(format!(
            "{owner} has an invalid persisted scan invocation: {error}"
        ))
    })
}

fn validate_pending_pre_write_write_domain(operation: &OperationRecord) -> Result<(), StoreError> {
    let mut declared = operation.declared_write_set.clone();
    declared.sort();
    declared.dedup();
    let mut leases = operation.leased_write_set.clone();
    leases.sort();
    leases.dedup();
    if declared != operation.declared_write_set
        || leases != operation.leased_write_set
        || declared.len() != leases.len()
    {
        return Err(StoreError::Integrity(format!(
            "pending pre-write operation {} has an incoherent provisional write domain",
            operation.operation_id.as_str()
        )));
    }
    for path in &declared {
        let matching = leases
            .iter()
            .filter(|lease| lease.path == *path)
            .collect::<Vec<_>>();
        if matching.len() != 1 {
            return Err(StoreError::Integrity(format!(
                "pending pre-write operation {} does not bind each declared path exactly once",
                operation.operation_id.as_str()
            )));
        }
        let lease = matching[0];
        match lease.kind {
            WriteLeaseKind::ExistingFile | WriteLeaseKind::Directory
                if lease.physical_identity.is_some() && lease.nearest_existing_parent.is_none() =>
            {
                validate_existing_lease_prefixes(operation.operation_id.as_str(), lease)?;
            }
            WriteLeaseKind::NewFile => {
                validate_new_file_lease_prefixes(operation.operation_id.as_str(), lease)?;
            }
            _ => {
                return Err(StoreError::Integrity(format!(
                    "pending pre-write operation {} has an invalid direct lease for {}",
                    operation.operation_id.as_str(),
                    path.display
                )));
            }
        }
    }
    Ok(())
}

fn validate_interrupted_operation_state(operation: &OperationRecord) -> Result<(), StoreError> {
    if operation.operation_liveness.is_some()
        || !operation.leased_write_set.is_empty()
        || !operation.semantic_read_reservations.is_empty()
        || !operation.semantic_read_reservation_bindings.is_empty()
    {
        return Err(StoreError::Integrity(format!(
            "interrupted operation retained provisional state: {}",
            operation.operation_id.as_str()
        )));
    }
    Ok(())
}

fn validate_operation_observation(
    operation: &OperationRecord,
    result: &lumin_evidence::GateOperationResult,
) -> Result<(), StoreError> {
    if operation.kind != GateOperationKind::GateAbandon && result.reason.is_some() {
        return Err(StoreError::Integrity(format!(
            "non-administrative operation {} retained a reason",
            operation.operation_id.as_str()
        )));
    }
    if operation.kind == GateOperationKind::GateAbandon {
        let reason = operation.reason.as_deref().ok_or_else(|| {
            StoreError::Integrity(format!(
                "administrative abandon operation {} omitted its reason",
                operation.operation_id.as_str()
            ))
        })?;
        if result.decision != GateDecision::Deny
            || result.reason.as_deref() != Some(reason)
            || operation.request_digest
                != gate_abandon_request_digest(
                    &operation.gate_id,
                    operation.target_revision,
                    reason,
                )
        {
            return Err(StoreError::Integrity(format!(
                "administrative abandon operation {} disagrees with its authenticated request",
                operation.operation_id.as_str()
            )));
        }
    }
    if operation.kind != GateOperationKind::GateAbandon
        && result.decision != gate_policy::decision(&result.signals)
    {
        return Err(StoreError::Integrity(format!(
            "operation {} decision disagrees with canonical signal policy",
            operation.operation_id.as_str()
        )));
    }
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
            GateOperationKind::GateAbandon => false,
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
