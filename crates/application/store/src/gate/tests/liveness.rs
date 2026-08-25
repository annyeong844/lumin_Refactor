use super::*;

#[test]
fn process_death_releases_pre_write_reservations_and_allows_same_operation_retry()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    run_death_fixture("pre-write", root.path(), None)?;
    let store = open_store(root.path())?;
    let operation_id = OperationId::from_string("op-dead-pre-write".to_owned());

    let interrupted = store.load_operation(&operation_id)?;
    assert_eq!(interrupted.status, GateOperationStatus::Interrupted);
    assert_eq!(interrupted.interruption_count, 1);
    assert!(interrupted.leased_write_set.is_empty());
    assert!(interrupted.semantic_read_reservations.is_empty());
    assert!(interrupted.operation_liveness.is_none());

    let demanded = path("config/dead-reader.json")?;
    let writer_id = OperationId::from_string("op-after-dead-reader".to_owned());
    let writer = store.begin_operation(&writer_id)?;
    let writer_digest = canonical_pre_write_digest(&demanded);
    let writer_lease = current_lease(root.path(), &demanded)?;
    let writer_gate = match writer.reserve_pre_write(
        &writer_digest,
        std::slice::from_ref(&demanded),
        std::slice::from_ref(&writer_lease),
        &options(),
        rejected_test_observation,
    )? {
        PreWriteStart::Analyze { gate_id, .. } => gate_id,
        PreWriteStart::Committed(_) => {
            return Err("a dead semantic-read reservation still blocked a writer".into());
        }
    };
    writer.finish_pre_write(
        &writer_digest,
        &writer_gate,
        PreWriteFinish {
            baseline: None,
            leased_write_set: vec![writer_lease],
            alias_closures: Vec::new(),
            attempted_semantic_inputs: Vec::new(),
            signals: vec![GateSignal::AnalysisFailed {
                detail: "test rejection".to_owned(),
            }],
        },
        |_, _, signals| baseline_finalization(Vec::new(), signals),
    )?;
    let committed_writer = store.load_operation(&writer_id)?;
    assert_eq!(committed_writer.status, GateOperationStatus::Committed);
    assert!(committed_writer.semantic_read_reservations.is_empty());
    assert!(committed_writer.operation_liveness.is_none());

    let retry = store.begin_operation(&operation_id)?;
    let retried_source = path("src/dead-reader.ts")?;
    let retried_digest = canonical_pre_write_digest(&retried_source);
    let retried_lease = current_lease(root.path(), &retried_source)?;
    let retried_gate = match retry.reserve_pre_write(
        &retried_digest,
        std::slice::from_ref(&retried_source),
        std::slice::from_ref(&retried_lease),
        &options(),
        rejected_test_observation,
    )? {
        PreWriteStart::Analyze { gate_id, .. } => gate_id,
        PreWriteStart::Committed(_) => return Err("interrupted operation did not retry".into()),
    };
    assert_eq!(retried_gate, interrupted.gate_id);
    let pending = store.load_operation(&operation_id)?;
    assert_eq!(pending.status, GateOperationStatus::Pending);
    assert_eq!(pending.interruption_count, 1);
    assert!(pending.operation_liveness.is_some());
    Ok(())
}

#[test]
fn process_death_releases_post_write_revision_without_mutating_the_gate()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let active_source = path("src/active.ts")?;
    let active_digest = canonical_pre_write_digest(&active_source);
    let gate_id = open_active_gate(&store, "op-active-gate", &active_digest, "src/active.ts")?;
    run_death_fixture("post-write", root.path(), Some(&gate_id))?;

    let dead_operation = OperationId::from_string("op-dead-post-write".to_owned());
    let interrupted = store.load_operation(&dead_operation)?;
    assert_eq!(interrupted.status, GateOperationStatus::Interrupted);
    assert_eq!(interrupted.interruption_count, 1);
    assert!(interrupted.semantic_read_reservations.is_empty());
    assert_eq!(store.load_gate(&gate_id)?.current_revision, 0);

    let replacement_id = OperationId::from_string("op-replacement-post-write".to_owned());
    let replacement = store.begin_operation(&replacement_id)?;
    let post_write_digest = lumin_evidence::post_write_request_digest(&gate_id);
    assert!(matches!(
        replacement.begin_post_write(&post_write_digest, &gate_id)?,
        PostWriteStart::Analyze { .. }
    ));

    let retry = store.begin_operation(&dead_operation)?;
    assert!(matches!(
        retry.begin_post_write(&post_write_digest, &gate_id),
        Err(StoreError::GateRevisionBusy(_))
    ));
    assert_eq!(store.load_gate(&gate_id)?.current_revision, 0);
    Ok(())
}

#[test]
fn a_live_operation_session_cannot_be_duplicated() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let operation_id = OperationId::from_string("op-live-session".to_owned());
    let _live = store.begin_operation(&operation_id)?;
    assert!(matches!(
        store.begin_operation(&operation_id),
        Err(StoreError::OperationBusy(id)) if id == operation_id.as_str()
    ));
    Ok(())
}

