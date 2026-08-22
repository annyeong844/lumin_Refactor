use lumin_evidence::{
    CapabilityRecord, DEAD_CODE_CAPABILITY_ID, PathPrefixIdentity, RunEvidence, SemanticInputState,
    seal_analysis_snapshot,
};
use lumin_model::{
    CapabilityState, GateBaselineObservationId, GateCloseObservationId, ObservationBinding,
    RepoPath, SealedGateObservation, UnsealedObservationReason,
};

use super::*;

mod abandon;
mod catalog;
mod liveness;

fn open_store(root: &std::path::Path) -> Result<RepositoryStore, StoreError> {
    let admission = lumin_inventory::repository_admission(root)
        .map_err(|error| StoreError::Integrity(error.to_string()))?;
    RepositoryStore::open(&admission.canonical_root, &admission.binding)
}

fn baseline_observation_id() -> GateBaselineObservationId {
    GateBaselineObservationId::from_string("gate_baseline_observation_test".to_owned())
}

fn sealed_baseline_binding() -> GateObservationBinding {
    ObservationBinding::Sealed {
        observation: SealedGateObservation::Baseline {
            observation_id: baseline_observation_id(),
        },
    }
}

fn sealed_close_binding() -> GateObservationBinding {
    ObservationBinding::Sealed {
        observation: SealedGateObservation::Close {
            observation_id: GateCloseObservationId::from_string(
                "gate_close_observation_test".to_owned(),
            ),
        },
    }
}

fn unsealed_test_binding() -> GateObservationBinding {
    ObservationBinding::Unsealed {
        reason: UnsealedObservationReason::ObservationDomainUnbounded,
        attempted_domain: Vec::new(),
        last_complete_read_set: Vec::new(),
        conflicting_or_unbounded_inputs: Vec::new(),
    }
}

fn rejected_test_observation(_signals: &[GateSignal]) -> GateObservationBinding {
    unsealed_test_binding()
}

fn baseline_finalization(
    extra: Vec<GateSignal>,
    signals: &[GateSignal],
) -> ObservationFinalization {
    let unsealed = signals.iter().chain(&extra).any(|signal| {
        matches!(
            signal,
            GateSignal::WriteConflict { .. }
                | GateSignal::SemanticInputConflict { .. }
                | GateSignal::AnalysisFailed { .. }
                | GateSignal::DeclaredPathUnsupported { .. }
                | GateSignal::TransitionCatalogChanged
        )
    });
    ObservationFinalization {
        signals: extra,
        binding: if unsealed {
            unsealed_test_binding()
        } else {
            sealed_baseline_binding()
        },
    }
}

fn close_finalization(extra: Vec<GateSignal>, signals: &[GateSignal]) -> ObservationFinalization {
    let unsealed = signals.iter().chain(&extra).any(|signal| {
        matches!(
            signal,
            GateSignal::WriteConflict { .. }
                | GateSignal::SemanticInputConflict { .. }
                | GateSignal::AnalysisFailed { .. }
                | GateSignal::DeclaredPathUnsupported { .. }
                | GateSignal::ActiveTransitionPending { .. }
                | GateSignal::TransitionChainBroken { .. }
                | GateSignal::TransitionCatalogChanged
        )
    });
    ObservationFinalization {
        signals: extra,
        binding: if unsealed {
            unsealed_test_binding()
        } else {
            sealed_close_binding()
        },
    }
}

