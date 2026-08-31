use super::*;
use crate::RetentionPlanRequest;
use lumin_evidence::{RetentionMutationResult, RetentionPlanScope};

#[test]
fn gate_queries_and_conflicts_authenticate_the_complete_gate_projection()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let source = path("src/authenticated-gate.ts")?;
    let gate_id = open_active_gate(
        &store,
        "op-authenticated-gate-open",
        "authenticated-gate-open",
        &source.display,
    )?;

    store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        let mut gate = records::read_record::<GateRecord>(&write, GATES, gate_id.as_str())?
            .ok_or_else(|| StoreError::GateNotFound(gate_id.as_str().to_owned()))?;
        gate.leased_write_set.clear();
        records::write_record(&write, GATES, gate_id.as_str(), &gate)?;
        guard.commit(write)
    })?;

    assert!(matches!(
        store.load_gate(&gate_id),
        Err(StoreError::Integrity(message))
            if message.contains("store-owned validation receipt")
    ));

    let conflicting = store.begin_operation(&OperationId::from_string(
        "op-authenticated-gate-conflict".to_owned(),
    ))?;
    assert!(matches!(
        conflicting.reserve_pre_write(
            "authenticated-gate-conflict",
            std::slice::from_ref(&source),
            &[lease(source.clone())?],
            &options(),
            rejected_test_observation,
        ),
        Err(StoreError::Integrity(message))
            if message.contains("store-owned validation receipt")
    ));
    Ok(())
}

#[test]
fn operation_replay_authenticates_the_complete_committed_result()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let operation_id = OperationId::from_string("op-authenticated-result-open".to_owned());
    let source = path("src/authenticated-result.ts")?;
    let analysis_options = options();
    let request_digest = lumin_evidence::pre_write_request_digest(
        std::slice::from_ref(&source),
        &analysis_options.scan_invocation,
    );
    open_active_gate(
        &store,
        operation_id.as_str(),
        &request_digest,
        "src/authenticated-result.ts",
    )?;

    store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        let mut operation =
            records::read_record::<OperationRecord>(&write, OPERATIONS, operation_id.as_str())?
                .ok_or_else(|| StoreError::OperationNotFound(operation_id.as_str().to_owned()))?;
        let result = operation.result.as_mut().ok_or_else(|| {
            StoreError::Integrity("committed fixture omitted its result".to_owned())
        })?;
        result.decision = GateDecision::Deny;
        result.lifecycle = GateLifecycle::Rejected;
        records::write_record(&write, OPERATIONS, operation_id.as_str(), &operation)?;
        guard.commit(write)
    })?;

    assert!(matches!(
        store.replay_pre_write_result(&operation_id, &request_digest),
        Err(StoreError::Integrity(message))
            if message.contains("complete gate revision")
    ));
    assert!(matches!(
        store.load_operation(&operation_id),
        Err(StoreError::Integrity(message))
            if message.contains("complete gate revision")
    ));
    Ok(())
}

#[test]
fn operation_replay_authenticates_the_complete_committed_operation_projection()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let operation_id = OperationId::from_string("op-authenticated-operation-open".to_owned());
    let source = path("src/authenticated-operation.ts")?;
    let analysis_options = options();
    let request_digest = lumin_evidence::pre_write_request_digest(
        std::slice::from_ref(&source),
        &analysis_options.scan_invocation,
    );
    open_active_gate(
        &store,
        operation_id.as_str(),
        &request_digest,
        "src/authenticated-operation.ts",
    )?;

    store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        let mut operation =
            records::read_record::<OperationRecord>(&write, OPERATIONS, operation_id.as_str())?
                .ok_or_else(|| StoreError::OperationNotFound(operation_id.as_str().to_owned()))?;
        operation.transition_sequence = operation
            .transition_sequence
            .checked_add(1)
            .ok_or_else(|| StoreError::Integrity("test transition sequence overflow".to_owned()))?;
        records::write_record(&write, OPERATIONS, operation_id.as_str(), &operation)?;
        guard.commit(write)
    })?;

    assert!(matches!(
        store.replay_pre_write_result(&operation_id, &request_digest),
        Err(StoreError::Integrity(message))
            if message.contains("store-owned validation receipt")
    ));
    assert!(matches!(
        store.load_operation(&operation_id),
        Err(StoreError::Integrity(message))
            if message.contains("store-owned validation receipt")
    ));
    Ok(())
}

