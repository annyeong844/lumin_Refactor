use lumin_evidence::{
    ActualWriteSet, AnalysisSnapshot, GateAnalysisOptions, GateBaseline, GateLifecycle,
    GateOperationKind, GateOperationResult, GateOperationStatus, GateRecord, GateRevision,
    GateSignal, OperationRecord, PhysicalAliasClosureRecord, RepoPathProjection,
    SemanticInputRecord, SemanticReadReservationBinding, TransitionCapsule, WorktreeTransition,
    WriteLease, gate_policy,
};
use lumin_model::{GateDeltaRecord, GateId, OperationId};
use redb::{ReadableTable, TableDefinition, WriteTransaction};

use super::{RepositoryStore, StoreError};

mod abandon;
mod coordination;
mod liveness;
mod operations;
pub(crate) mod records;
#[cfg(test)]
mod tests;

use coordination::{
    active_write_conflicts, attach_transition_references, conflicts, post_write_analysis_context,
    semantic_read_conflicts, transition_sequences_for_gate,
};
pub use liveness::OperationSession;
use records::{
    current_transition_sequence, load_record, next_gate_id, next_transition_sequence, read_record,
    read_records, write_record,
};

pub(crate) const GATES: TableDefinition<&str, &[u8]> = TableDefinition::new("gates");
pub(crate) const OPERATIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("operations");
pub(crate) const TRANSITIONS: TableDefinition<&str, &[u8]> =
    TableDefinition::new("worktree-transitions");

