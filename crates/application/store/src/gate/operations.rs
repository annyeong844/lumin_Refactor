use super::*;

impl OperationSession<'_> {
    pub fn reserve_pre_write(
        &self,
        request_digest: &str,
        declared_write_set: &[RepoPathProjection],
        initial_leases: &[WriteLease],
        analysis_options: &GateAnalysisOptions,
    ) -> Result<PreWriteStart, StoreError> {
        let operation_id = &self.operation_id;
        self.store.with_exclusive_lock(|guard| {
            let database = self.open_database(guard)?;
            let write = database.begin_write()?;
            reject_retention_operation_collision(&write, operation_id)?;
            let mut operation = if let Some(mut operation) =
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
                if operation.status == GateOperationStatus::Pending {
                    self.validate_pending_operation(&operation)?;
                    return Ok(PreWriteStart::Analyze {
                        gate_id: operation.gate_id,
                        transition_sequence: operation.transition_sequence,
                    });
                }
                operation.transition_sequence = current_transition_sequence(&write)?;
                operation.declared_write_set = declared_write_set.to_vec();
                operation.leased_write_set = initial_leases.to_vec();
                operation.semantic_read_reservations.clear();
                operation.semantic_read_reservation_bindings.clear();
                operation.analysis_options = Some(analysis_options.clone());
                self.bind_pending_operation(&mut operation)?;
                operation
            } else {
                let mut operation = OperationRecord {
                    schema_version: "lumin-operation.v1".to_owned(),
                    operation_id: operation_id.clone(),
                    kind: GateOperationKind::PreWrite,
                    request_digest: request_digest.to_owned(),
                    status: GateOperationStatus::Pending,
                    gate_id: next_gate_id(&write)?,
                    target_revision: 0,
                    reason: None,
                    transition_sequence: current_transition_sequence(&write)?,
                    declared_write_set: declared_write_set.to_vec(),
                    leased_write_set: initial_leases.to_vec(),
                    semantic_read_reservations: Vec::new(),
                    semantic_read_reservation_bindings: Vec::new(),
                    interruption_count: 0,
                    operation_liveness: None,
                    analysis_options: Some(analysis_options.clone()),
                    result: None,
                };
                self.bind_pending_operation(&mut operation)?;
                operation
            };

            let gate_id = operation.gate_id.clone();
            let transition_sequence = operation.transition_sequence;
            let (paths, gate_ids) = conflicts(&write, operation_id, initial_leases, &[], None)?;

            if !paths.is_empty() {
                let signals = vec![GateSignal::WriteConflict { paths, gate_ids }];
                let result = rejected_open_result(&operation, &signals);
                let gate = rejected_gate(&operation, analysis_options.clone(), &signals, None)?;
                operation.status = GateOperationStatus::Committed;
                operation.operation_liveness = None;
                operation.result = Some(result.clone());
                write_record(&write, GATES, gate.gate_id.as_str(), &gate)?;
                write_record(
                    &write,
                    OPERATIONS,
                    operation.operation_id.as_str(),
                    &operation,
                )?;
                guard.commit(write)?;
                return Ok(PreWriteStart::Committed(result));
            }

            write_record(
                &write,
                OPERATIONS,
                operation.operation_id.as_str(),
                &operation,
            )?;
            guard.commit(write)?;
            Ok(PreWriteStart::Analyze {
                gate_id,
                transition_sequence,
            })
        })
    }

    pub fn finish_pre_write(
        &self,
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
        let operation_id = &self.operation_id;
        self.store.with_exclusive_lock(|guard| {
            let database = self.open_database(guard)?;
            let write = database.begin_write()?;
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
            self.validate_pending_operation(&operation)?;
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
            guard.commit(write)?;
            Ok(result)
        })
    }

    pub fn begin_post_write(
        &self,
        request_digest: &str,
        gate_id: &GateId,
    ) -> Result<PostWriteStart, StoreError> {
        let operation_id = &self.operation_id;
        self.store.with_exclusive_lock(|guard| {
            let database = self.open_database(guard)?;
            let write = database.begin_write()?;
            reject_retention_operation_collision(&write, operation_id)?;
            if let Some(mut operation) =
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
                if operation.status == GateOperationStatus::Pending {
                    self.validate_pending_operation(&operation)?;
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
                let gate = load_active_gate_for_retry(&write, gate_id)?;
                ensure_post_write_revision_available(&write, operation_id, &gate)?;
                operation.target_revision = gate.current_revision;
                operation.transition_sequence = current_transition_sequence(&write)?;
                operation.leased_write_set = gate.leased_write_set.clone();
                operation.semantic_read_reservations.clear();
                operation.semantic_read_reservation_bindings.clear();
                self.bind_pending_operation(&mut operation)?;
                let (transitions, active_gates) =
                    post_write_analysis_context(&write, &gate, operation.transition_sequence)?;
                write_record(
                    &write,
                    OPERATIONS,
                    operation.operation_id.as_str(),
                    &operation,
                )?;
                guard.commit(write)?;
                return Ok(PostWriteStart::Analyze {
                    gate: Box::new(gate),
                    transitions,
                    active_gates,
                });
            }

            let gate = load_active_gate_for_retry(&write, gate_id)?;
            ensure_post_write_revision_available(&write, operation_id, &gate)?;
            let mut operation = OperationRecord {
                schema_version: "lumin-operation.v1".to_owned(),
                operation_id: operation_id.clone(),
                kind: GateOperationKind::PostWrite,
                request_digest: request_digest.to_owned(),
                status: GateOperationStatus::Pending,
                gate_id: gate_id.clone(),
                target_revision: gate.current_revision,
                reason: None,
                transition_sequence: current_transition_sequence(&write)?,
                declared_write_set: Vec::new(),
                leased_write_set: gate.leased_write_set.clone(),
                semantic_read_reservations: Vec::new(),
                semantic_read_reservation_bindings: Vec::new(),
                interruption_count: 0,
                operation_liveness: None,
                analysis_options: None,
                result: None,
            };
            self.bind_pending_operation(&mut operation)?;
            let (transitions, active_gates) =
                post_write_analysis_context(&write, &gate, operation.transition_sequence)?;
            write_record(
                &write,
                OPERATIONS,
                operation.operation_id.as_str(),
                &operation,
            )?;
            guard.commit(write)?;
            Ok(PostWriteStart::Analyze {
                gate: Box::new(gate),
                transitions,
                active_gates,
            })
        })
    }

    pub fn reserve_post_write_semantic_inputs(
        &self,
        request_digest: &str,
        gate_id: &GateId,
        demanded_inputs: &[SemanticReadReservationBinding],
    ) -> Result<SemanticReadReservation, StoreError> {
        self.reserve_semantic_inputs(
            request_digest,
            gate_id,
            demanded_inputs,
            GateOperationKind::PostWrite,
            "post-write semantic-read reservation",
        )
    }

    pub fn reserve_pre_write_semantic_inputs(
        &self,
        request_digest: &str,
        gate_id: &GateId,
        demanded_inputs: &[SemanticReadReservationBinding],
    ) -> Result<SemanticReadReservation, StoreError> {
        self.reserve_semantic_inputs(
            request_digest,
            gate_id,
            demanded_inputs,
            GateOperationKind::PreWrite,
            "pre-write semantic-read reservation",
        )
    }

    fn reserve_semantic_inputs(
        &self,
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
        let operation_id = &self.operation_id;
        self.store.with_exclusive_lock(|guard| {
            let database = self.open_database(guard)?;
            let write = database.begin_write()?;
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
            self.validate_pending_operation(&operation)?;
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
            guard.commit(write)?;
            Ok(SemanticReadReservation::Reserved)
        })
    }

    pub fn finish_post_write(
        &self,
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
        let operation_id = &self.operation_id;
        self.store.with_exclusive_lock(|guard| {
            let database = self.open_database(guard)?;
            let write = database.begin_write()?;
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
            self.validate_pending_operation(&operation)?;
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
                reason: None,
                signals: signals.clone(),
                leased_write_set: gate.leased_write_set.clone(),
                deltas: deltas.clone(),
            };
            gate.revisions.push(GateRevision {
                revision,
                operation_id: operation_id.clone(),
                committed_unix_millis: Some(crate::unix_millis()?),
                decision,
                reason: None,
                signals: signals.clone(),
                changed_paths,
                snapshot,
                protected_semantic_inputs,
                alias_closures,
                reconciled_transition_sequences,
                deltas,
            });
            persist_operation_result(&write, &gate, &mut operation, &result)?;
            guard.commit(write)?;
            Ok(result)
        })
    }
}