#[test]
fn pre_write_rejects_a_gate_allocator_behind_a_retained_gate()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    open_active_gate(
        &store,
        "op-gate-allocator-owner",
        "gate-allocator-owner",
        "src/gate-allocator-owner.ts",
    )?;

    store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        let mut sequences = write
            .open_table(crate::SEQUENCES)
            .map_err(crate::backend_error)?;
        sequences.insert("gate", 0).map_err(crate::backend_error)?;
        drop(sequences);
        guard.commit(write)
    })?;

    let source = path("src/gate-allocator-candidate.ts")?;
    let candidate = store.begin_operation(&OperationId::from_string(
        "op-gate-allocator-candidate".to_owned(),
    ))?;
    assert!(matches!(
        candidate.reserve_pre_write(
            "gate-allocator-candidate",
            std::slice::from_ref(&source),
            &[lease(source.clone())?],
            &options(),
            rejected_test_observation,
        ),
        Err(StoreError::Integrity(message))
            if message.contains("gate allocator sequence 0 trails retained allocation 1")
    ));
    Ok(())
}

#[test]
fn gate_queries_reject_an_active_catalog_counter_below_durable_history()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate(
        &store,
        "op-active-catalog-floor",
        "active-catalog-floor",
        "src/active-catalog-floor.ts",
    )?;

    store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        let mut sequences = write
            .open_table(crate::SEQUENCES)
            .map_err(crate::backend_error)?;
        sequences
            .insert(records::ACTIVE_GATE_CATALOG_SEQUENCE_KEY, 0)
            .map_err(crate::backend_error)?;
        drop(sequences);
        guard.commit(write)
    })?;

    assert!(matches!(
        store.load_gate(&gate_id),
        Err(StoreError::Integrity(message))
            if message.contains("active-gate catalog sequence regressed below durable gate history")
    ));
    Ok(())
}

#[test]
fn pre_write_rejects_an_exhausted_active_catalog_counter() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    open_active_gate(
        &store,
        "op-active-catalog-exhausted-owner",
        "active-catalog-exhausted-owner",
        "src/active-catalog-exhausted-owner.ts",
    )?;

    store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        let mut sequences = write
            .open_table(crate::SEQUENCES)
            .map_err(crate::backend_error)?;
        sequences
            .insert(records::ACTIVE_GATE_CATALOG_SEQUENCE_KEY, u64::MAX)
            .map_err(crate::backend_error)?;
        drop(sequences);
        guard.commit(write)
    })?;

    let source = path("src/active-catalog-exhausted-candidate.ts")?;
    let candidate = store.begin_operation(&OperationId::from_string(
        "op-active-catalog-exhausted-candidate".to_owned(),
    ))?;
    assert!(matches!(
        candidate.reserve_pre_write(
            "active-catalog-exhausted-candidate",
            std::slice::from_ref(&source),
            &[lease(source.clone())?],
            &options(),
            rejected_test_observation,
        ),
        Err(StoreError::Integrity(message))
            if message.contains("active-gate catalog sequence is exhausted")
    ));
    Ok(())
}

#[test]
fn pre_write_rejects_an_orphaned_pending_validation_receipt()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let orphaned_operation_id = OperationId::from_string("op-orphaned-pending-receipt".to_owned());
    let orphaned = store.begin_operation(&orphaned_operation_id)?;
    let orphaned_source = path("src/orphaned-pending-receipt.ts")?;
    assert!(matches!(
        orphaned.reserve_pre_write(
            "orphaned-pending-receipt",
            std::slice::from_ref(&orphaned_source),
            &[lease(orphaned_source.clone())?],
            &options(),
            rejected_test_observation,
        )?,
        PreWriteStart::Analyze { .. }
    ));

    store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        let mut operations = write.open_table(OPERATIONS).map_err(crate::backend_error)?;
        let removed = operations
            .remove(orphaned_operation_id.as_str())
            .map_err(crate::backend_error)?;
        let operation_was_present = removed.is_some();
        drop(removed);
        if !operation_was_present {
            return Err(StoreError::Integrity(
                "pending operation fixture disappeared".to_owned(),
            ));
        }
        drop(operations);
        guard.commit(write)
    })?;

    let candidate_source = path("src/orphaned-pending-receipt-candidate.ts")?;
    let candidate = store.begin_operation(&OperationId::from_string(
        "op-orphaned-pending-receipt-candidate".to_owned(),
    ))?;
    assert!(matches!(
        candidate.reserve_pre_write(
            "orphaned-pending-receipt-candidate",
            std::slice::from_ref(&candidate_source),
            &[lease(candidate_source.clone())?],
            &options(),
            rejected_test_observation,
        ),
        Err(StoreError::Integrity(message))
            if message.contains("lost its owning operation")
    ));
    Ok(())
}

