use lumin_evidence::{
    AnalysisSnapshot, GateAnalysisOptions, GateBaseline, GateLifecycle, GateOperationKind,
    GateOperationResult, GateOperationStatus, GateRecord, GateRevision, GateSignal,
    OperationRecord, PhysicalAliasClosureRecord, RepoPathProjection, SemanticInputRecord,
    SemanticReadReservationBinding, TransitionCapsule, WorktreeTransition, WriteLease, gate_policy,
};
use lumin_model::{GateDeltaRecord, GateId, OperationId};
use redb::{TableDefinition, WriteTransaction};

use super::{RepositoryStore, StoreError, backend_error, open_lifecycle_database};

mod coordination;
mod records;
#[cfg(test)]
mod tests;

use coordination::{
    active_write_conflicts, attach_transition_references, conflicts, post_write_analysis_context,
    semantic_read_conflicts, transition_sequences_for_gate,
};
use records::{
    current_transition_sequence, load_record, next_gate_id, next_transition_sequence, read_record,
    read_records, transition_key, write_record,
};

const GATES: TableDefinition<&str, &[u8]> = TableDefinition::new("gates");
const OPERATIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("operations");
const TRANSITIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("worktree-transitions");

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveGateLease {
    pub gate_id: GateId,
    pub leased_write_set: Vec<WriteLease>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreWriteFinish {
    pub baseline: Option<GateBaseline>,
    pub leased_write_set: Vec<WriteLease>,
    pub alias_closures: Vec<PhysicalAliasClosureRecord>,
    pub signals: Vec<GateSignal>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostWriteFinish {
    pub snapshot: Option<AnalysisSnapshot>,
    pub protected_semantic_inputs: Vec<SemanticInputRecord>,
    pub reconciled_baseline: Option<AnalysisSnapshot>,
    pub changed_paths: Vec<RepoPathProjection>,
    pub alias_closures: Vec<PhysicalAliasClosureRecord>,
    pub reconciled_transition_sequences: Vec<u64>,
    pub signals: Vec<GateSignal>,
    pub deltas: Vec<GateDeltaRecord>,
}

struct ConflictSet {
    paths: Vec<RepoPathProjection>,
    gate_ids: Vec<GateId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PreWriteStart {
    Analyze {
        gate_id: GateId,
        transition_sequence: u64,
    },
    Committed(GateOperationResult),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PostWriteStart {
    Analyze {
        gate: Box<GateRecord>,
        transitions: Vec<WorktreeTransition>,
        active_gates: Vec<ActiveGateLease>,
    },
    Committed(GateOperationResult),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SemanticReadReservation {
    Reserved,
    Conflict {
        paths: Vec<RepoPathProjection>,
        gate_ids: Vec<GateId>,
    },
    TransitionCatalogChanged,
    Committed(GateOperationResult),
}

impl RepositoryStore {
    pub fn reserve_pre_write(
        &self,
        operation_id: &OperationId,
        request_digest: &str,
        declared_write_set: &[RepoPathProjection],
        initial_leases: &[WriteLease],
        analysis_options: &GateAnalysisOptions,
    ) -> Result<PreWriteStart, StoreError> {
        self.with_exclusive_lock(|| {
            let database = open_lifecycle_database(&self.state_dir)?;
            let write = database.begin_write().map_err(backend_error)?;
            if let Some(operation) =
                read_record::<OperationRecord>(&write, OPERATIONS, operation_id.as_str())?
            {
                validate_operation(
                    &operation,
                    GateOperationKind::PreWrite,
                    request_digest,
                    None,
                )?;
                if let Some(result) = operation.result {
                    return Ok(PreWriteStart::Committed(result));
                }
                return Ok(PreWriteStart::Analyze {
                    gate_id: operation.gate_id,
                    transition_sequence: operation.transition_sequence,
                });
            }

            let gate_id = next_gate_id(&write)?;
            let transition_sequence = current_transition_sequence(&write)?;
            let (paths, gate_ids) = conflicts(&write, operation_id, initial_leases, &[], None)?;
            let mut operation = OperationRecord {
                schema_version: "lumin-operation.v1".to_owned(),
                operation_id: operation_id.clone(),
                kind: GateOperationKind::PreWrite,
                request_digest: request_digest.to_owned(),
                status: GateOperationStatus::Pending,
                gate_id: gate_id.clone(),
                target_revision: 0,
                transition_sequence,
                declared_write_set: declared_write_set.to_vec(),
                leased_write_set: initial_leases.to_vec(),
                semantic_read_reservations: Vec::new(),
                semantic_read_reservation_bindings: Vec::new(),
                analysis_options: Some(analysis_options.clone()),
                result: None,
            };

            if !paths.is_empty() {
                let signals = vec![GateSignal::WriteConflict { paths, gate_ids }];
                let result = rejected_open_result(&operation, &signals);
                let gate = rejected_gate(&operation, analysis_options.clone(), &signals, None);
                operation.status = GateOperationStatus::Committed;
                operation.result = Some(result.clone());
                write_record(&write, GATES, gate.gate_id.as_str(), &gate)?;
                write_record(
                    &write,
                    OPERATIONS,
                    operation.operation_id.as_str(),
                    &operation,
                )?;
                write.commit().map_err(backend_error)?;
                return Ok(PreWriteStart::Committed(result));
            }

            write_record(
                &write,
                OPERATIONS,
                operation.operation_id.as_str(),
                &operation,
            )?;
            write.commit().map_err(backend_error)?;
            Ok(PreWriteStart::Analyze {
                gate_id,
                transition_sequence,
            })
        })
    }

    pub fn finish_pre_write(
        &self,
        operation_id: &OperationId,
        request_digest: &str,
        gate_id: &GateId,
        finish: PreWriteFinish,
    ) -> Result<GateOperationResult, StoreError> {
        let PreWriteFinish {
            baseline,
            leased_write_set,
            alias_closures,
            mut signals,
        } = finish;
        self.with_exclusive_lock(|| {
            let database = open_lifecycle_database(&self.state_dir)?;
            let write = database.begin_write().map_err(backend_error)?;
            let mut operation = load_operation_for_finish(
                &write,
                operation_id,
                GateOperationKind::PreWrite,
                request_digest,
                Some(gate_id),
                "pre-write",
            )?;
            if let Some(result) = operation.result {
                return Ok(result);
            }
            validate_pre_write_context(
                &write,
                &operation,
                baseline.as_ref(),
                &leased_write_set,
                &mut signals,
            )?;
            let (gate, result) = completed_pre_write_records(
                &operation,
                baseline,
                leased_write_set,
                alias_closures,
                signals,
            )?;
            operation.leased_write_set = result.leased_write_set.clone();
            persist_operation_result(&write, &gate, &mut operation, &result)?;
            write.commit().map_err(backend_error)?;
            Ok(result)
        })
    }

    pub fn begin_post_write(
        &self,
        operation_id: &OperationId,
        request_digest: &str,
        gate_id: &GateId,
    ) -> Result<PostWriteStart, StoreError> {
        self.with_exclusive_lock(|| {
            let database = open_lifecycle_database(&self.state_dir)?;
            let write = database.begin_write().map_err(backend_error)?;
            if let Some(operation) =
                read_record::<OperationRecord>(&write, OPERATIONS, operation_id.as_str())?
            {
                validate_operation(
                    &operation,
                    GateOperationKind::PostWrite,
                    request_digest,
                    Some(gate_id),
                )?;
                if let Some(result) = operation.result {
                    return Ok(PostWriteStart::Committed(result));
                }
                let gate = read_record::<GateRecord>(&write, GATES, gate_id.as_str())?
                    .ok_or_else(|| StoreError::GateNotFound(gate_id.as_str().to_owned()))?;
                let (transitions, active_gates) =
                    post_write_analysis_context(&write, &gate, operation.transition_sequence)?;
                return Ok(PostWriteStart::Analyze {
                    gate: Box::new(gate),
                    transitions,
                    active_gates,
                });
            }

            let gate = read_record::<GateRecord>(&write, GATES, gate_id.as_str())?
                .ok_or_else(|| StoreError::GateNotFound(gate_id.as_str().to_owned()))?;
            if gate.lifecycle != GateLifecycle::Active {
                return Err(StoreError::GateNotActive(gate_id.as_str().to_owned()));
            }
            for operation in read_records::<OperationRecord>(&write, OPERATIONS)? {
                if operation.status == GateOperationStatus::Pending
                    && operation.kind == GateOperationKind::PostWrite
                    && operation.gate_id == *gate_id
                    && operation.target_revision == gate.current_revision
                {
                    return Err(StoreError::GateRevisionBusy(format!(
                        "{}@{}",
                        gate_id.as_str(),
                        gate.current_revision
                    )));
                }
            }
            let operation = OperationRecord {
                schema_version: "lumin-operation.v1".to_owned(),
                operation_id: operation_id.clone(),
                kind: GateOperationKind::PostWrite,
                request_digest: request_digest.to_owned(),
                status: GateOperationStatus::Pending,
                gate_id: gate_id.clone(),
                target_revision: gate.current_revision,
                transition_sequence: current_transition_sequence(&write)?,
                declared_write_set: Vec::new(),
                leased_write_set: gate.leased_write_set.clone(),
                semantic_read_reservations: Vec::new(),
                semantic_read_reservation_bindings: Vec::new(),
                analysis_options: None,
                result: None,
            };
            let (transitions, active_gates) =
                post_write_analysis_context(&write, &gate, operation.transition_sequence)?;
            write_record(
                &write,
                OPERATIONS,
                operation.operation_id.as_str(),
                &operation,
            )?;
            write.commit().map_err(backend_error)?;
            Ok(PostWriteStart::Analyze {
                gate: Box::new(gate),
                transitions,
                active_gates,
            })
        })
    }

    pub fn reserve_post_write_semantic_inputs(
        &self,
        operation_id: &OperationId,
        request_digest: &str,
        gate_id: &GateId,
        demanded_inputs: &[SemanticReadReservationBinding],
    ) -> Result<SemanticReadReservation, StoreError> {
        self.reserve_semantic_inputs(
            operation_id,
            request_digest,
            gate_id,
            demanded_inputs,
            GateOperationKind::PostWrite,
            "post-write semantic-read reservation",
        )
    }

    pub fn reserve_pre_write_semantic_inputs(
        &self,
        operation_id: &OperationId,
        request_digest: &str,
        gate_id: &GateId,
        demanded_inputs: &[SemanticReadReservationBinding],
    ) -> Result<SemanticReadReservation, StoreError> {
        self.reserve_semantic_inputs(
            operation_id,
            request_digest,
            gate_id,
            demanded_inputs,
            GateOperationKind::PreWrite,
            "pre-write semantic-read reservation",
        )
    }

    fn reserve_semantic_inputs(
        &self,
        operation_id: &OperationId,
        request_digest: &str,
        gate_id: &GateId,
        demanded_inputs: &[SemanticReadReservationBinding],
        kind: GateOperationKind,
        phase: &str,
    ) -> Result<SemanticReadReservation, StoreError> {
        let mut demanded_inputs = demanded_inputs.to_vec();
        demanded_inputs.sort();
        for pair in demanded_inputs.windows(2) {
            if pair[0].path == pair[1].path && pair[0] != pair[1] {
                return Err(StoreError::Integrity(format!(
                    "semantic-read demand has conflicting physical identities: {}",
                    pair[0].path.display
                )));
            }
        }
        demanded_inputs.dedup();
        let mut demanded_paths = demanded_inputs
            .iter()
            .map(|input| input.path.clone())
            .collect::<Vec<_>>();
        demanded_paths.sort();
        demanded_paths.dedup();
        self.with_exclusive_lock(|| {
            let database = open_lifecycle_database(&self.state_dir)?;
            let write = database.begin_write().map_err(backend_error)?;
            let mut operation = load_operation_for_finish(
                &write,
                operation_id,
                kind,
                request_digest,
                Some(gate_id),
                phase,
            )?;
            if let Some(result) = operation.result {
                return Ok(SemanticReadReservation::Committed(result));
            }
            if kind == GateOperationKind::PostWrite {
                load_active_gate_for_post_write(&write, gate_id, &operation)?;
            }
            if current_transition_sequence(&write)? != operation.transition_sequence {
                return Ok(SemanticReadReservation::TransitionCatalogChanged);
            }
            let conflicts =
                semantic_read_conflicts(&write, operation_id, gate_id, &demanded_inputs)?;
            if !conflicts.paths.is_empty() {
                return Ok(SemanticReadReservation::Conflict {
                    paths: conflicts.paths,
                    gate_ids: conflicts.gate_ids,
                });
            }
            for demanded in &demanded_inputs {
                if let Some(existing) = operation
                    .semantic_read_reservation_bindings
                    .iter()
                    .find(|existing| existing.path == demanded.path)
                    && existing != demanded
                {
                    return Err(StoreError::Integrity(format!(
                        "semantic-read reservation identity changed before capture: {}",
                        demanded.path.display
                    )));
                }
            }
            operation.semantic_read_reservations.extend(demanded_paths);
            operation.semantic_read_reservations.sort();
            operation.semantic_read_reservations.dedup();
            operation
                .semantic_read_reservation_bindings
                .extend(demanded_inputs);
            operation.semantic_read_reservation_bindings.sort();
            operation.semantic_read_reservation_bindings.dedup();
            validate_reservation_binding_set(&operation)?;
            write_record(
                &write,
                OPERATIONS,
                operation.operation_id.as_str(),
                &operation,
            )?;
            write.commit().map_err(backend_error)?;
            Ok(SemanticReadReservation::Reserved)
        })
    }

    pub fn finish_post_write(
        &self,
        operation_id: &OperationId,
        request_digest: &str,
        gate_id: &GateId,
        finish: PostWriteFinish,
    ) -> Result<GateOperationResult, StoreError> {
        let PostWriteFinish {
            snapshot,
            protected_semantic_inputs,
            reconciled_baseline,
            changed_paths,
            alias_closures,
            reconciled_transition_sequences,
            mut signals,
            deltas,
        } = finish;
        self.with_exclusive_lock(|| {
            let database = open_lifecycle_database(&self.state_dir)?;
            let write = database.begin_write().map_err(backend_error)?;
            let mut operation = load_operation_for_finish(
                &write,
                operation_id,
                GateOperationKind::PostWrite,
                request_digest,
                Some(gate_id),
                "post-write",
            )?;
            if let Some(result) = operation.result {
                return Ok(result);
            }
            let mut gate = load_active_gate_for_post_write(&write, gate_id, &operation)?;
            if let Some(snapshot) = snapshot.as_ref() {
                validate_captured_reservations(
                    &operation,
                    &snapshot.inputs,
                    "post-write snapshot",
                )?;
            }
            validate_post_write_context(
                &write,
                &gate,
                &operation,
                &changed_paths,
                &reconciled_transition_sequences,
                &mut signals,
            )?;
            let decision = gate_policy::decision(&signals);
            let revision = gate
                .current_revision
                .checked_add(1)
                .ok_or_else(|| StoreError::Integrity("gate revision overflow".to_owned()))?;
            if decision.authorizes() {
                publish_authorized_transition(
                    &write,
                    &mut gate,
                    revision,
                    snapshot.as_ref(),
                    reconciled_baseline.as_ref(),
                    &changed_paths,
                    &alias_closures,
                )?;
            }
            if snapshot_can_protect_current_reads(snapshot.as_ref(), &signals) {
                gate.protected_semantic_inputs = protected_semantic_inputs.clone();
            }
            gate.current_revision = revision;
            let result = GateOperationResult {
                operation_id: operation_id.clone(),
                request_digest: request_digest.to_owned(),
                gate_id: gate_id.clone(),
                revision,
                lifecycle: gate.lifecycle,
                decision,
                signals: signals.clone(),
                leased_write_set: gate.leased_write_set.clone(),
                deltas: deltas.clone(),
            };
            gate.revisions.push(GateRevision {
                revision,
                operation_id: operation_id.clone(),
                decision,
                signals: signals.clone(),
                changed_paths,
                snapshot,
                protected_semantic_inputs,
                alias_closures,
                reconciled_transition_sequences,
                deltas,
            });
            persist_operation_result(&write, &gate, &mut operation, &result)?;
            write.commit().map_err(backend_error)?;
            Ok(result)
        })
    }

    pub fn load_gate(&self, gate_id: &GateId) -> Result<GateRecord, StoreError> {
        self.with_shared_lock(|| {
            let database = open_lifecycle_database(&self.state_dir)?;
            load_record::<GateRecord>(&database, GATES, gate_id.as_str())?
                .ok_or_else(|| StoreError::GateNotFound(gate_id.as_str().to_owned()))
        })
    }

    pub fn load_operation(
        &self,
        operation_id: &OperationId,
    ) -> Result<OperationRecord, StoreError> {
        self.with_shared_lock(|| {
            let database = open_lifecycle_database(&self.state_dir)?;
            load_record::<OperationRecord>(&database, OPERATIONS, operation_id.as_str())?
                .ok_or_else(|| StoreError::OperationNotFound(operation_id.as_str().to_owned()))
        })
    }
}

fn load_operation_for_finish(
    write: &WriteTransaction,
    operation_id: &OperationId,
    kind: GateOperationKind,
    request_digest: &str,
    gate_id: Option<&GateId>,
    phase: &str,
) -> Result<OperationRecord, StoreError> {
    let operation = read_record::<OperationRecord>(write, OPERATIONS, operation_id.as_str())?
        .ok_or_else(|| {
            StoreError::Integrity(format!(
                "pending {phase} operation disappeared: {}",
                operation_id.as_str()
            ))
        })?;
    validate_operation(&operation, kind, request_digest, gate_id)?;
    Ok(operation)
}

fn validate_pre_write_context(
    write: &WriteTransaction,
    operation: &OperationRecord,
    baseline: Option<&GateBaseline>,
    leased_write_set: &[WriteLease],
    signals: &mut Vec<GateSignal>,
) -> Result<(), StoreError> {
    let missing_initial_paths = operation
        .leased_write_set
        .iter()
        .filter(|lease| !leased_write_set.contains(lease))
        .map(|lease| lease.path.clone())
        .collect::<Vec<_>>();
    if !missing_initial_paths.is_empty() {
        signals.push(GateSignal::ProtectedInputChanged {
            paths: missing_initial_paths,
        });
    }
    if baseline
        .is_some_and(|baseline| baseline.transition_sequence != operation.transition_sequence)
    {
        return Err(StoreError::Integrity(
            "pre-write baseline used the wrong transition sequence".to_owned(),
        ));
    }
    if current_transition_sequence(write)? != operation.transition_sequence
        && !signals.contains(&GateSignal::TransitionCatalogChanged)
    {
        signals.push(GateSignal::TransitionCatalogChanged);
    }
    if let Some(baseline) = baseline {
        let omitted_reservations = operation
            .semantic_read_reservations
            .iter()
            .filter(|reserved| {
                !baseline
                    .protected_semantic_inputs
                    .iter()
                    .any(|input| input.path == **reserved)
            })
            .map(|path| path.display.clone())
            .collect::<Vec<_>>();
        if !omitted_reservations.is_empty() {
            return Err(StoreError::Integrity(format!(
                "pre-write baseline omitted reserved semantic inputs: {}",
                omitted_reservations.join(", ")
            )));
        }
        validate_captured_reservations(
            operation,
            &baseline.protected_semantic_inputs,
            "pre-write baseline",
        )?;
    }
    let semantic_inputs = baseline.map_or(&[][..], |baseline| {
        baseline.protected_semantic_inputs.as_slice()
    });
    let (paths, gate_ids) = conflicts(
        write,
        &operation.operation_id,
        leased_write_set,
        semantic_inputs,
        None,
    )?;
    if !paths.is_empty() {
        signals.push(GateSignal::WriteConflict { paths, gate_ids });
    }
    Ok(())
}

fn completed_pre_write_records(
    operation: &OperationRecord,
    baseline: Option<GateBaseline>,
    leased_write_set: Vec<WriteLease>,
    alias_closures: Vec<PhysicalAliasClosureRecord>,
    signals: Vec<GateSignal>,
) -> Result<(GateRecord, GateOperationResult), StoreError> {
    let decision = gate_policy::decision(&signals);
    if decision.authorizes() && baseline.is_none() {
        return Err(StoreError::Integrity(
            "authorizing pre-write omitted its sealed baseline".to_owned(),
        ));
    }
    let lifecycle = if decision.authorizes() {
        GateLifecycle::Active
    } else {
        GateLifecycle::Rejected
    };
    let result = GateOperationResult {
        operation_id: operation.operation_id.clone(),
        request_digest: operation.request_digest.clone(),
        gate_id: operation.gate_id.clone(),
        revision: 0,
        lifecycle,
        decision,
        signals: signals.clone(),
        leased_write_set: leased_write_set.clone(),
        deltas: Vec::new(),
    };
    let analysis_options = operation.analysis_options.clone().ok_or_else(|| {
        StoreError::Integrity("pre-write operation omitted analysis options".to_owned())
    })?;
    let protected_semantic_inputs = baseline.as_ref().map_or_else(Vec::new, |baseline| {
        baseline.protected_semantic_inputs.clone()
    });
    let gate = GateRecord {
        schema_version: "lumin-gate.v1".to_owned(),
        gate_id: operation.gate_id.clone(),
        lifecycle,
        current_revision: 0,
        declared_write_set: operation.declared_write_set.clone(),
        leased_write_set,
        alias_closures: alias_closures.clone(),
        transition_refs: Vec::new(),
        analysis_options,
        baseline,
        protected_semantic_inputs: protected_semantic_inputs.clone(),
        revisions: vec![GateRevision {
            revision: 0,
            operation_id: operation.operation_id.clone(),
            decision,
            signals,
            changed_paths: Vec::new(),
            snapshot: None,
            protected_semantic_inputs,
            alias_closures,
            reconciled_transition_sequences: Vec::new(),
            deltas: Vec::new(),
        }],
    };
    Ok((gate, result))
}

fn load_active_gate_for_post_write(
    write: &WriteTransaction,
    gate_id: &GateId,
    operation: &OperationRecord,
) -> Result<GateRecord, StoreError> {
    let gate = read_record::<GateRecord>(write, GATES, gate_id.as_str())?
        .ok_or_else(|| StoreError::GateNotFound(gate_id.as_str().to_owned()))?;
    if gate.lifecycle != GateLifecycle::Active {
        return Err(StoreError::GateNotActive(gate_id.as_str().to_owned()));
    }
    if gate.current_revision != operation.target_revision {
        return Err(StoreError::Integrity(format!(
            "gate revision changed during post-write: expected {}, observed {}",
            operation.target_revision, gate.current_revision
        )));
    }
    Ok(gate)
}

fn validate_post_write_context(
    write: &WriteTransaction,
    gate: &GateRecord,
    operation: &OperationRecord,
    changed_paths: &[RepoPathProjection],
    reconciled_transition_sequences: &[u64],
    signals: &mut Vec<GateSignal>,
) -> Result<(), StoreError> {
    if current_transition_sequence(write)? != operation.transition_sequence
        && !signals.contains(&GateSignal::TransitionCatalogChanged)
    {
        signals.push(GateSignal::TransitionCatalogChanged);
    }
    let expected_sequences =
        transition_sequences_for_gate(write, gate, operation.transition_sequence)?;
    if expected_sequences != reconciled_transition_sequences
        && !signals.contains(&GateSignal::TransitionCatalogChanged)
    {
        signals.push(GateSignal::TransitionCatalogChanged);
    }
    if let Some(conflicts) = active_write_conflicts(write, &gate.gate_id, changed_paths)? {
        signals.push(GateSignal::ActiveTransitionPending {
            paths: conflicts.paths,
            gate_ids: conflicts.gate_ids,
        });
    }
    if !operation.semantic_read_reservation_bindings.is_empty() {
        let conflicts = semantic_read_conflicts(
            write,
            &operation.operation_id,
            &gate.gate_id,
            &operation.semantic_read_reservation_bindings,
        )?;
        if !conflicts.paths.is_empty() {
            signals.push(GateSignal::SemanticInputConflict {
                paths: conflicts.paths,
                gate_ids: conflicts.gate_ids,
            });
        }
    }
    Ok(())
}

fn snapshot_can_protect_current_reads(
    snapshot: Option<&AnalysisSnapshot>,
    signals: &[GateSignal],
) -> bool {
    snapshot.is_some()
        && !signals.iter().any(|signal| {
            matches!(
                signal,
                GateSignal::AnalysisFailed { .. }
                    | GateSignal::DeclaredPathUnsupported { .. }
                    | GateSignal::WriteConflict { .. }
                    | GateSignal::SemanticInputConflict { .. }
                    | GateSignal::ProtectedInputChanged { .. }
                    | GateSignal::UnplannedWrite { .. }
                    | GateSignal::AnalysisContractChanged
                    | GateSignal::ActiveTransitionPending { .. }
                    | GateSignal::TransitionChainBroken { .. }
                    | GateSignal::TransitionCatalogChanged
            )
        })
}

fn publish_authorized_transition(
    write: &WriteTransaction,
    gate: &mut GateRecord,
    revision: u64,
    snapshot: Option<&AnalysisSnapshot>,
    reconciled_baseline: Option<&AnalysisSnapshot>,
    changed_paths: &[RepoPathProjection],
    alias_closures: &[PhysicalAliasClosureRecord],
) -> Result<(), StoreError> {
    let (Some(before_snapshot), Some(after_snapshot)) = (reconciled_baseline, snapshot) else {
        return Err(StoreError::Integrity(
            "authorizing post-write omitted its sealed transition snapshots".to_owned(),
        ));
    };
    let sequence = next_transition_sequence(write)?;
    let gate_id = gate.gate_id.clone();
    let transition = WorktreeTransition {
        sequence,
        capsule: TransitionCapsule {
            gate_id: gate_id.clone(),
            revision,
            before_snapshot: before_snapshot.clone(),
            after_snapshot: after_snapshot.clone(),
            changed_paths: changed_paths.to_vec(),
            leased_write_set: gate.leased_write_set.clone(),
        },
    };
    write_record(write, TRANSITIONS, &transition_key(sequence), &transition)?;
    attach_transition_references(write, &gate_id, sequence)?;
    gate.lifecycle = GateLifecycle::Closed;
    gate.transition_refs.clear();
    gate.alias_closures = alias_closures.to_vec();
    Ok(())
}

fn persist_operation_result(
    write: &WriteTransaction,
    gate: &GateRecord,
    operation: &mut OperationRecord,
    result: &GateOperationResult,
) -> Result<(), StoreError> {
    operation.status = GateOperationStatus::Committed;
    operation.result = Some(result.clone());
    write_record(write, GATES, gate.gate_id.as_str(), gate)?;
    write_record(
        write,
        OPERATIONS,
        operation.operation_id.as_str(),
        operation,
    )
}

fn rejected_open_result(
    operation: &OperationRecord,
    signals: &[GateSignal],
) -> GateOperationResult {
    GateOperationResult {
        operation_id: operation.operation_id.clone(),
        request_digest: operation.request_digest.clone(),
        gate_id: operation.gate_id.clone(),
        revision: 0,
        lifecycle: GateLifecycle::Rejected,
        decision: gate_policy::decision(signals),
        signals: signals.to_vec(),
        leased_write_set: operation.leased_write_set.clone(),
        deltas: Vec::new(),
    }
}

fn rejected_gate(
    operation: &OperationRecord,
    analysis_options: GateAnalysisOptions,
    signals: &[GateSignal],
    baseline: Option<GateBaseline>,
) -> GateRecord {
    let decision = gate_policy::decision(signals);
    let protected_semantic_inputs = baseline.as_ref().map_or_else(Vec::new, |baseline| {
        baseline.protected_semantic_inputs.clone()
    });
    GateRecord {
        schema_version: "lumin-gate.v1".to_owned(),
        gate_id: operation.gate_id.clone(),
        lifecycle: GateLifecycle::Rejected,
        current_revision: 0,
        declared_write_set: operation.declared_write_set.clone(),
        leased_write_set: operation.leased_write_set.clone(),
        alias_closures: Vec::new(),
        transition_refs: Vec::new(),
        analysis_options,
        baseline,
        protected_semantic_inputs: protected_semantic_inputs.clone(),
        revisions: vec![GateRevision {
            revision: 0,
            operation_id: operation.operation_id.clone(),
            decision,
            signals: signals.to_vec(),
            changed_paths: Vec::new(),
            snapshot: None,
            protected_semantic_inputs,
            alias_closures: Vec::new(),
            reconciled_transition_sequences: Vec::new(),
            deltas: Vec::new(),
        }],
    }
}

fn validate_operation(
    operation: &OperationRecord,
    kind: GateOperationKind,
    request_digest: &str,
    gate_id: Option<&GateId>,
) -> Result<(), StoreError> {
    if operation.kind != kind
        || operation.request_digest != request_digest
        || gate_id.is_some_and(|gate_id| operation.gate_id != *gate_id)
    {
        return Err(StoreError::OperationConflict(
            operation.operation_id.as_str().to_owned(),
        ));
    }
    validate_reservation_binding_set(operation)?;
    Ok(())
}

fn validate_reservation_binding_set(operation: &OperationRecord) -> Result<(), StoreError> {
    if operation.status == GateOperationStatus::Committed
        && operation.semantic_read_reservation_bindings.is_empty()
    {
        return Ok(());
    }
    let mut bindings = operation.semantic_read_reservation_bindings.clone();
    bindings.sort();
    for pair in bindings.windows(2) {
        if pair[0].path == pair[1].path && pair[0] != pair[1] {
            return Err(StoreError::Integrity(format!(
                "semantic-read reservation has conflicting physical identities: {}",
                pair[0].path.display
            )));
        }
    }
    let mut bound_paths = operation
        .semantic_read_reservation_bindings
        .iter()
        .map(|binding| binding.path.clone())
        .collect::<Vec<_>>();
    bound_paths.sort();
    bound_paths.dedup();
    if bound_paths != operation.semantic_read_reservations {
        return Err(StoreError::Integrity(format!(
            "pending semantic-read reservation bindings disagree with paths: {}",
            operation.operation_id.as_str()
        )));
    }
    Ok(())
}

fn validate_captured_reservations(
    operation: &OperationRecord,
    inputs: &[SemanticInputRecord],
    phase: &str,
) -> Result<(), StoreError> {
    for binding in &operation.semantic_read_reservation_bindings {
        let captured = inputs
            .iter()
            .find(|input| input.path == binding.path)
            .ok_or_else(|| {
                StoreError::Integrity(format!(
                    "{phase} omitted reserved semantic input: {}",
                    binding.path.display
                ))
            })?;
        if captured.physical_identity != binding.physical_identity {
            return Err(StoreError::Integrity(format!(
                "{phase} physical identity disagrees with reservation: {}",
                binding.path.display
            )));
        }
    }
    Ok(())
}
