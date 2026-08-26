mod cache;
mod external;
mod retention;

use std::collections::{BTreeMap, BTreeSet};

use lumin_evidence::{
    AnalysisSnapshot, GATE_OPERATION_SCHEMA_VERSION, GATE_RECORD_SCHEMA_VERSION,
    GATE_VALIDATION_RECEIPT_SCHEMA_VERSION, GateBaseline, GateBaselineObservationInput,
    GateCloseObservationInput, GateDecision, GateLifecycle, GateOperationKind, GateOperationStatus,
    GateRecord, GateRevision, GateSignal, GateValidationReceipt, OperationRecord,
    PhysicalAliasClosureRecord, PreWriteAdmissionConflictOwner, PreWriteAdmissionEvidence,
    RUN_EVIDENCE_SCHEMA_VERSION, RetentionItemKind, RetentionOperationKind,
    RetentionOperationRecord, RetentionOperationResult, RetentionOperationStatus,
    RetentionPlanState, SUPPORTED_ACTIVE_GATE_ANALYSIS_CONTRACT_ID, WorktreeTransition,
    WriteLeaseKind, apply_worktree_transition_for_domain, derive_gate_baseline_observation_id,
    derive_gate_close_observation_id, derive_post_write_final_validation_signals,
    derive_pre_write_admission_signals, derive_pre_write_final_validation_signals,
    derive_protected_semantic_inputs, derive_unsealed_gate_observation_binding,
    gate_abandon_request_digest, gate_policy, post_write_request_digest, pre_write_request_digest,
    seal_analysis_snapshot,
};
use lumin_model::{ObservationBinding, RepoPath, SealedGateObservation};
use serde::{Serialize, de::DeserializeOwned};

use crate::gate::{
    records::ACTIVE_GATE_CATALOG_SEQUENCE_KEY, transition_key, validate_reservation_binding_set,
};
use crate::retention::records::{StoredRetentionPlan, StoredTombstone};
use crate::{RunCatalogRecord, StoreError};

use super::super::super::NamespaceGuard;
use super::{LogicalStoreSnapshot, decode_closed_json};

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
    validate_sequence_key_set(snapshot)?;
    let (transitions, transition_sequences) = read_transitions(snapshot)?;
    let validation_receipts = read_validation_receipts(snapshot)?;
    let operations = read_operations(snapshot)?;
    let gates = read_gates(snapshot, &operations, &transitions, &transition_sequences)?;
    validate_pre_write_admissions(snapshot, &operations, &gates)?;
    validate_active_gate_conflicts(&gates)?;
    validate_baseline_transition_boundaries(&transitions, &gates)?;
    validate_gate_id_sequence(snapshot, &gates, &operations)?;
    validate_transition_catalog_sequence(snapshot, &transition_sequences, &gates, &operations)?;
    validate_active_gate_catalog(snapshot, &gates, &operations)?;
    validate_operation_gate_refs(&operations, &gates)?;
    validate_transition_gate_refs(&transitions, &gates)?;
    validate_validation_receipts(snapshot, &gates, &operations, &validation_receipts)?;
    crate::publication::validate_attempt_leases(&snapshot.attempt_leases)?;
    validate_attempt_allocator_sequence(snapshot)?;
    validate_run_catalog(snapshot)?;
    cache::validate_cache(snapshot, &operations)?;
    validate_retention_allocator_sequences(snapshot)?;
    retention::validate_retention(snapshot, &operations)?;
    validate_pointers(snapshot)
}

fn validate_sequence_key_set(snapshot: &LogicalStoreSnapshot) -> Result<(), StoreError> {
    for key in snapshot.sequences.keys() {
        if !matches!(
            key.as_str(),
            "active-gate-catalog"
                | "attempt"
                | "gate"
                | "retention-catalog"
                | "retention-plan"
                | "run-catalog"
                | "run-pin"
                | "transition"
        ) {
            return Err(StoreError::Integrity(format!(
                "sequence table contains an unsupported allocator key: {key}"
            )));
        }
    }
    Ok(())
}

fn validate_attempt_allocator_sequence(snapshot: &LogicalStoreSnapshot) -> Result<(), StoreError> {
    let mut maximum = 0_u64;
    for attempt_id in snapshot.attempt_leases.keys() {
        maximum = maximum.max(canonical_allocated_sequence(
            attempt_id,
            "attempt_",
            "attempt lease",
        )?);
    }
    for (key, bytes) in &snapshot.run_catalog {
        let record = parse_record::<RunCatalogRecord>("run-catalog", key, bytes)?;
        maximum = maximum.max(canonical_allocated_sequence(
            record.attempt_id.as_str(),
            "attempt_",
            "run attempt",
        )?);
    }
    if let Some(bytes) = snapshot.pointers.get("latest-attempt") {
        let attempt_id = std::str::from_utf8(bytes).map_err(|error| {
            StoreError::Integrity(format!("latest-attempt pointer is not UTF-8: {error}"))
        })?;
        maximum = maximum.max(canonical_allocated_sequence(
            attempt_id,
            "attempt_",
            "latest attempt",
        )?);
    }
    maximum = maximum.max(retained_allocator_floor(
        snapshot,
        RetentionItemKind::Attempt,
        "attempt_",
        "retained attempt",
        true,
    )?);
    validate_allocator_sequence(snapshot, "attempt", maximum)
}

fn validate_retention_allocator_sequences(
    snapshot: &LogicalStoreSnapshot,
) -> Result<(), StoreError> {
    let maximum_plan = snapshot
        .retention_plans
        .keys()
        .map(|id| canonical_allocated_sequence(id, "retention_plan_", "retention plan"))
        .try_fold(0_u64, |maximum, sequence| {
            sequence.map(|sequence| maximum.max(sequence))
        })?;
    validate_allocator_sequence(snapshot, "retention-plan", maximum_plan)?;

    let mut maximum_pin = snapshot
        .run_pins
        .keys()
        .map(|id| canonical_allocated_sequence(id, "pin_", "run pin"))
        .try_fold(0_u64, |maximum, sequence| {
            sequence.map(|sequence| maximum.max(sequence))
        })?;
    maximum_pin = maximum_pin.max(retained_allocator_floor(
        snapshot,
        RetentionItemKind::PinOrReference,
        "pin_",
        "retained run pin",
        false,
    )?);
    validate_allocator_sequence(snapshot, "run-pin", maximum_pin)?;

    let maximum_catalog_revision = snapshot
        .retention_plans
        .iter()
        .map(|(key, bytes)| {
            parse_record::<StoredRetentionPlan>("retention-plans", key, bytes)
                .map(|plan| plan.record.catalog_revision)
        })
        .try_fold(0_u64, |maximum, revision| {
            revision.map(|revision| maximum.max(revision))
        })?;
    let retained_catalog_mutations =
        snapshot
            .retention_operations
            .iter()
            .try_fold(0_u64, |count, (key, bytes)| {
                let operation =
                    parse_record::<RetentionOperationRecord>("retention-operations", key, bytes)?;
                let advances_catalog = matches!(
                    operation.kind,
                    RetentionOperationKind::RunPin
                        | RetentionOperationKind::RunUnpin
                        | RetentionOperationKind::RunPrunePlan
                        | RetentionOperationKind::GatePrunePlan
                ) || matches!(
                    (&operation.kind, operation.status, &operation.result),
                    (
                        RetentionOperationKind::RunPruneConfirm
                            | RetentionOperationKind::GatePruneConfirm,
                        RetentionOperationStatus::Committed,
                        RetentionOperationResult::Retention {
                            result: lumin_evidence::RetentionMutationResult::Pruned { .. }
                        }
                    )
                );
                if advances_catalog {
                    count.checked_add(1).ok_or_else(|| {
                        StoreError::Integrity(
                            "retention-catalog retained mutation count overflow".to_owned(),
                        )
                    })
                } else {
                    Ok(count)
                }
            })?;
    validate_allocator_sequence(
        snapshot,
        "retention-catalog",
        maximum_catalog_revision.max(retained_catalog_mutations),
    )
}