#[test]
fn pre_write_promotion_revalidates_the_complete_gate_catalog()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    open_active_gate(
        &store,
        "op-promotion-catalog-owner",
        "promotion-catalog-owner",
        "src/promotion-catalog-owner.ts",
    )?;

    let operation_id = OperationId::from_string("op-promotion-catalog-candidate".to_owned());
    let candidate = store.begin_operation(&operation_id)?;
    let source = path("src/promotion-catalog-candidate.ts")?;
    let source_lease = lease(source.clone())?;
    let (gate_id, transition_sequence) = match candidate.reserve_pre_write(
        "promotion-catalog-candidate",
        std::slice::from_ref(&source),
        std::slice::from_ref(&source_lease),
        &options(),
        rejected_test_observation,
    )? {
        PreWriteStart::Analyze {
            gate_id,
            transition_sequence,
            ..
        } => (gate_id, transition_sequence),
        PreWriteStart::Committed(_) => return Err("candidate committed before analysis".into()),
    };

    store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        let mut sequences = write
            .open_table(crate::SEQUENCES)
            .map_err(crate::backend_error)?;
        sequences
            .insert(records::ACTIVE_GATE_CATALOG_SEQUENCE_KEY, 0)
            .map_err(crate::backend_error)?;
        drop(sequences);
        guard.commit(write)
    })?;

    let final_validation_called = std::cell::Cell::new(false);
    let error = match candidate.finish_pre_write(
        "promotion-catalog-candidate",
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
        |_, _, signals| {
            final_validation_called.set(true);
            baseline_finalization(Vec::new(), signals)
        },
    ) {
        Ok(_) => return Err("a regressed catalog reached pre-write promotion".into()),
        Err(error) => error,
    };
    assert!(!final_validation_called.get());
    assert!(matches!(
        error,
        StoreError::Integrity(message)
            if message.contains("active-gate catalog sequence regressed below durable gate history")
    ));
    store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        assert!(records::read_record::<GateRecord>(&write, GATES, gate_id.as_str())?.is_none());
        let operation =
            records::read_record::<OperationRecord>(&write, OPERATIONS, operation_id.as_str())?
                .ok_or_else(|| StoreError::OperationNotFound(operation_id.as_str().to_owned()))?;
        assert_eq!(operation.status, GateOperationStatus::Pending);
        Ok(())
    })?;
    Ok(())
}

#[test]
fn gate_queries_reject_a_transition_ahead_of_its_allocator()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let active_gate = open_active_gate(
        &store,
        "op-transition-allocator-active",
        "transition-allocator-active",
        "src/transition-allocator-active.ts",
    )?;
    let closing_gate = open_active_gate(
        &store,
        "op-transition-allocator-owner",
        "transition-allocator-owner",
        "src/transition-allocator-owner.ts",
    )?;
    close_active_gate(
        &store,
        &closing_gate,
        "op-transition-allocator-close",
        "transition-allocator-close",
    )?;

    store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        let old_key = transition_key(1);
        let mut transition =
            records::read_record::<WorktreeTransition>(&write, TRANSITIONS, &old_key)?.ok_or_else(
                || StoreError::Integrity("terminal transition fixture is missing".to_owned()),
            )?;
        {
            let mut transitions = write
                .open_table(TRANSITIONS)
                .map_err(crate::backend_error)?;
            let removed = transitions
                .remove(old_key.as_str())
                .map_err(crate::backend_error)?;
            if removed.is_none() {
                return Err(StoreError::Integrity(
                    "terminal transition fixture disappeared".to_owned(),
                ));
            }
        }
        transition.sequence = 2;
        records::write_record(&write, TRANSITIONS, &transition_key(2), &transition)?;

        let mut gate = records::read_record::<GateRecord>(&write, GATES, active_gate.as_str())?
            .ok_or_else(|| StoreError::GateNotFound(active_gate.as_str().to_owned()))?;
        gate.transition_refs = vec![2];
        records::write_record(&write, GATES, active_gate.as_str(), &gate)?;
        guard.commit(write)
    })?;

    assert!(matches!(
        store.load_gate(&active_gate),
        Err(StoreError::Integrity(message))
            if message.contains("transition allocator sequence 1 trails authenticated catalog 2")
    ));
    Ok(())
}

