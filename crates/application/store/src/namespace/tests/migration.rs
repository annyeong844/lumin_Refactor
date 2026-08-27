mod integrity;

use std::fs;

use lumin_evidence::{
    ActualWriteSet, CacheCleanupExecutionLease, CacheCleanupOperationRecord,
    CacheCleanupOperationStatus, CacheEvictionAuthorization, CapabilityRecord,
    DeclaredPathUnsupportedReason, GateAnalysisOptions, GateBaselineObservationInput,
    GateCloseObservationInput, GateObservationBinding, GateSignal, PathPrefixIdentity,
    PostWriteFinalValidationEvidence, PreWriteDeclaredPathInspection,
    PreWriteFinalValidationEvidence, RUN_EVIDENCE_CAPABILITY_IDS, RepoPathProjection,
    RetentionPlanScope, RunEvidence, SUPPORTED_ACTIVE_GATE_ANALYSIS_CONTRACT_ID,
    SemanticInputRecord, SemanticInputState, SemanticReadReservationBinding,
    UnsealedGateObservationInputs, WriteLease, WriteLeaseKind, apply_worktree_transition,
    derive_gate_baseline_observation_id, derive_gate_close_observation_id,
    derive_protected_semantic_inputs, derive_unsealed_gate_observation_binding, gate_policy,
    post_write_request_digest, pre_write_request_digest, seal_analysis_snapshot,
};
use lumin_model::{
    AttemptId, AttemptStatus, CacheEvictionAuthorizationSetId, CapabilityState, GateId,
    ObservationBinding, OperationId, PhysicalFileIdentity, RepoPath, SealedGateObservation,
    UnsealedObservationReason,
};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

use crate::{
    ATTEMPT_LEASES, AttemptEnvelope, GateBaselineDraft, ObservationFinalization, POINTERS,
    PostWriteFinish, PostWriteStart, PreWriteFinish, PreWriteStart,
    PriorCacheCleanupDeliveryStatusForTest, RepositoryStore, RetentionPlanRequest, SEQUENCES,
    SemanticReadReservation, StoreError, StoreGeneration,
};

use super::super::migration::{MigrationCrashPoint, migrate_with_hook};
use super::super::platform::{EntryAccess, EntryKind, HeldEntry};
use super::open_store;

const RETAINED_MIGRATION_SOURCE_PREFIX: &str =
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "lifecycle.store.migration-target-"
    } else {
        "lifecycle.store.migration-source-"
    };

const CRASH_POINTS: &[MigrationCrashPoint] = &[
    MigrationCrashPoint::PendingIntentCreated,
    MigrationCrashPoint::RootAuthorizationCommitted,
    MigrationCrashPoint::IntentPrepared,
    MigrationCrashPoint::RootCandidateWriteStarted,
    MigrationCrashPoint::RootCandidatePartiallyWritten,
    MigrationCrashPoint::RootCandidateWritten,
    MigrationCrashPoint::RootNamePublished,
    MigrationCrashPoint::RootReopened,
    MigrationCrashPoint::RootFileFlushed,
    MigrationCrashPoint::RootParentFlushed,
    MigrationCrashPoint::IntentRenamed,
    MigrationCrashPoint::IntentPublished,
    MigrationCrashPoint::RevisionCandidateCreated,
    MigrationCrashPoint::RevisionCandidateWriteStarted,
    MigrationCrashPoint::RevisionCandidatePartiallyWritten,
    MigrationCrashPoint::RevisionCandidateWritten,
    MigrationCrashPoint::RevisionNamePublished,
    MigrationCrashPoint::RevisionReopened,
    MigrationCrashPoint::RevisionFileFlushed,
    MigrationCrashPoint::RevisionParentFlushed,
    MigrationCrashPoint::CopiesValidated,
    MigrationCrashPoint::TargetNamePublished,
    MigrationCrashPoint::TargetReopened,
    MigrationCrashPoint::TargetFileFlushed,
    MigrationCrashPoint::TargetParentFlushed,
    MigrationCrashPoint::TargetPublished,
    MigrationCrashPoint::BeforeExchange,
    MigrationCrashPoint::ExchangeInputsOpened,
    MigrationCrashPoint::ExchangeExternalReferencesValidated,
    #[cfg(windows)]
    MigrationCrashPoint::SourceRetired,
    #[cfg(windows)]
    MigrationCrashPoint::CanonicalMoveExternalReferencesValidated,
    MigrationCrashPoint::CanonicalReplaced,
    MigrationCrashPoint::ParentFlushed,
    MigrationCrashPoint::IntentRemoved,
    MigrationCrashPoint::TerminalSourceValidated,
];

#[test]
fn prior_store_migrates_once_and_retains_terminal_provenance()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    super::super::store_header::rewrite_current_store_header_as_prior_for_test(
        &root.path().join(".lumin/lifecycle.store"),
        &store.namespace.binding,
    )?;

    let migrated = store.migrate_lifecycle_store()?;
    assert_eq!(migrated, next_generation()?);
    assert_eq!(store.migrate_lifecycle_store()?, migrated);
    assert!(
        root.path()
            .join(".lumin/lifecycle-migration.json")
            .is_file()
    );
    assert!(fs::read_dir(root.path().join(".lumin"))?.any(|entry| {
        entry.is_ok_and(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(RETAINED_MIGRATION_SOURCE_PREFIX)
        })
    }));
    Ok(())
}

fn rejected_test_observation(_signals: &[GateSignal]) -> GateObservationBinding {
    ObservationBinding::Unsealed {
        reason: UnsealedObservationReason::AdmissionConflict,
        attempted_domain: Vec::new(),
        last_complete_read_set: Vec::new(),
        conflicting_or_unbounded_inputs: Vec::new(),
    }
}

fn clean_pre_write_final_validation_evidence(
    semantic_inputs: Vec<SemanticInputRecord>,
    leased_write_set: Vec<WriteLease>,
) -> PreWriteFinalValidationEvidence {
    PreWriteFinalValidationEvidence {
        expected_semantic_read_bindings: Vec::new(),
        observed_semantic_read_bindings: Vec::new(),
        observed_semantic_inputs: semantic_inputs,
        observed_leased_write_set: leased_write_set,
        observed_alias_closures: Vec::new(),
        write_domain_drift_paths: Vec::new(),
        semantic_input_validation_drift_paths: Vec::new(),
    }
}

fn clean_post_write_final_validation_evidence(
    semantic_inputs: Vec<SemanticInputRecord>,
    leased_write_set: Vec<WriteLease>,
    alias_closures: Vec<lumin_evidence::PhysicalAliasClosureRecord>,
) -> PostWriteFinalValidationEvidence {
    PostWriteFinalValidationEvidence {
        expected_leased_write_set: leased_write_set.clone(),
        expected_alias_closures: alias_closures.clone(),
        observation: PreWriteFinalValidationEvidence {
            expected_semantic_read_bindings: Vec::new(),
            observed_semantic_read_bindings: Vec::new(),
            observed_semantic_inputs: semantic_inputs,
            observed_leased_write_set: leased_write_set,
            observed_alias_closures: alias_closures,
            write_domain_drift_paths: Vec::new(),
            semantic_input_validation_drift_paths: Vec::new(),
        },
    }
}

#[test]
fn migration_preserves_an_admission_conflict_without_final_validation()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let owner_gate_id =
        open_active_gate_for(&store, "op-admission-conflict-owner", "src/conflict.ts")?;

    let operation_id = OperationId::from_string("op-admission-conflict-rejected".to_owned());
    let session = store.begin_operation(&operation_id)?;
    let source = path("src/conflict.ts")?;
    let source_lease = lease(source.clone());
    let analysis_options = options();
    let request_digest = pre_write_digest(std::slice::from_ref(&source), &analysis_options);
    let unsealed_inputs =
        UnsealedGateObservationInputs::new(vec![source_lease.clone()], Vec::new(), Vec::new());
    let source_for_binding = source.clone();
    let rejected = match session.reserve_pre_write(
        &request_digest,
        std::slice::from_ref(&source),
        std::slice::from_ref(&source_lease),
        &analysis_options,
        |signals| {
            derive_unsealed_gate_observation_binding(
                std::slice::from_ref(&source_for_binding),
                &unsealed_inputs,
                signals,
            )
        },
    )? {
        PreWriteStart::Committed(result) => *result,
        PreWriteStart::Analyze { .. } => {
            return Err("conflicting pre-write unexpectedly reached analysis".into());
        }
    };
    assert!(matches!(
        rejected.signals.as_slice(),
        [GateSignal::WriteConflict { .. }]
    ));
    let persisted = store.load_operation(&operation_id)?;
    let admission = persisted
        .pre_write_admission_evidence
        .as_ref()
        .ok_or("admission rejection omitted its operation-owned evidence")?;
    assert!(!admission.conflict_owners.is_empty());
    assert!(persisted.pre_write_final_validation.is_none());
    close_active_gate_for_migration(&store, &owner_gate_id)?;
    make_prior_store(&store, root.path())?;
    store.migrate_lifecycle_store()?;
    assert_eq!(
        store.replay_pre_write_result(&operation_id, &request_digest)?,
        Some(rejected)
    );
    Ok(())
}

#[test]
fn migration_preserves_run_gate_and_pending_operation_records()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(
        root.path().join("src/pending.ts"),
        b"export const pending = true;\n",
    )?;
    let store = open_store(root.path())?;
    let evidence = evidence();
    let mut attempt = store.begin_attempt()?;
    let published = store.publish_run(&mut attempt, &evidence, |_| Ok(()))?;
    let gate_id = open_active_gate(&store)?;
    let gate_before = store.load_gate(&gate_id)?;

    let operation_id = OperationId::from_string("op-migrate-pending".to_owned());
    let session = store.begin_operation(&operation_id)?;
    let source_path = RepoPath::from_portable("src/pending.ts")?;
    let source = RepoPathProjection::from(&source_path);
    let source_lease = observed_lease(root.path(), &source_path)?;
    let analysis_options = options();
    let request_digest = pre_write_digest(std::slice::from_ref(&source), &analysis_options);
    assert!(matches!(
        session.reserve_pre_write(
            &request_digest,
            std::slice::from_ref(&source),
            std::slice::from_ref(&source_lease),
            &analysis_options,
            rejected_test_observation,
        )?,
        PreWriteStart::Analyze { .. }
    ));
    let before = store.load_operation(&operation_id)?;
    make_prior_store(&store, root.path())?;

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
            &request_digest,
            &[],
            &[],
            &analysis_options,
            rejected_test_observation,
        ),
        Err(StoreError::StoreGenerationChanged { .. })
    ));
    assert_terminal_migration_paths(root.path())?;
    Ok(())
}