fn for_each_retained_item(
    snapshot: &LogicalStoreSnapshot,
    mut visit: impl FnMut(RetentionItemKind, &str, u64) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    for (key, bytes) in &snapshot.retention_plans {
        let plan = parse_record::<StoredRetentionPlan>("retention-plans", key, bytes)?;
        for item in &plan.record.items {
            visit(item.kind, &item.record_id, item.owning_sequence)?;
        }
    }
    for (key, bytes) in &snapshot.retention_tombstones {
        let tombstone = parse_record::<StoredTombstone>("retention-tombstones", key, bytes)?;
        visit(
            tombstone.envelope.record_kind,
            &tombstone.envelope.record_id,
            tombstone.owning_sequence,
        )?;
    }
    Ok(())
}

fn retained_allocator_floor(
    snapshot: &LogicalStoreSnapshot,
    expected_kind: RetentionItemKind,
    prefix: &str,
    label: &str,
    require_owning_sequence: bool,
) -> Result<u64, StoreError> {
    let mut maximum = 0_u64;
    for_each_retained_item(snapshot, |kind, record_id, owning_sequence| {
        if kind != expected_kind {
            return Ok(());
        }
        let sequence = canonical_allocated_sequence(record_id, prefix, label)?;
        if require_owning_sequence && sequence != owning_sequence {
            return Err(StoreError::Integrity(format!(
                "{label} ID disagrees with its retention owning sequence"
            )));
        }
        maximum = maximum.max(sequence);
        Ok(())
    })?;
    Ok(maximum)
}

fn validate_allocator_sequence(
    snapshot: &LogicalStoreSnapshot,
    key: &str,
    minimum: u64,
) -> Result<(), StoreError> {
    let observed = snapshot.sequences.get(key).copied().unwrap_or(0);
    if observed == u64::MAX {
        return Err(StoreError::Integrity(format!(
            "{key} sequence is exhausted and cannot allocate another record"
        )));
    }
    if observed < minimum {
        return Err(StoreError::Integrity(format!(
            "{key} sequence regressed below retained allocation: observed {observed}, minimum {minimum}"
        )));
    }
    Ok(())
}

fn canonical_allocated_sequence(value: &str, prefix: &str, label: &str) -> Result<u64, StoreError> {
    let suffix = value.strip_prefix(prefix).ok_or_else(|| {
        StoreError::Integrity(format!("{label} ID is outside its canonical grammar"))
    })?;
    if suffix.len() != 16
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(StoreError::Integrity(format!(
            "{label} ID is outside its canonical grammar"
        )));
    }
    let sequence = u64::from_str_radix(suffix, 16).map_err(|error| {
        StoreError::Integrity(format!("{label} ID sequence is malformed: {error}"))
    })?;
    if sequence == 0 {
        return Err(StoreError::Integrity(format!(
            "{label} ID sequence must be nonzero"
        )));
    }
    Ok(sequence)
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
        })?
        .max(retained_allocator_floor(
            snapshot,
            RetentionItemKind::Gate,
            "gate_",
            "retained gate",
            true,
        )?);
    if observed == u64::MAX {
        return Err(StoreError::Integrity(
            "gate sequence is exhausted and cannot allocate another gate".to_owned(),
        ));
    }
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