#[test]
fn gate_queries_and_post_write_reject_an_unauthenticated_transition_capsule()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let active_gate = open_active_gate(
        &store,
        "op-transition-auth-active",
        "transition-auth-active",
        "src/transition-auth-active.ts",
    )?;
    let closing_gate = open_active_gate(
        &store,
        "op-transition-auth-owner",
        "transition-auth-owner",
        "src/transition-auth-owner.ts",
    )?;
    let closed = close_active_gate(
        &store,
        &closing_gate,
        "op-transition-auth-close",
        "transition-auth-close",
    )?;
    assert!(closed.decision.authorizes());
    let prepared_plan = store.prepare_retention_plan(&RetentionPlanRequest {
        scope: RetentionPlanScope::Gates {
            terminal_before_unix_millis: 9_000_000_000_000,
        },
        operation_id: OperationId::from_string("op-transition-auth-plan".to_owned()),
    })?;
    let plan_id = match prepared_plan {
        RetentionMutationResult::Prepared { plan_id, .. } => plan_id,
        _ => return Err("gate retention plan was not prepared".into()),
    };

    store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        let key = transition_key(1);
        let mut transition = records::read_record::<WorktreeTransition>(&write, TRANSITIONS, &key)?
            .ok_or_else(|| {
                StoreError::Integrity("terminal transition fixture is missing".to_owned())
            })?;
        transition.capsule.after_snapshot.evidence.schema_version =
            "lumin-evidence.forged".to_owned();
        records::write_record(&write, TRANSITIONS, &key, &transition)?;
        guard.commit(write)
    })?;

    assert!(matches!(
        store.load_gate(&active_gate),
        Err(StoreError::Integrity(message))
            if message.contains("transition 1 payload")
    ));
    assert!(matches!(
        store.prepare_retention_plan(&RetentionPlanRequest {
            scope: RetentionPlanScope::Gates {
                terminal_before_unix_millis: 9_000_000_000_000,
            },
            operation_id: OperationId::from_string("op-transition-auth-plan-after".to_owned()),
        }),
        Err(StoreError::Integrity(message)) if message.contains("transition 1 payload")
    ));
    assert!(matches!(
        store.confirm_retention_plan(
            &plan_id,
            &OperationId::from_string("op-transition-auth-confirm".to_owned()),
        ),
        Err(StoreError::Integrity(message)) if message.contains("transition 1 payload")
    ));

    let close = store.begin_operation(&OperationId::from_string(
        "op-transition-auth-active-close".to_owned(),
    ))?;
    assert!(matches!(
        close.begin_post_write("transition-auth-active-close", &active_gate),
        Err(StoreError::Integrity(message))
            if message.contains("transition 1 payload")
    ));
    Ok(())
}