#[test]
fn migration_preserves_a_pending_rejected_path_inspection() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let operation_id = OperationId::from_string("op-migrate-pending-rejection".to_owned());
    let session = store.begin_operation(&operation_id)?;
    let path = RepoPathProjection::from(&RepoPath::from_portable("notes/new.txt")?);
    let rejection = GateSignal::DeclaredPathUnsupported {
        path: path.clone(),
        reason: DeclaredPathUnsupportedReason::NotAnalyzedSource,
    };
    let inspection = PreWriteDeclaredPathInspection {
        path: path.clone(),
        lease: None,
        rejection: Some(rejection),
    };
    let analysis_options = options();
    let request_digest = pre_write_digest(std::slice::from_ref(&path), &analysis_options);
    assert!(matches!(
        session.reserve_pre_write_with_inspection(
            &request_digest,
            std::slice::from_ref(&path),
            &[],
            std::slice::from_ref(&inspection),
            &analysis_options,
            rejected_test_observation,
        )?,
        PreWriteStart::Analyze { .. }
    ));
    let before = store.load_operation(&operation_id)?;
    make_prior_store(&store, root.path())?;

    store.migrate_lifecycle_store()?;

    assert_eq!(store.load_operation(&operation_id)?, before);
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
    make_prior_store(&store, root.path())?;

    let result = store.migrate_lifecycle_store();
    assert!(
        matches!(
            &result,
            Err(StoreError::Integrity(message)) if message.contains("lock contents changed")
        ),
        "migration returned an unexpected liveness result: {result:?}"
    );
    let state = root.path().join(".lumin");
    assert!(!state.join("lifecycle-migration.json").exists());
    Ok(())
}

#[test]
fn migration_rejects_attempt_allocator_regression_and_exhaustion()
-> Result<(), Box<dyn std::error::Error>> {
    for (observed, expected) in [
        (0, "attempt sequence regressed below retained allocation"),
        (u64::MAX, "attempt sequence is exhausted"),
    ] {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        let mut attempt = store.begin_attempt()?;
        store.fail_attempt(&mut attempt, "retained allocator owner")?;
        set_sequence_for_test(&store, "attempt", observed)?;
        make_prior_store(&store, root.path())?;

        assert!(matches!(
            store.migrate_lifecycle_store(),
            Err(StoreError::Integrity(message)) if message.contains(expected)
        ));
    }
    Ok(())
}

#[test]
fn migration_validates_every_retained_attempt_envelope() -> Result<(), Box<dyn std::error::Error>> {
    for mutation in ["schema", "owner", "pending"] {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        let mut first = store.begin_attempt()?;
        let first_id = first.attempt_id().clone();
        store.fail_attempt(&mut first, "older retained attempt")?;
        let mut second = store.begin_attempt()?;
        let second_id = second.attempt_id().clone();
        store.fail_attempt(&mut second, "latest retained attempt")?;
        make_prior_store(&store, root.path())?;

        let path = root
            .path()
            .join(".lumin/attempts")
            .join(first_id.as_str())
            .join("attempt.json");
        if mutation == "pending" {
            fs::copy(&path, path.with_extension("json.pending"))?;
        } else {
            let mut envelope = serde_json::from_slice::<serde_json::Value>(&fs::read(&path)?)?;
            match mutation {
                "schema" => envelope["schemaVersion"] = "lumin-attempt.foreign".into(),
                "owner" => {
                    envelope["attemptId"] = second_id.as_str().into();
                    envelope["sequence"] = 2_u64.into();
                }
                _ => unreachable!(),
            }
            let mut bytes = serde_json::to_vec_pretty(&envelope)?;
            bytes.push(b'\n');
            fs::write(path, bytes)?;
        }

        assert!(
            matches!(
                store.migrate_lifecycle_store(),
                Err(StoreError::Integrity(_))
            ),
            "migration accepted {mutation} corruption in a non-latest attempt"
        );
    }
    Ok(())
}

#[test]
fn migration_requires_every_live_run_to_have_a_completed_attempt_owner()
-> Result<(), Box<dyn std::error::Error>> {
    for mutation in ["missing", "failed"] {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        let mut first = store.begin_attempt()?;
        let first_id = first.attempt_id().clone();
        store.publish_run(&mut first, &evidence(), |_| Ok(()))?;
        let mut second = store.begin_attempt()?;
        store.publish_run(&mut second, &evidence(), |_| Ok(()))?;

        let attempt_dir = root.path().join(".lumin/attempts").join(first_id.as_str());
        if mutation == "missing" {
            fs::remove_dir_all(&attempt_dir)?;
        } else {
            let path = attempt_dir.join("attempt.json");
            let mut envelope = serde_json::from_slice::<AttemptEnvelope>(&fs::read(&path)?)?;
            envelope.state = AttemptStatus::Failed;
            envelope.run_id = None;
            envelope.failure = Some("injected failed owner".to_owned());
            let mut bytes = serde_json::to_vec_pretty(&envelope)?;
            bytes.push(b'\n');
            fs::write(path, bytes)?;
        }
        make_prior_store(&store, root.path())?;

        let result = store.migrate_lifecycle_store();
        assert!(
            matches!(
                &result,
                Err(StoreError::Integrity(message))
                    if message.contains("is not owned by its completed attempt")
            ),
            "migration accepted a live run with a {mutation} attempt owner: {result:?}"
        );
    }
    Ok(())
}

#[test]
fn migration_rejects_retention_plan_allocator_below_a_retained_plan()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    store.prepare_retention_plan(&RetentionPlanRequest {
        scope: RetentionPlanScope::Runs {
            before_unix_millis: u64::MAX,
        },
        operation_id: OperationId::from_string("migration-plan-sequence-floor".to_owned()),
    })?;
    set_sequence_for_test(&store, "retention-plan", 0)?;
    make_prior_store(&store, root.path())?;

    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("retention-plan sequence regressed below retained allocation")
    ));
    Ok(())
}

#[test]
fn migration_rejects_unknown_sequence_allocator_rows() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    set_sequence_for_test(&store, "foreign-allocator", 1)?;
    make_prior_store(&store, root.path())?;

    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("unsupported allocator key: foreign-allocator")
    ));
    Ok(())
}

#[test]
fn migration_rejects_run_pin_allocator_below_a_retained_pin()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let mut attempt = store.begin_attempt()?;
    let published = store.publish_run(&mut attempt, &evidence(), |_| Ok(()))?;
    store.pin_run(
        &published.run_id,
        &OperationId::from_string("migration-pin-sequence-floor".to_owned()),
        "retain allocator owner",
    )?;
    set_sequence_for_test(&store, "run-pin", 0)?;
    make_prior_store(&store, root.path())?;

    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("run-pin sequence regressed below retained allocation")
    ));
    Ok(())
}

#[test]
fn migration_rejects_retention_catalog_below_a_retained_plan_revision()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    store.prepare_retention_plan(&RetentionPlanRequest {
        scope: RetentionPlanScope::Runs {
            before_unix_millis: u64::MAX,
        },
        operation_id: OperationId::from_string("migration-catalog-sequence-floor".to_owned()),
    })?;
    set_sequence_for_test(&store, "retention-catalog", 0)?;
    make_prior_store(&store, root.path())?;

    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("retention-catalog sequence regressed below retained allocation")
    ));
    Ok(())
}

#[test]
fn migration_reconciles_latest_replace_before_pointer_index_sync()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let mut attempt = store.begin_attempt()?;
    let published = store.publish_run(&mut attempt, &evidence(), |_| Ok(()))?;
    clear_latest_pointer_index_for_test(&store)?;
    make_prior_store(&store, root.path())?;

    store.migrate_lifecycle_store()?;
    assert_eq!(store.latest_run_id()?, Some(published.run_id.clone()));
    let latest = store.latest_snapshot()?;
    assert_eq!(
        latest
            .latest_attempt
            .as_ref()
            .map(|attempt| &attempt.attempt_id),
        Some(&published.attempt_id)
    );
    assert_eq!(
        latest.completed.as_ref().map(|(record, _)| &record.run_id),
        Some(&published.run_id)
    );
    Ok(())
}

#[test]
fn migration_rejects_latest_pointers_regressed_behind_completed_history()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let mut first_attempt = store.begin_attempt()?;
    let first = store.publish_run(&mut first_attempt, &evidence(), |_| Ok(()))?;
    let first_latest = fs::read(root.path().join(".lumin/latest.json"))?;

    let mut second_attempt = store.begin_attempt()?;
    store.publish_run(&mut second_attempt, &evidence(), |_| Ok(()))?;
    fs::write(root.path().join(".lumin/latest.json"), first_latest)?;
    set_latest_pointer_index_for_test(&store, &first.attempt_id, &first.run_id)?;
    make_prior_store(&store, root.path())?;

    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("regresses behind authenticated attempt history")
    ));
    Ok(())
}

#[test]
fn migration_rejects_a_pending_latest_pointer_behind_the_canonical_frontier()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let mut first_attempt = store.begin_attempt()?;
    store.publish_run(&mut first_attempt, &evidence(), |_| Ok(()))?;
    let older = fs::read(root.path().join(".lumin/latest.json"))?;

    let mut second_attempt = store.begin_attempt()?;
    store.publish_run(&mut second_attempt, &evidence(), |_| Ok(()))?;
    fs::write(root.path().join(".lumin/latest.json.pending"), older)?;
    make_prior_store(&store, root.path())?;

    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("pending latest pointer is not the exact next publication")
    ));
    Ok(())
}

