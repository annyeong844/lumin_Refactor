use lumin_evidence::{
    AnalysisSnapshot, GateAnalysisOptions, GateBaseline, GateLifecycle, GateOperationKind,
    GateOperationResult, GateOperationStatus, GateRecord, GateRevision, GateSignal,
    OperationRecord, RepoPathProjection, SemanticInputRecord, gate_policy,
};
use lumin_model::{GateId, OperationId};
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PreWriteStart {
    Analyze { gate_id: GateId },
    Committed(GateOperationResult),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PostWriteStart {
    Analyze { gate: Box<GateRecord> },
    Committed(GateOperationResult),
}

impl RepositoryStore {
    pub fn reserve_pre_write(
        &self,
        operation_id: &OperationId,
        request_digest: &str,
        declared_write_set: &[RepoPathProjection],
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
                });
            }

            let gate_id = next_gate_id(&write)?;
            let (paths, gate_ids) = conflicts(&write, operation_id, declared_write_set, &[])?;
            let mut operation = OperationRecord {
                schema_version: "lumin-operation.v1".to_owned(),
                operation_id: operation_id.clone(),
                kind: GateOperationKind::PreWrite,
                request_digest: request_digest.to_owned(),
                status: GateOperationStatus::Pending,
                gate_id: gate_id.clone(),
                target_revision: 0,
                declared_write_set: declared_write_set.to_vec(),
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
            Ok(PreWriteStart::Analyze { gate_id })
        })
    }

    pub fn finish_pre_write(
        &self,
        operation_id: &OperationId,
        request_digest: &str,
        gate_id: &GateId,
        baseline: Option<GateBaseline>,
        mut signals: Vec<GateSignal>,
    ) -> Result<GateOperationResult, StoreError> {
        self.with_exclusive_lock(|| {
            let database = open_lifecycle_database(&self.state_dir)?;
            let write = database.begin_write().map_err(backend_error)?;
            let mut operation =
                read_record::<OperationRecord>(&write, OPERATIONS, operation_id.as_str())?
                    .ok_or_else(|| {
                        StoreError::Integrity(format!(
                            "pending pre-write operation disappeared: {}",
                            operation_id.as_str()
                        ))
                    })?;
            validate_operation(
                &operation,
                GateOperationKind::PreWrite,
                request_digest,
                Some(gate_id),
            )?;
            if let Some(result) = operation.result {
                return Ok(result);
            }

            let semantic_inputs = baseline
                .as_ref()
                .map_or(&[][..], |baseline| baseline.snapshot.inputs.as_slice());
            let (paths, gate_ids) = conflicts(
                &write,
                operation_id,
                &operation.declared_write_set,
                semantic_inputs,
            )?;
            if !paths.is_empty() {
                signals.push(GateSignal::WriteConflict { paths, gate_ids });
            }
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
                operation_id: operation_id.clone(),
                request_digest: request_digest.to_owned(),
                gate_id: gate_id.clone(),
                revision: 0,
                lifecycle,
                decision,
                signals: signals.clone(),
            };
            let analysis_options = operation.analysis_options.clone().ok_or_else(|| {
                StoreError::Integrity("pre-write operation omitted analysis options".to_owned())
            })?;
            let gate = GateRecord {
                schema_version: "lumin-gate.v1".to_owned(),
                gate_id: gate_id.clone(),
                lifecycle,
                current_revision: 0,
                declared_write_set: operation.declared_write_set.clone(),
                analysis_options,
                baseline,
                revisions: vec![GateRevision {
                    revision: 0,
                    operation_id: operation_id.clone(),
                    decision,
                    signals,
                    changed_paths: Vec::new(),
                    snapshot: None,
                }],
            };
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
                return Ok(PostWriteStart::Analyze {
                    gate: Box::new(gate),
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
                declared_write_set: Vec::new(),
                analysis_options: None,
                result: None,
            };
            write_record(
                &write,
                OPERATIONS,
                operation.operation_id.as_str(),
                &operation,
            )?;
            write.commit().map_err(backend_error)?;
            Ok(PostWriteStart::Analyze {
                gate: Box::new(gate),
            })
        })
    }

    pub fn finish_post_write(
        &self,
        operation_id: &OperationId,
        request_digest: &str,
        gate_id: &GateId,
        snapshot: Option<AnalysisSnapshot>,
        changed_paths: Vec<RepoPathProjection>,
        signals: Vec<GateSignal>,
    ) -> Result<GateOperationResult, StoreError> {
        self.with_exclusive_lock(|| {
            let database = open_lifecycle_database(&self.state_dir)?;
            let write = database.begin_write().map_err(backend_error)?;
            let mut operation =
                read_record::<OperationRecord>(&write, OPERATIONS, operation_id.as_str())?
                    .ok_or_else(|| {
                        StoreError::Integrity(format!(
                            "pending post-write operation disappeared: {}",
                            operation_id.as_str()
                        ))
                    })?;
            validate_operation(
                &operation,
                GateOperationKind::PostWrite,
                request_digest,
                Some(gate_id),
            )?;
            if let Some(result) = operation.result {
                return Ok(result);
            }
            let mut gate = read_record::<GateRecord>(&write, GATES, gate_id.as_str())?
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

            let decision = gate_policy::decision(&signals);
            let revision = gate
                .current_revision
                .checked_add(1)
                .ok_or_else(|| StoreError::Integrity("gate revision overflow".to_owned()))?;
            if decision.authorizes() && snapshot.is_none() {
                return Err(StoreError::Integrity(
                    "authorizing post-write omitted its sealed snapshot".to_owned(),
                ));
            }
            if decision.authorizes() {
                gate.lifecycle = GateLifecycle::Closed;
            }
            gate.current_revision = revision;
            gate.revisions.push(GateRevision {
                revision,
                operation_id: operation_id.clone(),
                decision,
                signals: signals.clone(),
                changed_paths,
                snapshot,
            });
            let result = GateOperationResult {
                operation_id: operation_id.clone(),
                request_digest: request_digest.to_owned(),
                gate_id: gate_id.clone(),
                revision,
                lifecycle: gate.lifecycle,
                decision,
                signals,
            };
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
    }
}

fn rejected_gate(
    operation: &OperationRecord,
    analysis_options: GateAnalysisOptions,
    signals: &[GateSignal],
    baseline: Option<GateBaseline>,
) -> GateRecord {
    let decision = gate_policy::decision(signals);
    GateRecord {
        schema_version: "lumin-gate.v1".to_owned(),
        gate_id: operation.gate_id.clone(),
        lifecycle: GateLifecycle::Rejected,
        current_revision: 0,
        declared_write_set: operation.declared_write_set.clone(),
        analysis_options,
        baseline,
        revisions: vec![GateRevision {
            revision: 0,
            operation_id: operation.operation_id.clone(),
            decision,
            signals: signals.to_vec(),
            changed_paths: Vec::new(),
            snapshot: None,
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
    Ok(())
}

fn conflicts(
    write: &WriteTransaction,
    own_operation_id: &OperationId,
    declared_write_set: &[RepoPathProjection],
    semantic_inputs: &[SemanticInputRecord],
) -> Result<(Vec<RepoPathProjection>, Vec<GateId>), StoreError> {
    let mut paths = Vec::new();
    let mut gate_ids = Vec::new();
    for operation in read_records::<OperationRecord>(write, OPERATIONS)? {
        if operation.operation_id == *own_operation_id
            || operation.status != GateOperationStatus::Pending
            || operation.kind != GateOperationKind::PreWrite
        {
            continue;
        }
        for path in declared_write_set {
            if contains_path(&operation.declared_write_set, path) {
                paths.push(path.clone());
                gate_ids.push(operation.gate_id.clone());
            }
        }
        for input in semantic_inputs {
            if contains_path(&operation.declared_write_set, &input.path) {
                paths.push(input.path.clone());
                gate_ids.push(operation.gate_id.clone());
            }
        }
    }
    for gate in read_records::<GateRecord>(write, GATES)? {
        if gate.lifecycle != GateLifecycle::Active {
            continue;
        }
        let protected_inputs = gate
            .baseline
            .as_ref()
            .map_or(&[][..], |baseline| baseline.snapshot.inputs.as_slice());
        for path in declared_write_set {
            if contains_path(&gate.declared_write_set, path)
                || protected_inputs.iter().any(|input| input.path == *path)
            {
                paths.push(path.clone());
                gate_ids.push(gate.gate_id.clone());
            }
        }
        for input in semantic_inputs {
            if contains_path(&gate.declared_write_set, &input.path) {
                paths.push(input.path.clone());
                gate_ids.push(gate.gate_id.clone());
            }
        }
    }
    paths.sort();
    paths.dedup();
    gate_ids.sort();
    gate_ids.dedup();
    Ok((paths, gate_ids))
}

fn contains_path(paths: &[RepoPathProjection], candidate: &RepoPathProjection) -> bool {
    paths
        .iter()
        .any(|path| path.canonical == candidate.canonical)
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