#[test]
fn post_write_catalog_race_discards_actual_write_attribution_idempotently()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_a = open_active_gate(&store, "op-open-a", "open-a", "src/a.ts")?;
    let gate_b = open_active_gate(&store, "op-open-b", "open-b", "src/b.ts")?;

    let close_a = store.begin_operation(&OperationId::from_string("op-close-a".to_owned()))?;
    let (gate, transitions) = match close_a.begin_post_write("close-a", &gate_a)? {
        PostWriteStart::Analyze {
            gate, transitions, ..
        } => (gate, transitions),
        PostWriteStart::Committed(_) => return Err("close A committed before analysis".into()),
    };
    assert!(transitions.is_empty());
    let baseline = gate
        .baseline
        .as_ref()
        .ok_or("gate A omitted its baseline")?
        .snapshot
        .clone();

    let close_b = close_active_gate(&store, &gate_b, "op-close-b", "close-b")?;
    assert!(close_b.decision.authorizes());

    let finish = || PostWriteFinish {
        snapshot: Some(baseline.clone()),
        protected_semantic_inputs: Vec::new(),
        reconciled_baseline: Some(baseline.clone()),
        changed_paths: Vec::new(),
        actual_write_set: Some(Default::default()),
        alias_closures: Vec::new(),
        reconciled_transition_sequences: Vec::new(),
        attempted_semantic_inputs: Vec::new(),
        signals: Vec::new(),
        deltas: Vec::new(),
    };
    let first = close_a.finish_post_write("close-a", &gate_a, finish(), |_, _, signals| {
        close_finalization(Vec::new(), signals)
    })?;
    assert!(!first.decision.authorizes());
    assert!(first.actual_write_set.is_none());
    assert!(
        first
            .signals
            .contains(&GateSignal::TransitionCatalogChanged)
    );

    let retry = close_a.finish_post_write("close-a", &gate_a, finish(), |_, _, signals| {
        close_finalization(Vec::new(), signals)
    })?;
    assert_eq!(retry, first);
    let persisted = store.load_gate(&gate_a)?;
    assert!(
        persisted
            .revisions
            .last()
            .ok_or("gate A omitted the stale revision")?
            .actual_write_set
            .is_none()
    );
    Ok(())
}

#[test]
fn active_gate_catalog_order_and_revision_increment() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;

    // Initially empty
    let snapshot = store.list_active_gates(None, 100)?;
    assert_eq!(snapshot.total, 0);
    assert_eq!(snapshot.revision, 0);

    // Open two gates with different sources to get different transition_sequences
    let gate_a = open_active_gate(&store, "op-a", "digest-a", "src/a.ts")?;
    let after_a = store.list_active_gates(None, 100)?;
    assert_eq!(after_a.total, 1);
    assert_eq!(after_a.revision, 1);
    assert_eq!(after_a.items[0].gate_id, gate_a);

    // Exact committed open retry does not advance active membership revision.
    let retry_source = path("src/a.ts")?;
    let retry_options = options();
    let retry_digest = lumin_evidence::pre_write_request_digest(
        std::slice::from_ref(&retry_source),
        &retry_options.scan_invocation,
    );
    let retry_path = RepoPath::from_canonical_bytes(&retry_source.canonical)?;
    let retry_lease = observed_lease(root.path(), &retry_path)?;
    let retry_open = store.begin_operation(&OperationId::from_string("op-a".to_owned()))?;
    assert!(matches!(
        retry_open.reserve_pre_write(
            &retry_digest,
            std::slice::from_ref(&retry_source),
            std::slice::from_ref(&retry_lease),
            &retry_options,
            rejected_test_observation,
        )?,
        PreWriteStart::Committed(result) if result.gate_id == gate_a
    ));
    assert_eq!(store.list_active_gates(None, 100)?.revision, 1);

    let gate_b = open_active_gate(&store, "op-b", "digest-b", "src/b.ts")?;
    let after_b = store.list_active_gates(None, 100)?;
    assert_eq!(after_b.total, 2);
    assert_eq!(after_b.revision, 2);
    // Order: transition_sequence ASC then gate_id ASC
    assert_eq!(after_b.items[0].gate_id, gate_a);
    assert_eq!(after_b.items[1].gate_id, gate_b);
    assert!(
        after_b.items[0].opening_transition_sequence
            <= after_b.items[1].opening_transition_sequence
    );

    // Abandon gate_a ??revision increments, gate_a disappears
    let abandon_op = OperationId::from_string("op-abandon-a".to_owned());
    let session = store.begin_operation(&abandon_op)?;
    session.abandon_gate("abandon-digest", &gate_a, 0, "test abandon")?;
    let after_abandon = store.list_active_gates(None, 100)?;
    assert_eq!(after_abandon.total, 1);
    assert_eq!(after_abandon.revision, 3);
    assert_eq!(after_abandon.items[0].gate_id, gate_b);
    session.abandon_gate("abandon-digest", &gate_a, 0, "test abandon")?;
    assert_eq!(store.list_active_gates(None, 100)?.revision, 3);

    let closed = close_active_gate(&store, &gate_b, "op-close-b", "close-digest-b")?;
    assert!(closed.decision.authorizes());
    let after_close = store.list_active_gates(None, 100)?;
    assert_eq!(after_close.total, 0);
    assert_eq!(after_close.revision, 4);
    let retried = close_active_gate(&store, &gate_b, "op-close-b", "close-digest-b")?;
    assert_eq!(retried, closed);
    assert_eq!(store.list_active_gates(None, 100)?.revision, 4);

    Ok(())
}