#[test]
fn migration_validates_the_complete_latest_pointer_document()
-> Result<(), Box<dyn std::error::Error>> {
    for field in [
        "attempt-sequence",
        "attempt-status",
        "completed-sequence",
        "pending-attempt-status",
    ] {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        let mut attempt = store.begin_attempt()?;
        store.publish_run(&mut attempt, &evidence(), |_| Ok(()))?;
        make_prior_store(&store, root.path())?;

        let path = root.path().join(".lumin/latest.json");
        let mut latest = serde_json::from_slice::<serde_json::Value>(&fs::read(&path)?)?;
        let destination = match field {
            "attempt-sequence" => {
                let sequence = latest["latestAttempt"]["sequence"]
                    .as_u64()
                    .ok_or("latest attempt omitted its sequence")?;
                latest["latestAttempt"]["sequence"] = sequence
                    .checked_add(1)
                    .ok_or("latest attempt sequence overflowed")?
                    .into();
                path.clone()
            }
            "attempt-status" => {
                latest["latestAttempt"]["status"] = "failed".into();
                path.clone()
            }
            "completed-sequence" => {
                let sequence = latest["latestCompleted"]["sequence"]
                    .as_u64()
                    .ok_or("latest completed pointer omitted its sequence")?;
                latest["latestCompleted"]["sequence"] = sequence
                    .checked_add(1)
                    .ok_or("latest completed sequence overflowed")?
                    .into();
                path.clone()
            }
            "pending-attempt-status" => {
                latest["latestAttempt"]["status"] = "failed".into();
                path.with_extension("json.pending")
            }
            _ => unreachable!(),
        };
        let mut bytes = serde_json::to_vec_pretty(&latest)?;
        bytes.push(b'\n');
        fs::write(destination, bytes)?;

        let outcome = store.migrate_lifecycle_store();
        assert!(
            matches!(outcome, Err(StoreError::Integrity(_))),
            "migration accepted a corrupt latest pointer {field}: {outcome:?}"
        );
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn migration_rejects_non_utf8_migration_artifact_names() -> Result<(), Box<dyn std::error::Error>> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    make_prior_store(&store, root.path())?;
    let mut name = b"lifecycle-migration".to_vec();
    name.push(0xff);
    fs::write(
        root.path().join(".lumin").join(OsString::from_vec(name)),
        b"foreign",
    )?;

    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message)) if message.contains("non-UTF-8 migration journal")
    ));
    Ok(())
}

#[cfg(unix)]
#[test]
fn migration_rejects_non_utf8_private_artifacts_with_or_without_a_journal()
-> Result<(), Box<dyn std::error::Error>> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    for journal in [false, true] {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        make_prior_store(&store, root.path())?;
        if journal {
            inject_crash(&store, MigrationCrashPoint::IntentPublished)?;
        }
        let mut name = b"lifecycle.store.migration-".to_vec();
        name.push(0xff);
        let artifact = root.path().join(".lumin").join(OsString::from_vec(name));
        fs::write(&artifact, b"foreign")?;

        assert!(matches!(
            store.migrate_lifecycle_store(),
            Err(StoreError::Integrity(message))
                if message.contains("non-UTF-8 entry during migration artifact validation")
        ));
        assert_eq!(fs::read(artifact)?, b"foreign");
    }
    Ok(())
}

#[test]
fn migration_authenticates_present_releasing_attempt_locks_and_allows_absence()
-> Result<(), Box<dyn std::error::Error>> {
    for mode in ["present", "absent", "corrupt", "linked"] {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        let attempt = store.begin_attempt()?;
        let attempt_id = attempt.attempt_id().clone();
        drop(attempt);
        let lock_name = mark_attempt_releasing_for_test(&store, root.path(), attempt_id.as_str())?;
        let lock_path = root.path().join(".lumin").join(lock_name);
        make_prior_store(&store, root.path())?;

        let foreign_link = root.path().join("foreign-releasing-lock-link");
        match mode {
            "present" => {}
            "absent" => fs::remove_file(&lock_path)?,
            "corrupt" => fs::write(&lock_path, b"{}")?,
            "linked" => fs::hard_link(&lock_path, &foreign_link)?,
            _ => unreachable!(),
        }

        let result = store.migrate_lifecycle_store();
        if matches!(mode, "present" | "absent") {
            assert_eq!(result?, next_generation()?);
        } else {
            assert!(
                matches!(result, Err(StoreError::Integrity(_))),
                "migration accepted a {mode} releasing lock"
            );
            assert!(lock_path.is_file());
            if mode == "linked" {
                assert!(foreign_link.is_file());
            }
        }
    }
    Ok(())
}

#[test]
fn migration_rejects_an_incoherent_latest_attempt_envelope()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let mut attempt = store.begin_attempt()?;
    let attempt_id = attempt.attempt_id().clone();
    store.publish_run(&mut attempt, &evidence(), |_| Ok(()))?;
    make_prior_store(&store, root.path())?;

    let path = root
        .path()
        .join(".lumin/attempts")
        .join(attempt_id.as_str())
        .join("attempt.json");
    let mut envelope = serde_json::from_slice::<serde_json::Value>(&fs::read(&path)?)?;
    envelope["runId"] = serde_json::Value::Null;
    let mut bytes = serde_json::to_vec_pretty(&envelope)?;
    bytes.push(b'\n');
    fs::write(path, bytes)?;

    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("has incoherent terminal fields")
    ));
    Ok(())
}

#[test]
fn migration_rejects_a_pending_cleanup_with_a_forged_liveness_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let operation_id = OperationId::from_string("migration-pending-cleanup-liveness".to_owned());
    let session = store.begin_operation(&operation_id)?;
    let request_digest =
        lumin_evidence::cache_cleanup_request_digest(&store.namespace.binding.global.repository_id);
    let mut liveness = session.liveness().clone();
    liveness.lock_physical_identity = liveness
        .lock_physical_identity
        .take()
        .map(different_physical_identity);
    let operation = CacheCleanupOperationRecord {
        schema_version: "lumin-cache-cleanup-operation.v2".to_owned(),
        repository_id: store.namespace.binding.global.repository_id.clone(),
        operation_id: operation_id.clone(),
        request_digest,
        status: CacheCleanupOperationStatus::Pending,
        interruption_count: 0,
        invocation_id: "0".repeat(32),
        initial_authorization_set_id: CacheEvictionAuthorizationSetId::for_canonical_rows(&[]),
        initial_authorization_count: 0,
        plan_initialized: false,
        authorization_keys: Vec::new(),
        validated_count: 0,
        execution_lease: Some(CacheCleanupExecutionLease {
            execution_attempt_id: "migration-pending-cleanup-attempt".to_owned(),
            liveness,
        }),
        recovery_reservation: None,
        result: None,
        greatest_allocated_delivery_sequence: 0,
        greatest_completed_delivery_sequence: None,
        delivery_completions: Vec::new(),
    };
    store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        crate::gate::records::write_record(
            &write,
            crate::cache::CACHE_CLEANUP_OPERATIONS,
            operation_id.as_str(),
            &operation,
        )?;
        guard.commit(write)
    })?;
    store.rewrite_cache_cleanup_operation_as_prior_for_test(
        &operation_id,
        PriorCacheCleanupDeliveryStatusForTest::NotAttempted,
    )?;
    make_prior_store(&store, root.path())?;

    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("operation liveness lock physical identity changed")
    ));
    drop(session);
    Ok(())
}

#[test]
fn migration_rejects_an_unfinished_cleanup_with_a_forged_request_digest()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let operation_id = OperationId::from_string("migration-pending-cleanup-digest".to_owned());
    let session = store.begin_operation(&operation_id)?;
    let canonical =
        lumin_evidence::cache_cleanup_request_digest(&store.namespace.binding.global.repository_id);
    let operation = CacheCleanupOperationRecord {
        schema_version: "lumin-cache-cleanup-operation.v2".to_owned(),
        repository_id: store.namespace.binding.global.repository_id.clone(),
        operation_id: operation_id.clone(),
        request_digest: format!("{canonical}-forged"),
        status: CacheCleanupOperationStatus::Pending,
        interruption_count: 0,
        invocation_id: "0".repeat(32),
        initial_authorization_set_id: CacheEvictionAuthorizationSetId::for_canonical_rows(&[]),
        initial_authorization_count: 0,
        plan_initialized: false,
        authorization_keys: Vec::new(),
        validated_count: 0,
        execution_lease: Some(CacheCleanupExecutionLease {
            execution_attempt_id: "migration-pending-cleanup-digest-attempt".to_owned(),
            liveness: session.liveness().clone(),
        }),
        recovery_reservation: None,
        result: None,
        greatest_allocated_delivery_sequence: 0,
        greatest_completed_delivery_sequence: None,
        delivery_completions: Vec::new(),
    };
    store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        crate::gate::records::write_record(
            &write,
            crate::cache::CACHE_CLEANUP_OPERATIONS,
            operation_id.as_str(),
            &operation,
        )?;
        guard.commit(write)
    })?;
    store.rewrite_cache_cleanup_operation_as_prior_for_test(
        &operation_id,
        PriorCacheCleanupDeliveryStatusForTest::NotAttempted,
    )?;
    make_prior_store(&store, root.path())?;

    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("has an unauthenticated request digest")
    ));
    drop(session);
    Ok(())
}

