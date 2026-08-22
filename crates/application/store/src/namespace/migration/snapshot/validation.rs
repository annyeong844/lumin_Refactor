mod cache;
mod external;
mod retention;

use std::collections::{BTreeMap, BTreeSet};

use lumin_evidence::{
    AnalysisSnapshot, GATE_RECORD_SCHEMA_VERSION, GateBaseline, GateBaselineObservationInput,
    GateCloseObservationInput, GateDecision, GateLifecycle, GateOperationKind, GateOperationStatus,
    GateRecord, GateRevision, GateSignal, OperationRecord, WorktreeTransition,
    apply_worktree_transition, derive_gate_baseline_observation_id,
    derive_gate_close_observation_id, derive_unsealed_gate_observation_binding, gate_policy,
    seal_analysis_snapshot,
};
use lumin_model::{ObservationBinding, SealedGateObservation};
use serde::de::DeserializeOwned;

use crate::gate::{records::ACTIVE_GATE_CATALOG_SEQUENCE_KEY, transition_key};
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
    let opening_analysis_options = operations
        .get(opening.operation_id.as_str())
        .and_then(|operation| operation.analysis_options.as_ref())
        .ok_or_else(|| {
            StoreError::Integrity(format!(
                "gate {key} opening operation omitted its analysis options"
            ))
        })?;
    if opening_analysis_options != &gate.analysis_options {
        return Err(StoreError::Integrity(format!(
            "gate {key} analysis options disagree with its opening operation"
        )));
    }
    if gate.analysis_options.resolution_profile
        != gate.analysis_options.scan_invocation.resolution_profile
    {
        return Err(StoreError::Integrity(format!(
            "gate {key} analysis options have inconsistent resolution profiles"
        )));
    }
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
                validate_analysis_snapshot(key, "baseline", &baseline.snapshot)?;
                if gate.lifecycle == GateLifecycle::Active
                    && (gate.leased_write_set.iter().collect::<BTreeSet<_>>()
                        != baseline.leased_write_set.iter().collect::<BTreeSet<_>>()
                        || gate.alias_closures.iter().collect::<BTreeSet<_>>()
                            != baseline.alias_closures.iter().collect::<BTreeSet<_>>())
                {
                    return Err(StoreError::Integrity(format!(
                        "active gate {key} lease/alias domain disagrees with its sealed baseline"
                    )));
                }
                let derived = derive_gate_baseline_observation_id(GateBaselineObservationInput {
                    catalog_revision: baseline.catalog_revision,
                    transition_sequence: baseline.transition_sequence,
                    analysis_contract: &baseline.analysis_contract,
                    analysis_input_id: &baseline.snapshot.analysis_input_id,
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
                revision.changed_paths.as_slice()
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
            validate_analysis_snapshot(
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

    if gate.lifecycle == GateLifecycle::Active
        && gate
            .protected_semantic_inputs
            .iter()
            .collect::<BTreeSet<_>>()
            != expected_protected_semantic_inputs
                .iter()
                .collect::<BTreeSet<_>>()
    {
        return Err(StoreError::Integrity(format!(
            "active gate {key} protected read set disagrees with its latest sealed observation"
        )));
    }
    validate_gate_lifecycle(key, gate, opening, operations)
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
        if !apply_worktree_transition(&mut reconciled, transition) {
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
) -> Result<(), StoreError> {
    let (expected_signals, _, expected_deltas) = gate_policy::closing_signals(
        reconciled_baseline,
        snapshot,
        prior_protected_semantic_inputs,
        leased_write_set,
    );
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
) -> Result<(), StoreError> {
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
    Ok(())
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
        }
        GateLifecycle::Closed => {
            sealed_authorizing_close && tail_kind == Some(GateOperationKind::PostWrite)
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
                GateOperationKind::PreWrite if gate.baseline.is_none() => Vec::new(),
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
        apply_worktree_transition(&mut reconstructed, transitions.get(&sequence)?).then_some(())?;
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
    if operation.kind == GateOperationKind::GateAbandon && result.decision != GateDecision::Deny {
        return Err(StoreError::Integrity(format!(
            "administrative abandon operation {} must deny",
            operation.operation_id.as_str()
        )));
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