#[test]
fn active_gate_catalog_advances_when_a_sealed_close_replaces_protected_reads()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let prior = semantic_input("config/prior.json", "prior")?;
    let gate_id = open_active_gate_with_protected_inputs(
        &store,
        "op-catalog-protected-open",
        "catalog-protected-open",
        "src/catalog-protected.ts",
        vec![prior],
    )?;
    assert_eq!(store.list_active_gates(None, 100)?.revision, 1);

    let operation = store.begin_operation(&OperationId::from_string(
        "op-catalog-protected-close".to_owned(),
    ))?;
    let gate = match operation.begin_post_write("catalog-protected-close", &gate_id)? {
        PostWriteStart::Analyze { gate, .. } => gate,
        PostWriteStart::Committed(_) => return Err("protected-read close committed early".into()),
    };
    let baseline = gate
        .baseline
        .as_ref()
        .ok_or("protected-read gate omitted its baseline")?
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
        "catalog-protected-close",
        &gate_id,
        PostWriteFinish {
            snapshot: Some(current_snapshot),
            protected_semantic_inputs: vec![current],
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
    assert_eq!(result.decision, GateDecision::Incomplete);
    assert_eq!(result.lifecycle, GateLifecycle::Active);
    let active = store.list_active_gates(None, 100)?;
    assert_eq!(active.total, 1);
    assert_eq!(active.revision, 2);
    Ok(())
}

#[test]
fn active_gate_catalog_stale_revision_exits_correctly() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let _gate_a = open_active_gate(&store, "op-stale-a", "digest-sa", "src/a.ts")?;
    let snapshot = store.list_active_gates(None, 100)?;
    assert_eq!(snapshot.revision, 1);
    let _gate_b = open_active_gate(&store, "op-stale-b", "digest-sb", "src/b.ts")?;

    // Use old revision as cursor
    let stale_cursor = super::ActiveGateCatalogCursor {
        repository_id: snapshot.repository_id.clone(),
        revision: 1, // stale - store is now at revision 2
        page_size: 100,
        opening_sequence: snapshot.items[0].opening_transition_sequence,
        gate_id: snapshot.items[0].gate_id.clone(),
    };
    let result = store.list_active_gates(Some(&stale_cursor), 100);
    assert!(matches!(
        result,
        Err(StoreError::ActiveGateCatalogRevisionChanged {
            expected: 1,
            observed: 2
        })
    ));
    Ok(())
}

#[test]
fn active_gate_catalog_page_boundary() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    // Open 3 active gates with page size 2
    let gate_a = open_active_gate(&store, "op-page-a", "digest-pa", "src/a.ts")?;
    let gate_b = open_active_gate(&store, "op-page-b", "digest-pb", "src/b.ts")?;
    let gate_c = open_active_gate(&store, "op-page-c", "digest-pc", "src/c.ts")?;

    let page1 = store.list_active_gates(None, 2)?;
    assert_eq!(page1.total, 3);
    assert_eq!(page1.items.len(), 2);
    assert!(page1.truncated);

    let cursor = super::ActiveGateCatalogCursor {
        repository_id: page1.repository_id.clone(),
        revision: page1.revision,
        page_size: 2,
        opening_sequence: page1.items[1].opening_transition_sequence,
        gate_id: page1.items[1].gate_id.clone(),
    };
    let page2 = store.list_active_gates(Some(&cursor), 2)?;
    assert_eq!(page2.total, 3);
    assert_eq!(page2.items.len(), 1);
    assert!(!page2.truncated);

    // Verify all 3 gates appear across pages
    let mut all_gate_ids: Vec<String> = page1
        .items
        .iter()
        .chain(page2.items.iter())
        .map(|item| item.gate_id.as_str().to_owned())
        .collect();
    all_gate_ids.sort();
    let mut expected: Vec<String> = vec![
        gate_a.as_str().to_owned(),
        gate_b.as_str().to_owned(),
        gate_c.as_str().to_owned(),
    ];
    expected.sort();
    assert_eq!(all_gate_ids, expected);
    Ok(())
}

