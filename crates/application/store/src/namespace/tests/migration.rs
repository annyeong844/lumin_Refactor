mod integrity;

use std::fs;

use lumin_evidence::{
    ActualWriteSet, CapabilityRecord, DEAD_CODE_CAPABILITY_ID, GateAnalysisOptions,
    GateBaselineObservationInput, GateCloseObservationInput, GateObservationBinding, GateSignal,
    RepoPathProjection, RunEvidence, SemanticInputRecord, SemanticInputState,
    SemanticReadReservationBinding, UnsealedGateObservationInputs, WriteLease, WriteLeaseKind,
    apply_worktree_transition, derive_gate_baseline_observation_id,
    derive_gate_close_observation_id, derive_unsealed_gate_observation_binding, gate_policy,
    seal_analysis_snapshot,
};
use lumin_model::{
    CapabilityState, GateId, ObservationBinding, OperationId, RepoPath, SealedGateObservation,
    UnsealedObservationReason,
};

use crate::{
    GateBaselineDraft, ObservationFinalization, PostWriteFinish, PostWriteStart, PreWriteFinish,
    PreWriteStart, RepositoryStore, SemanticReadReservation, StoreError, StoreGeneration,
};

use super::super::migration::{MigrationCrashPoint, migrate_with_hook};
use super::open_store;

const CRASH_POINTS: [MigrationCrashPoint; 8] = [
    MigrationCrashPoint::PendingIntentCreated,
    MigrationCrashPoint::IntentPrepared,
    MigrationCrashPoint::IntentRenamed,
    MigrationCrashPoint::IntentPublished,
    MigrationCrashPoint::CopiesValidated,
    MigrationCrashPoint::CanonicalReplaced,
    MigrationCrashPoint::ParentFlushed,
    MigrationCrashPoint::IntentRemoved,
];

fn rejected_test_observation(_signals: &[GateSignal]) -> GateObservationBinding {
    ObservationBinding::Unsealed {
        reason: UnsealedObservationReason::AdmissionConflict,
        attempted_domain: Vec::new(),
        last_complete_read_set: Vec::new(),
        conflicting_or_unbounded_inputs: Vec::new(),
    }
}

#[test]
fn migration_preserves_run_gate_and_pending_operation_records()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let evidence = evidence();
    let mut attempt = store.begin_attempt()?;
    let published = store.publish_run(&mut attempt, &evidence, |_| Ok(()))?;
    let gate_id = open_active_gate(&store)?;
    let gate_before = store.load_gate(&gate_id)?;

    let operation_id = OperationId::from_string("op-migrate-pending".to_owned());
    let session = store.begin_operation(&operation_id)?;
    let source = path("src/pending.ts")?;
    assert!(matches!(
        session.reserve_pre_write(
            "migrate-pending-digest",
            std::slice::from_ref(&source),
            &[lease(source.clone())],
            &options(),
            rejected_test_observation,
        )?,
        PreWriteStart::Analyze { .. }
    ));
    let before = store.load_operation(&operation_id)?;

    assert_eq!(
        store.migrate_lifecycle_store()?,
        StoreGeneration::INITIAL
            .checked_next()
            .ok_or("missing generation")?
    );
    assert_eq!(store.latest_run_id()?, Some(published.run_id.clone()));
    assert_eq!(store.load_run(&published.run_id)?.1, evidence);
    assert_eq!(store.load_gate(&gate_id)?, gate_before);
    assert_eq!(store.load_operation(&operation_id)?, before);
    assert!(matches!(
        session.reserve_pre_write(
            "migrate-pending-digest",
            &[],
            &[],
            &options(),
            rejected_test_observation,
        ),
        Err(StoreError::StoreGenerationChanged { .. })
    ));
    assert_migration_paths_absent(root.path())?;
    Ok(())
}

