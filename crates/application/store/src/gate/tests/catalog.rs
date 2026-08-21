use super::*;

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
    let retry_open = store.begin_operation(&OperationId::from_string("op-a".to_owned()))?;
    assert!(matches!(
        retry_open.reserve_pre_write(
            "digest-a",
            std::slice::from_ref(&retry_source),
            &[lease(retry_source.clone())],
            &options(),
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
