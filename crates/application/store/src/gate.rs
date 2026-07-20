use lumin_evidence::{
    AnalysisSnapshot, GateAnalysisOptions, GateBaseline, GateLifecycle, GateOperationKind,
    GateOperationResult, GateOperationStatus, GateRecord, GateRevision, GateSignal,
    OperationRecord, PhysicalAliasClosureRecord, RepoPathProjection, SemanticInputRecord,
    SemanticReadReservationBinding, TransitionCapsule, WorktreeTransition, WriteLease, gate_policy,
};
use lumin_model::{GateDeltaRecord, GateId, OperationId};
use redb::{
    Database, ReadableDatabase, ReadableTable, TableDefinition, TableError, WriteTransaction,
};
use serde::{Serialize, de::DeserializeOwned};

use super::{
    RepositoryStore, SEQUENCES, StoreError, backend_error, open_lifecycle_database,
    serialization_error,
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

fn conflicts(
    write: &WriteTransaction,
    own_operation_id: &OperationId,
    leased_write_set: &[WriteLease],
    semantic_inputs: &[SemanticInputRecord],
    own_gate_id: Option<&GateId>,
) -> Result<(Vec<RepoPathProjection>, Vec<GateId>), StoreError> {
    let mut paths = Vec::new();
    let mut gate_ids = Vec::new();
    for operation in read_records::<OperationRecord>(write, OPERATIONS)? {
        if operation.operation_id == *own_operation_id
            || operation.status != GateOperationStatus::Pending
        {
            continue;
        }
        validate_reservation_binding_set(&operation)?;
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
    for gate in read_records::<GateRecord>(write, GATES)? {
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
                lease.conflicts_with_semantic_read(&input.path, input.physical_identity.as_ref())
            })
        {
            paths.push(lease.path.clone());
            gate_ids.push(existing_gate_id.clone());
        }
    }
    for input in candidate_inputs {
        if existing_leases.iter().any(|lease| {
            lease.conflicts_with_semantic_read(&input.path, input.physical_identity.as_ref())
        }) {
            paths.push(input.path.clone());
            gate_ids.push(existing_gate_id.clone());
        }
    }
}

fn post_write_analysis_context(
    write: &WriteTransaction,
    gate: &GateRecord,
    transition_sequence: u64,
) -> Result<(Vec<WorktreeTransition>, Vec<ActiveGateLease>), StoreError> {
    let sequences = transition_sequences_for_gate(write, gate, transition_sequence)?;
    let mut transitions = Vec::with_capacity(sequences.len());
    for sequence in sequences {
        let transition =
            read_record::<WorktreeTransition>(write, TRANSITIONS, &transition_key(sequence))?
                .ok_or_else(|| {
                    StoreError::Integrity(format!(
                        "referenced worktree transition is missing: {sequence}"
                    ))
                })?;
        transitions.push(transition);
    }
    let mut active_gates = read_records::<GateRecord>(write, GATES)?
        .into_iter()
        .filter(|other| other.lifecycle == GateLifecycle::Active && other.gate_id != gate.gate_id)
        .map(|other| ActiveGateLease {
            gate_id: other.gate_id,
            leased_write_set: other.leased_write_set,
        })
        .collect::<Vec<_>>();
    active_gates.sort_by(|left, right| left.gate_id.cmp(&right.gate_id));
    Ok((transitions, active_gates))
}

fn transition_sequences_for_gate(
    write: &WriteTransaction,
    gate: &GateRecord,
    ceiling: u64,
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

    let mut catalog = read_records::<WorktreeTransition>(write, TRANSITIONS)?
        .into_iter()
        .filter(|transition| {
            transition.sequence > baseline_sequence && transition.sequence <= ceiling
        })
        .map(|transition| transition.sequence)
        .collect::<Vec<_>>();
    catalog.sort_unstable();
    catalog.dedup();
    if references != catalog {
        return Err(StoreError::Integrity(format!(
            "active gate transition references disagree with the catalog: {}",
            gate.gate_id.as_str()
        )));
    }
    Ok(references)
}