#[test]
fn migration_rejects_changed_attempt_liveness_lock_before_copy()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let attempt = store.begin_attempt()?;
    let lock_path = fs::read_dir(root.path().join(".lumin"))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .find(|path| {
            path.file_name()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|name| {
                    name.starts_with("attempt-liveness-") && name.ends_with(".lock")
                })
        })
        .ok_or("active attempt omitted its liveness lock")?;
    drop(attempt);
    fs::write(lock_path, b"changed liveness binding")?;

    let result = store.migrate_lifecycle_store();
    assert!(matches!(
        result,
        Err(StoreError::Integrity(message)) if message.contains("lock contents changed")
    ));
    let state = root.path().join(".lumin");
    assert!(state.join("lifecycle-migration.json").is_file());
    assert!(!state.join("lifecycle.store.migration-source").exists());
    assert!(!state.join("lifecycle.store.migration-target").exists());
    Ok(())
}

#[test]
fn every_migration_process_death_boundary_recovers_on_reopen()
-> Result<(), Box<dyn std::error::Error>> {
    for point in CRASH_POINTS {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        let evidence = evidence();
        let mut attempt = store.begin_attempt()?;
        let published = store.publish_run(&mut attempt, &evidence, |_| Ok(()))?;
        drop(store);

        run_death_fixture(root.path(), point)?;

        let recovered = open_store(root.path())?;
        let expected_generation = match point {
            MigrationCrashPoint::PendingIntentCreated | MigrationCrashPoint::IntentPrepared => {
                StoreGeneration::INITIAL
            }
            _ => next_generation()?,
        };
        assert_eq!(current_generation(&recovered)?, expected_generation);
        assert_eq!(recovered.latest_run_id()?, Some(published.run_id.clone()));
        assert_eq!(recovered.load_run(&published.run_id)?.1, evidence);
        assert_migration_paths_absent(root.path())?;
    }
    Ok(())
}

#[test]
fn live_migration_intent_blocks_ordinary_store_work() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    inject_crash(&store, MigrationCrashPoint::IntentPublished)?;
    assert!(matches!(
        store.begin_attempt(),
        Err(StoreError::LifecycleMigrationPending { .. })
    ));
    Ok(())
}

#[test]
fn retry_after_intent_removal_finishes_cleanup_without_advancing_again()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    inject_crash(&store, MigrationCrashPoint::IntentRemoved)?;
    assert!(matches!(
        store.begin_attempt(),
        Err(StoreError::LifecycleMigrationCleanupPending)
    ));

    assert_eq!(store.migrate_lifecycle_store()?, next_generation()?);
    assert_eq!(current_generation(&store)?, next_generation()?);
    assert_migration_paths_absent(root.path())?;
    Ok(())
}

#[test]
fn external_payload_change_before_replace_keeps_source_generation_authoritative()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let mut attempt = store.begin_attempt()?;
    let published = store.publish_run(&mut attempt, &evidence(), |_| Ok(()))?;
    let evidence_path = root
        .path()
        .join(".lumin/runs")
        .join(published.run_id.as_str())
        .join("evidence.store");

    let result = store.namespace.with_migration_lock(|guard| {
        migrate_with_hook(guard, &mut |point| {
            if point == MigrationCrashPoint::CopiesValidated {
                fs::write(&evidence_path, b"tampered evidence").map_err(crate::io_error)?;
            }
            Ok(())
        })
    });
    assert!(matches!(result, Err(StoreError::Integrity(_))));
    let observed = store
        .namespace
        .with_migration_lock(|guard| Ok(guard.open_database()?.generation()))?;
    assert_eq!(observed, StoreGeneration::INITIAL);
    Ok(())
}

#[test]
fn missing_canonical_store_during_live_migration_is_a_hard_stop()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    inject_crash(&store, MigrationCrashPoint::IntentPublished)?;
    fs::remove_file(root.path().join(".lumin/lifecycle.store"))?;
    drop(store);

    assert!(matches!(
        open_store(root.path()),
        Err(StoreError::Integrity(_))
    ));
    Ok(())
}

#[test]
fn orphaned_target_never_bootstraps_a_new_canonical_store() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let canonical = root.path().join(".lumin/lifecycle.store");
    let target = root.path().join(".lumin/lifecycle.store.migration-target");
    drop(store);
    fs::copy(&canonical, target)?;
    fs::remove_file(&canonical)?;

    assert!(matches!(
        open_store(root.path()),
        Err(StoreError::Integrity(_))
    ));
    assert!(!canonical.exists());
    Ok(())
}

