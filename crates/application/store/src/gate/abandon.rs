use lumin_evidence::GateDecision;

use super::*;

impl OperationSession<'_> {
    pub fn abandon_gate(
        &self,
        request_digest: &str,
        gate_id: &GateId,
        target_revision: u64,
        reason: &str,
    ) -> Result<GateOperationResult, StoreError> {
        let operation_id = &self.operation_id;
        self.store.with_exclusive_lock(|| {
            let database = open_lifecycle_database(&self.store.state_dir)?;
            let write = database.begin_write().map_err(backend_error)?;
            let mut operation = load_or_create_abandon_operation(
                &write,
                operation_id,
                request_digest,
                gate_id,
                target_revision,
                reason,
            )?;
            if let Some(result) = operation.result.clone() {
                return Ok(result);
            }

            let mut gate = load_abandon_target(&write, gate_id, target_revision)?;
            ensure_post_write_revision_available(&write, operation_id, &gate)?;
            if operation.status == GateOperationStatus::Pending {
                self.validate_pending_operation(&operation)?;
            } else {
                self.bind_pending_operation(&mut operation)?;
            }

            let result = apply_abandon(&mut gate, &operation, reason)?;
            persist_operation_result(&write, &gate, &mut operation, &result)?;
            write.commit().map_err(backend_error)?;
            Ok(result)
        })
    }
}

fn load_or_create_abandon_operation(
    write: &WriteTransaction,
    operation_id: &OperationId,
    request_digest: &str,
    gate_id: &GateId,
    target_revision: u64,
    reason: &str,
) -> Result<OperationRecord, StoreError> {
    let Some(operation) = read_record::<OperationRecord>(write, OPERATIONS, operation_id.as_str())?
    else {
        return Ok(OperationRecord {
            schema_version: "lumin-operation.v1".to_owned(),
            operation_id: operation_id.clone(),
            kind: GateOperationKind::GateAbandon,
            request_digest: request_digest.to_owned(),
            status: GateOperationStatus::Interrupted,
            gate_id: gate_id.clone(),
            target_revision,
            reason: Some(reason.to_owned()),
            transition_sequence: current_transition_sequence(write)?,
            declared_write_set: Vec::new(),
            leased_write_set: Vec::new(),
            semantic_read_reservations: Vec::new(),
            semantic_read_reservation_bindings: Vec::new(),
            interruption_count: 0,
            operation_liveness: None,
            analysis_options: None,
            result: None,
        });
    };
    validate_operation(
        &operation,
        GateOperationKind::GateAbandon,
        request_digest,
        Some(gate_id),
    )?;
    if operation.target_revision != target_revision || operation.reason.as_deref() != Some(reason) {
        return Err(StoreError::OperationConflict(
            operation_id.as_str().to_owned(),
        ));
    }
    Ok(operation)
}

fn apply_abandon(
    gate: &mut GateRecord,
    operation: &OperationRecord,
    reason: &str,
) -> Result<GateOperationResult, StoreError> {
    let revision = gate
        .current_revision
        .checked_add(1)
        .ok_or_else(|| StoreError::Integrity("gate revision overflow".to_owned()))?;
    gate.lifecycle = GateLifecycle::Abandoned;
    gate.current_revision = revision;
    gate.leased_write_set.clear();
    gate.alias_closures.clear();
    gate.transition_refs.clear();
    gate.protected_semantic_inputs.clear();

    let reason = reason.to_owned();
    gate.revisions.push(GateRevision {
        revision,
        operation_id: operation.operation_id.clone(),
        decision: GateDecision::Allow,
        reason: Some(reason.clone()),
        signals: Vec::new(),
        changed_paths: Vec::new(),
        snapshot: None,
        protected_semantic_inputs: Vec::new(),
        alias_closures: Vec::new(),
        reconciled_transition_sequences: Vec::new(),
        deltas: Vec::new(),
    });
    Ok(GateOperationResult {
        operation_id: operation.operation_id.clone(),
        request_digest: operation.request_digest.clone(),
        gate_id: gate.gate_id.clone(),
        revision,
        lifecycle: GateLifecycle::Abandoned,
        decision: GateDecision::Allow,
        reason: Some(reason),
        signals: Vec::new(),
        leased_write_set: Vec::new(),
        deltas: Vec::new(),
    })
}

fn load_abandon_target(
    write: &WriteTransaction,
    gate_id: &GateId,
    target_revision: u64,
) -> Result<GateRecord, StoreError> {
    let gate = read_record::<GateRecord>(write, GATES, gate_id.as_str())?
        .ok_or_else(|| StoreError::GateNotFound(gate_id.as_str().to_owned()))?;
    if gate.lifecycle != GateLifecycle::Active {
        return Err(StoreError::GateNotActive(gate_id.as_str().to_owned()));
    }
    if gate.current_revision != target_revision {
        return Err(StoreError::GateRevisionChanged(format!(
            "{}: expected {}, observed {}",
            gate_id.as_str(),
            target_revision,
            gate.current_revision
        )));
    }
    Ok(gate)
}