fn active_write_conflicts(
    write: &WriteTransaction,
    own_gate_id: &GateId,
    changed_paths: &[RepoPathProjection],
) -> Result<Option<ConflictSet>, StoreError> {
    let mut paths = Vec::new();
    let mut gate_ids = Vec::new();
    for gate in read_records::<GateRecord>(write, GATES)? {
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

fn semantic_read_conflicts(
    write: &WriteTransaction,
    own_operation_id: &OperationId,
    own_gate_id: &GateId,
    demanded_inputs: &[SemanticReadReservationBinding],
) -> Result<ConflictSet, StoreError> {
    let mut paths = Vec::new();
    let mut gate_ids = Vec::new();
    for operation in read_records::<OperationRecord>(write, OPERATIONS)? {
        if operation.operation_id == *own_operation_id
            || operation.status != GateOperationStatus::Pending
            || operation.kind != GateOperationKind::PreWrite
        {
            continue;
        }
        validate_reservation_binding_set(&operation)?;
        collect_reservation_conflicts(
            demanded_inputs,
            &operation.leased_write_set,
            &operation.gate_id,
            &mut paths,
            &mut gate_ids,
        );
    }
    for gate in read_records::<GateRecord>(write, GATES)? {
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

fn attach_transition_references(
    write: &WriteTransaction,
    originating_gate_id: &GateId,
    sequence: u64,
) -> Result<(), StoreError> {
    for mut gate in read_records::<GateRecord>(write, GATES)? {
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

fn current_transition_sequence(write: &WriteTransaction) -> Result<u64, StoreError> {
    let table = write.open_table(SEQUENCES).map_err(backend_error)?;
    table
        .get("transition")
        .map_err(backend_error)
        .map(|value| value.map_or(0, |value| value.value()))
}

fn next_transition_sequence(write: &WriteTransaction) -> Result<u64, StoreError> {
    let mut table = write.open_table(SEQUENCES).map_err(backend_error)?;
    let current = table
        .get("transition")
        .map_err(backend_error)?
        .map_or(0, |value| value.value());
    let next = current
        .checked_add(1)
        .ok_or_else(|| StoreError::Integrity("transition sequence overflow".to_owned()))?;
    table.insert("transition", next).map_err(backend_error)?;
    Ok(next)
}

fn transition_key(sequence: u64) -> String {
    format!("transition_{sequence:016x}")
}

fn next_gate_id(write: &WriteTransaction) -> Result<GateId, StoreError> {
    let next = {
        let mut table = write.open_table(SEQUENCES).map_err(backend_error)?;
        let current = table
            .get("gate")
            .map_err(backend_error)?
            .map_or(0, |value| value.value());
        let next = current
            .checked_add(1)
            .ok_or_else(|| StoreError::Integrity("gate sequence overflow".to_owned()))?;
        table.insert("gate", next).map_err(backend_error)?;
        next
    };
    Ok(GateId::from_string(format!("gate_{next:016x}")))
}

fn load_record<T: DeserializeOwned>(
    database: &Database,
    definition: TableDefinition<'static, &str, &[u8]>,
    key: &str,
) -> Result<Option<T>, StoreError> {
    let read = database.begin_read().map_err(backend_error)?;
    let table = match read.open_table(definition) {
        Ok(table) => table,
        Err(TableError::TableDoesNotExist(_)) => return Ok(None),
        Err(error) => return Err(backend_error(error)),
    };
    let bytes = table
        .get(key)
        .map_err(backend_error)?
        .map(|value| value.value().to_vec());
    bytes
        .map(|bytes| serde_json::from_slice(&bytes).map_err(serialization_error))
        .transpose()
}

fn read_record<T: DeserializeOwned>(
    write: &WriteTransaction,
    definition: TableDefinition<'static, &str, &[u8]>,
    key: &str,
) -> Result<Option<T>, StoreError> {
    let table = write.open_table(definition).map_err(backend_error)?;
    let bytes = table
        .get(key)
        .map_err(backend_error)?
        .map(|value| value.value().to_vec());
    bytes
        .map(|bytes| serde_json::from_slice(&bytes).map_err(serialization_error))
        .transpose()
}

fn read_records<T: DeserializeOwned>(
    write: &WriteTransaction,
    definition: TableDefinition<'static, &str, &[u8]>,
) -> Result<Vec<T>, StoreError> {
    let table = write.open_table(definition).map_err(backend_error)?;
    let mut records = Vec::new();
    for item in table.iter().map_err(backend_error)? {
        let (_, value) = item.map_err(backend_error)?;
        records.push(serde_json::from_slice(value.value()).map_err(serialization_error)?);
    }
    Ok(records)
}

fn write_record<T: Serialize>(
    write: &WriteTransaction,
    definition: TableDefinition<'static, &str, &[u8]>,
    key: &str,
    record: &T,
) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec(record).map_err(serialization_error)?;
    let mut table = write.open_table(definition).map_err(backend_error)?;
    table.insert(key, bytes.as_slice()).map_err(backend_error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use lumin_evidence::{
        CapabilityRecord, DEAD_CODE_CAPABILITY_ID, RunEvidence, SemanticInputState,
        seal_analysis_snapshot,
    };
    use lumin_model::{CapabilityState, RepoPath};

    use super::*;

    #[test]
    fn persisted_v1_gate_additions_default_when_absent() -> Result<(), Box<dyn std::error::Error>> {
        let operation_id = OperationId::from_string("operation-1".to_owned());
        let gate_id = GateId::from_string("gate-1".to_owned());
        let protected = SemanticInputRecord {
            path: path("config/base.json")?,
            state: SemanticInputState::ConfigPresent,
            payload_sha256: Some("baseline".to_owned()),
            physical_identity: None,
        };
        let baseline = GateBaseline {
            analysis_contract: "contract".to_owned(),
            snapshot: empty_snapshot(),
            protected_semantic_inputs: vec![protected.clone()],
            transition_sequence: 0,
        };
        let revision = GateRevision {
            revision: 0,
            operation_id: operation_id.clone(),
            decision: lumin_evidence::GateDecision::Allow,
            signals: Vec::new(),
            changed_paths: Vec::new(),
            snapshot: None,
            protected_semantic_inputs: vec![protected.clone()],
            alias_closures: Vec::new(),
            reconciled_transition_sequences: Vec::new(),
            deltas: Vec::new(),
        };
        let gate = GateRecord {
            schema_version: "lumin-gate.v1".to_owned(),
            gate_id: gate_id.clone(),
            lifecycle: GateLifecycle::Active,
            current_revision: 0,
            declared_write_set: Vec::new(),
            leased_write_set: Vec::new(),
            alias_closures: Vec::new(),
            transition_refs: Vec::new(),
            analysis_options: GateAnalysisOptions {
                jobs: 1,
                resolution_profile: None,
            },
            baseline: Some(baseline),
            protected_semantic_inputs: vec![protected],
            revisions: vec![revision],
        };
        let mut gate_json = serde_json::to_value(gate)?;
        gate_json
            .as_object_mut()
            .ok_or("gate JSON is not an object")?
            .remove("protectedSemanticInputs");
        gate_json
            .pointer_mut("/revisions/0")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or("gate revision JSON is not an object")?
            .remove("protectedSemanticInputs");
        let loaded_gate: GateRecord = serde_json::from_value(gate_json)?;
        assert_eq!(
            loaded_gate.protected_semantic_inputs,
            loaded_gate
                .baseline
                .as_ref()
                .ok_or("loaded gate baseline is missing")?
                .protected_semantic_inputs
        );
        assert!(
            loaded_gate.revisions[0]
                .protected_semantic_inputs
                .is_empty()
        );

        let operation = OperationRecord {
            schema_version: "lumin-operation.v1".to_owned(),
            operation_id,
            kind: GateOperationKind::PostWrite,
            request_digest: "digest".to_owned(),
            status: GateOperationStatus::Pending,
            gate_id,
            target_revision: 0,
            transition_sequence: 0,
            declared_write_set: Vec::new(),
            leased_write_set: Vec::new(),
            semantic_read_reservations: vec![path("config/base.json")?],
            semantic_read_reservation_bindings: Vec::new(),
            analysis_options: None,
            result: None,
        };
        let mut operation_json = serde_json::to_value(operation)?;
        let operation_object = operation_json
            .as_object_mut()
            .ok_or("operation JSON is not an object")?;
        operation_object.remove("semanticReadReservations");
        operation_object.remove("semanticReadReservationBindings");
        let loaded_operation: OperationRecord = serde_json::from_value(operation_json)?;
        assert!(loaded_operation.semantic_read_reservations.is_empty());
        assert!(
            loaded_operation
                .semantic_read_reservation_bindings
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn persisted_reservation_rejects_conflicting_physical_identities()
    -> Result<(), Box<dyn std::error::Error>> {
        let reserved_path = path("config/base.json")?;
        let operation = OperationRecord {
            schema_version: "lumin-operation.v1".to_owned(),
            operation_id: OperationId::from_string("operation-conflicting-binding".to_owned()),
            kind: GateOperationKind::PostWrite,
            request_digest: "digest".to_owned(),
            status: GateOperationStatus::Pending,
            gate_id: GateId::from_string("gate-conflicting-binding".to_owned()),
            target_revision: 1,
            transition_sequence: 0,
            declared_write_set: Vec::new(),
            leased_write_set: Vec::new(),
            semantic_read_reservations: vec![reserved_path.clone()],
            semantic_read_reservation_bindings: vec![
                reservation(
                    reserved_path.clone(),
                    Some(lumin_model::PhysicalFileIdentity::Unix {
                        device: 7,
                        inode: 11,
                    }),
                ),
                reservation(
                    reserved_path,
                    Some(lumin_model::PhysicalFileIdentity::Unix {
                        device: 7,
                        inode: 12,
                    }),
                ),
            ],
            analysis_options: None,
            result: None,
        };

        assert!(matches!(
            validate_reservation_binding_set(&operation),
            Err(StoreError::Integrity(detail))
                if detail.contains("conflicting physical identities")
        ));
        Ok(())
    }

    #[test]
    fn pre_write_semantic_read_reservation_blocks_later_write_admission()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let store = RepositoryStore::open(root.path())?;
        let reader_operation = OperationId::from_string("op-reader".to_owned());
        let reader_path = path("src/new.ts")?;
        let options = GateAnalysisOptions {
            jobs: 1,
            resolution_profile: None,
        };
        let reader_gate = match store.reserve_pre_write(
            &reader_operation,
            "reader-digest",
            std::slice::from_ref(&reader_path),
            &[lease(reader_path.clone())],
            &options,
        )? {
            PreWriteStart::Analyze { gate_id, .. } => gate_id,
            PreWriteStart::Committed(_) => {
                return Err("the reader operation was unexpectedly committed".into());
            }
        };
        let demanded = path("config/base.json")?;
        assert_eq!(
            store.reserve_pre_write_semantic_inputs(
                &reader_operation,
                "reader-digest",
                &reader_gate,
                std::slice::from_ref(&reservation(demanded.clone(), None)),
            )?,
            SemanticReadReservation::Reserved
        );

        let writer_operation = OperationId::from_string("op-writer".to_owned());
        let rejected = match store.reserve_pre_write(
            &writer_operation,
            "writer-digest",
            std::slice::from_ref(&demanded),
            &[lease(demanded.clone())],
            &options,
        )? {
            PreWriteStart::Committed(result) => result,
            PreWriteStart::Analyze { .. } => {
                return Err("a writer crossed a provisional semantic-read reservation".into());
            }
        };
        assert_eq!(rejected.decision, lumin_evidence::GateDecision::Incomplete);
        assert!(rejected.signals.iter().any(|signal| {
            matches!(
                signal,
                GateSignal::WriteConflict { paths, gate_ids }
                    if paths == std::slice::from_ref(&demanded)
                        && gate_ids == std::slice::from_ref(&reader_gate)
            )
        }));
        assert_eq!(
            store
                .load_operation(&reader_operation)?
                .semantic_read_reservations,
            vec![demanded]
        );
        Ok(())
    }

    #[test]
    fn pre_write_finish_rejects_a_baseline_that_omits_a_reserved_input()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let store = RepositoryStore::open(root.path())?;
        let operation_id = OperationId::from_string("op-open".to_owned());
        let source = path("src/new.ts")?;
        let source_lease = lease(source.clone());
        let options = GateAnalysisOptions {
            jobs: 1,
            resolution_profile: None,
        };
        let (gate_id, transition_sequence) = match store.reserve_pre_write(
            &operation_id,
            "open-digest",
            std::slice::from_ref(&source),
            std::slice::from_ref(&source_lease),
            &options,
        )? {
            PreWriteStart::Analyze {
                gate_id,
                transition_sequence,
            } => (gate_id, transition_sequence),
            PreWriteStart::Committed(_) => {
                return Err("the opening operation was unexpectedly committed".into());
            }
        };
        let demanded = path("config/base.json")?;
        assert_eq!(
            store.reserve_pre_write_semantic_inputs(
                &operation_id,
                "open-digest",
                &gate_id,
                std::slice::from_ref(&reservation(demanded.clone(), None)),
            )?,
            SemanticReadReservation::Reserved
        );

        let error = match store.finish_pre_write(
            &operation_id,
            "open-digest",
            &gate_id,
            PreWriteFinish {
                baseline: Some(GateBaseline {
                    analysis_contract: "test-contract".to_owned(),
                    snapshot: empty_snapshot(),
                    protected_semantic_inputs: Vec::new(),
                    transition_sequence,
                }),
                leased_write_set: vec![source_lease],
                alias_closures: Vec::new(),
                signals: Vec::new(),
            },
        ) {
            Ok(_) => return Err("an unbound semantic-read reservation was accepted".into()),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            StoreError::Integrity(detail)
                if detail.contains("pre-write baseline omitted reserved semantic inputs")
                    && detail.contains("config/base.json")
        ));
        Ok(())
    }

    #[test]
    fn semantic_read_reservation_blocks_later_write_admission()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let store = RepositoryStore::open(root.path())?;
        let opening_operation = OperationId::from_string("op-open".to_owned());
        let source = path("src/a.ts")?;
        let source_lease = lease(source.clone());
        let options = GateAnalysisOptions {
            jobs: 1,
            resolution_profile: None,
        };
        let (gate_id, transition_sequence) = match store.reserve_pre_write(
            &opening_operation,
            "open-digest",
            std::slice::from_ref(&source),
            std::slice::from_ref(&source_lease),
            &options,
        )? {
            PreWriteStart::Analyze {
                gate_id,
                transition_sequence,
            } => (gate_id, transition_sequence),
            PreWriteStart::Committed(_) => {
                return Err("the first gate was unexpectedly committed".into());
            }
        };
        let baseline = GateBaseline {
            analysis_contract: "test-contract".to_owned(),
            snapshot: empty_snapshot(),
            protected_semantic_inputs: Vec::new(),
            transition_sequence,
        };
        let opened = store.finish_pre_write(
            &opening_operation,
            "open-digest",
            &gate_id,
            PreWriteFinish {
                baseline: Some(baseline),
                leased_write_set: vec![source_lease],
                alias_closures: Vec::new(),
                signals: Vec::new(),
            },
        )?;
        assert!(opened.decision.authorizes());

        let close_operation = OperationId::from_string("op-close".to_owned());
        assert!(matches!(
            store.begin_post_write(&close_operation, "close-digest", &gate_id)?,
            PostWriteStart::Analyze { .. }
        ));
        let demanded = path("config/base.json")?;
        assert_eq!(
            store.reserve_post_write_semantic_inputs(
                &close_operation,
                "close-digest",
                &gate_id,
                std::slice::from_ref(&reservation(demanded.clone(), None)),
            )?,
            SemanticReadReservation::Reserved
        );

        let writer_operation = OperationId::from_string("op-writer".to_owned());
        let rejected = match store.reserve_pre_write(
            &writer_operation,
            "writer-digest",
            std::slice::from_ref(&demanded),
            &[lease(demanded.clone())],
            &options,
        )? {
            PreWriteStart::Committed(result) => result,
            PreWriteStart::Analyze { .. } => {
                return Err("a writer crossed a live semantic-read reservation".into());
            }
        };
        assert_eq!(rejected.decision, lumin_evidence::GateDecision::Incomplete);
        assert!(rejected.signals.iter().any(|signal| {
            matches!(
                signal,
                GateSignal::WriteConflict { paths, gate_ids }
                    if paths == std::slice::from_ref(&demanded)
                        && gate_ids == std::slice::from_ref(&gate_id)
            )
        }));
        Ok(())
    }

    #[test]
    fn physical_alias_writer_cannot_cross_a_pending_semantic_read_reservation()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let store = RepositoryStore::open(root.path())?;
        let options = GateAnalysisOptions {
            jobs: 1,
            resolution_profile: None,
        };
        let reader_operation = OperationId::from_string("op-alias-reader".to_owned());
        let reader_source = path("src/new.ts")?;
        let reader_gate = match store.reserve_pre_write(
            &reader_operation,
            "reader-digest",
            std::slice::from_ref(&reader_source),
            &[lease(reader_source.clone())],
            &options,
        )? {
            PreWriteStart::Analyze { gate_id, .. } => gate_id,
            PreWriteStart::Committed(_) => {
                return Err("the alias reader was unexpectedly committed".into());
            }
        };
        let read_alias = path("config/read-alias.json")?;
        let physical_identity = lumin_model::PhysicalFileIdentity::Unix {
            device: 7,
            inode: 11,
        };
        assert_eq!(
            store.reserve_pre_write_semantic_inputs(
                &reader_operation,
                "reader-digest",
                &reader_gate,
                std::slice::from_ref(&reservation(
                    read_alias.clone(),
                    Some(physical_identity.clone()),
                )),
            )?,
            SemanticReadReservation::Reserved
        );

        let write_alias = path("config/write-alias.json")?;
        let writer_operation = OperationId::from_string("op-alias-writer".to_owned());
        let rejected = match store.reserve_pre_write(
            &writer_operation,
            "writer-digest",
            std::slice::from_ref(&write_alias),
            &[lease_with_identity(write_alias.clone(), physical_identity)],
            &options,
        )? {
            PreWriteStart::Committed(result) => result,
            PreWriteStart::Analyze { .. } => {
                return Err("a physical alias crossed the semantic-read reservation".into());
            }
        };
        assert_eq!(rejected.decision, lumin_evidence::GateDecision::Incomplete);
        assert!(rejected.signals.iter().any(|signal| {
            matches!(
                signal,
                GateSignal::WriteConflict { paths, gate_ids }
                    if paths == std::slice::from_ref(&read_alias)
                        && gate_ids == std::slice::from_ref(&reader_gate)
            )
        }));
        Ok(())
    }

    fn path(value: &str) -> Result<RepoPathProjection, Box<dyn std::error::Error>> {
        Ok(RepoPathProjection::from(&RepoPath::from_portable(value)?))
    }

    fn lease(path: RepoPathProjection) -> WriteLease {
        WriteLease {
            path,
            kind: lumin_evidence::WriteLeaseKind::ExistingFile,
            physical_identity: None,
            nearest_existing_parent: None,
            prefix_identities: Vec::new(),
        }
    }

    fn lease_with_identity(
        path: RepoPathProjection,
        physical_identity: lumin_model::PhysicalFileIdentity,
    ) -> WriteLease {
        WriteLease {
            physical_identity: Some(physical_identity),
            ..lease(path)
        }
    }

    fn reservation(
        path: RepoPathProjection,
        physical_identity: Option<lumin_model::PhysicalFileIdentity>,
    ) -> SemanticReadReservationBinding {
        SemanticReadReservationBinding {
            path,
            physical_identity,
        }
    }

    fn empty_snapshot() -> AnalysisSnapshot {
        seal_analysis_snapshot(
            Vec::new(),
            RunEvidence {
                schema_version: "lumin-evidence.v1".to_owned(),
                capabilities: vec![CapabilityRecord {
                    capability_id: DEAD_CODE_CAPABILITY_ID.to_owned(),
                    state: CapabilityState::Complete,
                }],
                resolution_profiles: Vec::new(),
                findings: Vec::new(),
                limitations: Vec::new(),
            },
        )
    }
}