#[test]
fn process_death_migration_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let Ok(label) = std::env::var("LUMIN_MIGRATION_DEATH_POINT") else {
        return Ok(());
    };
    let root = std::path::PathBuf::from(std::env::var("LUMIN_MIGRATION_DEATH_ROOT")?);
    let point = crash_point(&label)?;
    let store = open_store(&root)?;
    let _ = store.namespace.with_migration_lock(|guard| {
        migrate_with_hook(guard, &mut |observed| {
            if observed == point {
                std::process::exit(92);
            }
            Ok(())
        })
    });
    Err(format!("migration death fixture did not reach {label}").into())
}

fn inject_crash(
    store: &RepositoryStore,
    point: MigrationCrashPoint,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut injected = false;
    let result = store.namespace.with_migration_lock(|guard| {
        migrate_with_hook(guard, &mut |observed| {
            if observed == point && !injected {
                injected = true;
                return Err(StoreError::Integrity(format!(
                    "injected migration crash at {point:?}"
                )));
            }
            Ok(())
        })
    });
    assert!(injected);
    assert!(matches!(result, Err(StoreError::Integrity(_))));
    Ok(())
}

fn run_death_fixture(
    root: &std::path::Path,
    point: MigrationCrashPoint,
) -> Result<(), Box<dyn std::error::Error>> {
    let status = std::process::Command::new(std::env::current_exe()?)
        .arg("--exact")
        .arg("namespace::tests::migration::process_death_migration_fixture")
        .arg("--nocapture")
        .env("LUMIN_MIGRATION_DEATH_POINT", crash_point_label(point))
        .env("LUMIN_MIGRATION_DEATH_ROOT", root)
        .status()?;
    if status.code() != Some(92) {
        return Err(format!("migration death fixture exited with {status}").into());
    }
    Ok(())
}

fn crash_point_label(point: MigrationCrashPoint) -> &'static str {
    match point {
        MigrationCrashPoint::PendingIntentCreated => "after-pending-intent-create",
        MigrationCrashPoint::IntentPrepared => "after-pending-intent-sync",
        MigrationCrashPoint::IntentRenamed => "after-intent-rename",
        MigrationCrashPoint::IntentPublished => "after-intent",
        MigrationCrashPoint::CopiesValidated => "after-validated-replacement",
        MigrationCrashPoint::CanonicalReplaced => "after-replace",
        MigrationCrashPoint::ParentFlushed => "after-parent-flush",
        MigrationCrashPoint::IntentRemoved => "after-intent-removal",
    }
}

fn crash_point(label: &str) -> Result<MigrationCrashPoint, Box<dyn std::error::Error>> {
    match label {
        "after-pending-intent-create" => Ok(MigrationCrashPoint::PendingIntentCreated),
        "after-pending-intent-sync" => Ok(MigrationCrashPoint::IntentPrepared),
        "after-intent-rename" => Ok(MigrationCrashPoint::IntentRenamed),
        "after-intent" => Ok(MigrationCrashPoint::IntentPublished),
        "after-validated-replacement" => Ok(MigrationCrashPoint::CopiesValidated),
        "after-replace" => Ok(MigrationCrashPoint::CanonicalReplaced),
        "after-parent-flush" => Ok(MigrationCrashPoint::ParentFlushed),
        "after-intent-removal" => Ok(MigrationCrashPoint::IntentRemoved),
        _ => Err(format!("unknown migration death point: {label}").into()),
    }
}

fn current_generation(store: &RepositoryStore) -> Result<StoreGeneration, StoreError> {
    store.with_shared_lock(|guard| Ok(guard.open_database()?.generation()))
}

fn next_generation() -> Result<StoreGeneration, Box<dyn std::error::Error>> {
    StoreGeneration::INITIAL
        .checked_next()
        .ok_or_else(|| "missing next generation".into())
}

fn evidence() -> RunEvidence {
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
    }
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
        kind: WriteLeaseKind::ExistingFile,
        physical_identity: None,
        nearest_existing_parent: None,
        prefix_identities: Vec::new(),
    }
}

fn open_active_gate(store: &RepositoryStore) -> Result<GateId, Box<dyn std::error::Error>> {
    open_active_gate_for(store, "op-migrate-gate", "src/active.ts")
}