#[test]
fn migration_rejects_an_unfinished_cleanup_with_an_exhausted_interruption_count()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let operation_id = OperationId::from_string("migration-pending-cleanup-exhausted".to_owned());
    let session = store.begin_operation(&operation_id)?;
    let request_digest =
        lumin_evidence::cache_cleanup_request_digest(&store.namespace.binding.global.repository_id);
    let operation = CacheCleanupOperationRecord {
        schema_version: "lumin-cache-cleanup-operation.v2".to_owned(),
        repository_id: store.namespace.binding.global.repository_id.clone(),
        operation_id: operation_id.clone(),
        request_digest,
        status: CacheCleanupOperationStatus::Pending,
        interruption_count: u64::MAX,
        invocation_id: "0".repeat(32),
        initial_authorization_set_id: CacheEvictionAuthorizationSetId::for_canonical_rows(&[]),
        initial_authorization_count: 0,
        plan_initialized: false,
        authorization_keys: Vec::new(),
        validated_count: 0,
        execution_lease: Some(CacheCleanupExecutionLease {
            execution_attempt_id: "migration-pending-cleanup-exhausted-attempt".to_owned(),
            liveness: session.liveness().clone(),
        }),
        recovery_reservation: None,
        result: None,
        greatest_allocated_delivery_sequence: 0,
        greatest_completed_delivery_sequence: None,
        delivery_completions: Vec::new(),
    };
    let legacy = serde_json::json!({
        "schemaVersion": "lumin-cache-cleanup-operation.v1",
        "repositoryId": operation.repository_id,
        "operationId": operation.operation_id,
        "requestDigest": operation.request_digest,
        "status": operation.status,
        "interruptionCount": operation.interruption_count,
        "invocationId": operation.invocation_id,
        "initialAuthorizationSetId": operation.initial_authorization_set_id,
        "initialAuthorizationCount": operation.initial_authorization_count,
        "planInitialized": operation.plan_initialized,
        "authorizationKeys": operation.authorization_keys,
        "validatedCount": operation.validated_count,
        "executionLease": operation.execution_lease,
        "recoveryReservation": operation.recovery_reservation,
        "result": operation.result,
        "lastDeliveryStatus": "not-attempted",
    });
    store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        crate::gate::records::write_record(
            &write,
            crate::cache::CACHE_CLEANUP_OPERATIONS,
            operation_id.as_str(),
            &legacy,
        )?;
        guard.commit(write)
    })?;
    make_prior_store(&store, root.path())?;

    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::IncompatibleStateSchema(message))
            if message.contains("private v1 cache cleanup operation")
                && message.contains("incoherent")
    ));
    drop(session);
    Ok(())
}

#[test]
fn migration_authenticates_pending_cleanup_initial_authorization_provenance()
-> Result<(), Box<dyn std::error::Error>> {
    for corrupt_identity in [true, false] {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        fs::write(root.path().join(".lumin/cache/prior.bin"), b"prior")?;
        let prior_operation =
            OperationId::from_string(format!("migration-prior-cleanup-{corrupt_identity}"));
        let request_digest = lumin_evidence::cache_cleanup_request_digest(
            &store.namespace.binding.global.repository_id,
        );
        store.clean_cache_payloads(&prior_operation, &request_digest)?;
        let initial_rows = store.with_exclusive_lock(|guard| {
            let database = guard.open_database()?;
            let write = database.begin_write()?;
            let authorizations = crate::gate::records::read_records::<CacheEvictionAuthorization>(
                &write,
                crate::cache::CACHE_EVICTION_AUTHORIZATIONS,
            )?;
            Ok(authorizations
                .iter()
                .map(crate::cache::authorization_set_frame)
                .collect::<Vec<_>>())
        })?;
        assert_eq!(initial_rows.len(), 1);
        let correct_id = CacheEvictionAuthorizationSetId::for_canonical_rows(&initial_rows);
        let correct_count = u64::try_from(initial_rows.len())?;
        store.rewrite_cache_cleanup_operation_as_prior_for_test(
            &prior_operation,
            PriorCacheCleanupDeliveryStatusForTest::NotAttempted,
        )?;

        let operation_id = OperationId::from_string(format!(
            "migration-pending-cleanup-provenance-{corrupt_identity}"
        ));
        let session = store.begin_operation(&operation_id)?;
        let operation = CacheCleanupOperationRecord {
            schema_version: "lumin-cache-cleanup-operation.v2".to_owned(),
            repository_id: store.namespace.binding.global.repository_id.clone(),
            operation_id: operation_id.clone(),
            request_digest: request_digest.clone(),
            status: CacheCleanupOperationStatus::Pending,
            interruption_count: 0,
            invocation_id: "1".repeat(32),
            initial_authorization_set_id: if corrupt_identity {
                CacheEvictionAuthorizationSetId::for_canonical_rows(&[])
            } else {
                correct_id
            },
            initial_authorization_count: if corrupt_identity { correct_count } else { 0 },
            plan_initialized: false,
            authorization_keys: Vec::new(),
            validated_count: 0,
            execution_lease: Some(CacheCleanupExecutionLease {
                execution_attempt_id: "migration-pending-cleanup-provenance-attempt".to_owned(),
                liveness: session.liveness().clone(),
            }),
            recovery_reservation: None,
            result: None,
            greatest_allocated_delivery_sequence: 0,
            greatest_completed_delivery_sequence: None,
            delivery_completions: Vec::new(),
        };
        store.with_exclusive_lock(|guard| {
            let database = guard.open_database()?;
            let write = database.begin_write()?;
            crate::gate::records::write_record(
                &write,
                crate::cache::CACHE_CLEANUP_OPERATIONS,
                operation_id.as_str(),
                &operation,
            )?;
            guard.commit(write)
        })?;
        store.rewrite_cache_cleanup_operation_as_prior_for_test(
            &operation_id,
            PriorCacheCleanupDeliveryStatusForTest::NotAttempted,
        )?;
        make_prior_store(&store, root.path())?;

        let result = store.migrate_lifecycle_store();
        assert!(
            matches!(
            &result,
            Err(StoreError::Integrity(message))
                if message.contains("initial authorization provenance is not exact")
            ),
            "unexpected migration result: {result:?}"
        );
        drop(session);
    }
    Ok(())
}

#[test]
fn native_current_reopens_after_external_reference_validation()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let current = root.path().join(".lumin/lifecycle.store");
    let mut changed = false;
    let result = store.namespace.with_migration_lock(|guard| {
        super::super::migration::validate_native_current_recheck_for_test(guard, &mut || {
            set_store_sequence_at_path(&current, "gate", 1)?;
            changed = true;
            Ok(())
        })
    });
    assert!(changed);
    assert!(matches!(
        result,
        Err(StoreError::Integrity(message))
            if message.contains("changed after external reference validation")
    ));
    Ok(())
}

#[test]
fn native_current_repeats_external_reference_validation_before_success()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let attempt_id = AttemptId::from_string("attempt_0000000000000001".to_owned());
    let attempt_dir = root
        .path()
        .join(".lumin/attempts")
        .join(attempt_id.as_str());
    let envelope = AttemptEnvelope {
        schema_version: "lumin-attempt.v1".to_owned(),
        attempt_id,
        sequence: 1,
        state: AttemptStatus::Failed,
        started_unix_millis: 1,
        finished_unix_millis: Some(1),
        run_id: None,
        failure: Some("injected after external validation".to_owned()),
    };
    let mut changed = false;
    let result = store.namespace.with_migration_lock(|guard| {
        super::super::migration::validate_native_current_recheck_for_test(guard, &mut || {
            fs::create_dir(&attempt_dir).map_err(crate::io_error)?;
            fs::write(
                attempt_dir.join("attempt.json"),
                serde_json::to_vec(&envelope).map_err(crate::serialization_error)?,
            )
            .map_err(crate::io_error)?;
            changed = true;
            Ok(())
        })
    });
    assert!(changed);
    assert!(matches!(
        result,
        Err(StoreError::Integrity(message))
            if message.contains("attempt sequence regressed below retained allocation")
    ));
    Ok(())
}

#[test]
fn migration_recovers_incomplete_attempt_allocations_before_transform()
-> Result<(), Box<dyn std::error::Error>> {
    for lock_binding in [None, Some(false), Some(true)] {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        let (attempt_id, lock_name) =
            crate::publication::reserve_migration_attempt_allocation_for_test(
                &store,
                lock_binding,
            )?;
        make_prior_store(&store, root.path())?;

        store.migrate_lifecycle_store()?;

        let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
        let read = database.begin_read()?;
        let table = read.open_table(ATTEMPT_LEASES)?;
        assert!(table.get(attempt_id.as_str())?.is_none());
        assert!(!root.path().join(".lumin").join(lock_name).exists());
        drop(table);
        drop(read);
        drop(database);

        let mut next = store.begin_attempt()?;
        assert_eq!(next.attempt_id().as_str(), "attempt_0000000000000002");
        store.fail_attempt(&mut next, "migration allocation recovery test complete")?;
    }
    Ok(())
}

#[test]
fn migration_rejects_multiple_incomplete_attempt_allocations_before_recovery()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let (first, _) =
        crate::publication::reserve_migration_attempt_allocation_for_test(&store, None)?;
    let (second, _) =
        crate::publication::reserve_migration_attempt_allocation_for_test(&store, None)?;
    make_prior_store(&store, root.path())?;

    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("multiple incomplete attempt allocations")
    ));

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let read = database.begin_read()?;
    let table = read.open_table(ATTEMPT_LEASES)?;
    assert!(table.get(first.as_str())?.is_some());
    assert!(table.get(second.as_str())?.is_some());
    Ok(())
}

#[test]
fn terminal_validation_reopens_the_target_after_external_reference_validation()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    make_prior_store(&store, root.path())?;
    let current = root.path().join(".lumin/lifecycle.store");
    let mut changed = false;
    let result = store.namespace.with_migration_lock(|guard| {
        migrate_with_hook(guard, &mut |point| {
            if point == MigrationCrashPoint::TerminalSourceValidated && !changed {
                set_store_sequence_at_path(&current, "gate", 1)?;
                changed = true;
            }
            Ok(())
        })
    });
    assert!(changed);
    assert!(matches!(
        result,
        Err(StoreError::Integrity(message))
            if message.contains("changed after external reference validation")
    ));
    Ok(())
}

#[test]
fn terminal_validation_rechecks_external_state_after_the_exact_barrier()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    make_prior_store(&store, root.path())?;
    let injected = root.path().join(".lumin/attempts/attempt_0000000000000001");
    let mut reached = false;
    let result = store.namespace.with_migration_lock(|guard| {
        migrate_with_hook(guard, &mut |point| {
            if point == MigrationCrashPoint::TerminalSourceValidated && !reached {
                fs::create_dir(&injected).map_err(crate::io_error)?;
                reached = true;
            }
            Ok(())
        })
    });
    assert!(reached, "migration skipped the terminal validation barrier");
    assert!(
        matches!(result, Err(StoreError::Integrity(_))),
        "migration accepted external state injected after validation: {result:?}"
    );
    assert!(injected.is_dir(), "migration removed unauthenticated state");

    fs::remove_dir(&injected)?;
    assert_eq!(store.migrate_lifecycle_store()?, next_generation()?);
    Ok(())
}