#[test]
fn active_gate_catalog_scope_mismatch() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let _gate = open_active_gate(&store, "op-scope", "digest-scope", "src/x.ts")?;
    let snapshot = store.list_active_gates(None, 100)?;

    // Wrong repository_id
    let wrong_repo_cursor = super::ActiveGateCatalogCursor {
        repository_id: lumin_model::RepositoryId::from_string("wrong-repo".to_owned()),
        revision: snapshot.revision,
        page_size: 100,
        opening_sequence: snapshot.items[0].opening_transition_sequence,
        gate_id: snapshot.items[0].gate_id.clone(),
    };
    assert!(matches!(
        store.list_active_gates(Some(&wrong_repo_cursor), 100),
        Err(StoreError::ActiveGateCatalogScopeMismatch)
    ));

    // Wrong page_size
    let wrong_page_cursor = super::ActiveGateCatalogCursor {
        repository_id: snapshot.repository_id.clone(),
        revision: snapshot.revision,
        page_size: 50, // cursor says 50, but we pass limit=100
        opening_sequence: snapshot.items[0].opening_transition_sequence,
        gate_id: snapshot.items[0].gate_id.clone(),
    };
    assert!(matches!(
        store.list_active_gates(Some(&wrong_page_cursor), 100),
        Err(StoreError::ActiveGateCatalogScopeMismatch)
    ));
    Ok(())
}

#[test]
fn empty_active_gate_catalog_rejects_a_forged_cursor() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let snapshot = store.list_active_gates(None, 100)?;
    let cursor = super::ActiveGateCatalogCursor {
        repository_id: snapshot.repository_id,
        revision: snapshot.revision,
        page_size: 100,
        opening_sequence: 0,
        gate_id: GateId::from_string("gate-forged".to_owned()),
    };

    assert!(matches!(
        store.list_active_gates(Some(&cursor), 100),
        Err(StoreError::ActiveGateCatalogAnchorMissing(gate_id))
            if gate_id == "gate-forged"
    ));
    Ok(())
}

#[test]
fn active_gate_catalog_rejects_invalid_page_size() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    assert!(matches!(
        store.list_active_gates(None, 0),
        Err(StoreError::ActiveGateCatalogPageSize {
            requested: 0,
            max: 100
        })
    ));
    Ok(())
}

#[test]
fn active_gate_catalog_rejects_a_record_key_mismatch() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate(&store, "op-key", "digest-key", "src/key.ts")?;
    let gate = store.load_gate(&gate_id)?;
    store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        records::write_record(&write, GATES, "gate-wrong-key", &gate)?;
        guard.commit(write)
    })?;

    assert!(matches!(
        store.list_active_gates(None, 100),
        Err(StoreError::Integrity(message)) if message.contains("disagrees with gate_id")
    ));
    Ok(())
}

#[test]
fn active_gate_catalog_rejects_an_active_gate_without_baseline()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate(
        &store,
        "op-no-baseline",
        "digest-no-baseline",
        "src/no-baseline.ts",
    )?;
    let mut gate = store.load_gate(&gate_id)?;
    gate.baseline = None;
    store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        records::write_record(&write, GATES, gate_id.as_str(), &gate)?;
        guard.commit(write)
    })?;

    assert!(matches!(
        store.list_active_gates(None, 100),
        Err(StoreError::Integrity(message))
            if message.contains("complete gate revision")
    ));
    Ok(())
}

#[test]
fn active_gate_catalog_rejects_an_active_gate_tombstone() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate(
        &store,
        "op-tombstone",
        "digest-tombstone",
        "src/tombstone.ts",
    )?;
    store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        let key = crate::retention::records::tombstone_key(
            lumin_evidence::RetentionItemKind::Gate,
            gate_id.as_str(),
        );
        records::write_record(
            &write,
            crate::retention::RETENTION_TOMBSTONES,
            &key,
            &"test-tombstone",
        )?;
        guard.commit(write)
    })?;

    assert!(matches!(
        store.list_active_gates(None, 100),
        Err(StoreError::Integrity(message)) if message.contains("has a tombstone")
    ));
    Ok(())
}
