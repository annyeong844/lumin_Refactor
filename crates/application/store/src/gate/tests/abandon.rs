use super::*;

#[test]
fn abandon_is_atomic_idempotent_and_creates_one_terminal_revision()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate_with_protected_inputs(
        &store,
        "op-abandon-open",
        "abandon-open-digest",
        "src/abandoned.ts",
        vec![semantic_input("config/base.json", "protected")?],
    )?;
    let transition_owner = open_active_gate(
        &store,
        "op-abandon-transition-owner",
        "abandon-transition-owner",
        "src/abandon-transition-owner.ts",
    )?;
    let transition = close_active_gate(
        &store,
        &transition_owner,
        "op-abandon-transition-close",
        "abandon-transition-close",
    )?;
    assert!(transition.decision.authorizes());
    let before = store.load_gate(&gate_id)?;
    assert!(!before.leased_write_set.is_empty());
    assert!(!before.protected_semantic_inputs.is_empty());
    assert_eq!(before.transition_refs, [1]);

    let operation_id = OperationId::from_string("op-abandon".to_owned());
    let first = store.begin_operation(&operation_id)?.abandon_gate(
        "abandon-digest",
        &gate_id,
        0,
        "planned edit cancelled",
    )?;
    assert_eq!(first.lifecycle, GateLifecycle::Abandoned);
    assert_eq!(first.decision, lumin_evidence::GateDecision::Deny);
    assert!(first.observation_binding.is_none());
    assert_eq!(first.revision, 1);
    assert_eq!(first.reason.as_deref(), Some("planned edit cancelled"));

    let gate = store.load_gate(&gate_id)?;
    assert_eq!(gate.lifecycle, GateLifecycle::Abandoned);
    assert_eq!(gate.current_revision, 1);
    assert!(gate.leased_write_set.is_empty());
    assert!(gate.alias_closures.is_empty());
    assert!(gate.transition_refs.is_empty());
    assert!(gate.protected_semantic_inputs.is_empty());
    assert_eq!(gate.revisions.len(), 2);
    assert_eq!(
        gate.revisions[1].decision,
        lumin_evidence::GateDecision::Deny
    );
    assert!(gate.revisions[1].catalog_revision.is_none());
    assert!(gate.revisions[1].observation_binding.is_none());
    assert_eq!(
        gate.revisions[1].reason.as_deref(),
        Some("planned edit cancelled")
    );

    let operation = store.load_operation(&operation_id)?;
    assert_eq!(operation.kind, GateOperationKind::GateAbandon);
    assert_eq!(operation.status, GateOperationStatus::Committed);
    assert_eq!(operation.target_revision, 0);
    assert_eq!(operation.reason.as_deref(), Some("planned edit cancelled"));
    assert_eq!(operation.result.as_ref(), Some(&first));

    let retry = store.begin_operation(&operation_id)?.abandon_gate(
        "abandon-digest",
        &gate_id,
        0,
        "planned edit cancelled",
    )?;
    assert_eq!(retry, first);
    assert!(matches!(
        store
            .begin_operation(&operation_id)?
            .abandon_gate("different-digest", &gate_id, 0, "different reason"),
        Err(StoreError::OperationConflict(id)) if id == operation_id.as_str()
    ));

    let second_id = OperationId::from_string("op-second-abandon".to_owned());
    assert!(matches!(
        store.begin_operation(&second_id)?.abandon_gate(
            "second-abandon-digest",
            &gate_id,
            1,
            "second terminal attempt",
        ),
        Err(StoreError::GateNotActive(id)) if id == gate_id.as_str()
    ));
    assert_eq!(store.load_gate(&gate_id)?.revisions.len(), 2);
    Ok(())
}

#[test]
fn abandon_rejects_stale_revision_and_a_live_close() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let gate_id = open_active_gate(&store, "op-busy-open", "busy-open-digest", "src/busy.ts")?;

    let stale_id = OperationId::from_string("op-stale-abandon".to_owned());
    assert!(matches!(
        store.begin_operation(&stale_id)?.abandon_gate(
            "stale-abandon-digest",
            &gate_id,
            1,
            "stale request",
        ),
        Err(StoreError::GateRevisionChanged(_))
    ));
    assert!(matches!(
        store.load_operation(&stale_id),
        Err(StoreError::OperationNotFound(_))
    ));

    let close_id = OperationId::from_string("op-live-close".to_owned());
    let close = store.begin_operation(&close_id)?;
    assert!(matches!(
        close.begin_post_write("live-close-digest", &gate_id)?,
        PostWriteStart::Analyze { .. }
    ));
    let abandon_id = OperationId::from_string("op-busy-abandon".to_owned());
    assert!(matches!(
        store.begin_operation(&abandon_id)?.abandon_gate(
            "busy-abandon-digest",
            &gate_id,
            0,
            "close still active",
        ),
        Err(StoreError::GateRevisionBusy(_))
    ));
    assert_eq!(store.load_gate(&gate_id)?.lifecycle, GateLifecycle::Active);
    Ok(())
}