#[test]
fn persisted_v2_optional_gate_additions_default_when_absent()
-> Result<(), Box<dyn std::error::Error>> {
    let operation_id = OperationId::from_string("operation-1".to_owned());
    let gate_id = GateId::from_string("gate-1".to_owned());
    let protected = SemanticInputRecord {
        path: path("config/base.json")?,
        state: SemanticInputState::ConfigPresent,
        payload_sha256: Some("baseline".to_owned()),
        physical_identity: None,
        absence_parent: None,
        physical_redirect_sha256: None,
    };
    let baseline = GateBaseline {
        observation_id: baseline_observation_id(),
        catalog_revision: 0,
        analysis_contract: "contract".to_owned(),
        snapshot: empty_snapshot(),
        leased_write_set: Vec::new(),
        alias_closures: Vec::new(),
        protected_semantic_inputs: vec![protected.clone()],
        transition_sequence: 0,
    };
    let revision = GateRevision {
        revision: 0,
        operation_id: operation_id.clone(),
        committed_unix_millis: None,
        decision: lumin_evidence::GateDecision::Allow,
        catalog_revision: Some(0),
        observation_binding: Some(sealed_baseline_binding()),
        reason: None,
        signals: Vec::new(),
        changed_paths: Vec::new(),
        actual_write_set: None,
        snapshot: None,
        protected_semantic_inputs: vec![protected.clone()],
        alias_closures: Vec::new(),
        reconciled_transition_sequences: Vec::new(),
        deltas: Vec::new(),
    };
    let gate = GateRecord {
        schema_version: GATE_RECORD_SCHEMA_VERSION.to_owned(),
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
            scan_invocation: Default::default(),
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
    assert!(loaded_gate.revisions[0].reason.is_none());

    let operation = OperationRecord {
        schema_version: "lumin-operation.v1".to_owned(),
        operation_id,
        kind: GateOperationKind::PostWrite,
        request_digest: "digest".to_owned(),
        status: GateOperationStatus::Pending,
        gate_id,
        target_revision: 0,
        reason: None,
        transition_sequence: 0,
        declared_write_set: Vec::new(),
        leased_write_set: Vec::new(),
        semantic_read_reservations: vec![path("config/base.json")?],
        semantic_read_reservation_bindings: Vec::new(),
        interruption_count: 0,
        operation_liveness: None,
        analysis_options: None,
        result: None,
    };
    let mut operation_json = serde_json::to_value(operation)?;
    let operation_object = operation_json
        .as_object_mut()
        .ok_or("operation JSON is not an object")?;
    operation_object.remove("semanticReadReservations");
    operation_object.remove("semanticReadReservationBindings");
    operation_object.remove("interruptionCount");
    operation_object.remove("operationLiveness");
    let loaded_operation: OperationRecord = serde_json::from_value(operation_json)?;
    assert!(loaded_operation.semantic_read_reservations.is_empty());
    assert!(
        loaded_operation
            .semantic_read_reservation_bindings
            .is_empty()
    );
    assert_eq!(loaded_operation.interruption_count, 0);
    assert!(loaded_operation.operation_liveness.is_none());
    assert!(loaded_operation.reason.is_none());
    Ok(())
}

#[test]
fn persisted_write_lease_reconstructs_missing_components_and_rejects_desync()
-> Result<(), Box<dyn std::error::Error>> {
    let mut directory = lease(path("src")?);
    directory.kind = lumin_evidence::WriteLeaseKind::Directory;
    let child = lease(path("src/lib.ts")?);

    let mut legacy_json = serde_json::to_value(&directory)?;
    legacy_json
        .pointer_mut("/path")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or("write-lease path JSON is not an object")?
        .remove("components");
    let restored: WriteLease = serde_json::from_value(legacy_json)?;
    assert!(restored.covers(&child.path));
    assert!(restored.conflicts_with(&child));

    let mut components_desync = serde_json::to_value(&directory)?;
    *components_desync
        .pointer_mut("/path/components")
        .ok_or("write-lease components are missing")? = serde_json::json!([]);
    let components_error = match serde_json::from_value::<WriteLease>(components_desync) {
        Ok(_) => return Err("contradictory path components were accepted".into()),
        Err(error) => error,
    };
    assert!(
        components_error
            .to_string()
            .contains("components disagree with canonical path")
    );

    let mut display_desync = serde_json::to_value(&directory)?;
    *display_desync
        .pointer_mut("/path/display")
        .ok_or("write-lease display is missing")? = serde_json::json!("elsewhere");
    let display_error = match serde_json::from_value::<WriteLease>(display_desync) {
        Ok(_) => return Err("contradictory path display was accepted".into()),
        Err(error) => error,
    };
    assert!(
        display_error
            .to_string()
            .contains("display disagrees with canonical path")
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
        reason: None,
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
        interruption_count: 0,
        operation_liveness: None,
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
fn persisted_reservation_rejects_direct_and_absence_identities_together()
-> Result<(), Box<dyn std::error::Error>> {
    let reserved_path = path("config/base.json")?;
    let mut binding = reservation(
        reserved_path.clone(),
        Some(lumin_model::PhysicalFileIdentity::Unix {
            device: 7,
            inode: 11,
        }),
    );
    binding.absence_parent = Some(PathPrefixIdentity {
        path: path("config")?,
        physical_identity: lumin_model::PhysicalFileIdentity::Unix {
            device: 7,
            inode: 12,
        },
    });
    let operation = OperationRecord {
        schema_version: "lumin-operation.v1".to_owned(),
        operation_id: OperationId::from_string("operation-invalid-absence-binding".to_owned()),
        kind: GateOperationKind::PostWrite,
        request_digest: "digest".to_owned(),
        status: GateOperationStatus::Pending,
        gate_id: GateId::from_string("gate-invalid-absence-binding".to_owned()),
        target_revision: 1,
        reason: None,
        transition_sequence: 0,
        declared_write_set: Vec::new(),
        leased_write_set: Vec::new(),
        semantic_read_reservations: vec![reserved_path],
        semantic_read_reservation_bindings: vec![binding],
        interruption_count: 0,
        operation_liveness: None,
        analysis_options: None,
        result: None,
    };

    assert!(matches!(
        validate_reservation_binding_set(&operation),
        Err(StoreError::Integrity(detail))
            if detail.contains("both direct and absence identities")
    ));
    Ok(())
}

#[test]
fn pre_write_semantic_read_reservation_blocks_later_write_admission()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let reader_operation = OperationId::from_string("op-reader".to_owned());
    let reader_path = path("src/new.ts")?;
    let options = GateAnalysisOptions {
        jobs: 1,
        resolution_profile: None,
        scan_invocation: Default::default(),
    };
    let reader = store.begin_operation(&reader_operation)?;
    let reader_gate = match reader.reserve_pre_write(
        "reader-digest",
        std::slice::from_ref(&reader_path),
        &[lease(reader_path.clone())],
        &options,
        rejected_test_observation,
    )? {
        PreWriteStart::Analyze { gate_id, .. } => gate_id,
        PreWriteStart::Committed(_) => {
            return Err("the reader operation was unexpectedly committed".into());
        }
    };
    let demanded = path("config/base.json")?;
    assert_eq!(
        reader.reserve_pre_write_semantic_inputs(
            "reader-digest",
            &reader_gate,
            std::slice::from_ref(&reservation(demanded.clone(), None)),
        )?,
        SemanticReadReservation::Reserved
    );

    let writer_operation = OperationId::from_string("op-writer".to_owned());
    let writer = store.begin_operation(&writer_operation)?;
    let rejected = match writer.reserve_pre_write(
        "writer-digest",
        std::slice::from_ref(&demanded),
        &[lease(demanded.clone())],
        &options,
        rejected_test_observation,
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
fn new_path_cannot_advance_a_pending_missing_semantic_branch()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let options = options();
    let root_path = RepoPathProjection::from(&RepoPath::empty());
    let root_identity = lumin_model::PhysicalFileIdentity::Unix {
        device: 7,
        inode: 11,
    };
    let reader_operation = OperationId::from_string("op-missing-reader".to_owned());
    let reader_source = path("src/new.ts")?;
    let reader = store.begin_operation(&reader_operation)?;
    let reader_gate = match reader.reserve_pre_write(
        "missing-reader-digest",
        std::slice::from_ref(&reader_source),
        &[lease(reader_source.clone())],
        &options,
        rejected_test_observation,
    )? {
        PreWriteStart::Analyze { gate_id, .. } => gate_id,
        PreWriteStart::Committed(_) => {
            return Err("the missing-input reader was unexpectedly committed".into());
        }
    };
    let missing_candidate = path("generated/deep/package.json")?;
    let missing_binding = SemanticReadReservationBinding {
        path: missing_candidate.clone(),
        physical_identity: None,
        absence_parent: Some(PathPrefixIdentity {
            path: root_path.clone(),
            physical_identity: root_identity.clone(),
        }),
    };
    assert_eq!(
        reader.reserve_pre_write_semantic_inputs(
            "missing-reader-digest",
            &reader_gate,
            std::slice::from_ref(&missing_binding),
        )?,
        SemanticReadReservation::Reserved
    );

    let writer_path = path("generated/main.ts")?;
    let writer_lease = WriteLease {
        path: writer_path.clone(),
        kind: lumin_evidence::WriteLeaseKind::NewFile,
        physical_identity: None,
        nearest_existing_parent: Some(root_path.clone()),
        prefix_identities: vec![PathPrefixIdentity {
            path: root_path,
            physical_identity: root_identity,
        }],
    };
    let writer = store.begin_operation(&OperationId::from_string(
        "op-missing-branch-writer".to_owned(),
    ))?;
    let rejected = match writer.reserve_pre_write(
        "missing-branch-writer-digest",
        std::slice::from_ref(&writer_path),
        std::slice::from_ref(&writer_lease),
        &options,
        rejected_test_observation,
    )? {
        PreWriteStart::Committed(result) => result,
        PreWriteStart::Analyze { .. } => {
            return Err("a new path advanced a reserved missing branch".into());
        }
    };
    assert_eq!(rejected.decision, lumin_evidence::GateDecision::Incomplete);
    assert!(rejected.signals.iter().any(|signal| {
        matches!(
            signal,
            GateSignal::WriteConflict { paths, gate_ids }
                if paths == std::slice::from_ref(&missing_candidate)
                    && gate_ids == std::slice::from_ref(&reader_gate)
        )
    }));
    Ok(())
}

#[test]
fn pre_write_finish_rejects_a_baseline_that_omits_a_reserved_input()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let operation_id = OperationId::from_string("op-open".to_owned());
    let source = path("src/new.ts")?;
    let source_lease = lease(source.clone());
    let options = GateAnalysisOptions {
        jobs: 1,
        resolution_profile: None,
        scan_invocation: Default::default(),
    };
    let operation = store.begin_operation(&operation_id)?;
    let (gate_id, transition_sequence) = match operation.reserve_pre_write(
        "open-digest",
        std::slice::from_ref(&source),
        std::slice::from_ref(&source_lease),
        &options,
        rejected_test_observation,
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
        operation.reserve_pre_write_semantic_inputs(
            "open-digest",
            &gate_id,
            std::slice::from_ref(&reservation(demanded, None)),
        )?,
        SemanticReadReservation::Reserved
    );

    let error = match operation.finish_pre_write(
        "open-digest",
        &gate_id,
        PreWriteFinish {
            baseline: Some(GateBaselineDraft {
                analysis_contract: "test-contract".to_owned(),
                snapshot: empty_snapshot(),
                protected_semantic_inputs: Vec::new(),
                transition_sequence,
            }),
            leased_write_set: vec![source_lease],
            alias_closures: Vec::new(),
            attempted_semantic_inputs: Vec::new(),
            signals: Vec::new(),
        },
        |_, _, signals| baseline_finalization(Vec::new(), signals),
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
fn final_validation_can_stop_pre_write_promotion() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let operation = store.begin_operation(&OperationId::from_string("op-open".to_owned()))?;
    let source = path("src/main.ts")?;
    let source_lease = lease(source.clone());
    let (gate_id, transition_sequence) = match operation.reserve_pre_write(
        "open-digest",
        std::slice::from_ref(&source),
        std::slice::from_ref(&source_lease),
        &options(),
        rejected_test_observation,
    )? {
        PreWriteStart::Analyze {
            gate_id,
            transition_sequence,
        } => (gate_id, transition_sequence),
        PreWriteStart::Committed(_) => return Err("the opening operation committed early".into()),
    };

    let cache_anchor = lumin_inventory::physical_file_identity(
        &root.path().join(".lumin/cache/namespace.anchor"),
    )?;
    let result = operation.finish_pre_write(
        "open-digest",
        &gate_id,
        PreWriteFinish {
            baseline: Some(GateBaselineDraft {
                analysis_contract: "test-contract".to_owned(),
                snapshot: empty_snapshot(),
                protected_semantic_inputs: Vec::new(),
                transition_sequence,
            }),
            leased_write_set: vec![source_lease],
            alias_closures: Vec::new(),
            attempted_semantic_inputs: Vec::new(),
            signals: Vec::new(),
        },
        |reserved_identities, _, signals| {
            assert!(reserved_identities.contains(&cache_anchor));
            baseline_finalization(
                vec![GateSignal::ProtectedInputChanged {
                    paths: vec![source.clone()],
                }],
                signals,
            )
        },
    )?;

    assert_eq!(result.lifecycle, GateLifecycle::Rejected);
    assert!(!result.decision.authorizes());
    assert!(matches!(
        result.observation_binding,
        Some(ObservationBinding::Sealed {
            observation: SealedGateObservation::Baseline { .. }
        })
    ));
    assert_eq!(
        result.signals,
        [GateSignal::ProtectedInputChanged {
            paths: vec![source]
        }]
    );
    let persisted = store.load_gate(&gate_id)?;
    assert!(persisted.baseline.is_some());
    assert!(matches!(
        persisted
            .revisions
            .last()
            .and_then(|revision| revision.observation_binding.as_ref()),
        Some(ObservationBinding::Sealed {
            observation: SealedGateObservation::Baseline { .. }
        })
    ));
    Ok(())
}

#[test]
fn final_validation_can_stop_post_write_promotion() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let prior = semantic_input("config/prior.json", "prior")?;
    let gate_id = open_active_gate_with_protected_inputs(
        &store,
        "op-open",
        "open-digest",
        "src/main.ts",
        vec![prior.clone()],
    )?;
    let operation = store.begin_operation(&OperationId::from_string("op-close".to_owned()))?;
    let gate = match operation.begin_post_write("close-digest", &gate_id)? {
        PostWriteStart::Analyze { gate, .. } => gate,
        PostWriteStart::Committed(_) => return Err("the closing operation committed early".into()),
    };
    let baseline = gate
        .baseline
        .as_ref()
        .ok_or("active gate fixture omitted its baseline")?
        .snapshot
        .clone();
    let current = semantic_input("config/current.json", "current")?;
    let current_snapshot = seal_analysis_snapshot(
        vec![current.clone()],
        baseline.evidence.clone(),
        baseline.scan_invocation.clone(),
        baseline.entry_selections.clone(),
    );
    let source = path("src/main.ts")?;

    let result = operation.finish_post_write(
        "close-digest",
        &gate_id,
        PostWriteFinish {
            snapshot: Some(current_snapshot),
            protected_semantic_inputs: vec![current.clone()],
            reconciled_baseline: Some(baseline),
            changed_paths: Vec::new(),
            actual_write_set: Some(Default::default()),
            alias_closures: Vec::new(),
            reconciled_transition_sequences: Vec::new(),
            attempted_semantic_inputs: Vec::new(),
            signals: Vec::new(),
            deltas: Vec::new(),
        },
        |_, _, signals| {
            close_finalization(
                vec![GateSignal::ProtectedInputChanged {
                    paths: vec![source.clone()],
                }],
                signals,
            )
        },
    )?;

    assert!(!result.decision.authorizes());
    assert!(result.actual_write_set.is_some());
    assert_eq!(
        result.signals,
        [GateSignal::ProtectedInputChanged {
            paths: vec![source]
        }]
    );
    assert!(matches!(
        result.observation_binding,
        Some(ObservationBinding::Sealed { .. })
    ));
    let persisted = store.load_gate(&gate_id)?;
    assert_eq!(persisted.protected_semantic_inputs, vec![prior]);
    let revision = persisted
        .revisions
        .last()
        .ok_or("stale close revision is missing")?;
    assert_eq!(revision.protected_semantic_inputs, vec![current]);
    assert!(revision.snapshot.is_some());
    assert!(revision.actual_write_set.is_some());
    Ok(())
}

#[test]
fn sealed_incomparable_close_advances_complete_read_protection()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let prior = semantic_input("config/prior.json", "prior")?;
    let gate_id = open_active_gate_with_protected_inputs(
        &store,
        "op-open-protected",
        "open-protected-digest",
        "src/main.ts",
        vec![prior.clone()],
    )?;
    let operation =
        store.begin_operation(&OperationId::from_string("op-close-unsealed".to_owned()))?;
    let gate = match operation.begin_post_write("close-unsealed-digest", &gate_id)? {
        PostWriteStart::Analyze { gate, .. } => gate,
        PostWriteStart::Committed(_) => return Err("the closing operation committed early".into()),
    };
    let baseline = gate
        .baseline
        .as_ref()
        .ok_or("active gate fixture omitted its baseline")?
        .snapshot
        .clone();
    let current = semantic_input("config/current.json", "current")?;
    let current_snapshot = seal_analysis_snapshot(
        vec![current.clone()],
        baseline.evidence.clone(),
        baseline.scan_invocation.clone(),
        baseline.entry_selections.clone(),
    );

    let result = operation.finish_post_write(
        "close-unsealed-digest",
        &gate_id,
        PostWriteFinish {
            snapshot: Some(current_snapshot),
            protected_semantic_inputs: vec![current.clone()],
            reconciled_baseline: Some(baseline),
            changed_paths: Vec::new(),
            actual_write_set: Some(Default::default()),
            alias_closures: Vec::new(),
            reconciled_transition_sequences: Vec::new(),
            attempted_semantic_inputs: Vec::new(),
            signals: vec![GateSignal::LifecycleDeltaIncomparable { count: 1 }],
            deltas: Vec::new(),
        },
        |_, _, signals| close_finalization(Vec::new(), signals),
    )?;

    assert!(matches!(
        result.observation_binding,
        Some(ObservationBinding::Sealed {
            observation: SealedGateObservation::Close { .. }
        })
    ));
    assert!(result.actual_write_set.is_some());
    let persisted = store.load_gate(&gate_id)?;
    assert_eq!(persisted.protected_semantic_inputs, vec![current.clone()]);
    let revision = persisted
        .revisions
        .last()
        .ok_or("unsealed close revision is missing")?;
    assert_eq!(revision.protected_semantic_inputs, vec![current]);
    assert!(revision.snapshot.is_some());
    assert!(revision.actual_write_set.is_some());
    assert!(revision.alias_closures.is_empty());
    assert!(revision.reconciled_transition_sequences.is_empty());
    Ok(())
}

#[test]
fn semantic_read_reservation_blocks_later_write_admission() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let opening_operation = OperationId::from_string("op-open".to_owned());
    let source = path("src/a.ts")?;
    let source_lease = lease(source.clone());
    let options = GateAnalysisOptions {
        jobs: 1,
        resolution_profile: None,
        scan_invocation: Default::default(),
    };
    let opening = store.begin_operation(&opening_operation)?;
    let (gate_id, transition_sequence) = match opening.reserve_pre_write(
        "open-digest",
        std::slice::from_ref(&source),
        std::slice::from_ref(&source_lease),
        &options,
        rejected_test_observation,
    )? {
        PreWriteStart::Analyze {
            gate_id,
            transition_sequence,
        } => (gate_id, transition_sequence),
        PreWriteStart::Committed(_) => {
            return Err("the first gate was unexpectedly committed".into());
        }
    };
    let baseline = GateBaselineDraft {
        analysis_contract: "test-contract".to_owned(),
        snapshot: empty_snapshot(),
        protected_semantic_inputs: Vec::new(),
        transition_sequence,
    };
    let opened = opening.finish_pre_write(
        "open-digest",
        &gate_id,
        PreWriteFinish {
            baseline: Some(baseline),
            leased_write_set: vec![source_lease],
            alias_closures: Vec::new(),
            attempted_semantic_inputs: Vec::new(),
            signals: Vec::new(),
        },
        |_, _, signals| baseline_finalization(Vec::new(), signals),
    )?;
    assert!(opened.decision.authorizes());

    let close_operation = OperationId::from_string("op-close".to_owned());
    let closing = store.begin_operation(&close_operation)?;
    assert!(matches!(
        closing.begin_post_write("close-digest", &gate_id)?,
        PostWriteStart::Analyze { .. }
    ));
    let demanded = path("config/base.json")?;
    assert_eq!(
        closing.reserve_post_write_semantic_inputs(
            "close-digest",
            &gate_id,
            std::slice::from_ref(&reservation(demanded.clone(), None)),
        )?,
        SemanticReadReservation::Reserved
    );

    let writer_operation = OperationId::from_string("op-writer".to_owned());
    let writer = store.begin_operation(&writer_operation)?;
    let rejected = match writer.reserve_pre_write(
        "writer-digest",
        std::slice::from_ref(&demanded),
        &[lease(demanded.clone())],
        &options,
        rejected_test_observation,
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
    let store = open_store(root.path())?;
    let options = GateAnalysisOptions {
        jobs: 1,
        resolution_profile: None,
        scan_invocation: Default::default(),
    };
    let reader_operation = OperationId::from_string("op-alias-reader".to_owned());
    let reader_source = path("src/new.ts")?;
    let reader = store.begin_operation(&reader_operation)?;
    let reader_gate = match reader.reserve_pre_write(
        "reader-digest",
        std::slice::from_ref(&reader_source),
        &[lease(reader_source.clone())],
        &options,
        rejected_test_observation,
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
        reader.reserve_pre_write_semantic_inputs(
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
    let writer = store.begin_operation(&writer_operation)?;
    let rejected = match writer.reserve_pre_write(
        "writer-digest",
        std::slice::from_ref(&write_alias),
        &[lease_with_identity(write_alias.clone(), physical_identity)],
        &options,
        rejected_test_observation,
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

fn options() -> GateAnalysisOptions {
    GateAnalysisOptions {
        jobs: 1,
        resolution_profile: None,
        scan_invocation: Default::default(),
    }
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
        absence_parent: None,
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
            source_classifications: Vec::new(),
            source_contexts: Vec::new(),
            source_observations: Vec::new(),
            dependency_owners: Vec::new(),
            resolutions: Vec::new(),
            metrics: Default::default(),
            findings: Vec::new(),
            limitations: Vec::new(),
        },
        Default::default(),
        Vec::new(),
    )
}

fn semantic_input(
    value: &str,
    payload: &str,
) -> Result<SemanticInputRecord, Box<dyn std::error::Error>> {
    Ok(SemanticInputRecord {
        path: path(value)?,
        state: SemanticInputState::ConfigPresent,
        payload_sha256: Some(payload.to_owned()),
        physical_identity: None,
        absence_parent: None,
        physical_redirect_sha256: None,
    })
}

fn open_active_gate(
    store: &RepositoryStore,
    operation_id: &str,
    request_digest: &str,
    source: &str,
) -> Result<GateId, Box<dyn std::error::Error>> {
    open_active_gate_with_protected_inputs(store, operation_id, request_digest, source, Vec::new())
}

fn open_active_gate_with_protected_inputs(
    store: &RepositoryStore,
    operation_id: &str,
    request_digest: &str,
    source: &str,
    protected_semantic_inputs: Vec<SemanticInputRecord>,
) -> Result<GateId, Box<dyn std::error::Error>> {
    let operation_id = OperationId::from_string(operation_id.to_owned());
    let session = store.begin_operation(&operation_id)?;
    let source = path(source)?;
    let source_lease = lease(source.clone());
    let (gate_id, transition_sequence) = match session.reserve_pre_write(
        request_digest,
        std::slice::from_ref(&source),
        std::slice::from_ref(&source_lease),
        &options(),
        rejected_test_observation,
    )? {
        PreWriteStart::Analyze {
            gate_id,
            transition_sequence,
        } => (gate_id, transition_sequence),
        PreWriteStart::Committed(_) => return Err("active gate fixture was rejected".into()),
    };
    session.finish_pre_write(
        request_digest,
        &gate_id,
        PreWriteFinish {
            baseline: Some(GateBaselineDraft {
                analysis_contract: "test-contract".to_owned(),
                snapshot: empty_snapshot(),
                protected_semantic_inputs,
                transition_sequence,
            }),
            leased_write_set: vec![source_lease],
            alias_closures: Vec::new(),
            attempted_semantic_inputs: Vec::new(),
            signals: Vec::new(),
        },
        |_, _, signals| baseline_finalization(Vec::new(), signals),
    )?;
    Ok(gate_id)
}

fn close_active_gate(
    store: &RepositoryStore,
    gate_id: &GateId,
    operation_id: &str,
    request_digest: &str,
) -> Result<GateOperationResult, Box<dyn std::error::Error>> {
    let session = store.begin_operation(&OperationId::from_string(operation_id.to_owned()))?;
    let gate = match session.begin_post_write(request_digest, gate_id)? {
        PostWriteStart::Analyze { gate, .. } => gate,
        PostWriteStart::Committed(result) => return Ok(*result),
    };
    let baseline = gate
        .baseline
        .as_ref()
        .ok_or("active gate fixture omitted its baseline")?
        .snapshot
        .clone();
    session
        .finish_post_write(
            request_digest,
            gate_id,
            PostWriteFinish {
                snapshot: Some(baseline.clone()),
                protected_semantic_inputs: Vec::new(),
                reconciled_baseline: Some(baseline),
                changed_paths: Vec::new(),
                actual_write_set: Some(Default::default()),
                alias_closures: Vec::new(),
                reconciled_transition_sequences: Vec::new(),
                attempted_semantic_inputs: Vec::new(),
                signals: Vec::new(),
                deltas: Vec::new(),
            },
            |_, _, signals| close_finalization(Vec::new(), signals),
        )
        .map_err(Into::into)
}