#[test]
fn exchange_revalidates_every_bound_input_after_external_reference_validation()
-> Result<(), Box<dyn std::error::Error>> {
    for mutation in ["journal", "source", "target"] {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        make_prior_store(&store, root.path())?;
        let state = root.path().join(".lumin");
        let canonical_path = state.join("lifecycle.store");
        let canonical = HeldEntry::open(
            &canonical_path,
            EntryKind::RegularFile,
            EntryAccess::ReadOnly,
            true,
            "migration source before final exchange validation",
        )?;
        let source_identity = canonical.identity().clone();
        drop(canonical);

        let mut reached = false;
        let mut target_path = None;
        let mut target_identity = None;
        let result = store.namespace.with_migration_lock(|guard| {
            migrate_with_hook(guard, &mut |point| {
                if point == MigrationCrashPoint::ExchangeExternalReferencesValidated && !reached {
                    let target = pending_target_path(&state)
                        .map_err(|error| StoreError::Integrity(error.to_string()))?;
                    let entry = HeldEntry::open(
                        &target,
                        EntryKind::RegularFile,
                        EntryAccess::ReadOnly,
                        true,
                        "migration target at final exchange validation barrier",
                    )?;
                    target_identity = Some(entry.identity().clone());
                    drop(entry);
                    match mutation {
                        "journal" => fs::write(journal_head_path(&state)?, b"{}\n")
                            .map_err(crate::io_error)?,
                        "source" => set_store_sequence_at_path(&canonical_path, "gate", 1)?,
                        "target" => set_store_sequence_at_path(&target, "gate", 1)?,
                        _ => unreachable!(),
                    }
                    target_path = Some(target);
                    reached = true;
                }
                Ok(())
            })
        });
        assert!(reached, "migration skipped the final exchange barrier");
        assert!(
            matches!(result, Err(StoreError::Integrity(_))),
            "unexpected {mutation} mutation result: {result:?}"
        );

        let canonical = HeldEntry::open(
            &canonical_path,
            EntryKind::RegularFile,
            EntryAccess::ReadOnly,
            true,
            "migration source after rejected exchange",
        )?;
        assert_eq!(canonical.identity(), &source_identity);
        let target = HeldEntry::open(
            target_path
                .as_deref()
                .ok_or("exchange barrier omitted target path")?,
            EntryKind::RegularFile,
            EntryAccess::ReadOnly,
            true,
            "migration target after rejected exchange",
        )?;
        assert_eq!(
            Some(target.identity()),
            target_identity.as_ref(),
            "{mutation} mutation changed durable exchange placement"
        );
    }
    Ok(())
}

#[test]
fn exchange_rechecks_link_counts_after_movement_handles_open()
-> Result<(), Box<dyn std::error::Error>> {
    for binding in ["source", "target"] {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        make_prior_store(&store, root.path())?;
        let state = root.path().join(".lumin");
        let foreign_link = root.path().join(format!("foreign-{binding}-link"));
        let mut reached = false;
        let result = store.namespace.with_migration_lock(|guard| {
            migrate_with_hook(guard, &mut |point| {
                if point == MigrationCrashPoint::ExchangeInputsOpened && !reached {
                    let protected = if binding == "source" {
                        state.join("lifecycle.store")
                    } else {
                        pending_target_path(&state)
                            .map_err(|error| StoreError::Integrity(error.to_string()))?
                    };
                    fs::hard_link(protected, &foreign_link).map_err(crate::io_error)?;
                    reached = true;
                }
                Ok(())
            })
        });
        assert!(reached, "migration skipped the exchange-input barrier");
        assert!(
            matches!(
                &result,
                Err(StoreError::Integrity(message))
                    if message.contains("exactly one physical link")
            ),
            "migration moved a multiply linked {binding}: {result:?}"
        );
        assert!(foreign_link.is_file());
        assert!(state.join("lifecycle.store").is_file());

        fs::remove_file(&foreign_link)?;
        assert_eq!(store.migrate_lifecycle_store()?, next_generation()?);
    }
    Ok(())
}

#[test]
fn exchange_rechecks_pending_write_leases_after_movement_handles_open()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let pending_path = root.path().join("src/pending.ts");
    fs::create_dir_all(
        pending_path
            .parent()
            .ok_or("pending path omitted its parent")?,
    )?;
    fs::write(&pending_path, b"export const pending = true;\n")?;
    let store = open_store(root.path())?;
    let operation_id = OperationId::from_string("op-exchange-pending-write".to_owned());
    let session = store.begin_operation(&operation_id)?;
    let source_path = RepoPath::from_portable("src/pending.ts")?;
    let source = RepoPathProjection::from(&source_path);
    let source_lease = observed_lease(root.path(), &source_path)?;
    let analysis_options = options();
    let request_digest = pre_write_digest(std::slice::from_ref(&source), &analysis_options);
    assert!(matches!(
        session.reserve_pre_write(
            &request_digest,
            std::slice::from_ref(&source),
            std::slice::from_ref(&source_lease),
            &analysis_options,
            rejected_test_observation,
        )?,
        PreWriteStart::Analyze { .. }
    ));
    make_prior_store(&store, root.path())?;

    let canonical_path = root.path().join(".lumin/lifecycle.store");
    let canonical = HeldEntry::open(
        &canonical_path,
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        true,
        "migration source before pending-lease exchange barrier",
    )?;
    let source_identity = canonical.identity().clone();
    drop(canonical);

    let replacement = root.path().join("src/pending-replacement.ts");
    fs::write(&replacement, b"export const pending = false;\n")?;
    let mut reached = false;
    let result = store.namespace.with_migration_lock(|guard| {
        migrate_with_hook(guard, &mut |point| {
            if point == MigrationCrashPoint::ExchangeInputsOpened && !reached {
                fs::remove_file(&pending_path).map_err(crate::io_error)?;
                fs::rename(&replacement, &pending_path).map_err(crate::io_error)?;
                reached = true;
            }
            Ok(())
        })
    });
    assert!(reached, "migration skipped the exchange-input barrier");
    assert!(
        matches!(result, Err(StoreError::Integrity(_))),
        "migration exchanged a store after its pending lease changed: {result:?}"
    );

    let canonical = HeldEntry::open(
        &canonical_path,
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        true,
        "migration source after rejected pending-lease exchange",
    )?;
    assert_eq!(canonical.identity(), &source_identity);
    Ok(())
}

#[test]
fn terminal_validation_rejects_a_late_unbound_migration_artifact()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    make_prior_store(&store, root.path())?;
    let injected = root.path().join(".lumin/lifecycle.store.migration-opaque");
    let mut reached = false;
    let result = store.namespace.with_migration_lock(|guard| {
        migrate_with_hook(guard, &mut |point| {
            if point == MigrationCrashPoint::TerminalSourceValidated && !reached {
                fs::write(&injected, b"unbound migration artifact").map_err(crate::io_error)?;
                reached = true;
            }
            Ok(())
        })
    });

    assert!(reached, "migration skipped the terminal validation barrier");
    assert!(matches!(
        result,
        Err(StoreError::Integrity(message))
            if message.contains("state namespace contains an unowned entry")
    ));
    assert_eq!(fs::read(&injected)?, b"unbound migration artifact");
    Ok(())
}

#[test]
fn terminal_validation_rereads_the_journal_after_external_reference_validation()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    make_prior_store(&store, root.path())?;
    let state = root.path().join(".lumin");
    let canonical = state.join("lifecycle.store");
    let mut reached = false;
    let mut canonical_before = None;
    let result = store.namespace.with_migration_lock(|guard| {
        migrate_with_hook(guard, &mut |point| {
            if point == MigrationCrashPoint::TerminalSourceValidated && !reached {
                canonical_before = Some(fs::read(&canonical).map_err(crate::io_error)?);
                fs::write(journal_head_path(&state)?, b"{}\n").map_err(crate::io_error)?;
                reached = true;
            }
            Ok(())
        })
    });
    assert!(reached, "migration skipped the terminal validation barrier");
    assert!(matches!(result, Err(StoreError::Integrity(_))));
    assert_eq!(
        fs::read(&canonical)?,
        canonical_before.ok_or("terminal barrier omitted canonical target")?
    );
    assert_terminal_migration_paths(root.path())?;
    Ok(())
}

#[test]
fn migration_rejects_extra_store_header_rows() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    make_prior_store(&store, root.path())?;
    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let definition = TableDefinition::<&str, &[u8]>::new("store-header");
        let mut table = write.open_table(definition)?;
        table.insert("foreign", b"{}".as_slice())?;
    }
    write.commit()?;
    drop(database);

    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message))
            if message.contains("header table must contain exactly its canonical row")
    ));
    Ok(())
}

#[test]
fn migration_recomputes_the_root_core_from_source_facts() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    make_prior_store(&store, root.path())?;
    inject_crash(&store, MigrationCrashPoint::IntentPublished)?;

    let root_path = root.path().join(".lumin/lifecycle-migration.json");
    let root_bytes = fs::read(&root_path)?;
    let root_value = serde_json::from_slice::<serde_json::Value>(&root_bytes)?;
    let previous = root_value
        .get("rootCoreSha256")
        .and_then(serde_json::Value::as_str)
        .ok_or("migration root omitted its core digest")?;
    let forged = "f".repeat(64);
    if previous == forged {
        return Err("migration root unexpectedly used the forged test digest".into());
    }
    fs::write(
        &root_path,
        replace_digest_bytes(&root_bytes, previous, &forged)?,
    )?;

    let database = Database::open(root.path().join(".lumin/lifecycle.store"))?;
    let write = database.begin_write()?;
    {
        let definition = TableDefinition::<&str, &[u8]>::new("migration-root-authorizations");
        let mut table = write.open_table(definition)?;
        let key = "0000000000000001";
        let authorization = table
            .get(key)?
            .ok_or("migration source omitted its root authorization")?
            .value()
            .to_vec();
        let changed = replace_digest_bytes(&authorization, previous, &forged)?;
        table.insert(key, changed.as_slice())?;
    }
    write.commit()?;
    drop(database);

    assert!(matches!(
        store.migrate_lifecycle_store(),
        Err(StoreError::Integrity(message)) if message.contains("root core disagrees")
    ));
    Ok(())
}