#[test]
fn replacing_a_pending_operation_lock_cannot_authorize_interruption()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    run_death_fixture("pre-write", root.path(), None)?;
    let operation_id = OperationId::from_string("op-dead-pre-write".to_owned());

    let lock_path = root.path().join(".lumin").join(format!(
        "operation-liveness-{}.lock",
        lumin_model::digest_hex(operation_id.as_str().as_bytes())
    ));
    let displaced_path = lock_path.with_extension("lock.displaced");
    std::fs::rename(&lock_path, &displaced_path)?;
    std::fs::copy(&displaced_path, &lock_path)?;

    let store = open_store(root.path())?;
    assert!(matches!(
        store.begin_operation(&operation_id),
        Err(StoreError::Integrity(detail))
            if detail.contains("operation liveness lock physical identity changed")
    ));
    assert!(matches!(
        store.load_operation(&operation_id),
        Err(StoreError::Integrity(detail))
            if detail.contains("operation liveness lock physical identity changed")
    ));

    std::fs::remove_file(&lock_path)?;
    std::fs::rename(&displaced_path, &lock_path)?;
    let _retry = store.begin_operation(&operation_id)?;
    let interrupted = store.load_operation(&operation_id)?;
    assert_eq!(interrupted.status, GateOperationStatus::Interrupted);
    assert_eq!(interrupted.interruption_count, 1);
    Ok(())
}

#[test]
fn old_generation_operation_reopens_before_any_late_mutation()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let operation_id = OperationId::from_string("op-generation-change".to_owned());
    let stale = store.begin_operation(&operation_id)?;
    store.rewrite_current_store_header_as_prior_for_test()?;
    let observed = store.migrate_lifecycle_store()?;
    let source = path("src/generation.ts")?;
    let request_digest = canonical_pre_write_digest(&source);
    let source_lease = current_lease(root.path(), &source)?;

    assert!(matches!(
        stale.reserve_pre_write(
            &request_digest,
            std::slice::from_ref(&source),
            std::slice::from_ref(&source_lease),
            &options(),
            rejected_test_observation,
        ),
        Err(StoreError::StoreGenerationChanged {
            expected,
            observed: actual,
        }) if expected == crate::StoreGeneration::INITIAL && actual == observed
    ));
    assert!(matches!(
        store.load_operation(&operation_id),
        Err(StoreError::OperationNotFound(_))
    ));

    drop(stale);
    let reopened = store.begin_operation(&operation_id)?;
    assert!(matches!(
        reopened.reserve_pre_write(
            &request_digest,
            std::slice::from_ref(&source),
            std::slice::from_ref(&source_lease),
            &options(),
            rejected_test_observation,
        )?,
        PreWriteStart::Analyze { .. }
    ));
    Ok(())
}

#[test]
fn process_death_pre_write_fixture() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("LUMIN_GATE_DEATH_FIXTURE").as_deref() != Ok("pre-write") {
        return Ok(());
    }
    let root = std::path::PathBuf::from(std::env::var("LUMIN_GATE_DEATH_ROOT")?);
    let store = open_store(&root)?;
    let operation_id = OperationId::from_string("op-dead-pre-write".to_owned());
    let session = store.begin_operation(&operation_id)?;
    let source = path("src/dead-reader.ts")?;
    let request_digest = canonical_pre_write_digest(&source);
    let source_lease = current_lease(&root, &source)?;
    let gate_id = match session.reserve_pre_write(
        &request_digest,
        std::slice::from_ref(&source),
        std::slice::from_ref(&source_lease),
        &options(),
        rejected_test_observation,
    )? {
        PreWriteStart::Analyze { gate_id, .. } => gate_id,
        PreWriteStart::Committed(_) => return Err("fixture operation committed early".into()),
    };
    let demanded = path("config/dead-reader.json")?;
    assert_eq!(
        session.reserve_pre_write_semantic_inputs(
            &request_digest,
            &gate_id,
            &[reservation(demanded, None)],
        )?,
        SemanticReadReservation::Reserved
    );
    std::process::exit(91)
}

#[test]
fn process_death_post_write_fixture() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("LUMIN_GATE_DEATH_FIXTURE").as_deref() != Ok("post-write") {
        return Ok(());
    }
    let root = std::path::PathBuf::from(std::env::var("LUMIN_GATE_DEATH_ROOT")?);
    let gate_id = GateId::from_string(std::env::var("LUMIN_GATE_DEATH_GATE")?);
    let store = open_store(&root)?;
    let operation_id = OperationId::from_string("op-dead-post-write".to_owned());
    let session = store.begin_operation(&operation_id)?;
    let request_digest = lumin_evidence::post_write_request_digest(&gate_id);
    assert!(matches!(
        session.begin_post_write(&request_digest, &gate_id)?,
        PostWriteStart::Analyze { .. }
    ));
    let demanded = path("config/dead-close.json")?;
    assert_eq!(
        session.reserve_post_write_semantic_inputs(
            &request_digest,
            &gate_id,
            &[reservation(demanded, None)],
        )?,
        SemanticReadReservation::Reserved
    );
    std::process::exit(91)
}

fn run_death_fixture(
    fixture: &str,
    root: &std::path::Path,
    gate_id: Option<&GateId>,
) -> Result<(), Box<dyn std::error::Error>> {
    let test_name = format!(
        "gate::tests::liveness::process_death_{}_fixture",
        fixture.replace('-', "_")
    );
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .arg("--exact")
        .arg(test_name)
        .arg("--nocapture")
        .env("LUMIN_GATE_DEATH_FIXTURE", fixture)
        .env("LUMIN_GATE_DEATH_ROOT", root);
    if let Some(gate_id) = gate_id {
        command.env("LUMIN_GATE_DEATH_GATE", gate_id.as_str());
    }
    let status = command.status()?;
    if status.code() != Some(91) {
        return Err(format!("process-death fixture exited with {status}").into());
    }
    Ok(())
}

fn canonical_pre_write_digest(path: &RepoPathProjection) -> String {
    let options = options();
    lumin_evidence::pre_write_request_digest(std::slice::from_ref(path), &options.scan_invocation)
}

fn current_lease(
    root: &std::path::Path,
    path: &RepoPathProjection,
) -> Result<WriteLease, Box<dyn std::error::Error>> {
    let path = RepoPath::from_canonical_bytes(&path.canonical)?;
    observed_lease(root, &path)
}
