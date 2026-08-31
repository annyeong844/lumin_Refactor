use super::*;

impl OperationSession<'_> {
    pub fn reserve_pre_write(
        &self,
        request_digest: &str,
        declared_write_set: &[RepoPathProjection],
        initial_leases: &[WriteLease],
        analysis_options: &GateAnalysisOptions,
        rejected_observation: impl FnOnce(&[GateSignal]) -> GateObservationBinding,
    ) -> Result<PreWriteStart, StoreError> {
        let declared_path_inspection = initial_leases
            .iter()
            .cloned()
            .map(|lease| lumin_evidence::PreWriteDeclaredPathInspection {
                path: lease.path.clone(),
                lease: Some(lease),
                rejection: None,
            })
            .collect::<Vec<_>>();
        self.reserve_pre_write_with_inspection(
            request_digest,
            declared_write_set,
            initial_leases,
            &declared_path_inspection,
            analysis_options,
            rejected_observation,
        )
    }

    pub fn reserve_pre_write_with_inspection(
        &self,
        request_digest: &str,
        declared_write_set: &[RepoPathProjection],
        initial_leases: &[WriteLease],
        declared_path_inspection: &[lumin_evidence::PreWriteDeclaredPathInspection],
        analysis_options: &GateAnalysisOptions,
        rejected_observation: impl FnOnce(&[GateSignal]) -> GateObservationBinding,
    ) -> Result<PreWriteStart, StoreError> {
        let operation_id = &self.operation_id;
        self.store.with_exclusive_lock(|guard| {
            let database = self.open_database(guard)?;
            let write = database.begin_write()?;
            reject_retention_operation_collision(&write, operation_id)?;
            super::integrity::validate_stored_gate_catalog(&write)?;
            let mut operation = if let Some(mut operation) =
                read_record::<OperationRecord>(&write, OPERATIONS, operation_id.as_str())?
            {
                validate_operation(
                    &operation,
                    GateOperationKind::PreWrite,
                    request_digest,
                    None,
                )?;
                validate_stored_validation_receipt(&write, &operation)?;
                if let Some(result) = operation.result {
                    return Ok(PreWriteStart::Committed(Box::new(result)));
                }
                if operation.status == GateOperationStatus::Pending {
                    self.validate_pending_operation(&operation)?;
                    let analysis_options = operation.analysis_options.clone().ok_or_else(|| {
                        StoreError::Integrity(format!(
                            "pending pre-write operation omitted its analysis options: {}",
                            operation.operation_id.as_str()
                        ))
                    })?;
                    return Ok(PreWriteStart::Analyze {
                        gate_id: operation.gate_id,
                        transition_sequence: operation.transition_sequence,
                        analysis_options: Box::new(analysis_options),
                    });
                }
                operation.transition_sequence = current_transition_sequence(&write)?;
                operation.declared_write_set = declared_write_set.to_vec();
                operation.leased_write_set = initial_leases.to_vec();
                operation.semantic_read_reservations.clear();
                operation.semantic_read_reservation_bindings.clear();
                operation.pre_write_declared_path_inspection = declared_path_inspection.to_vec();
                operation.pre_write_admission_evidence = None;
                operation.pre_write_final_validation = None;
                operation.analysis_options = Some(analysis_options.clone());
                self.bind_pending_operation(&mut operation)?;
                operation
            } else {
                let mut operation = OperationRecord {
                    schema_version: GATE_OPERATION_SCHEMA_VERSION.to_owned(),
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
                    pre_write_declared_path_inspection: declared_path_inspection.to_vec(),
                    pre_write_admission_evidence: None,
                    pre_write_final_validation: None,
                    post_write_final_validation: None,
                    analysis_options: Some(analysis_options.clone()),
                    result: None,
                };
                self.bind_pending_operation(&mut operation)?;
                operation
            };

            let gate_id = operation.gate_id.clone();
            let transition_sequence = operation.transition_sequence;
            let catalog_revision = current_active_gate_catalog(&write)?;
            let admission_evidence = pre_write_admission_evidence(
                &write,
                operation_id,
                &operation.leased_write_set,
                catalog_revision,
            )?;
            let signals = derive_pre_write_admission_signals(&admission_evidence);

            if !signals.is_empty() {
                operation.pre_write_admission_evidence = Some(admission_evidence);
                let observation_binding = rejected_observation(&signals);
                if !matches!(
                    &observation_binding,
                    lumin_model::ObservationBinding::Unsealed { .. }
                ) {
                    return Err(StoreError::Integrity(
                        "rejected pre-write admission returned a sealed observation".to_owned(),
                    ));
                }
                let result =
                    rejected_open_result(&operation, &signals, observation_binding.clone());
                let unsealed_observation_inputs = UnsealedGateObservationInputs::new(
                    operation.leased_write_set.clone(),
                    Vec::new(),
                    Vec::new(),
                );
                let gate = rejected_gate(
                    &operation,
                    analysis_options.clone(),
                    &signals,
                    None,
                    observation_binding,
                    unsealed_observation_inputs,
                    catalog_revision,
                )?;
                operation.leased_write_set.clear();
                persist_operation_result(&write, &gate, &mut operation, &result)?;
                guard.commit(write)?;
                return Ok(PreWriteStart::Committed(Box::new(result)));
            }

            persist_validation_receipt(&write, &operation, None)?;
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
                analysis_options: Box::new(analysis_options.clone()),
            })
        })
    }

    pub fn finish_pre_write(
        &self,
        request_digest: &str,
        gate_id: &GateId,
        finish: PreWriteFinish,
        final_validation: impl FnOnce(
            &std::collections::BTreeSet<lumin_model::PhysicalFileIdentity>,
            u64,
            &[GateSignal],
        ) -> ObservationFinalization,
    ) -> Result<GateOperationResult, StoreError> {
        let PreWriteFinish {
            baseline,
            mut leased_write_set,
            mut alias_closures,
            attempted_semantic_inputs,
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
            super::integrity::validate_stored_gate_catalog(&write)?;
            validate_pre_write_context(
                &write,
                &operation,
                baseline.as_ref(),
                &leased_write_set,
                &attempted_semantic_inputs,
                &mut signals,
            )?;
            let reserved_state_identities = guard.reserved_state_identities()?;
            let catalog_revision = current_active_gate_catalog(&write)?;
            let finalization =
                final_validation(&reserved_state_identities, catalog_revision, &signals);
            signals.extend(finalization.signals);
            operation.pre_write_final_validation = Some(
                lumin_evidence::PreWriteFinalValidation {
                    catalog_revision,
                    signals: signals.clone(),
                    evidence: finalization.pre_write_evidence,
                },
            );
            let observation_binding = finalization.binding;
            let unsealed_observation_inputs = matches!(
                &observation_binding,
                lumin_model::ObservationBinding::Unsealed { .. }
            )
            .then(|| {
                UnsealedGateObservationInputs::new(
                    leased_write_set.clone(),
                    attempted_semantic_inputs.clone(),
                    baseline.as_ref().map_or_else(Vec::new, |baseline| {
                        baseline
                            .protected_semantic_inputs
                            .iter()
                            .map(|input| input.path.clone())
                            .collect()
                    }),
                )
            });
            let baseline = match (baseline, &observation_binding) {
                (
                    Some(baseline),
                    lumin_model::ObservationBinding::Sealed {
                        observation:
                            lumin_model::SealedGateObservation::Baseline { observation_id },
                    },
                ) => Some(baseline.seal(
                    observation_id.clone(),
                    catalog_revision,
                    leased_write_set.clone(),
                    alias_closures.clone(),
                )),
                (Some(_), lumin_model::ObservationBinding::Unsealed { .. })
                | (None, lumin_model::ObservationBinding::Unsealed { .. }) => None,
                (None, lumin_model::ObservationBinding::Sealed { .. }) => {
                    return Err(StoreError::Integrity(
                        "sealed pre-write observation omitted its baseline".to_owned(),
                    ));
                }
                (Some(_), lumin_model::ObservationBinding::Sealed { .. }) => {
                    return Err(StoreError::Integrity(
                        "pre-write returned a non-baseline sealed observation".to_owned(),
                    ));
                }
            };
            if unsealed_observation_inputs.is_some() {
                leased_write_set.clear();
                alias_closures.clear();
            }
            let (gate, result) = completed_pre_write_records(
                &operation,
                CompletedPreWriteInput {
                    baseline,
                    leased_write_set,
                    alias_closures,
                    unsealed_observation_inputs,
                    signals,
                    observation_binding,
                    catalog_revision,
                },
            )?;
            operation.leased_write_set = result.leased_write_set.clone();
            persist_operation_result(&write, &gate, &mut operation, &result)?;
            if result.lifecycle == GateLifecycle::Active {
                records::increment_active_gate_catalog(&write)?;
            }
            guard.commit_at_namespace_test_boundary(write)?;
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
                validate_stored_validation_receipt(&write, &operation)?;
                if let Some(result) = operation.result {
                    return Ok(PostWriteStart::Committed(Box::new(result)));
                }
                if operation.status == GateOperationStatus::Pending {
                    self.validate_pending_operation(&operation)?;
                    let gate = read_validated_gate(&write, gate_id)?
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
                operation.post_write_final_validation = None;
                self.bind_pending_operation(&mut operation)?;
                let (transitions, active_gates) =
                    post_write_analysis_context(&write, &gate, operation.transition_sequence)?;
                persist_validation_receipt(&write, &operation, None)?;
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
                schema_version: GATE_OPERATION_SCHEMA_VERSION.to_owned(),
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
                pre_write_declared_path_inspection: Vec::new(),
                pre_write_admission_evidence: None,
                pre_write_final_validation: None,
                post_write_final_validation: None,
                analysis_options: None,
                result: None,
            };
            self.bind_pending_operation(&mut operation)?;
            let (transitions, active_gates) =
                post_write_analysis_context(&write, &gate, operation.transition_sequence)?;
            persist_validation_receipt(&write, &operation, None)?;
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
                return Ok(SemanticReadReservation::Committed(Box::new(result)));
            }
            self.validate_pending_operation(&operation)?;
            super::integrity::validate_stored_gate_catalog(&write)?;
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
            persist_validation_receipt(&write, &operation, None)?;
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
        final_validation: impl FnOnce(
            &std::collections::BTreeSet<lumin_model::PhysicalFileIdentity>,
            u64,
            &[GateSignal],
        ) -> ObservationFinalization,
    ) -> Result<GateOperationResult, StoreError> {
        let PostWriteFinish {
            mut snapshot,
            mut protected_semantic_inputs,
            reconciled_baseline,
            mut changed_paths,
            mut actual_write_set,
            mut alias_closures,
            mut reconciled_transition_sequences,
            attempted_semantic_inputs,
            mut signals,
            mut deltas,
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
            super::integrity::validate_stored_gate_catalog(&write)?;
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
                &attempted_semantic_inputs,
                &mut signals,
            )?;
            let reserved_state_identities = guard.reserved_state_identities()?;
            let catalog_revision = current_active_gate_catalog(&write)?;
            let finalization =
                final_validation(&reserved_state_identities, catalog_revision, &signals);
            signals.extend(finalization.signals);
            operation.post_write_final_validation =
                Some(lumin_evidence::PostWriteFinalValidation {
                    catalog_revision,
                    signals: signals.clone(),
                    evidence: finalization.post_write_evidence,
                });
            let observation_binding = finalization.binding;
            let sealed_close = match &observation_binding {
                lumin_model::ObservationBinding::Sealed {
                    observation: lumin_model::SealedGateObservation::Close { .. },
                } => true,
                lumin_model::ObservationBinding::Unsealed { .. } => false,
                lumin_model::ObservationBinding::Sealed { .. } => {
                    return Err(StoreError::Integrity(
                        "post-write returned a non-close sealed observation".to_owned(),
                    ));
                }
            };
            let unsealed_observation_inputs = (!sealed_close).then(|| {
                UnsealedGateObservationInputs::new(
                    gate.leased_write_set.clone(),
                    attempted_semantic_inputs.clone(),
                    gate.protected_semantic_inputs
                        .iter()
                        .map(|input| input.path.clone())
                        .collect(),
                )
            });
            if sealed_close && (snapshot.is_none() || actual_write_set.is_none()) {
                return Err(StoreError::Integrity(
                    "sealed close observation omitted its complete snapshot or actual-write set"
                        .to_owned(),
                ));
            }
            if !sealed_close {
                snapshot = None;
                protected_semantic_inputs.clear();
                changed_paths.clear();
                actual_write_set = None;
                alias_closures.clear();
                reconciled_transition_sequences.clear();
                deltas.clear();
            }
            let decision = gate_policy::decision(&signals);
            let revision = gate
                .current_revision
                .checked_add(1)
                .ok_or_else(|| StoreError::Integrity("gate revision overflow".to_owned()))?;
            if decision.authorizes() {
                if !matches!(
                    &observation_binding,
                    lumin_model::ObservationBinding::Sealed {
                        observation: lumin_model::SealedGateObservation::Close { .. }
                    }
                ) {
                    return Err(StoreError::Integrity(
                        "authorizing post-write omitted its sealed close observation".to_owned(),
                    ));
                }
                publish_authorized_transition(
                    &write,
                    &mut gate,
                    AuthorizedTransitionInput {
                        revision,
                        observation_binding: &observation_binding,
                        snapshot: snapshot.as_ref(),
                        reconciled_baseline: reconciled_baseline.as_ref(),
                        changed_paths: &changed_paths,
                        alias_closures: &alias_closures,
                    },
                )?;
            }
            let can_replace_protected_reads = snapshot_can_protect_current_reads(
                snapshot.as_ref(),
                &observation_binding,
                decision,
            );
            let replaces_protected_reads = can_replace_protected_reads
                && gate.protected_semantic_inputs != protected_semantic_inputs;
            if can_replace_protected_reads {
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
                observation_binding: Some(observation_binding.clone()),
                reason: None,
                signals: signals.clone(),
                leased_write_set: gate.leased_write_set.clone(),
                actual_write_set: actual_write_set.clone(),
                deltas: deltas.clone(),
            };
            gate.revisions.push(GateRevision {
                revision,
                operation_id: operation_id.clone(),
                committed_unix_millis: Some(crate::unix_millis()?),
                decision,
                catalog_revision: Some(catalog_revision),
                observation_binding: Some(observation_binding),
                unsealed_observation_inputs,
                reason: None,
                signals: signals.clone(),
                changed_paths,
                actual_write_set,
                snapshot,
                protected_semantic_inputs,
                alias_closures,
                reconciled_transition_sequences,
                deltas,
            });
            persist_operation_result(&write, &gate, &mut operation, &result)?;
            if result.decision.authorizes() || replaces_protected_reads {
                records::increment_active_gate_catalog(&write)?;
            }
            guard.commit(write)?;
            Ok(result)
        })
    }
}