#[test]
fn migration_revalidates_journal_payloads_after_folding() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    make_prior_store(&store, root.path())?;
    inject_crash(&store, MigrationCrashPoint::IntentPublished)?;
    let journal = root.path().join(".lumin/lifecycle-migration.json");
    let mut changed = false;
    let result = store.namespace.with_migration_lock(|guard| {
        super::super::migration::validate_journal_payload_recheck_for_test(guard, &mut || {
            fs::write(&journal, b"{}\n").map_err(crate::io_error)?;
            changed = true;
            Ok(())
        })
    });
    assert!(changed);
    assert!(matches!(
        result,
        Err(StoreError::Integrity(message)) if message.contains("payload changed after")
    ));
    Ok(())
}

#[test]
fn migration_authenticates_a_rebound_target_before_exchange()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    make_prior_store(&store, root.path())?;
    inject_crash(&store, MigrationCrashPoint::TargetPublished)?;
    let target = pending_target_path(&root.path().join(".lumin"))?;
    let database = Database::open(&target)?;
    let write = database.begin_write()?;
    {
        let definition = TableDefinition::<&str, &[u8]>::new("store-header");
        let mut table = write.open_table(definition)?;
        table.insert("foreign", b"{}".as_slice())?;
    }
    write.commit()?;
    drop(database);

    let result = store.namespace.with_migration_lock(|guard| {
        super::super::migration::validate_rebound_target_for_test(guard)
    });
    assert!(matches!(
        result,
        Err(StoreError::Integrity(message))
            if message.contains("header table must contain exactly its canonical row")
    ));
    Ok(())
}

#[cfg(not(windows))]
#[test]
fn terminal_validation_rechecks_the_retained_source_path_after_detached_reads()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    make_prior_store(&store, root.path())?;
    let state = root.path().join(".lumin");
    let mut replaced = false;
    let result = store.namespace.with_migration_lock(|guard| {
        migrate_with_hook(guard, &mut |point| {
            if point == MigrationCrashPoint::TerminalSourceValidated && !replaced {
                let retained = fs::read_dir(&state)
                    .map_err(crate::io_error)?
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .find(|path| {
                        path.file_name()
                            .and_then(std::ffi::OsStr::to_str)
                            .is_some_and(|name| name.starts_with(RETAINED_MIGRATION_SOURCE_PREFIX))
                    })
                    .ok_or_else(|| {
                        StoreError::Integrity(
                            "terminal migration omitted its retained source".to_owned(),
                        )
                    })?;
                replace_with_same_bytes(&retained)
                    .map_err(|error| StoreError::Integrity(error.to_string()))?;
                replaced = true;
            }
            Ok(())
        })
    });
    assert!(replaced);
    assert!(matches!(
        result,
        Err(StoreError::Integrity(message))
            if message.contains("retained migration source physical identity changed")
    ));
    Ok(())
}

#[test]
fn terminal_validation_rehashes_the_retained_source_after_detached_reads()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    make_prior_store(&store, root.path())?;
    let state = root.path().join(".lumin");
    let mut changed = false;
    let result = store.namespace.with_migration_lock(|guard| {
        migrate_with_hook(guard, &mut |point| {
            if point == MigrationCrashPoint::TerminalSourceValidated && !changed {
                let retained = fs::read_dir(&state)
                    .map_err(crate::io_error)?
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .find(|path| {
                        path.file_name()
                            .and_then(std::ffi::OsStr::to_str)
                            .is_some_and(|name| name.starts_with(RETAINED_MIGRATION_SOURCE_PREFIX))
                    })
                    .ok_or_else(|| {
                        StoreError::Integrity(
                            "terminal migration omitted its retained source".to_owned(),
                        )
                    })?;
                fs::write(retained, b"changed retained migration source")
                    .map_err(crate::io_error)?;
                changed = true;
            }
            Ok(())
        })
    });
    assert!(changed);
    assert!(matches!(
        result,
        Err(StoreError::Integrity(message))
            if message.contains("payload changed during terminal validation")
    ));
    Ok(())
}

#[test]
fn every_migration_process_death_boundary_recovers_on_reopen()
-> Result<(), Box<dyn std::error::Error>> {
    for &point in CRASH_POINTS {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        let evidence = evidence();
        let mut attempt = store.begin_attempt()?;
        let published = store.publish_run(&mut attempt, &evidence, |_| Ok(()))?;
        make_prior_store(&store, root.path())?;
        drop(store);

        run_death_fixture(root.path(), point)?;

        migrate_public_store(root.path())?;
        let recovered = open_store(root.path())?;
        assert_eq!(current_generation(&recovered)?, next_generation()?);
        assert_eq!(recovered.latest_run_id()?, Some(published.run_id.clone()));
        assert_eq!(recovered.load_run(&published.run_id)?.1, evidence);
        assert_terminal_migration_paths(root.path())?;
    }
    Ok(())
}

#[test]
fn live_migration_intent_blocks_ordinary_store_work() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    make_prior_store(&store, root.path())?;
    inject_crash(&store, MigrationCrashPoint::IntentPublished)?;
    assert!(matches!(
        store.begin_attempt(),
        Err(StoreError::LifecycleMigrationRequired)
    ));
    Ok(())
}

#[test]
fn ordinary_admission_rejects_a_substituted_pending_migration_target()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    make_prior_store(&store, root.path())?;
    inject_crash(&store, MigrationCrashPoint::TargetPublished)?;

    let state = root.path().join(".lumin");
    let target = pending_target_path(&state)?;
    let before = HeldEntry::open(
        &target,
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        true,
        "pending migration target before substitution",
    )?;
    let before_identity = before.identity().clone();
    drop(before);
    replace_with_same_bytes(&target)?;
    let after = HeldEntry::open(
        &target,
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        true,
        "pending migration target after substitution",
    )?;
    assert_ne!(after.identity(), &before_identity);
    drop(after);

    assert!(matches!(
        store.begin_attempt(),
        Err(StoreError::Integrity(_))
    ));
    Ok(())
}

#[test]
fn ordinary_admission_rejects_an_extra_link_to_a_pending_migration_target()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    make_prior_store(&store, root.path())?;
    inject_crash(&store, MigrationCrashPoint::TargetPublished)?;

    let state = root.path().join(".lumin");
    let target = pending_target_path(&state)?;
    fs::hard_link(&target, root.path().join("linked-migration-target"))?;
    let linked = HeldEntry::open(
        &target,
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        false,
        "multiply-linked pending migration target",
    )?;
    assert_eq!(linked.links(), 2);
    drop(linked);

    assert!(matches!(
        store.begin_attempt(),
        Err(StoreError::Integrity(_))
    ));
    Ok(())
}

#[test]
fn retry_after_intent_removal_finishes_cleanup_without_advancing_again()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    make_prior_store(&store, root.path())?;
    inject_crash(&store, MigrationCrashPoint::IntentRemoved)?;
    assert!(store.begin_attempt().is_ok());

    assert_eq!(store.migrate_lifecycle_store()?, next_generation()?);
    assert_eq!(current_generation(&store)?, next_generation()?);
    assert_terminal_migration_paths(root.path())?;
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
    let canonical_path = root.path().join(".lumin/lifecycle.store");
    make_prior_store(&store, root.path())?;

    let mut source_before_failed_exchange = None;
    let result = store.namespace.with_migration_lock(|guard| {
        migrate_with_hook(guard, &mut |point| {
            if point == MigrationCrashPoint::CopiesValidated {
                source_before_failed_exchange =
                    Some(fs::read(&canonical_path).map_err(crate::io_error)?);
                fs::write(&evidence_path, b"tampered evidence").map_err(crate::io_error)?;
            }
            Ok(())
        })
    });
    assert!(matches!(result, Err(StoreError::Integrity(_))));
    assert_eq!(
        fs::read(canonical_path)?,
        source_before_failed_exchange.ok_or("migration skipped the validation barrier")?
    );
    Ok(())
}

#[test]
fn missing_canonical_store_during_live_migration_is_a_hard_stop()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    make_prior_store(&store, root.path())?;
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
    let admission = lumin_inventory::repository_admission(&root)?;
    let namespace = super::super::NamespaceState::open_for_migration(
        &admission.canonical_root,
        &admission.binding,
    )?
    .ok_or(StoreError::LifecycleStoreNotInitialized)?;
    let _ = namespace.with_migration_lock(|guard| {
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

fn replace_with_same_bytes(path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = fs::read(path)?;
    let replacement = path.with_extension("same-bytes-replacement");
    fs::write(&replacement, bytes)?;
    fs::remove_file(path)?;
    fs::rename(replacement, path)?;
    Ok(())
}

fn pending_target_path(
    state: &std::path::Path,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let mut revisions = fs::read_dir(state)?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("lifecycle-migration.revision-")
        })
        .collect::<Vec<_>>();
    revisions.sort_by_key(fs::DirEntry::file_name);
    for revision in revisions.into_iter().rev() {
        let value = serde_json::from_slice::<serde_json::Value>(&fs::read(revision.path())?)?;
        let Some(events) = value.get("events").and_then(serde_json::Value::as_array) else {
            continue;
        };
        for event in events.iter().rev() {
            if event.get("kind").and_then(serde_json::Value::as_str) == Some("pendingPublication") {
                let name = event
                    .pointer("/binding/preExchangeName")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("pending migration target omitted its path")?;
                return Ok(state.join(name));
            }
        }
    }
    Err("pending migration target binding was not journaled".into())
}