fn read_validation_receipts(
    snapshot: &LogicalStoreSnapshot,
) -> Result<BTreeMap<&str, GateValidationReceipt>, StoreError> {
    let mut receipts = BTreeMap::new();
    for (key, bytes) in &snapshot.validation_receipts {
        let receipt =
            parse_record::<GateValidationReceipt>("gate-validation-receipts", key, bytes)?;
        if receipt.schema_version != GATE_VALIDATION_RECEIPT_SCHEMA_VERSION {
            return Err(StoreError::IncompatibleStateSchema(format!(
                "gate validation receipt {key} uses unsupported schema {}; expected {GATE_VALIDATION_RECEIPT_SCHEMA_VERSION}",
                receipt.schema_version
            )));
        }
        if receipt.operation_id.as_str() != key {
            return Err(StoreError::Integrity(format!(
                "gate validation receipt key {key} disagrees with its operation"
            )));
        }
        receipts.insert(key.as_str(), receipt);
    }
    Ok(receipts)
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

fn validate_validation_receipts(
    snapshot: &LogicalStoreSnapshot,
    gates: &BTreeMap<&str, GateRecord>,
    operations: &BTreeMap<&str, OperationRecord>,
    receipts: &BTreeMap<&str, GateValidationReceipt>,
) -> Result<(), StoreError> {
    for (key, operation) in operations {
        let gate = gates.get(operation.gate_id.as_str());
        let expected = crate::gate::validation_receipt_for_operation(operation, gate)?;
        match (expected.as_ref(), receipts.get(key)) {
            (Some(expected), Some(observed)) if expected == observed => {}
            (None, None) => {}
            _ => {
                return Err(StoreError::Integrity(format!(
                    "operation {key} disagrees with its store-owned validation receipt"
                )));
            }
        }
    }
    if let Some(orphan) = receipts
        .keys()
        .find(|key| !snapshot.operations.contains_key(**key))
    {
        return Err(StoreError::Integrity(format!(
            "gate validation receipt {orphan} lost its owning operation"
        )));
    }
    Ok(())
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

fn validate_pre_write_admissions(
    snapshot: &LogicalStoreSnapshot,
    operations: &BTreeMap<&str, OperationRecord>,
    gates: &BTreeMap<&str, GateRecord>,
) -> Result<(), StoreError> {
    let observed_catalog_revision = snapshot
        .sequences
        .get(ACTIVE_GATE_CATALOG_SEQUENCE_KEY)
        .copied()
        .unwrap_or(0);
    for operation in operations
        .values()
        .filter(|operation| operation.kind == GateOperationKind::PreWrite)
    {
        let Some(evidence) = operation.pre_write_admission_evidence.as_ref() else {
            continue;
        };
        if evidence.catalog_revision > observed_catalog_revision
            || !canonical_set(&evidence.attempted_leased_write_set)
            || !canonical_set(&evidence.conflict_owners)
            || evidence.conflict_owners.is_empty()
        {
            return Err(StoreError::Integrity(format!(
                "pre-write operation {} has noncanonical admission evidence",
                operation.operation_id.as_str()
            )));
        }
        let derived_signals = derive_pre_write_admission_signals(evidence);
        let result = operation.result.as_ref().ok_or_else(|| {
            StoreError::Integrity(format!(
                "pre-write operation {} retained admission evidence without a committed result",
                operation.operation_id.as_str()
            ))
        })?;
        if operation.status != GateOperationStatus::Committed
            || operation.pre_write_final_validation.is_some()
            || result.signals != derived_signals
        {
            return Err(StoreError::Integrity(format!(
                "pre-write operation {} admission evidence disagrees with its result",
                operation.operation_id.as_str()
            )));
        }
        let gate = gates.get(operation.gate_id.as_str()).ok_or_else(|| {
            StoreError::Integrity(format!(
                "pre-write operation {} admission evidence lost its rejected gate",
                operation.operation_id.as_str()
            ))
        })?;
        let opening = gate.revisions.first().ok_or_else(|| {
            StoreError::Integrity(format!(
                "pre-write operation {} admission evidence lost its opening revision",
                operation.operation_id.as_str()
            ))
        })?;
        if opening.catalog_revision != Some(evidence.catalog_revision)
            || opening.signals != derived_signals
        {
            return Err(StoreError::Integrity(format!(
                "pre-write operation {} admission evidence disagrees with its rejected opening",
                operation.operation_id.as_str()
            )));
        }
        let attempted_inputs = opening
            .unsealed_observation_inputs
            .as_ref()
            .ok_or_else(|| {
                StoreError::Integrity(format!(
                    "pre-write operation {} admission rejection omitted its attempted domain",
                    operation.operation_id.as_str()
                ))
            })?;
        let attempted_paths = evidence
            .attempted_leased_write_set
            .iter()
            .map(|lease| lease.path.clone())
            .collect::<Vec<_>>();
        let mut unique_attempted_paths = attempted_paths.clone();
        unique_attempted_paths.sort();
        unique_attempted_paths.dedup();
        if evidence.attempted_leased_write_set != attempted_inputs.attempted_write_leases
            || unique_attempted_paths.len() != attempted_paths.len()
            || attempted_paths
                .iter()
                .any(|path| !operation.declared_write_set.contains(path))
        {
            return Err(StoreError::Integrity(format!(
                "pre-write operation {} admission evidence changed its attempted write domain",
                operation.operation_id.as_str()
            )));
        }
        for owner in &evidence.conflict_owners {
            validate_pre_write_admission_owner(
                operation,
                evidence.catalog_revision,
                owner,
                operations,
                gates,
            )?;
        }
    }
    Ok(())
}

fn validate_pre_write_admission_owner(
    operation: &OperationRecord,
    admission_catalog_revision: u64,
    owner: &PreWriteAdmissionConflictOwner,
    operations: &BTreeMap<&str, OperationRecord>,
    gates: &BTreeMap<&str, GateRecord>,
) -> Result<(), StoreError> {
    if owner.gate_id() == &operation.gate_id {
        return Err(StoreError::Integrity(format!(
            "pre-write operation {} cites itself as an admission conflict owner",
            operation.operation_id.as_str()
        )));
    }
    match owner {
        PreWriteAdmissionConflictOwner::ActiveGate {
            gate_id,
            revision,
            leased_write_set,
            protected_semantic_inputs,
        } => {
            if !canonical_set(leased_write_set) || !canonical_set(protected_semantic_inputs) {
                return Err(StoreError::Integrity(format!(
                    "pre-write operation {} has a noncanonical active-gate conflict witness",
                    operation.operation_id.as_str()
                )));
            }
            let gate = gates.get(gate_id.as_str()).ok_or_else(|| {
                StoreError::Integrity(format!(
                    "pre-write operation {} cites a missing active-gate conflict owner",
                    operation.operation_id.as_str()
                ))
            })?;
            if !gate_was_active_at_revision(gate, *revision, operations)? {
                return Err(StoreError::Integrity(format!(
                    "pre-write operation {} cites a gate that was not active at the witnessed revision",
                    operation.operation_id.as_str()
                )));
            }
            let owner_revision = gate
                .revisions
                .iter()
                .find(|observed| observed.revision == *revision)
                .ok_or_else(|| {
                    StoreError::Integrity(format!(
                        "pre-write operation {} active-gate conflict witness lost its revision",
                        operation.operation_id.as_str()
                    ))
                })?;
            if owner_revision
                .catalog_revision
                .is_none_or(|observed| observed > admission_catalog_revision)
            {
                return Err(StoreError::Integrity(format!(
                    "pre-write operation {} cites an active-gate state newer than its admission catalog",
                    operation.operation_id.as_str()
                )));
            }
            let baseline = gate.baseline.as_ref().ok_or_else(|| {
                StoreError::Integrity(format!(
                    "pre-write operation {} active-gate conflict owner omitted its baseline",
                    operation.operation_id.as_str()
                ))
            })?;
            let expected_protected = active_gate_protected_inputs_at_catalog(
                gate,
                admission_catalog_revision,
                operations,
            )?
            .ok_or_else(|| {
                StoreError::Integrity(format!(
                    "pre-write operation {} cites a gate that was not active in its admission catalog",
                    operation.operation_id.as_str()
                ))
            })?;
            if leased_write_set != &baseline.leased_write_set
                || protected_semantic_inputs != &expected_protected
            {
                return Err(StoreError::Integrity(format!(
                    "pre-write operation {} active-gate conflict witness disagrees with durable history",
                    operation.operation_id.as_str()
                )));
            }
        }
        PreWriteAdmissionConflictOwner::PendingOperation {
            operation_id,
            gate_id,
            leased_write_set,
            semantic_read_reservation_bindings,
        } => {
            if operation_id == &operation.operation_id
                || !canonical_set(leased_write_set)
                || !canonical_set(semantic_read_reservation_bindings)
            {
                return Err(StoreError::Integrity(format!(
                    "pre-write operation {} has a noncanonical pending-operation conflict witness",
                    operation.operation_id.as_str()
                )));
            }
            let owner_operation = operations.get(operation_id.as_str()).ok_or_else(|| {
                StoreError::Integrity(format!(
                    "pre-write operation {} cites a missing pending-operation conflict owner",
                    operation.operation_id.as_str()
                ))
            })?;
            if &owner_operation.gate_id != gate_id {
                return Err(StoreError::Integrity(format!(
                    "pre-write operation {} pending-operation conflict witness changed gate ownership",
                    operation.operation_id.as_str()
                )));
            }
            match owner_operation.kind {
                GateOperationKind::PreWrite => validate_admission_witness_write_domain(
                    operation,
                    owner_operation,
                    leased_write_set,
                )?,
                GateOperationKind::PostWrite if leased_write_set.is_empty() => {}
                GateOperationKind::PostWrite => {
                    return Err(StoreError::Integrity(format!(
                        "pre-write operation {} cites write leases for a post-write conflict owner",
                        operation.operation_id.as_str()
                    )));
                }
                GateOperationKind::GateAbandon => {
                    return Err(StoreError::Integrity(format!(
                        "pre-write operation {} cites an administrative operation as an admission conflict owner",
                        operation.operation_id.as_str()
                    )));
                }
            }
        }
    }
    Ok(())
}

fn validate_admission_witness_write_domain(
    operation: &OperationRecord,
    owner_operation: &OperationRecord,
    leased_write_set: &[lumin_evidence::WriteLease],
) -> Result<(), StoreError> {
    let mut witnessed_paths = leased_write_set
        .iter()
        .map(|lease| lease.path.clone())
        .collect::<Vec<_>>();
    witnessed_paths.sort();
    witnessed_paths.dedup();
    if witnessed_paths.len() != leased_write_set.len()
        || witnessed_paths
            .iter()
            .any(|path| !owner_operation.declared_write_set.contains(path))
    {
        return Err(StoreError::Integrity(format!(
            "pre-write operation {} cites an interrupted owner's incoherent write domain",
            operation.operation_id.as_str()
        )));
    }
    Ok(())
}

fn gate_was_active_at_revision(
    gate: &GateRecord,
    revision: u64,
    operations: &BTreeMap<&str, OperationRecord>,
) -> Result<bool, StoreError> {
    if revision > gate.current_revision
        || !gate
            .revisions
            .iter()
            .any(|observed| observed.revision == revision)
        || !gate
            .revisions
            .first()
            .is_some_and(|opening| opening.decision.authorizes())
    {
        return Ok(false);
    }
    for observed in gate
        .revisions
        .iter()
        .filter(|observed| observed.revision <= revision)
    {
        let kind = operations
            .get(observed.operation_id.as_str())
            .map(|operation| operation.kind)
            .ok_or_else(|| {
                StoreError::Integrity(format!(
                    "gate {} admission history references a missing operation",
                    gate.gate_id.as_str()
                ))
            })?;
        if kind == GateOperationKind::GateAbandon
            || (kind == GateOperationKind::PostWrite && observed.decision.authorizes())
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn active_gate_protected_inputs_at_catalog(
    gate: &GateRecord,
    catalog_revision: u64,
    operations: &BTreeMap<&str, OperationRecord>,
) -> Result<Option<Vec<lumin_evidence::SemanticInputRecord>>, StoreError> {
    let Some(effective_revision) = gate
        .revisions
        .iter()
        .filter(|revision| {
            revision
                .catalog_revision
                .is_some_and(|observed| observed < catalog_revision)
        })
        .max_by_key(|revision| revision.revision)
    else {
        return Ok(None);
    };
    if !gate_was_active_at_revision(gate, effective_revision.revision, operations)? {
        return Ok(None);
    }
    protected_semantic_inputs_at_revision(gate, effective_revision.revision, operations).map(Some)
}

fn protected_semantic_inputs_at_revision(
    gate: &GateRecord,
    revision: u64,
    operations: &BTreeMap<&str, OperationRecord>,
) -> Result<Vec<lumin_evidence::SemanticInputRecord>, StoreError> {
    let mut protected = gate.baseline.as_ref().map_or_else(Vec::new, |baseline| {
        baseline.protected_semantic_inputs.clone()
    });
    for observed in gate
        .revisions
        .iter()
        .filter(|observed| observed.revision > 0 && observed.revision <= revision)
    {
        let kind = operations
            .get(observed.operation_id.as_str())
            .map(|operation| operation.kind)
            .ok_or_else(|| {
                StoreError::Integrity(format!(
                    "gate {} protected-read history references a missing operation",
                    gate.gate_id.as_str()
                ))
            })?;
        if kind == GateOperationKind::PostWrite
            && observed.decision != GateDecision::Stale
            && matches!(
                observed.observation_binding.as_ref(),
                Some(ObservationBinding::Sealed {
                    observation: SealedGateObservation::Close { .. }
                })
            )
        {
            protected = observed.protected_semantic_inputs.clone();
        }
    }
    Ok(protected)
}

fn validate_active_gate_conflicts(gates: &BTreeMap<&str, GateRecord>) -> Result<(), StoreError> {
    let active = gates
        .values()
        .filter(|gate| gate.lifecycle == GateLifecycle::Active)
        .collect::<Vec<_>>();
    for (index, left) in active.iter().enumerate() {
        for right in active.iter().skip(index + 1) {
            let write_conflict = left.leased_write_set.iter().any(|left_lease| {
                right
                    .leased_write_set
                    .iter()
                    .any(|right_lease| left_lease.conflicts_with(right_lease))
            });
            let left_writes_right_reads = left.leased_write_set.iter().any(|lease| {
                right.protected_semantic_inputs.iter().any(|input| {
                    lease.conflicts_with_semantic_read(
                        &input.path,
                        input.physical_identity.as_ref(),
                        input.absence_parent.as_ref(),
                    )
                })
            });
            let right_writes_left_reads = right.leased_write_set.iter().any(|lease| {
                left.protected_semantic_inputs.iter().any(|input| {
                    lease.conflicts_with_semantic_read(
                        &input.path,
                        input.physical_identity.as_ref(),
                        input.absence_parent.as_ref(),
                    )
                })
            });
            if write_conflict || left_writes_right_reads || right_writes_left_reads {
                return Err(StoreError::Integrity(format!(
                    "active gates {} and {} retain conflicting write/read domains",
                    left.gate_id.as_str(),
                    right.gate_id.as_str()
                )));
            }
        }
    }
    Ok(())
}

fn validate_transition_catalog_sequence(
    snapshot: &LogicalStoreSnapshot,
    transition_sequences: &BTreeSet<u64>,
    gates: &BTreeMap<&str, GateRecord>,
    operations: &BTreeMap<&str, OperationRecord>,
) -> Result<(), StoreError> {
    let observed = snapshot.sequences.get("transition").copied().unwrap_or(0);
    if observed == u64::MAX {
        return Err(StoreError::Integrity(
            "transition sequence is exhausted and cannot publish another transition".to_owned(),
        ));
    }
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
    minimum = minimum.max(retained_allocator_floor(
        snapshot,
        RetentionItemKind::Transition,
        "transition_",
        "retained transition",
        true,
    )?);
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
    let retained_mutation_floor = retained_active_gate_mutation_floor(snapshot, gates)?;
    crate::gate::validate_active_gate_catalog_history(
        observed,
        retained_mutation_floor,
        gates.iter().map(|(key, gate)| (*key, gate)),
        |operation_id| {
            operations
                .get(operation_id.as_str())
                .map(|operation| operation.kind)
        },
    )
}

fn retained_active_gate_mutation_floor(
    snapshot: &LogicalStoreSnapshot,
    gates: &BTreeMap<&str, GateRecord>,
) -> Result<u64, StoreError> {
    let mut retained_gates = BTreeSet::<String>::new();
    let mut retained_revisions = BTreeMap::<String, BTreeSet<u64>>::new();
    for_each_retained_item(snapshot, |kind, record_id, owning_sequence| {
        match kind {
            RetentionItemKind::Gate => {
                let sequence = canonical_allocated_sequence(
                    record_id,
                    "gate_",
                    "retained active-catalog gate",
                )?;
                if sequence != owning_sequence {
                    return Err(StoreError::Integrity(
                        "retained active-catalog gate ID disagrees with its owning sequence"
                            .to_owned(),
                    ));
                }
                if !gates.contains_key(record_id) {
                    retained_gates.insert(record_id.to_owned());
                }
            }
            RetentionItemKind::GateRevision => {
                let Some(owner_and_revision) = record_id.strip_prefix("gate:") else {
                    return Err(StoreError::Integrity(
                        "retained gate revision has a noncanonical owner".to_owned(),
                    ));
                };
                let Some((gate_id, revision_text)) = owner_and_revision.rsplit_once("/revision:")
                else {
                    return Err(StoreError::Integrity(
                        "retained gate revision has a noncanonical ID".to_owned(),
                    ));
                };
                let gate_sequence =
                    canonical_allocated_sequence(gate_id, "gate_", "retained gate revision owner")?;
                if gate_sequence != owning_sequence {
                    return Err(StoreError::Integrity(
                        "retained gate revision owner disagrees with its owning sequence"
                            .to_owned(),
                    ));
                }
                let revision = revision_text.parse::<u64>().map_err(|error| {
                    StoreError::Integrity(format!(
                        "retained gate revision sequence is malformed: {error}"
                    ))
                })?;
                if revision.to_string() != revision_text {
                    return Err(StoreError::Integrity(
                        "retained gate revision sequence is noncanonical".to_owned(),
                    ));
                }
                if !gates.contains_key(gate_id) {
                    retained_revisions
                        .entry(gate_id.to_owned())
                        .or_default()
                        .insert(revision);
                }
            }
            _ => {}
        }
        Ok(())
    })?;

    let mut minimum = 0_u64;
    for gate_id in retained_gates {
        let revisions = retained_revisions.get(&gate_id).ok_or_else(|| {
            StoreError::Integrity(format!(
                "retained gate {gate_id} omitted its revision history"
            ))
        })?;
        if !revisions.contains(&0) {
            return Err(StoreError::Integrity(format!(
                "retained gate {gate_id} omitted its opening revision"
            )));
        }
        match (revisions.len(), revisions.last().copied()) {
            (1, Some(0)) => {}
            (2, Some(1)) => {
                minimum = minimum.checked_add(2).ok_or_else(|| {
                    StoreError::Integrity(
                        "retained active-gate catalog mutation history overflowed".to_owned(),
                    )
                })?;
            }
            _ => {
                return Err(StoreError::IncompatibleStateSchema(format!(
                    "retained gate {gate_id} has intermediate revisions whose active-gate catalog mutations cannot be reconstructed"
                )));
            }
        }
    }
    Ok(minimum)
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
    if gate.lifecycle == GateLifecycle::Active && gate.current_revision == u64::MAX {
        return Err(StoreError::Integrity(format!(
            "active gate {key} exhausted its revision sequence"
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
    match opening_operation.pre_write_final_validation.as_ref() {
        Some(final_validation)
            if opening.catalog_revision == Some(final_validation.catalog_revision)
                && final_validation.signals == opening.signals => {}
        Some(_) => {
            return Err(StoreError::Integrity(format!(
                "gate {key} opening revision disagrees with its operation-owned final validation"
            )));
        }
        None if gate.baseline.is_none()
            && opening_operation
                .pre_write_admission_evidence
                .as_ref()
                .is_some_and(|evidence| is_admission_conflict_rejection(opening, evidence)) => {}
        None => {
            return Err(StoreError::Integrity(format!(
                "gate {key} opening operation omitted its final validation record"
            )));
        }
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
                let final_validation = opening_operation
                    .pre_write_final_validation
                    .as_ref()
                    .ok_or_else(|| {
                        StoreError::Integrity(format!(
                            "gate {key} sealed opening omitted its final validation record"
                        ))
                    })?;
                let final_evidence = final_validation.evidence.as_ref().ok_or_else(|| {
                    StoreError::Integrity(format!(
                        "gate {key} sealed opening omitted independently reconstructable final-freshness evidence"
                    ))
                })?;
                validate_final_freshness_evidence(key, &baseline.snapshot, final_evidence)?;
                let final_freshness_signals = derive_pre_write_final_validation_signals(
                    &baseline.snapshot.inputs,
                    &baseline.leased_write_set,
                    &baseline.alias_closures,
                    final_evidence,
                );
                if matches!(
                    gate.lifecycle,
                    GateLifecycle::Active | GateLifecycle::Closed
                ) && gate.leased_write_set != baseline.leased_write_set
                {
                    return Err(StoreError::Integrity(format!(
                        "gate {key} retained lease domain disagrees with its sealed baseline"
                    )));
                }
                if gate.lifecycle == GateLifecycle::Active
                    && gate.alias_closures != baseline.alias_closures
                {
                    return Err(StoreError::Integrity(format!(
                        "gate {key} retained alias domain disagrees with its sealed baseline"
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
                validate_opening_snapshot_policy(key, opening, baseline, &final_freshness_signals)?;
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
            let closing_operation =
                operations
                    .get(revision.operation_id.as_str())
                    .ok_or_else(|| {
                        StoreError::Integrity(format!(
                            "gate {key} sealed close revision {} lost its operation",
                            revision.revision
                        ))
                    })?;
            let final_validation = closing_operation
                .post_write_final_validation
                .as_ref()
                .ok_or_else(|| {
                    StoreError::Integrity(format!(
                        "gate {key} sealed close revision {} omitted its final validation record",
                        revision.revision
                    ))
                })?;
            let final_evidence = final_validation.evidence.as_ref().ok_or_else(|| {
                StoreError::Integrity(format!(
                    "gate {key} sealed close revision {} omitted independently reconstructable final-freshness evidence",
                    revision.revision
                ))
            })?;
            let final_freshness_signals =
                derive_post_write_final_validation_signals(&snapshot.inputs, final_evidence);
            validate_close_snapshot_policy(
                key,
                revision,
                &reconciled_baseline,
                snapshot,
                &expected_protected_semantic_inputs,
                baseline,
                &final_freshness_signals,
            )?;
            validate_post_write_final_validation_evidence(
                key,
                baseline,
                revision,
                snapshot,
                final_evidence,
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
            if final_validation.catalog_revision != revision.catalog_revision.unwrap_or_default()
                || final_validation.signals != revision.signals
            {
                return Err(StoreError::Integrity(format!(
                    "gate {key} sealed close revision {} disagrees with its operation-owned final validation",
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
    ) && gate.protected_semantic_inputs != expected_top_level_protected
    {
        return Err(StoreError::Integrity(format!(
            "gate {key} protected read set disagrees with its latest sealed observation"
        )));
    }
    if gate.lifecycle == GateLifecycle::Closed
        && gate.alias_closures
            != gate
                .revisions
                .last()
                .map_or(&[][..], |revision| revision.alias_closures.as_slice())
    {
        return Err(StoreError::Integrity(format!(
            "gate {key} closed alias domain disagrees with its terminal sealed observation"
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

fn is_admission_conflict_rejection(
    opening: &GateRevision,
    evidence: &PreWriteAdmissionEvidence,
) -> bool {
    !evidence.conflict_owners.is_empty()
        && opening.catalog_revision == Some(evidence.catalog_revision)
        && opening.signals == derive_pre_write_admission_signals(evidence)
        && matches!(
            opening.observation_binding.as_ref(),
            Some(ObservationBinding::Unsealed { .. })
        )
}

fn validate_final_freshness_evidence(
    key: &str,
    snapshot: &AnalysisSnapshot,
    evidence: &lumin_evidence::PreWriteFinalValidationEvidence,
) -> Result<(), StoreError> {
    if !canonical_set(&evidence.expected_semantic_read_bindings)
        || !canonical_set(&evidence.observed_semantic_read_bindings)
        || !canonical_set(&evidence.observed_semantic_inputs)
        || !canonical_set(&evidence.observed_leased_write_set)
        || !canonical_alias_set(&evidence.observed_alias_closures)
        || !canonical_set(&evidence.write_domain_drift_paths)
        || !canonical_set(&evidence.semantic_input_validation_drift_paths)
    {
        return Err(StoreError::Integrity(format!(
            "gate {key} final-freshness evidence is not a canonical deterministic set"
        )));
    }
    for binding in &evidence.expected_semantic_read_bindings {
        let matches_snapshot = snapshot.inputs.iter().any(|input| {
            input.path == binding.path
                && input.physical_identity == binding.physical_identity
                && input.absence_parent == binding.absence_parent
        });
        if !matches_snapshot {
            return Err(StoreError::Integrity(format!(
                "gate {key} final-freshness reservation is not owned by its sealed baseline: {}",
                binding.path.display
            )));
        }
    }
    let mut observed_paths = BTreeSet::new();
    if evidence
        .observed_semantic_inputs
        .iter()
        .any(|input| !observed_paths.insert(input.path.canonical.clone()))
    {
        return Err(StoreError::Integrity(format!(
            "gate {key} final-freshness evidence contains conflicting observations for one semantic path"
        )));
    }
    Ok(())
}

fn validate_post_write_final_validation_evidence(
    key: &str,
    baseline: &GateBaseline,
    revision: &GateRevision,
    snapshot: &AnalysisSnapshot,
    evidence: &lumin_evidence::PostWriteFinalValidationEvidence,
) -> Result<(), StoreError> {
    if !canonical_set(&evidence.expected_leased_write_set)
        || !canonical_alias_set(&evidence.expected_alias_closures)
        || evidence.expected_alias_closures != revision.alias_closures
        || evidence.expected_leased_write_set.iter().any(|lease| {
            !baseline
                .leased_write_set
                .iter()
                .any(|baseline_lease| baseline_lease.covers(&lease.path))
                && !revision.signals.iter().any(|signal| {
                    matches!(
                        signal,
                        GateSignal::UnplannedWrite { paths } if paths.contains(&lease.path)
                    )
                })
        })
    {
        return Err(StoreError::Integrity(format!(
            "gate {key} close revision {} has an invalid final write-domain observation",
            revision.revision
        )));
    }
    validate_final_freshness_evidence(key, snapshot, &evidence.observation)?;
    Ok(())
}

fn canonical_set<T>(items: &[T]) -> bool
where
    T: Clone + Ord,
{
    let mut canonical = items.to_vec();
    canonical.sort();
    canonical.dedup();
    canonical == items
}

fn canonical_alias_set(items: &[PhysicalAliasClosureRecord]) -> bool {
    items.iter().all(|closure| canonical_set(&closure.members)) && canonical_set(items)
}

fn validate_opening_snapshot_policy(
    key: &str,
    opening: &GateRevision,
    baseline: &GateBaseline,
    final_freshness_signals: &[GateSignal],
) -> Result<(), StoreError> {
    let mut expected = gate_policy::opening_signals(&baseline.snapshot, &baseline.leased_write_set);
    expected.extend_from_slice(final_freshness_signals);
    if opening.signals != expected {
        return Err(StoreError::Integrity(format!(
            "gate {key} opening signals disagree with its sealed analysis and final-freshness observations"
        )));
    }
    Ok(())
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
    baseline: &GateBaseline,
    final_freshness_signals: &[GateSignal],
) -> Result<(), StoreError> {
    validate_close_alias_closures(key, revision, snapshot, &baseline.leased_write_set)?;
    let (expected_signals, expected_changed_paths, expected_deltas) = gate_policy::closing_signals(
        reconciled_baseline,
        snapshot,
        prior_protected_semantic_inputs,
        &baseline.leased_write_set,
    );
    let expected_actual_write_set = gate_policy::closure_expanded_actual_write_set(
        &expected_changed_paths,
        &baseline.alias_closures,
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
    let mut expected_contextual = expected_signals
        .iter()
        .filter(|signal| {
            matches!(
                signal,
                GateSignal::ProtectedInputChanged { .. } | GateSignal::UnplannedWrite { .. }
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    for signal in final_freshness_signals {
        if matches!(
            signal,
            GateSignal::ProtectedInputChanged { .. } | GateSignal::UnplannedWrite { .. }
        ) && !expected_contextual.contains(signal)
        {
            expected_contextual.push(signal.clone());
        }
    }
    let mut observed_contextual = revision
        .signals
        .iter()
        .filter(|signal| {
            matches!(
                signal,
                GateSignal::ProtectedInputChanged { .. } | GateSignal::UnplannedWrite { .. }
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    observed_contextual.dedup();
    let impossible = revision.signals.iter().any(|signal| {
        !is_strict_close_snapshot_signal(signal)
            && !matches!(
                signal,
                GateSignal::ProtectedInputChanged { .. } | GateSignal::UnplannedWrite { .. }
            )
    });
    if observed_owned != expected_owned || observed_contextual != expected_contextual || impossible
    {
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
        GateSignal::PreExistingAdverseFacts { .. }
            | GateSignal::RequiredEvidenceIncomplete { .. }
            | GateSignal::RequiredOwnerUnavailable { .. }
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
            if operation.kind == GateOperationKind::PostWrite
                && operation.status == GateOperationStatus::Pending
            {
                let baseline = gate.baseline.as_ref().ok_or_else(|| {
                    StoreError::Integrity(format!(
                        "pending post-write operation {} targets a gate without a sealed baseline",
                        operation.operation_id.as_str()
                    ))
                })?;
                if operation.leased_write_set != baseline.leased_write_set {
                    return Err(StoreError::Integrity(format!(
                        "pending post-write operation {} lease projection disagrees with its active gate",
                        operation.operation_id.as_str()
                    )));
                }
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
    let mut historical_run_ids = snapshot
        .run_catalog
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    for (key, bytes) in &snapshot.run_catalog {
        let record = parse_record::<RunCatalogRecord>("run-catalog", key, bytes)?;
        if record.run_id.as_str() != key {
            return Err(StoreError::Integrity(format!(
                "run catalog key {key} disagrees with its record"
            )));
        }
    }
    for_each_retained_item(snapshot, |kind, record_id, _| {
        if kind == RetentionItemKind::Run {
            historical_run_ids.insert(record_id.to_owned());
        }
        Ok(())
    })?;
    let retained_insertions = u64::try_from(historical_run_ids.len()).map_err(|_| {
        StoreError::Integrity("run-catalog retained insertion count overflow".to_owned())
    })?;
    let retained_pruning_revisions =
        snapshot
            .retention_plans
            .iter()
            .try_fold(0_u64, |count, (key, bytes)| {
                let plan = parse_record::<StoredRetentionPlan>("retention-plans", key, bytes)?;
                let advances_catalog = plan.record.state != RetentionPlanState::Prepared
                    && plan
                        .record
                        .items
                        .iter()
                        .any(|item| item.kind == RetentionItemKind::Run);
                if advances_catalog {
                    count.checked_add(1).ok_or_else(|| {
                        StoreError::Integrity(
                            "run-catalog retained pruning revision count overflow".to_owned(),
                        )
                    })
                } else {
                    Ok(count)
                }
            })?;
    let minimum = retained_insertions
        .checked_add(retained_pruning_revisions)
        .ok_or_else(|| {
            StoreError::Integrity("run-catalog retained revision count overflow".to_owned())
        })?;
    validate_allocator_sequence(snapshot, "run-catalog", minimum)
}

fn validate_operation_result(operation: &OperationRecord) -> Result<(), StoreError> {
    if operation.kind != GateOperationKind::GateAbandon && operation.reason.is_some() {
        return Err(StoreError::Integrity(format!(
            "non-administrative operation {} retained a reason",
            operation.operation_id.as_str()
        )));
    }
    if operation.kind == GateOperationKind::PreWrite && operation.target_revision != 0 {
        return Err(StoreError::Integrity(format!(
            "pre-write operation {} retained a nonzero target revision",
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
        if operation.request_digest
            != gate_abandon_request_digest(&operation.gate_id, operation.target_revision, reason)
        {
            return Err(StoreError::Integrity(format!(
                "administrative abandon operation {} disagrees with its authenticated request",
                operation.operation_id.as_str()
            )));
        }
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
        && (!operation.pre_write_declared_path_inspection.is_empty()
            || operation.pre_write_admission_evidence.is_some()
            || operation.pre_write_final_validation.is_some())
    {
        return Err(StoreError::Integrity(format!(
            "non-pre-write operation {} retained pre-write evidence",
            operation.operation_id.as_str()
        )));
    }
    if operation.kind != GateOperationKind::PostWrite
        && operation.post_write_final_validation.is_some()
    {
        return Err(StoreError::Integrity(format!(
            "non-post-write operation {} retained post-write evidence",
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
            if operation.leased_write_set != result.leased_write_set {
                return Err(StoreError::Integrity(format!(
                    "committed operation lease projection disagrees with its result: {}",
                    operation.operation_id.as_str()
                )));
            }
            if operation.kind == GateOperationKind::PreWrite {
                if operation.pre_write_admission_evidence.is_none()
                    && operation
                        .pre_write_declared_path_inspection
                        .iter()
                        .filter_map(|inspection| inspection.rejection.as_ref())
                        .any(|signal| !result.signals.contains(signal))
                {
                    return Err(StoreError::Integrity(format!(
                        "pre-write operation {} result omits its declared-path inspection rejection",
                        operation.operation_id.as_str()
                    )));
                }
                match operation.pre_write_final_validation.as_ref() {
                    Some(final_validation)
                        if operation.pre_write_admission_evidence.is_none()
                            && final_validation.signals == result.signals => {}
                    Some(_) => {
                        return Err(StoreError::Integrity(format!(
                            "pre-write operation {} result disagrees with its final validation record",
                            operation.operation_id.as_str()
                        )));
                    }
                    None if operation
                        .pre_write_admission_evidence
                        .as_ref()
                        .is_some_and(|evidence| is_admission_conflict_result(result, evidence)) => {
                    }
                    None => {
                        return Err(StoreError::Integrity(format!(
                            "committed pre-write operation {} omitted its final validation record",
                            operation.operation_id.as_str()
                        )));
                    }
                }
            }
            if operation.kind == GateOperationKind::PostWrite {
                match operation.post_write_final_validation.as_ref() {
                    Some(final_validation) if final_validation.signals == result.signals => {}
                    Some(_) => {
                        return Err(StoreError::Integrity(format!(
                            "post-write operation {} result disagrees with its final validation record",
                            operation.operation_id.as_str()
                        )));
                    }
                    None => {
                        return Err(StoreError::Integrity(format!(
                            "committed post-write operation {} omitted its final validation record",
                            operation.operation_id.as_str()
                        )));
                    }
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

fn is_admission_conflict_result(
    result: &lumin_evidence::GateOperationResult,
    evidence: &PreWriteAdmissionEvidence,
) -> bool {
    result.lifecycle == GateLifecycle::Rejected
        && !result.decision.authorizes()
        && !evidence.conflict_owners.is_empty()
        && result.signals == derive_pre_write_admission_signals(evidence)
        && matches!(
            result.observation_binding.as_ref(),
            Some(ObservationBinding::Unsealed { .. })
        )
}

fn reject_unfinished_pre_write_final_validation(
    operation: &OperationRecord,
) -> Result<(), StoreError> {
    if operation.pre_write_admission_evidence.is_some()
        || operation.pre_write_final_validation.is_some()
        || operation.post_write_final_validation.is_some()
    {
        return Err(StoreError::Integrity(format!(
            "unfinished operation {} retained completed pre-write evidence",
            operation.operation_id.as_str()
        )));
    }
    Ok(())
}

fn validate_pending_operation_state(operation: &OperationRecord) -> Result<(), StoreError> {
    validate_unfinished_interruption_capacity(operation)?;
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
    validate_declared_path_inspection(operation)?;
    Ok(())
}

fn validate_declared_path_inspection(
    operation: &OperationRecord,
) -> Result<Vec<lumin_evidence::WriteLease>, StoreError> {
    let observed_paths = operation
        .pre_write_declared_path_inspection
        .iter()
        .map(|inspection| inspection.path.clone())
        .collect::<Vec<_>>();
    if observed_paths != operation.declared_write_set {
        return Err(StoreError::Integrity(format!(
            "pre-write operation {} inspection does not cover its exact declared path set",
            operation.operation_id.as_str()
        )));
    }

    let mut leases = Vec::new();
    for inspection in &operation.pre_write_declared_path_inspection {
        match (&inspection.lease, &inspection.rejection) {
            (Some(lease), None) if lease.path == inspection.path => leases.push(lease.clone()),
            (None, Some(GateSignal::DeclaredPathUnsupported { path, .. }))
                if path == &inspection.path => {}
            (None, Some(GateSignal::AnalysisFailed { .. })) => {}
            _ => {
                return Err(StoreError::Integrity(format!(
                    "pre-write operation {} has an invalid inspection outcome for {}",
                    operation.operation_id.as_str(),
                    inspection.path.display
                )));
            }
        }
    }
    let mut canonical = leases.clone();
    canonical.sort();
    canonical.dedup();
    if canonical != leases {
        return Err(StoreError::Integrity(format!(
            "pre-write operation {} inspection contains a noncanonical lease set",
            operation.operation_id.as_str()
        )));
    }
    Ok(leases)
}

fn validate_scan_invocation_patterns(
    owner: &str,
    invocation: &lumin_evidence::ScanInvocationTier,
) -> Result<(), StoreError> {
    invocation
        .validate_patterns()
        .map_err(|error| {
            StoreError::Integrity(format!(
                "{owner} has an invalid persisted scan invocation: {error}"
            ))
        })
        .and_then(|()| {
            invocation.validate_canonical_shape().map_err(|error| {
                StoreError::Integrity(format!(
                    "{owner} has a noncanonical persisted scan invocation: {error}"
                ))
            })
        })
}

fn validate_pending_pre_write_write_domain(operation: &OperationRecord) -> Result<(), StoreError> {
    let leases = validate_declared_path_inspection(operation)?;
    if operation.leased_write_set != leases {
        return Err(StoreError::Integrity(format!(
            "pending pre-write operation {} has an incoherent provisional write domain",
            operation.operation_id.as_str()
        )));
    }
    for lease in &leases {
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
                    lease.path.display
                )));
            }
        }
    }
    Ok(())
}

fn validate_interrupted_operation_state(operation: &OperationRecord) -> Result<(), StoreError> {
    validate_unfinished_interruption_capacity(operation)?;
    if matches!(
        operation.kind,
        GateOperationKind::PreWrite | GateOperationKind::PostWrite
    ) && operation.interruption_count == 0
    {
        return Err(StoreError::Integrity(format!(
            "interrupted operation {} has a zero interruption count",
            operation.operation_id.as_str()
        )));
    }
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

fn validate_unfinished_interruption_capacity(
    operation: &OperationRecord,
) -> Result<(), StoreError> {
    if operation.interruption_count == u64::MAX {
        return Err(StoreError::Integrity(format!(
            "unfinished operation {} exhausted its interruption count",
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

fn parse_record<T: DeserializeOwned + Serialize>(
    table: &str,
    key: &str,
    bytes: &[u8],
) -> Result<T, StoreError> {
    decode_closed_json(bytes).map_err(|error| {
        StoreError::Integrity(format!("{table} record {key} is malformed: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumin_evidence::{
        GateAnalysisOptions, RetentionPlanItem, RetentionTombstoneEnvelope, SemanticInputRecord,
        SemanticInputState, WriteLease,
    };
    use lumin_model::{GateId, PhysicalFileIdentity, RetentionPlanId};

    fn empty_snapshot() -> LogicalStoreSnapshot {
        LogicalStoreSnapshot {
            sequences: BTreeMap::new(),
            attempt_leases: BTreeMap::new(),
            run_catalog: BTreeMap::new(),
            pointers: BTreeMap::new(),
            gates: BTreeMap::new(),
            operations: BTreeMap::new(),
            validation_receipts: BTreeMap::new(),
            transitions: BTreeMap::new(),
            retention_plans: BTreeMap::new(),
            retention_operations: BTreeMap::new(),
            retention_tombstones: BTreeMap::new(),
            run_pins: BTreeMap::new(),
            cache_cleanup_operations: BTreeMap::new(),
            cache_eviction_authorizations: BTreeMap::new(),
        }
    }

    #[test]
    fn allocator_floors_include_retention_plans_and_tombstones()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut snapshot = empty_snapshot();
        snapshot.sequences.insert("attempt".to_owned(), 7);
        let plan = StoredRetentionPlan {
            record: lumin_evidence::RetentionPlanRecord {
                schema_version: "lumin-retention-plan.v1".to_owned(),
                repository_id: lumin_model::RepositoryId::from_string("repository".to_owned()),
                plan_id: RetentionPlanId::from_string("retention_plan_0000000000000001".to_owned()),
                content_identity: lumin_model::RetentionContentIdentity::from_string(
                    "content".to_owned(),
                ),
                scope: lumin_evidence::RetentionPlanScope::Runs {
                    before_unix_millis: 0,
                },
                created_unix_millis: 0,
                catalog_revision: 1,
                state: RetentionPlanState::Prepared,
                items: vec![RetentionPlanItem {
                    kind: RetentionItemKind::Attempt,
                    owning_sequence: 8,
                    record_id: "attempt_0000000000000008".to_owned(),
                    identity_sha256: "identity".to_owned(),
                    byte_count: 1,
                }],
                exclusions: Vec::new(),
                confirmation_operation_id: None,
                recoverable_state: None,
                tombstone_identity: None,
                physical_reclamation_pending: false,
            },
            trash_nonce: "nonce".to_owned(),
            progress: None,
        };
        snapshot.retention_plans.insert(
            plan.record.plan_id.as_str().to_owned(),
            serde_json::to_vec(&plan)?,
        );
        assert!(matches!(
            validate_attempt_allocator_sequence(&snapshot),
            Err(StoreError::Integrity(message))
                if message.contains("minimum 8")
        ));

        snapshot.retention_plans.clear();
        let tombstone = StoredTombstone {
            schema_version: "lumin-retention-tombstone.v1".to_owned(),
            envelope: RetentionTombstoneEnvelope {
                record_kind: RetentionItemKind::Attempt,
                record_id: "attempt_0000000000000009".to_owned(),
                plan_id: RetentionPlanId::from_string("retention_plan_0000000000000001".to_owned()),
                recoverable_state: None,
                tombstone_identity: None,
                physical_reclamation_pending: false,
            },
            identity_sha256: "identity".to_owned(),
            owning_sequence: 9,
        };
        snapshot.retention_tombstones.insert(
            "0:attempt_0000000000000009".to_owned(),
            serde_json::to_vec(&tombstone)?,
        );
        assert!(matches!(
            validate_attempt_allocator_sequence(&snapshot),
            Err(StoreError::Integrity(message))
                if message.contains("minimum 9")
        ));
        Ok(())
    }

    fn active_gate(
        id: &str,
        lease_path: &str,
        lease_identity: PhysicalFileIdentity,
        protected_semantic_inputs: Vec<SemanticInputRecord>,
    ) -> Result<GateRecord, Box<dyn std::error::Error>> {
        let path = RepoPath::from_portable(lease_path)?;
        let projection = lumin_evidence::RepoPathProjection::from(&path);
        Ok(GateRecord {
            schema_version: GATE_RECORD_SCHEMA_VERSION.to_owned(),
            gate_id: GateId::from_string(id.to_owned()),
            lifecycle: GateLifecycle::Active,
            current_revision: 0,
            declared_write_set: vec![projection.clone()],
            leased_write_set: vec![WriteLease {
                path: projection,
                kind: WriteLeaseKind::ExistingFile,
                physical_identity: Some(lease_identity),
                nearest_existing_parent: None,
                prefix_identities: Vec::new(),
            }],
            alias_closures: Vec::new(),
            transition_refs: Vec::new(),
            analysis_options: GateAnalysisOptions {
                jobs: 1,
                resolution_profile: None,
                scan_invocation: Default::default(),
            },
            baseline: None,
            protected_semantic_inputs,
            revisions: Vec::new(),
        })
    }

    #[test]
    fn active_gate_catalog_rejects_pairwise_write_and_read_conflicts()
    -> Result<(), Box<dyn std::error::Error>> {
        let shared_identity = PhysicalFileIdentity::Unix {
            device: 17,
            inode: 29,
        };
        let first = active_gate(
            "gate-first",
            "src/first.ts",
            shared_identity.clone(),
            Vec::new(),
        )?;
        let second = active_gate(
            "gate-second",
            "src/second.ts",
            shared_identity.clone(),
            Vec::new(),
        )?;
        let write_conflict = BTreeMap::from([("gate-first", first), ("gate-second", second)]);
        assert!(matches!(
            validate_active_gate_conflicts(&write_conflict),
            Err(StoreError::Integrity(message)) if message.contains("conflicting write/read domains")
        ));

        let protected_path = RepoPath::from_portable("config/tsconfig.json")?;
        let protected = SemanticInputRecord {
            path: lumin_evidence::RepoPathProjection::from(&protected_path),
            state: SemanticInputState::ConfigPresent,
            payload_sha256: Some("payload".to_owned()),
            physical_identity: Some(shared_identity.clone()),
            absence_parent: None,
            physical_redirect_sha256: None,
        };
        let writer = active_gate("gate-writer", "src/writer.ts", shared_identity, Vec::new())?;
        let reader = active_gate(
            "gate-reader",
            "src/reader.ts",
            PhysicalFileIdentity::Unix {
                device: 17,
                inode: 31,
            },
            vec![protected],
        )?;
        let read_conflict = BTreeMap::from([("gate-reader", reader), ("gate-writer", writer)]);
        assert!(matches!(
            validate_active_gate_conflicts(&read_conflict),
            Err(StoreError::Integrity(message)) if message.contains("conflicting write/read domains")
        ));
        Ok(())
    }
}
