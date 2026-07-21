mod integrity;

use std::fs;

use lumin_evidence::{
    CapabilityRecord, DEAD_CODE_CAPABILITY_ID, GateAnalysisOptions, GateBaseline,
    RepoPathProjection, RunEvidence, WriteLease, WriteLeaseKind, seal_analysis_snapshot,
};
use lumin_model::{CapabilityState, GateId, OperationId, RepoPath};

use crate::{PreWriteFinish, PreWriteStart, RepositoryStore, StoreError, StoreGeneration};

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

#[test]
fn migration_preserves_run_gate_and_pending_operation_records()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let store = open_store(root.path())?;
    let evidence = evidence();
    let attempt = store.begin_attempt()?;
    let published = store.publish_run(&attempt, &evidence)?;
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
        session.reserve_pre_write("migrate-pending-digest", &[], &[], &options()),
        Err(StoreError::StoreGenerationChanged { .. })
    ));
    assert_migration_paths_absent(root.path())?;
    Ok(())
}

#[test]
fn every_migration_process_death_boundary_recovers_on_reopen()
-> Result<(), Box<dyn std::error::Error>> {
    for point in CRASH_POINTS {
        let root = tempfile::tempdir()?;
        let store = open_store(root.path())?;
        let evidence = evidence();
        let attempt = store.begin_attempt()?;
        let published = store.publish_run(&attempt, &evidence)?;
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
    let attempt = store.begin_attempt()?;
    let published = store.publish_run(&attempt, &evidence())?;
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
        findings: Vec::new(),
        limitations: Vec::new(),
    }
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
    let operation_id = OperationId::from_string(operation.to_owned());
    let session = store.begin_operation(&operation_id)?;
    let source = path(source)?;
    let source_lease = lease(source.clone());
    let (gate_id, transition_sequence) = match session.reserve_pre_write(
        "migrate-gate-digest",
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
        "migrate-gate-digest",
        &gate_id,
        PreWriteFinish {
            baseline: Some(GateBaseline {
                analysis_contract: "migration-test-contract".to_owned(),
                snapshot: seal_analysis_snapshot(Vec::new(), evidence()),
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