fn journal_head_path(state: &std::path::Path) -> Result<std::path::PathBuf, StoreError> {
    let mut entries = fs::read_dir(state)
        .map_err(crate::io_error)?
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name == "lifecycle-migration.json" || name.starts_with("lifecycle-migration.revision-")
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(fs::DirEntry::file_name);
    entries
        .pop()
        .map(|entry| entry.path())
        .ok_or_else(|| StoreError::Integrity("migration journal is missing".to_owned()))
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
        MigrationCrashPoint::RootAuthorizationCommitted => "after-root-authorization",
        MigrationCrashPoint::IntentPrepared => "after-pending-intent-sync",
        MigrationCrashPoint::RootCandidateWriteStarted => "after-root-write-start",
        MigrationCrashPoint::RootCandidatePartiallyWritten => "after-root-partial-write",
        MigrationCrashPoint::RootCandidateWritten => "after-root-write",
        MigrationCrashPoint::RootNamePublished => "after-root-name-publication",
        MigrationCrashPoint::RootReopened => "after-root-reopen",
        MigrationCrashPoint::RootFileFlushed => "after-root-file-flush",
        MigrationCrashPoint::RootParentFlushed => "after-root-parent-flush",
        MigrationCrashPoint::IntentRenamed => "after-intent-rename",
        MigrationCrashPoint::IntentPublished => "after-intent",
        MigrationCrashPoint::RevisionCandidateCreated => "after-revision-candidate-create",
        MigrationCrashPoint::RevisionCandidateWriteStarted => "after-revision-write-start",
        MigrationCrashPoint::RevisionCandidatePartiallyWritten => "after-revision-partial-write",
        MigrationCrashPoint::RevisionCandidateWritten => "after-revision-write",
        MigrationCrashPoint::RevisionNamePublished => "after-revision-name-publication",
        MigrationCrashPoint::RevisionReopened => "after-revision-reopen",
        MigrationCrashPoint::RevisionFileFlushed => "after-revision-file-flush",
        MigrationCrashPoint::RevisionParentFlushed => "after-revision-parent-flush",
        MigrationCrashPoint::CopiesValidated => "after-validated-replacement",
        MigrationCrashPoint::TargetNamePublished => "after-target-name-publication",
        MigrationCrashPoint::TargetReopened => "after-target-reopen",
        MigrationCrashPoint::TargetFileFlushed => "after-target-file-flush",
        MigrationCrashPoint::TargetParentFlushed => "after-target-parent-flush",
        MigrationCrashPoint::TargetPublished => "after-target-publication",
        MigrationCrashPoint::BeforeExchange => "before-exchange",
        MigrationCrashPoint::ExchangeInputsOpened => "after-exchange-input-open",
        MigrationCrashPoint::ExchangeExternalReferencesValidated => {
            "after-exchange-external-validation"
        }
        #[cfg(windows)]
        MigrationCrashPoint::SourceRetired => "after-source-retirement",
        #[cfg(windows)]
        MigrationCrashPoint::CanonicalMoveExternalReferencesValidated => {
            "after-canonical-move-external-validation"
        }
        MigrationCrashPoint::CanonicalReplaced => "after-replace",
        MigrationCrashPoint::ParentFlushed => "after-parent-flush",
        MigrationCrashPoint::IntentRemoved => "after-intent-removal",
        MigrationCrashPoint::TerminalSourceValidated => "after-terminal-source-validation",
    }
}

fn crash_point(label: &str) -> Result<MigrationCrashPoint, Box<dyn std::error::Error>> {
    match label {
        "after-pending-intent-create" => Ok(MigrationCrashPoint::PendingIntentCreated),
        "after-root-authorization" => Ok(MigrationCrashPoint::RootAuthorizationCommitted),
        "after-pending-intent-sync" => Ok(MigrationCrashPoint::IntentPrepared),
        "after-root-write-start" => Ok(MigrationCrashPoint::RootCandidateWriteStarted),
        "after-root-partial-write" => Ok(MigrationCrashPoint::RootCandidatePartiallyWritten),
        "after-root-write" => Ok(MigrationCrashPoint::RootCandidateWritten),
        "after-root-name-publication" => Ok(MigrationCrashPoint::RootNamePublished),
        "after-root-reopen" => Ok(MigrationCrashPoint::RootReopened),
        "after-root-file-flush" => Ok(MigrationCrashPoint::RootFileFlushed),
        "after-root-parent-flush" => Ok(MigrationCrashPoint::RootParentFlushed),
        "after-intent-rename" => Ok(MigrationCrashPoint::IntentRenamed),
        "after-intent" => Ok(MigrationCrashPoint::IntentPublished),
        "after-revision-candidate-create" => Ok(MigrationCrashPoint::RevisionCandidateCreated),
        "after-revision-write-start" => Ok(MigrationCrashPoint::RevisionCandidateWriteStarted),
        "after-revision-partial-write" => {
            Ok(MigrationCrashPoint::RevisionCandidatePartiallyWritten)
        }
        "after-revision-write" => Ok(MigrationCrashPoint::RevisionCandidateWritten),
        "after-revision-name-publication" => Ok(MigrationCrashPoint::RevisionNamePublished),
        "after-revision-reopen" => Ok(MigrationCrashPoint::RevisionReopened),
        "after-revision-file-flush" => Ok(MigrationCrashPoint::RevisionFileFlushed),
        "after-revision-parent-flush" => Ok(MigrationCrashPoint::RevisionParentFlushed),
        "after-validated-replacement" => Ok(MigrationCrashPoint::CopiesValidated),
        "after-target-name-publication" => Ok(MigrationCrashPoint::TargetNamePublished),
        "after-target-reopen" => Ok(MigrationCrashPoint::TargetReopened),
        "after-target-file-flush" => Ok(MigrationCrashPoint::TargetFileFlushed),
        "after-target-parent-flush" => Ok(MigrationCrashPoint::TargetParentFlushed),
        "after-target-publication" => Ok(MigrationCrashPoint::TargetPublished),
        "before-exchange" => Ok(MigrationCrashPoint::BeforeExchange),
        "after-exchange-input-open" => Ok(MigrationCrashPoint::ExchangeInputsOpened),
        "after-exchange-external-validation" => {
            Ok(MigrationCrashPoint::ExchangeExternalReferencesValidated)
        }
        #[cfg(windows)]
        "after-source-retirement" => Ok(MigrationCrashPoint::SourceRetired),
        #[cfg(windows)]
        "after-canonical-move-external-validation" => {
            Ok(MigrationCrashPoint::CanonicalMoveExternalReferencesValidated)
        }
        "after-replace" => Ok(MigrationCrashPoint::CanonicalReplaced),
        "after-parent-flush" => Ok(MigrationCrashPoint::ParentFlushed),
        "after-intent-removal" => Ok(MigrationCrashPoint::IntentRemoved),
        "after-terminal-source-validation" => Ok(MigrationCrashPoint::TerminalSourceValidated),
        _ => Err(format!("unknown migration death point: {label}").into()),
    }
}

fn set_sequence_for_test(store: &RepositoryStore, key: &str, value: u64) -> Result<(), StoreError> {
    store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        {
            let mut sequences = write.open_table(SEQUENCES).map_err(crate::backend_error)?;
            sequences.insert(key, value).map_err(crate::backend_error)?;
        }
        guard.commit(write)
    })
}

fn mark_attempt_releasing_for_test(
    store: &RepositoryStore,
    root: &std::path::Path,
    attempt_id: &str,
) -> Result<String, StoreError> {
    let lock_name = store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        let lock_name = {
            let mut leases = write
                .open_table(ATTEMPT_LEASES)
                .map_err(crate::backend_error)?;
            let bytes = leases
                .get(attempt_id)
                .map_err(crate::backend_error)?
                .map(|value| value.value().to_vec())
                .ok_or_else(|| {
                    StoreError::Integrity(format!("test attempt lease is missing: {attempt_id}"))
                })?;
            let value = serde_json::from_slice::<serde_json::Value>(&bytes)
                .map_err(crate::serialization_error)?;
            let lock_name = value["lockName"]
                .as_str()
                .ok_or_else(|| StoreError::Integrity("test lease omitted lockName".to_owned()))?
                .to_owned();
            let text = std::str::from_utf8(&bytes).map_err(|error| {
                StoreError::Integrity(format!("test lease is not UTF-8: {error}"))
            })?;
            let releasing = text.replacen("\"state\":\"active\"", "\"state\":\"releasing\"", 1);
            if releasing == text {
                return Err(StoreError::Integrity(
                    "test attempt lease was not active".to_owned(),
                ));
            }
            leases
                .insert(attempt_id, releasing.as_bytes())
                .map_err(crate::backend_error)?;
            lock_name
        };
        guard.commit(write)?;
        Ok(lock_name)
    })?;

    let attempt_path = root
        .join(".lumin/attempts")
        .join(attempt_id)
        .join("attempt.json");
    let mut envelope = serde_json::from_slice::<serde_json::Value>(
        &fs::read(&attempt_path).map_err(crate::io_error)?,
    )
    .map_err(crate::serialization_error)?;
    envelope["state"] = "interrupted".into();
    envelope["finishedUnixMillis"] = envelope["startedUnixMillis"].clone();
    envelope["failure"] = "test interruption before releasing cleanup".into();
    write_pretty_json_for_test(&attempt_path, &envelope)?;

    let latest_path = root.join(".lumin/latest.json");
    let mut latest = serde_json::from_slice::<serde_json::Value>(
        &fs::read(&latest_path).map_err(crate::io_error)?,
    )
    .map_err(crate::serialization_error)?;
    latest["latestAttempt"]["status"] = "interrupted".into();
    write_pretty_json_for_test(&latest_path, &latest)?;
    Ok(lock_name)
}

fn write_pretty_json_for_test(
    path: &std::path::Path,
    value: &serde_json::Value,
) -> Result<(), StoreError> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(crate::serialization_error)?;
    bytes.push(b'\n');
    fs::write(path, bytes).map_err(crate::io_error)
}

fn set_store_sequence_at_path(
    path: &std::path::Path,
    key: &str,
    value: u64,
) -> Result<(), StoreError> {
    let database = Database::open(path).map_err(crate::backend_error)?;
    let write = database.begin_write().map_err(crate::backend_error)?;
    {
        let mut sequences = write.open_table(SEQUENCES).map_err(crate::backend_error)?;
        sequences.insert(key, value).map_err(crate::backend_error)?;
    }
    write.commit().map_err(crate::backend_error)
}