fn open_active_gate_for(
    store: &RepositoryStore,
    operation: &str,
    source: &str,
) -> Result<GateId, Box<dyn std::error::Error>> {
    open_active_gate_for_with_protected_inputs(store, operation, source, Vec::new())
}

fn open_active_gate_for_with_protected_inputs(
    store: &RepositoryStore,
    operation: &str,
    source: &str,
    protected_semantic_inputs: Vec<SemanticInputRecord>,
) -> Result<GateId, Box<dyn std::error::Error>> {
    let operation_id = OperationId::from_string(operation.to_owned());
    let session = store.begin_operation(&operation_id)?;
    let source = path(source)?;
    let source_lease = lease(source.clone());
    let (gate_id, transition_sequence) = match session.reserve_pre_write(
        "migrate-gate-digest",
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
    let baseline = GateBaselineDraft {
        analysis_contract: "migration-test-contract".to_owned(),
        snapshot: seal_analysis_snapshot(
            protected_semantic_inputs.clone(),
            evidence(),
            Default::default(),
            Vec::new(),
        ),
        protected_semantic_inputs,
        transition_sequence,
    };
    let baseline_for_id = baseline.clone();
    let source_for_id = source.clone();
    let lease_for_id = source_lease.clone();
    session.finish_pre_write(
        "migrate-gate-digest",
        &gate_id,
        PreWriteFinish {
            baseline: Some(baseline),
            leased_write_set: vec![source_lease],
            alias_closures: Vec::new(),
            attempted_semantic_inputs: Vec::new(),
            signals: Vec::new(),
        },
        |_, catalog_revision, _| ObservationFinalization {
            signals: Vec::new(),
            binding: ObservationBinding::Sealed {
                observation: SealedGateObservation::Baseline {
                    observation_id: derive_gate_baseline_observation_id(
                        GateBaselineObservationInput {
                            catalog_revision,
                            transition_sequence: baseline_for_id.transition_sequence,
                            analysis_contract: &baseline_for_id.analysis_contract,
                            analysis_input_id: &baseline_for_id.snapshot.analysis_input_id,
                            declared_write_set: std::slice::from_ref(&source_for_id),
                            leased_write_set: std::slice::from_ref(&lease_for_id),
                            alias_closures: &[],
                            protected_semantic_inputs: &baseline_for_id.protected_semantic_inputs,
                        },
                    ),
                },
            },
        },
    )?;
    Ok(gate_id)
}

fn semantic_input(value: &str) -> Result<SemanticInputRecord, Box<dyn std::error::Error>> {
    Ok(SemanticInputRecord {
        path: path(value)?,
        state: SemanticInputState::ConfigPresent,
        payload_sha256: Some(format!("payload-{value}")),
        physical_identity: None,
        absence_parent: None,
        physical_redirect_sha256: None,
    })
}

fn append_non_authorizing_close_for_migration(
    store: &RepositoryStore,
    gate_id: &GateId,
    protected_semantic_inputs: Vec<SemanticInputRecord>,
) -> Result<(), Box<dyn std::error::Error>> {
    let operation_id = OperationId::from_string("op-migrate-incomplete-close".to_owned());
    let session = store.begin_operation(&operation_id)?;
    let gate = match session.begin_post_write("migrate-incomplete-close-digest", gate_id)? {
        PostWriteStart::Analyze { gate, .. } => gate,
        PostWriteStart::Committed(_) => return Err("migration close committed early".into()),
    };
    let baseline = gate
        .baseline
        .as_ref()
        .ok_or("migration close omitted its baseline")?;
    let mut current_evidence = evidence();
    current_evidence
        .capabilities
        .first_mut()
        .ok_or("migration close evidence omitted its required capability")?
        .state = CapabilityState::Failed;
    let snapshot = seal_analysis_snapshot(
        protected_semantic_inputs.clone(),
        current_evidence,
        Default::default(),
        Vec::new(),
    );
    let (signals, _, deltas) = gate_policy::closing_signals(
        &baseline.snapshot,
        &snapshot,
        &gate.protected_semantic_inputs,
        &gate.leased_write_set,
    );
    if gate_policy::decision(&signals).authorizes() {
        return Err("migration non-authorizing close fixture unexpectedly authorized".into());
    }
    let actual_write_set = ActualWriteSet::default();
    let opening_observation_id = baseline.observation_id.clone();
    let opening_analysis_contract = baseline.analysis_contract.clone();
    let prior_revision = gate.current_revision;
    let leased_write_set = gate.leased_write_set.clone();
    let alias_closures = gate.alias_closures.clone();
    let analysis_input_id = snapshot.analysis_input_id.clone();
    let actual_write_set_for_id = actual_write_set.clone();
    let protected_for_id = protected_semantic_inputs.clone();
    let aliases_for_id = alias_closures.clone();
    session.finish_post_write(
        "migrate-incomplete-close-digest",
        gate_id,
        PostWriteFinish {
            snapshot: Some(snapshot),
            protected_semantic_inputs,
            reconciled_baseline: Some(baseline.snapshot.clone()),
            changed_paths: Vec::new(),
            actual_write_set: Some(actual_write_set),
            alias_closures,
            reconciled_transition_sequences: Vec::new(),
            attempted_semantic_inputs: Vec::new(),
            signals,
            deltas,
        },
        |_, catalog_revision, _| ObservationFinalization {
            signals: Vec::new(),
            binding: ObservationBinding::Sealed {
                observation: SealedGateObservation::Close {
                    observation_id: derive_gate_close_observation_id(GateCloseObservationInput {
                        gate_id,
                        opening_observation_id: &opening_observation_id,
                        opening_analysis_contract: &opening_analysis_contract,
                        prior_revision,
                        catalog_revision,
                        analysis_input_id: &analysis_input_id,
                        leased_write_set: &leased_write_set,
                        protected_semantic_inputs: &protected_for_id,
                        changed_paths: &[],
                        actual_write_set: &actual_write_set_for_id,
                        alias_closures: &aliases_for_id,
                        reconciled_transition_sequences: &[],
                    }),
                },
            },
        },
    )?;
    Ok(())
}

fn close_active_gate_for_migration(
    store: &RepositoryStore,
    gate_id: &GateId,
) -> Result<(), Box<dyn std::error::Error>> {
    let operation_id = OperationId::from_string(format!("op-migrate-close-{}", gate_id.as_str()));
    let request_digest = format!("migrate-close-digest-{}", gate_id.as_str());
    let session = store.begin_operation(&operation_id)?;
    let (gate, transitions) = match session.begin_post_write(&request_digest, gate_id)? {
        PostWriteStart::Analyze {
            gate, transitions, ..
        } => (gate, transitions),
        PostWriteStart::Committed(_) => return Err("migration close committed early".into()),
    };
    let baseline = gate
        .baseline
        .as_ref()
        .ok_or("migration close omitted its baseline")?;
    let mut reconciled_baseline = baseline.snapshot.clone();
    let mut reconciled_transition_sequences = Vec::with_capacity(transitions.len());
    for transition in &transitions {
        if !apply_worktree_transition(&mut reconciled_baseline, transition) {
            return Err(format!(
                "migration fixture could not replay transition {}",
                transition.sequence
            )
            .into());
        }
        reconciled_transition_sequences.push(transition.sequence);
    }
    let source = gate
        .declared_write_set
        .first()
        .cloned()
        .ok_or("migration close omitted its declared source")?;
    let mut current_inputs = reconciled_baseline.inputs.clone();
    current_inputs.push(SemanticInputRecord {
        path: source.clone(),
        state: SemanticInputState::Source,
        payload_sha256: Some(format!("payload-{}", gate_id.as_str())),
        physical_identity: None,
        absence_parent: None,
        physical_redirect_sha256: None,
    });
    let snapshot = seal_analysis_snapshot(
        current_inputs,
        reconciled_baseline.evidence.clone(),
        reconciled_baseline.scan_invocation.clone(),
        reconciled_baseline.entry_selections.clone(),
    );
    let protected_semantic_inputs = baseline.protected_semantic_inputs.clone();
    let changed_paths = vec![source];
    let actual_write_set = ActualWriteSet {
        paths: changed_paths.clone(),
        ..ActualWriteSet::default()
    };
    let opening_observation_id = baseline.observation_id.clone();
    let opening_analysis_contract = baseline.analysis_contract.clone();
    let prior_revision = gate.current_revision;
    let leased_write_set = gate.leased_write_set.clone();
    let alias_closures = gate.alias_closures.clone();
    let analysis_input_id = snapshot.analysis_input_id.clone();
    let actual_write_set_for_id = actual_write_set.clone();
    let protected_for_id = protected_semantic_inputs.clone();
    let aliases_for_id = alias_closures.clone();
    let changed_paths_for_id = changed_paths.clone();
    let reconciled_sequences_for_id = reconciled_transition_sequences.clone();
    session.finish_post_write(
        &request_digest,
        gate_id,
        PostWriteFinish {
            snapshot: Some(snapshot.clone()),
            protected_semantic_inputs,
            reconciled_baseline: Some(reconciled_baseline),
            changed_paths,
            actual_write_set: Some(actual_write_set),
            alias_closures,
            reconciled_transition_sequences,
            attempted_semantic_inputs: Vec::new(),
            signals: Vec::new(),
            deltas: Vec::new(),
        },
        |_, catalog_revision, _| ObservationFinalization {
            signals: Vec::new(),
            binding: ObservationBinding::Sealed {
                observation: SealedGateObservation::Close {
                    observation_id: derive_gate_close_observation_id(GateCloseObservationInput {
                        gate_id,
                        opening_observation_id: &opening_observation_id,
                        opening_analysis_contract: &opening_analysis_contract,
                        prior_revision,
                        catalog_revision,
                        analysis_input_id: &analysis_input_id,
                        leased_write_set: &leased_write_set,
                        protected_semantic_inputs: &protected_for_id,
                        changed_paths: &changed_paths_for_id,
                        actual_write_set: &actual_write_set_for_id,
                        alias_closures: &aliases_for_id,
                        reconciled_transition_sequences: &reconciled_sequences_for_id,
                    }),
                },
            },
        },
    )?;
    Ok(())
}

fn append_unsealed_close_for_migration(
    store: &RepositoryStore,
    gate_id: &GateId,
) -> Result<OperationId, Box<dyn std::error::Error>> {
    let operation_id =
        OperationId::from_string(format!("op-migrate-unsealed-close-{}", gate_id.as_str()));
    let request_digest = format!("migrate-unsealed-close-digest-{}", gate_id.as_str());
    let session = store.begin_operation(&operation_id)?;
    let gate = match session.begin_post_write(&request_digest, gate_id)? {
        PostWriteStart::Analyze { gate, .. } => gate,
        PostWriteStart::Committed(_) => return Err("migration close committed early".into()),
    };
    let attempted = SemanticReadReservationBinding {
        path: path("config/unsealed-attempt.json")?,
        physical_identity: None,
        absence_parent: None,
    };
    if session.reserve_post_write_semantic_inputs(
        &request_digest,
        gate_id,
        std::slice::from_ref(&attempted),
    )? != SemanticReadReservation::Reserved
    {
        return Err("migration close could not reserve its attempted semantic input".into());
    }
    let signals = vec![GateSignal::AnalysisFailed {
        detail: "injected migration fixture failure".to_owned(),
    }];
    let inputs = UnsealedGateObservationInputs::new(
        gate.leased_write_set.clone(),
        vec![attempted.clone()],
        gate.protected_semantic_inputs
            .iter()
            .map(|input| input.path.clone())
            .collect(),
    );
    session.finish_post_write(
        &request_digest,
        gate_id,
        PostWriteFinish {
            snapshot: None,
            protected_semantic_inputs: Vec::new(),
            reconciled_baseline: None,
            changed_paths: Vec::new(),
            actual_write_set: None,
            alias_closures: gate.alias_closures.clone(),
            reconciled_transition_sequences: Vec::new(),
            attempted_semantic_inputs: vec![attempted],
            signals,
            deltas: Vec::new(),
        },
        |_, _, signals| ObservationFinalization {
            signals: Vec::new(),
            binding: derive_unsealed_gate_observation_binding(&[], &inputs, signals),
        },
    )?;
    Ok(operation_id)
}

fn assert_migration_paths_absent(root: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let state = root.join(".lumin");
    for name in [
        "lifecycle-migration.json",
        "lifecycle.store.migration-source",
        "lifecycle.store.migration-target",
    ] {
        if state.join(name).exists() {
            return Err(format!("migration path still exists: {name}").into());
        }
    }
    Ok(())
}
