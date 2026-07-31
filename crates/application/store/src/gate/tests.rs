use lumin_evidence::{
    CapabilityRecord, DEAD_CODE_CAPABILITY_ID, RunEvidence, SemanticInputState,
    seal_analysis_snapshot,
};
use lumin_model::{CapabilityState, RepoPath};

use super::*;

mod abandon;
mod liveness;

fn open_store(root: &std::path::Path) -> Result<RepositoryStore, StoreError> {
    let admission = lumin_inventory::repository_admission(root)
        .map_err(|error| StoreError::Integrity(error.to_string()))?;
    RepositoryStore::open(&admission.canonical_root, &admission.binding)
}

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
        committed_unix_millis: None,
        decision: lumin_evidence::GateDecision::Allow,
        reason: None,
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
fn pre_write_semantic_read_reservation_blocks_later_write_admission()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let reader_operation = OperationId::from_string("op-reader".to_owned());
    let reader_path = path("src/new.ts")?;
    let options = GateAnalysisOptions {
        jobs: 1,
        resolution_profile: None,
    };
    let reader = store.begin_operation(&reader_operation)?;
    let reader_gate = match reader.reserve_pre_write(
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
    let store = open_store(root.path())?;
    let operation_id = OperationId::from_string("op-open".to_owned());
    let source = path("src/new.ts")?;
    let source_lease = lease(source.clone());
    let options = GateAnalysisOptions {
        jobs: 1,
        resolution_profile: None,
    };
    let operation = store.begin_operation(&operation_id)?;
    let (gate_id, transition_sequence) = match operation.reserve_pre_write(
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
    };
    let opening = store.begin_operation(&opening_operation)?;
    let (gate_id, transition_sequence) = match opening.reserve_pre_write(
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
    let opened = opening.finish_pre_write(
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
    };
    let reader_operation = OperationId::from_string("op-alias-reader".to_owned());
    let reader_source = path("src/new.ts")?;
    let reader = store.begin_operation(&reader_operation)?;
    let reader_gate = match reader.reserve_pre_write(
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

fn open_active_gate(
    store: &RepositoryStore,
    operation_id: &str,
    request_digest: &str,
    source: &str,
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
        PostWriteStart::Committed(result) => return Ok(result),
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
                alias_closures: Vec::new(),
                reconciled_transition_sequences: Vec::new(),
                signals: Vec::new(),
                deltas: Vec::new(),
            },
        )
        .map_err(Into::into)
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
    let retry_open = store.begin_operation(&OperationId::from_string("op-a".to_owned()))?;
    assert!(matches!(
        retry_open.reserve_pre_write(
            "digest-a",
            std::slice::from_ref(&retry_source),
            &[lease(retry_source.clone())],
            &options(),
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

    // Abandon gate_a → revision increments, gate_a disappears
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
        Err(StoreError::Integrity(message)) if message.contains("omitted its baseline")
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