fn clear_latest_pointer_index_for_test(store: &RepositoryStore) -> Result<(), StoreError> {
    store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        {
            let mut pointers = write.open_table(POINTERS).map_err(crate::backend_error)?;
            for key in ["latest-attempt", "latest-completed"] {
                if pointers
                    .remove(key)
                    .map_err(crate::backend_error)?
                    .is_none()
                {
                    return Err(StoreError::Integrity(format!(
                        "test pointer {key} is missing"
                    )));
                }
            }
        }
        guard.commit(write)
    })
}

fn set_latest_pointer_index_for_test(
    store: &RepositoryStore,
    attempt_id: &AttemptId,
    run_id: &lumin_model::RunId,
) -> Result<(), StoreError> {
    store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        {
            let mut pointers = write.open_table(POINTERS).map_err(crate::backend_error)?;
            pointers
                .insert("latest-attempt", attempt_id.as_str().as_bytes())
                .map_err(crate::backend_error)?;
            pointers
                .insert("latest-completed", run_id.as_str().as_bytes())
                .map_err(crate::backend_error)?;
        }
        guard.commit(write)
    })
}

fn different_physical_identity(identity: PhysicalFileIdentity) -> PhysicalFileIdentity {
    match identity {
        PhysicalFileIdentity::Unix { device, inode } => PhysicalFileIdentity::Unix {
            device,
            inode: inode.wrapping_add(1),
        },
        PhysicalFileIdentity::Windows {
            volume_serial,
            file_index,
        } => PhysicalFileIdentity::Windows {
            volume_serial,
            file_index: file_index.wrapping_add(1),
        },
    }
}

fn replace_digest_bytes(
    bytes: &[u8],
    previous: &str,
    replacement: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let text = std::str::from_utf8(bytes)?;
    let needle = format!("\"rootCoreSha256\":\"{previous}\"");
    let replacement = format!("\"rootCoreSha256\":\"{replacement}\"");
    let changed = text.replacen(&needle, &replacement, 1);
    if changed == text {
        return Err("root-core fixture could not locate its canonical digest field".into());
    }
    Ok(changed.into_bytes())
}

fn current_generation(store: &RepositoryStore) -> Result<StoreGeneration, StoreError> {
    store.with_shared_lock(|guard| Ok(guard.open_database()?.generation()))
}

fn make_prior_store(store: &RepositoryStore, root: &std::path::Path) -> Result<(), StoreError> {
    super::super::store_header::rewrite_current_store_header_as_prior_for_test(
        &root.join(".lumin/lifecycle.store"),
        &store.namespace.binding,
    )
}

fn next_generation() -> Result<StoreGeneration, Box<dyn std::error::Error>> {
    StoreGeneration::INITIAL
        .checked_next()
        .ok_or_else(|| "missing next generation".into())
}

fn evidence() -> RunEvidence {
    RunEvidence {
        schema_version: "lumin-evidence.v1".to_owned(),
        capabilities: RUN_EVIDENCE_CAPABILITY_IDS
            .into_iter()
            .map(|capability_id| CapabilityRecord {
                capability_id: capability_id.to_owned(),
                state: if matches!(capability_id, "sfc/svelte.v1" | "sfc/astro.v1") {
                    CapabilityState::Unavailable
                } else {
                    CapabilityState::Complete
                },
            })
            .collect(),
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

fn pre_write_digest(paths: &[RepoPathProjection], options: &GateAnalysisOptions) -> String {
    pre_write_request_digest(paths, &options.scan_invocation)
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

fn observed_lease(
    root: &std::path::Path,
    path: &RepoPath,
) -> Result<WriteLease, Box<dyn std::error::Error>> {
    let observation = lumin_inventory::inspect_write_target(root, path)?;
    let kind = match observation.kind {
        lumin_inventory::WriteTargetKind::ExistingFile => WriteLeaseKind::ExistingFile,
        lumin_inventory::WriteTargetKind::ExistingDirectory => WriteLeaseKind::Directory,
        lumin_inventory::WriteTargetKind::NewFile => WriteLeaseKind::NewFile,
    };
    Ok(WriteLease {
        path: RepoPathProjection::from(&observation.path),
        kind,
        physical_identity: observation.physical_identity,
        nearest_existing_parent: observation
            .nearest_existing_parent
            .as_ref()
            .map(RepoPathProjection::from),
        prefix_identities: observation
            .prefix_identities
            .into_iter()
            .map(|(path, physical_identity)| PathPrefixIdentity {
                path: RepoPathProjection::from(&path),
                physical_identity,
            })
            .collect(),
    })
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
    let analysis_options = options();
    let request_digest = pre_write_digest(std::slice::from_ref(&source), &analysis_options);
    let (gate_id, transition_sequence) = match session.reserve_pre_write(
        &request_digest,
        std::slice::from_ref(&source),
        std::slice::from_ref(&source_lease),
        &analysis_options,
        rejected_test_observation,
    )? {
        PreWriteStart::Analyze {
            gate_id,
            transition_sequence,
        } => (gate_id, transition_sequence),
        PreWriteStart::Committed(_) => return Err("active gate fixture was rejected".into()),
    };
    let baseline = GateBaselineDraft {
        analysis_contract: SUPPORTED_ACTIVE_GATE_ANALYSIS_CONTRACT_ID.to_owned(),
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
    let evidence_payload_sha256 =
        crate::evidence_payload_sha256(&baseline_for_id.snapshot.evidence)?;
    let source_for_id = source.clone();
    let lease_for_id = source_lease.clone();
    let final_validation_evidence = clean_pre_write_final_validation_evidence(
        baseline_for_id.snapshot.inputs.clone(),
        vec![source_lease.clone()],
    );
    session.finish_pre_write(
        &request_digest,
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
                            evidence_payload_sha256: &evidence_payload_sha256,
                            signals: &[],
                            declared_write_set: std::slice::from_ref(&source_for_id),
                            leased_write_set: std::slice::from_ref(&lease_for_id),
                            alias_closures: &[],
                            protected_semantic_inputs: &baseline_for_id.protected_semantic_inputs,
                        },
                    ),
                },
            },
            pre_write_evidence: Some(final_validation_evidence),
            post_write_evidence: None,
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
    let request_digest = post_write_request_digest(gate_id);
    let session = store.begin_operation(&operation_id)?;
    let gate = match session.begin_post_write(&request_digest, gate_id)? {
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
    let evidence_payload_sha256 = crate::evidence_payload_sha256(&snapshot.evidence)?;
    let actual_write_set_for_id = actual_write_set.clone();
    let protected_for_id = protected_semantic_inputs.clone();
    let aliases_for_id = alias_closures.clone();
    let signals_for_id = signals.clone();
    let post_write_evidence = clean_post_write_final_validation_evidence(
        snapshot.inputs.clone(),
        leased_write_set.clone(),
        alias_closures.clone(),
    );
    session.finish_post_write(
        &request_digest,
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
                        evidence_payload_sha256: &evidence_payload_sha256,
                        signals: &signals_for_id,
                        leased_write_set: &leased_write_set,
                        protected_semantic_inputs: &protected_for_id,
                        changed_paths: &[],
                        actual_write_set: &actual_write_set_for_id,
                        alias_closures: &aliases_for_id,
                        reconciled_transition_sequences: &[],
                    }),
                },
            },
            pre_write_evidence: None,
            post_write_evidence: Some(post_write_evidence),
        },
    )?;
    Ok(())
}

fn close_active_gate_for_migration(
    store: &RepositoryStore,
    gate_id: &GateId,
) -> Result<(), Box<dyn std::error::Error>> {
    let operation_id = OperationId::from_string(format!("op-migrate-close-{}", gate_id.as_str()));
    let request_digest = post_write_request_digest(gate_id);
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
    let protected_semantic_inputs =
        derive_protected_semantic_inputs(&snapshot, &gate.leased_write_set);
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
    let evidence_payload_sha256 = crate::evidence_payload_sha256(&snapshot.evidence)?;
    let actual_write_set_for_id = actual_write_set.clone();
    let protected_for_id = protected_semantic_inputs.clone();
    let aliases_for_id = alias_closures.clone();
    let changed_paths_for_id = changed_paths.clone();
    let reconciled_sequences_for_id = reconciled_transition_sequences.clone();
    let post_write_evidence = clean_post_write_final_validation_evidence(
        snapshot.inputs.clone(),
        leased_write_set.clone(),
        alias_closures.clone(),
    );
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
                        evidence_payload_sha256: &evidence_payload_sha256,
                        signals: &[],
                        leased_write_set: &leased_write_set,
                        protected_semantic_inputs: &protected_for_id,
                        changed_paths: &changed_paths_for_id,
                        actual_write_set: &actual_write_set_for_id,
                        alias_closures: &aliases_for_id,
                        reconciled_transition_sequences: &reconciled_sequences_for_id,
                    }),
                },
            },
            pre_write_evidence: None,
            post_write_evidence: Some(post_write_evidence),
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
    let request_digest = post_write_request_digest(gate_id);
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
            pre_write_evidence: None,
            post_write_evidence: None,
        },
    )?;
    Ok(operation_id)
}

fn assert_terminal_migration_paths(
    root: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let state = root.join(".lumin");
    if !state.join("lifecycle-migration.json").is_file() {
        return Err("terminal migration omitted its root journal".into());
    }
    let retained = fs::read_dir(&state)?.any(|entry| {
        entry.is_ok_and(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(RETAINED_MIGRATION_SOURCE_PREFIX)
        })
    });
    if !retained {
        return Err("terminal migration omitted its retained source".into());
    }
    Ok(())
}

fn migrate_public_store(root: &std::path::Path) -> Result<StoreGeneration, StoreError> {
    let admission = lumin_inventory::repository_admission(root)
        .map_err(|error| StoreError::Integrity(error.to_string()))?;
    RepositoryStore::migrate_existing_lifecycle_store(&admission.canonical_root, &admission.binding)
}