pub(crate) use records::transition_key;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveGateLease {
    pub gate_id: GateId,
    pub leased_write_set: Vec<WriteLease>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveGateCatalogCursor {
    pub repository_id: lumin_model::RepositoryId,
    pub revision: u64,
    pub page_size: usize,
    pub opening_sequence: u64,
    pub gate_id: GateId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveGateCatalogItem {
    pub gate_id: GateId,
    pub current_revision: u64,
    pub opening_transition_sequence: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveGateCatalogSnapshot {
    pub repository_id: lumin_model::RepositoryId,
    pub revision: u64,
    pub scope_total: usize,
    pub total: usize,
    pub items: Vec<ActiveGateCatalogItem>,
    pub truncated: bool,
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
    pub actual_write_set: Option<ActualWriteSet>,
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
    Committed(Box<GateOperationResult>),
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
    Committed(Box<GateOperationResult>),
}

impl RepositoryStore {
    /// Return an exact committed pre-write result without allocating or joining
    /// operation liveness. Current filesystem validation belongs only to a new
    /// or interrupted attempt, not to delivery recovery for a committed result.
    pub fn replay_pre_write_result(
        &self,
        operation_id: &OperationId,
        request_digest: &str,
    ) -> Result<Option<GateOperationResult>, StoreError> {
        self.with_shared_lock(|guard| {
            let database = guard.open_database()?;
            let Some(operation) =
                load_record::<OperationRecord>(&database, OPERATIONS, operation_id.as_str())?
            else {
                return Ok(None);
            };
            validate_operation(
                &operation,
                GateOperationKind::PreWrite,
                request_digest,
                None,
            )?;
            Ok(operation.result)
        })
    }

    pub fn load_gate(&self, gate_id: &GateId) -> Result<GateRecord, StoreError> {
        self.with_shared_lock(|guard| {
            let database = guard.open_database()?;
            load_record::<GateRecord>(&database, GATES, gate_id.as_str())?
                .ok_or_else(|| StoreError::GateNotFound(gate_id.as_str().to_owned()))
        })
    }

    pub fn lookup_gate(
        &self,
        gate_id: &GateId,
    ) -> Result<lumin_evidence::RecordLookup<GateRecord>, StoreError> {
        self.with_shared_lock(|guard| {
            let database = guard.open_database()?;
            let read = database.begin_read()?;
            if let Some(tombstone) = crate::retention::records::read_validated_tombstone(
                &read,
                lumin_evidence::RetentionItemKind::Gate,
                gate_id.as_str(),
            )? {
                return if tombstone.envelope.tombstone_identity.is_some() {
                    Ok(lumin_evidence::RecordLookup::Pruned(tombstone.envelope))
                } else {
                    Ok(lumin_evidence::RecordLookup::Pruning(tombstone.envelope))
                };
            }
            drop(read);
            let gate = load_record::<GateRecord>(&database, GATES, gate_id.as_str())?
                .ok_or_else(|| StoreError::GateNotFound(gate_id.as_str().to_owned()))?;
            Ok(lumin_evidence::RecordLookup::Live(gate))
        })
    }

    pub fn load_operation(
        &self,
        operation_id: &OperationId,
    ) -> Result<OperationRecord, StoreError> {
        self.recover_interrupted_operations(None)?;
        self.with_shared_lock(|guard| {
            let database = guard.open_database()?;
            load_record::<OperationRecord>(&database, OPERATIONS, operation_id.as_str())?
                .ok_or_else(|| StoreError::OperationNotFound(operation_id.as_str().to_owned()))
        })
    }

    pub fn list_active_gates(
        &self,
        cursor: Option<&ActiveGateCatalogCursor>,
        limit: usize,
    ) -> Result<ActiveGateCatalogSnapshot, StoreError> {
        if !(1..=100).contains(&limit) {
            return Err(StoreError::ActiveGateCatalogPageSize {
                requested: limit,
                max: 100,
            });
        }
        self.with_shared_lock(|guard| {
            let repository_id = guard.repository_id().clone();
            let database = guard.open_database()?;
            let read = database.begin_read()?;
            // Read active-gate-catalog revision
            let revision = {
                match read.open_table(crate::SEQUENCES) {
                    Ok(table) => table
                        .get(records::ACTIVE_GATE_CATALOG_SEQUENCE_KEY)
                        .map_err(super::backend_error)?
                        .map_or(0, |value| value.value()),
                    Err(redb::TableError::TableDoesNotExist(_)) => 0,
                    Err(error) => return Err(super::backend_error(error)),
                }
            };
            // Validate cursor
            if let Some(cursor) = cursor {
                if cursor.repository_id != repository_id || cursor.page_size != limit {
                    return Err(StoreError::ActiveGateCatalogScopeMismatch);
                }
                if cursor.revision != revision {
                    return Err(StoreError::ActiveGateCatalogRevisionChanged {
                        expected: cursor.revision,
                        observed: revision,
                    });
                }
            }
            // Read all gates
            let table = match read.open_table(GATES) {
                Ok(table) => table,
                Err(redb::TableError::TableDoesNotExist(_)) => {
                    if let Some(cursor) = cursor {
                        return Err(StoreError::ActiveGateCatalogAnchorMissing(
                            cursor.gate_id.as_str().to_owned(),
                        ));
                    }
                    return Ok(ActiveGateCatalogSnapshot {
                        repository_id,
                        revision,
                        scope_total: 0,
                        total: 0,
                        items: Vec::new(),
                        truncated: false,
                    });
                }
                Err(error) => return Err(super::backend_error(error)),
            };
            // Check for tombstones
            let tombstones = match read.open_table(crate::retention::RETENTION_TOMBSTONES) {
                Ok(t) => Some(t),
                Err(redb::TableError::TableDoesNotExist(_)) => None,
                Err(error) => return Err(super::backend_error(error)),
            };
            let mut active_items = Vec::new();
            for row in table.iter().map_err(super::backend_error)? {
                let (key, value) = row.map_err(super::backend_error)?;
                let key_str = key.value();
                // Check tombstone
                if let Some(ref tombstones) = tombstones {
                    let tombstone_key = crate::retention::records::tombstone_key(
                        lumin_evidence::RetentionItemKind::Gate,
                        key_str,
                    );
                    if tombstones
                        .get(tombstone_key.as_str())
                        .map_err(super::backend_error)?
                        .is_some()
                    {
                        // active+tombstone is integrity error
                        let gate: GateRecord = serde_json::from_slice(value.value())
                            .map_err(super::serialization_error)?;
                        if gate.lifecycle == GateLifecycle::Active {
                            return Err(StoreError::Integrity(format!(
                                "active gate {} has a tombstone",
                                key_str
                            )));
                        }
                        continue;
                    }
                }
                let gate: GateRecord =
                    serde_json::from_slice(value.value()).map_err(super::serialization_error)?;
                // Validate key==gate_id
                if gate.gate_id.as_str() != key_str {
                    return Err(StoreError::Integrity(format!(
                        "gate key {key_str} disagrees with gate_id {}",
                        gate.gate_id.as_str()
                    )));
                }
                // Terminal ignored
                if gate.lifecycle != GateLifecycle::Active {
                    continue;
                }
                // Active requires baseline
                let baseline = gate.baseline.as_ref().ok_or_else(|| {
                    StoreError::Integrity(format!(
                        "active gate {} omitted its baseline",
                        gate.gate_id.as_str()
                    ))
                })?;
                active_items.push(ActiveGateCatalogItem {
                    gate_id: gate.gate_id.clone(),
                    current_revision: gate.current_revision,
                    opening_transition_sequence: baseline.transition_sequence,
                });
            }
            // Sort: baseline.transition_sequence ASC then gate_id ASC
            active_items.sort_by(|a, b| {
                a.opening_transition_sequence
                    .cmp(&b.opening_transition_sequence)
                    .then_with(|| a.gate_id.as_str().cmp(b.gate_id.as_str()))
            });
            let scope_total = active_items.len();
            let total = active_items.len();
            // Find cursor anchor in sorted vec
            let start = if let Some(cursor) = cursor {
                let anchor_index = active_items
                    .iter()
                    .position(|item| {
                        item.opening_transition_sequence == cursor.opening_sequence
                            && item.gate_id == cursor.gate_id
                    })
                    .ok_or_else(|| {
                        StoreError::ActiveGateCatalogAnchorMissing(
                            cursor.gate_id.as_str().to_owned(),
                        )
                    })?;
                let resume_offset = anchor_index + 1;
                // Validate canonical nonterminal boundary
                if !resume_offset.is_multiple_of(limit) || resume_offset >= total {
                    return Err(StoreError::ActiveGateCatalogAnchorMissing(
                        cursor.gate_id.as_str().to_owned(),
                    ));
                }
                resume_offset
            } else {
                0
            };
            let end = start.saturating_add(limit).min(total);
            let items = active_items[start..end].to_vec();
            let truncated = end < total;
            Ok(ActiveGateCatalogSnapshot {
                repository_id,
                revision,
                scope_total,
                total,
                items,
                truncated,
            })
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
    reject_retention_operation_collision(write, operation_id)?;
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

fn reject_retention_operation_collision(
    write: &WriteTransaction,
    operation_id: &OperationId,
) -> Result<(), StoreError> {
    if read_record::<lumin_evidence::RetentionOperationRecord>(
        write,
        crate::retention::RETENTION_OPERATIONS,
        operation_id.as_str(),
    )?
    .is_some()
    {
        return Err(StoreError::OperationConflict(
            operation_id.as_str().to_owned(),
        ));
    }
    Ok(())
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
        reason: None,
        signals: signals.clone(),
        leased_write_set: leased_write_set.clone(),
        actual_write_set: None,
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
            committed_unix_millis: Some(crate::unix_millis()?),
            decision,
            reason: None,
            signals,
            changed_paths: Vec::new(),
            actual_write_set: None,
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

fn load_active_gate_for_retry(
    write: &WriteTransaction,
    gate_id: &GateId,
) -> Result<GateRecord, StoreError> {
    let gate = read_record::<GateRecord>(write, GATES, gate_id.as_str())?
        .ok_or_else(|| StoreError::GateNotFound(gate_id.as_str().to_owned()))?;
    if gate.lifecycle != GateLifecycle::Active {
        return Err(StoreError::GateNotActive(gate_id.as_str().to_owned()));
    }
    Ok(gate)
}

fn ensure_post_write_revision_available(
    write: &WriteTransaction,
    own_operation_id: &OperationId,
    gate: &GateRecord,
) -> Result<(), StoreError> {
    for operation in read_records::<OperationRecord>(write, OPERATIONS)? {
        if operation.operation_id != *own_operation_id
            && operation.status == GateOperationStatus::Pending
            && operation.kind == GateOperationKind::PostWrite
            && operation.gate_id == gate.gate_id
            && operation.target_revision == gate.current_revision
        {
            return Err(StoreError::GateRevisionBusy(format!(
                "{}@{}",
                gate.gate_id.as_str(),
                gate.current_revision
            )));
        }
    }
    Ok(())
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
    operation.semantic_read_reservations.clear();
    operation.semantic_read_reservation_bindings.clear();
    operation.operation_liveness = None;
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
        reason: None,
        signals: signals.to_vec(),
        leased_write_set: operation.leased_write_set.clone(),
        actual_write_set: None,
        deltas: Vec::new(),
    }
}

fn rejected_gate(
    operation: &OperationRecord,
    analysis_options: GateAnalysisOptions,
    signals: &[GateSignal],
    baseline: Option<GateBaseline>,
) -> Result<GateRecord, StoreError> {
    let decision = gate_policy::decision(signals);
    let protected_semantic_inputs = baseline.as_ref().map_or_else(Vec::new, |baseline| {
        baseline.protected_semantic_inputs.clone()
    });
    Ok(GateRecord {
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
            committed_unix_millis: Some(crate::unix_millis()?),
            decision,
            reason: None,
            signals: signals.to_vec(),
            changed_paths: Vec::new(),
            actual_write_set: None,
            snapshot: None,
            protected_semantic_inputs,
            alias_closures: Vec::new(),
            reconciled_transition_sequences: Vec::new(),
            deltas: Vec::new(),
        }],
    })
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
    match operation.status {
        GateOperationStatus::Pending if operation.result.is_some() => {
            return Err(StoreError::Integrity(format!(
                "pending operation already contains a result: {}",
                operation.operation_id.as_str()
            )));
        }
        GateOperationStatus::Interrupted
            if operation.result.is_some()
                || operation.operation_liveness.is_some()
                || !operation.leased_write_set.is_empty()
                || !operation.semantic_read_reservations.is_empty()
                || !operation.semantic_read_reservation_bindings.is_empty() =>
        {
            return Err(StoreError::Integrity(format!(
                "interrupted operation retained provisional state: {}",
                operation.operation_id.as_str()
            )));
        }
        GateOperationStatus::Committed if operation.result.is_none() => {
            return Err(StoreError::Integrity(format!(
                "committed operation omitted its result: {}",
                operation.operation_id.as_str()
            )));
        }
        GateOperationStatus::Committed if operation.operation_liveness.is_some() => {
            return Err(StoreError::Integrity(format!(
                "committed operation retained a liveness binding: {}",
                operation.operation_id.as_str()
            )));
        }
        _ => {}
    }
    validate_reservation_binding_set(operation)?;
    Ok(())
}

fn validate_reservation_binding_set(operation: &OperationRecord) -> Result<(), StoreError> {
    if operation.status != GateOperationStatus::Pending
        && operation.semantic_read_reservation_bindings.is_empty()
    {
        return Ok(());
    }
    let mut bindings = operation.semantic_read_reservation_bindings.clone();
    bindings.sort();
    for binding in &bindings {
        if binding.physical_identity.is_some() && binding.absence_parent.is_some() {
            return Err(StoreError::Integrity(format!(
                "semantic-read reservation has both direct and absence identities: {}",
                binding.path.display
            )));
        }
        if let Some(parent) = &binding.absence_parent
            && !binding.path.components.starts_with(&parent.path.components)
        {
            return Err(StoreError::Integrity(format!(
                "semantic-read absence parent is outside its demanded path: {}",
                binding.path.display
            )));
        }
    }
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
        if captured.absence_parent != binding.absence_parent {
            return Err(StoreError::Integrity(format!(
                "{phase} absence parent disagrees with reservation: {}",
                binding.path.display
            )));
        }
    }
    Ok(())
}
