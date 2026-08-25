mod integrity;

use std::fs;

use lumin_evidence::{
    ActualWriteSet, CapabilityRecord, DEAD_CODE_CAPABILITY_ID, DeclaredPathUnsupportedReason,
    GateAnalysisOptions, GateBaselineObservationInput, GateCloseObservationInput,
    GateObservationBinding, GateSignal, PathPrefixIdentity, PostWriteFinalValidationEvidence,
    PreWriteDeclaredPathInspection, PreWriteFinalValidationEvidence, RepoPathProjection,
    RunEvidence, SUPPORTED_ACTIVE_GATE_ANALYSIS_CONTRACT_ID, SemanticInputRecord,
    SemanticInputState, SemanticReadReservationBinding, UnsealedGateObservationInputs, WriteLease,
    WriteLeaseKind, apply_worktree_transition, derive_gate_baseline_observation_id,
    derive_gate_close_observation_id, derive_protected_semantic_inputs,
    derive_unsealed_gate_observation_binding, gate_policy, post_write_request_digest,
    pre_write_request_digest, seal_analysis_snapshot,
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
    #[cfg(windows)]
    MigrationCrashPoint::SourceRetired,
    MigrationCrashPoint::CanonicalReplaced,
    MigrationCrashPoint::ParentFlushed,
    MigrationCrashPoint::IntentRemoved,
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
    assert!(matches!(
        result,
        Err(StoreError::Integrity(message)) if message.contains("lock contents changed")
    ));
    let state = root.path().join(".lumin");
    assert!(!state.join("lifecycle-migration.json").exists());
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
        #[cfg(windows)]
        MigrationCrashPoint::SourceRetired => "after-source-retirement",
        MigrationCrashPoint::CanonicalReplaced => "after-replace",
        MigrationCrashPoint::ParentFlushed => "after-parent-flush",
        MigrationCrashPoint::IntentRemoved => "after-intent-removal",
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
        #[cfg(windows)]
        "after-source-retirement" => Ok(MigrationCrashPoint::SourceRetired),
        "after-replace" => Ok(MigrationCrashPoint::CanonicalReplaced),
        "after-parent-flush" => Ok(MigrationCrashPoint::ParentFlushed),
        "after-intent-removal" => Ok(MigrationCrashPoint::IntentRemoved),
        _ => Err(format!("unknown migration death point: {label}").into()),
    }
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
