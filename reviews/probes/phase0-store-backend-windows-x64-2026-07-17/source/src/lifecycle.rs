use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::backend;
use crate::backend_contract;
use crate::model::{BackendKind, FaultBackendReport, FaultCaseResult};
use crate::namespace;
use crate::util::{
    atomic_replace_file, copy_directory, create_durable_marker, wait_for_path, wait_forever,
    write_json_durable,
};

const ATTEMPT_SEQUENCE: u64 = 1;
const RETENTION_PLAN_ID: &str = "plan-retain-1";
const RETAINED_RUN_ID: &str = "run-retained";
const RACE_RUN_ID: &str = "run-10";

pub const PUBLICATION_POINTS: [&str; 8] = [
    "before-attempt-catalog-allocation",
    "after-catalog-allocation",
    "after-running-envelope",
    "after-latest-running",
    "after-run-rename",
    "after-terminal-attempt",
    "after-latest-temp",
    "after-latest-replace",
];

pub const RETENTION_POINTS: [&str; 6] = [
    "before-prepared-plan",
    "after-prepared-plan",
    "after-pruning-commit",
    "after-payload-move",
    "after-pruned-commit",
    "after-physical-reclamation",
];

pub const MIGRATION_POINTS: [&str; 5] = [
    "before-migration-intent",
    "after-migration-intent",
    "after-validated-replacement",
    "after-canonical-replace",
    "after-intent-removal",
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Catalog {
    generation: u64,
    next_sequence: u64,
    attempts: BTreeMap<u64, AttemptCatalog>,
    retention_plans: BTreeMap<String, RetentionPlanState>,
    records: BTreeMap<String, RecordState>,
    sentinel: String,
}

impl Default for Catalog {
    fn default() -> Self {
        Self {
            generation: 1,
            next_sequence: 1,
            attempts: BTreeMap::new(),
            retention_plans: BTreeMap::new(),
            records: BTreeMap::new(),
            sentinel: "phase0-canonical-sentinel".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct AttemptCatalog {
    phase: AttemptPhase,
    run_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum AttemptPhase {
    Running,
    Interrupted,
    Complete,
}

impl AttemptPhase {
    const fn rank(self) -> u8 {
        match self {
            Self::Running => 0,
            Self::Interrupted | Self::Complete => 1,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct AttemptEnvelope {
    sequence: u64,
    phase: AttemptPhase,
    run_id: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
struct LatestPointer {
    latest_attempt: Option<AttemptEnvelope>,
    latest_completed: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RetentionPlanState {
    Prepared,
    Pruning,
    Pruned,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RecordState {
    Active,
    Pruning,
    Pruned,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct MigrationIntent {
    from_generation: u64,
    to_generation: u64,
}

#[derive(Clone, Debug, Serialize)]
struct PublicationObservation {
    next_sequence: u64,
    attempt_phase: Option<AttemptPhase>,
    latest_attempt_phase: Option<AttemptPhase>,
    latest_completed: Option<u64>,
    run_directory_exists: bool,
    orphan_run: bool,
}

#[derive(Clone, Debug, Serialize)]
struct RetentionObservation {
    plan_state: Option<RetentionPlanState>,
    record_state: RecordState,
    canonical_payload_exists: bool,
    trash_payload_exists: bool,
}

#[derive(Clone, Debug, Serialize)]
struct MigrationObservation {
    generation: u64,
    intent_exists: bool,
    replacement_exists: bool,
    stale_writer: Option<StaleWriterResult>,
}

pub struct LatestPublishRequest {
    pub backend: BackendKind,
    pub root: PathBuf,
    pub sequence: u64,
    pub phase: String,
    pub barrier: PathBuf,
    pub actor: String,
    pub hold_guard: bool,
    pub result: PathBuf,
    pub watchdog: Duration,
}

pub struct StaleWriterRequest {
    pub backend: BackendKind,
    pub root: PathBuf,
    pub ready: PathBuf,
    pub resume: PathBuf,
    pub result: PathBuf,
    pub watchdog: Duration,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum StaleWriterOutcome {
    GenerationChanged,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct StaleWriterResult {
    process_id: u32,
    token_generation: u64,
    observed_generation: u64,
    transaction_closed_before_analysis: bool,
    logical_catalog_preserved: bool,
    mutation_attempted: bool,
    outcome: StaleWriterOutcome,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct LatestPublishResult {
    actor: String,
    commit_ordinal: usize,
    outcome: String,
}

pub struct PublicationRetentionRequest {
    pub backend: BackendKind,
    pub root: PathBuf,
    pub role: String,
    pub barrier: PathBuf,
    pub result: PathBuf,
    pub hold_guard: bool,
    pub watchdog: Duration,
}

pub fn run_fault_backend(
    backend_kind: BackendKind,
    watchdog: Duration,
    work_root: &Path,
) -> FaultBackendReport {
    let backend_root = work_root.join(format!("fault-{backend_kind}"));
    if backend_root.exists()
        && let Err(error) = fs::remove_dir_all(&backend_root)
    {
        return FaultBackendReport {
            backend: backend_kind,
            status: "FAIL".to_owned(),
            cases: vec![FaultCaseResult {
                domain: "harness".to_owned(),
                crash_point: "clean-work-root".to_owned(),
                status: "FAIL".to_owned(),
                error: Some(format!(
                    "remove previous fault root {}: {error}",
                    backend_root.display()
                )),
                elapsed_micros: 0,
                observation: serde_json::Value::Null,
            }],
        };
    }
    let mut cases = Vec::new();
    cases.extend(backend_contract::run_backend_contract_cases(
        backend_kind,
        &backend_root,
    ));
    for point in PUBLICATION_POINTS {
        cases.push(capture_case("publication", point, || {
            run_publication_case(backend_kind, point, watchdog, &backend_root)
        }));
    }
    cases.push(capture_case(
        "publication-concurrency",
        "reverse-sequence-independent-fields",
        || run_latest_concurrency_case(backend_kind, false, watchdog, &backend_root),
    ));
    cases.push(capture_case(
        "publication-concurrency",
        "same-sequence-terminal-beats-running",
        || run_latest_concurrency_case(backend_kind, true, watchdog, &backend_root),
    ));
    cases.push(capture_case(
        "publication-retention-race",
        "publication-first-makes-retention-stale",
        || run_publication_retention_case(backend_kind, false, watchdog, &backend_root),
    ));
    cases.push(capture_case(
        "publication-retention-race",
        "retention-first-blocks-publication",
        || run_publication_retention_case(backend_kind, true, watchdog, &backend_root),
    ));
    for point in RETENTION_POINTS {
        cases.push(capture_case("retention", point, || {
            run_retention_case(backend_kind, point, watchdog, &backend_root)
        }));
    }
    for fault in ["both-canonical-and-trash", "neither-canonical-nor-trash"] {
        cases.push(capture_case("retention-integrity", fault, || {
            run_retention_integrity_case(backend_kind, fault, &backend_root)
        }));
    }
    for point in MIGRATION_POINTS {
        cases.push(capture_case("migration", point, || {
            run_migration_case(backend_kind, point, watchdog, &backend_root)
        }));
    }
    cases.push(capture_case("migration", "stale-generation-writer", || {
        run_stale_writer_case(backend_kind, watchdog, &backend_root)
    }));
    cases.extend(namespace::run_namespace_cases(
        backend_kind,
        watchdog,
        &backend_root,
    ));
    let status = if cases.iter().all(|case| case.status == "PASS") {
        "PASS"
    } else {
        "FAIL"
    };
    FaultBackendReport {
        backend: backend_kind,
        status: status.to_owned(),
        cases,
    }
}

pub fn run_publication_child(
    backend_kind: BackendKind,
    root: &Path,
    point: &str,
    ready: &Path,
) -> Result<()> {
    ensure!(
        PUBLICATION_POINTS.contains(&point),
        "unknown publication point"
    );
    checkpoint(point, "before-attempt-catalog-allocation", ready)?;

    update_catalog(backend_kind, root, |catalog| {
        ensure!(catalog.next_sequence == ATTEMPT_SEQUENCE);
        catalog.next_sequence += 1;
        Ok(())
    })?;
    checkpoint(point, "after-catalog-allocation", ready)?;

    let running = AttemptEnvelope {
        sequence: ATTEMPT_SEQUENCE,
        phase: AttemptPhase::Running,
        run_id: None,
    };
    write_json_durable(&attempt_path(root), &running)?;
    update_catalog(backend_kind, root, |catalog| {
        catalog.attempts.insert(
            ATTEMPT_SEQUENCE,
            AttemptCatalog {
                phase: AttemptPhase::Running,
                run_id: None,
            },
        );
        Ok(())
    })?;
    checkpoint(point, "after-running-envelope", ready)?;

    merge_latest(backend_kind, root, running.clone())?;
    checkpoint(point, "after-latest-running", ready)?;

    let staging = root.join("runs").join("run-1.staging");
    let run_path = root.join("runs").join("run-1");
    fs::create_dir_all(&staging)?;
    write_json_durable(
        &staging.join("run.json"),
        &serde_json::json!({"run_id":"run-1","sequence":ATTEMPT_SEQUENCE}),
    )?;
    fs::rename(&staging, &run_path)?;
    checkpoint(point, "after-run-rename", ready)?;

    let terminal = AttemptEnvelope {
        sequence: ATTEMPT_SEQUENCE,
        phase: AttemptPhase::Complete,
        run_id: Some("run-1".to_owned()),
    };
    write_json_durable(&attempt_path(root), &terminal)?;
    update_catalog(backend_kind, root, |catalog| {
        catalog.attempts.insert(
            ATTEMPT_SEQUENCE,
            AttemptCatalog {
                phase: AttemptPhase::Complete,
                run_id: Some("run-1".to_owned()),
            },
        );
        Ok(())
    })?;
    checkpoint(point, "after-terminal-attempt", ready)?;

    let latest = merged_latest(root, terminal)?;
    write_json_durable(&latest_temp_path(root), &latest)?;
    checkpoint(point, "after-latest-temp", ready)?;

    atomic_replace_file(&latest_temp_path(root), &latest_path(root))?;
    checkpoint(point, "after-latest-replace", ready)?;
    Ok(())
}

pub fn run_retention_child(
    backend_kind: BackendKind,
    root: &Path,
    point: &str,
    ready: &Path,
) -> Result<()> {
    ensure!(RETENTION_POINTS.contains(&point), "unknown retention point");
    checkpoint(point, "before-prepared-plan", ready)?;
    update_catalog(backend_kind, root, |catalog| {
        catalog
            .retention_plans
            .insert(RETENTION_PLAN_ID.to_owned(), RetentionPlanState::Prepared);
        Ok(())
    })?;
    checkpoint(point, "after-prepared-plan", ready)?;

    update_catalog(backend_kind, root, |catalog| {
        catalog
            .retention_plans
            .insert(RETENTION_PLAN_ID.to_owned(), RetentionPlanState::Pruning);
        catalog
            .records
            .insert(RETAINED_RUN_ID.to_owned(), RecordState::Pruning);
        Ok(())
    })?;
    checkpoint(point, "after-pruning-commit", ready)?;

    move_payload_to_trash(root)?;
    checkpoint(point, "after-payload-move", ready)?;

    mark_pruned(backend_kind, root)?;
    checkpoint(point, "after-pruned-commit", ready)?;

    let trash = retention_trash_payload(root);
    if trash.exists() {
        fs::remove_dir_all(trash)?;
    }
    checkpoint(point, "after-physical-reclamation", ready)?;
    Ok(())
}

pub fn run_migration_child(
    backend_kind: BackendKind,
    root: &Path,
    point: &str,
    ready: &Path,
) -> Result<()> {
    ensure!(MIGRATION_POINTS.contains(&point), "unknown migration point");
    checkpoint(point, "before-migration-intent", ready)?;
    write_json_durable(
        &migration_intent_path(root),
        &MigrationIntent {
            from_generation: 1,
            to_generation: 2,
        },
    )?;
    checkpoint(point, "after-migration-intent", ready)?;

    build_replacement(backend_kind, root)?;
    checkpoint(point, "after-validated-replacement", ready)?;

    replace_canonical_store(backend_kind, root)?;
    checkpoint(point, "after-canonical-replace", ready)?;

    fs::remove_file(migration_intent_path(root))?;
    checkpoint(point, "after-intent-removal", ready)?;
    Ok(())
}

pub fn run_stale_writer_child(request: StaleWriterRequest) -> Result<()> {
    let token_catalog = {
        let _guard = acquire_catalog_read_guard(&request.root)?;
        read_catalog(request.backend, &request.root)?
    };
    let token_generation = token_catalog.generation;
    create_durable_marker(&request.ready, b"generation-token-captured\n")?;
    wait_for_path(&request.resume, request.watchdog)?;

    let current_catalog = {
        let _guard = acquire_catalog_read_guard(&request.root)?;
        read_catalog(request.backend, &request.root)?
    };
    ensure!(
        current_catalog.generation != token_generation,
        "migration did not advance the generation"
    );
    let mut expected_logical_catalog = token_catalog;
    expected_logical_catalog.generation = current_catalog.generation;
    let logical_catalog_preserved = expected_logical_catalog == current_catalog;
    ensure!(
        logical_catalog_preserved,
        "migration changed the logical catalog while fencing the stale token"
    );
    write_json_durable(
        &request.result,
        &StaleWriterResult {
            process_id: std::process::id(),
            token_generation,
            observed_generation: current_catalog.generation,
            transaction_closed_before_analysis: true,
            logical_catalog_preserved,
            mutation_attempted: false,
            outcome: StaleWriterOutcome::GenerationChanged,
        },
    )
}

pub fn run_latest_publish_child(request: LatestPublishRequest) -> Result<()> {
    let phase = match request.phase.as_str() {
        "running" => AttemptPhase::Running,
        "interrupted" => AttemptPhase::Interrupted,
        "complete" => AttemptPhase::Complete,
        _ => bail!("unknown latest phase {}", request.phase),
    };
    create_durable_marker(
        &request.barrier.join(format!("{}.ready", request.actor)),
        b"ready\n",
    )?;
    wait_for_path(
        &request.barrier.join(format!("{}.start", request.actor)),
        request.watchdog,
    )?;
    create_durable_marker(
        &request
            .barrier
            .join(format!("{}.lock-attempted", request.actor)),
        b"lock-attempted\n",
    )?;
    let _guard = if request.hold_guard {
        acquire_catalog_guard(&request.root)?
    } else {
        acquire_catalog_guard_after_contention(
            &request.root,
            &request
                .barrier
                .join(format!("{}.lock-blocked", request.actor)),
            request.watchdog,
        )?
    };
    create_durable_marker(
        &request
            .barrier
            .join(format!("{}.guard-acquired", request.actor)),
        b"guard-acquired\n",
    )?;
    if request.hold_guard {
        wait_for_path(
            &request.barrier.join(format!("{}.release", request.actor)),
            request.watchdog,
        )?;
    }
    let outcome = publish_latest_guarded(
        request.backend,
        &request.root,
        AttemptEnvelope {
            sequence: request.sequence,
            phase,
            run_id: (phase == AttemptPhase::Complete).then(|| format!("run-{}", request.sequence)),
        },
    )?;
    ensure!(outcome == "published");
    let commit_ordinal = record_latest_commit(&request.root, &request.actor)?;
    write_json_durable(
        &request.result,
        &LatestPublishResult {
            actor: request.actor,
            commit_ordinal,
            outcome: outcome.to_owned(),
        },
    )
}

pub fn run_publication_retention_child(request: PublicationRetentionRequest) -> Result<()> {
    create_durable_marker(
        &request.barrier.join(format!("{}.started", request.role)),
        b"started\n",
    )?;
    let outcome = {
        let _guard = acquire_catalog_guard(&request.root)?;
        create_durable_marker(
            &request.barrier.join(format!("{}.locked", request.role)),
            b"locked\n",
        )?;
        if request.hold_guard {
            wait_for_path(
                &request.barrier.join(format!("{}.release", request.role)),
                request.watchdog,
            )?;
        }
        match request.role.as_str() {
            "publication" => publish_latest_guarded(
                request.backend,
                &request.root,
                AttemptEnvelope {
                    sequence: 10,
                    phase: AttemptPhase::Complete,
                    run_id: Some(RACE_RUN_ID.to_owned()),
                },
            )?,
            "retention" => confirm_retention_guarded(request.backend, &request.root)?,
            _ => bail!("unknown publication-retention role {}", request.role),
        }
    };
    write_json_durable(
        &request.result,
        &serde_json::json!({"role":request.role,"outcome":outcome}),
    )
}

fn capture_case<F>(domain: &str, point: &str, operation: F) -> FaultCaseResult
where
    F: FnOnce() -> Result<serde_json::Value>,
{
    let started = Instant::now();
    match operation() {
        Ok(observation) => FaultCaseResult {
            domain: domain.to_owned(),
            crash_point: point.to_owned(),
            status: "PASS".to_owned(),
            error: None,
            elapsed_micros: started.elapsed().as_micros(),
            observation,
        },
        Err(error) => FaultCaseResult {
            domain: domain.to_owned(),
            crash_point: point.to_owned(),
            status: "FAIL".to_owned(),
            error: Some(format!("{error:#}")),
            elapsed_micros: started.elapsed().as_micros(),
            observation: serde_json::Value::Null,
        },
    }
}

fn run_publication_case(
    backend_kind: BackendKind,
    point: &str,
    watchdog: Duration,
    backend_root: &Path,
) -> Result<serde_json::Value> {
    let root = case_root(backend_root, "publication", point)?;
    initialize_repository(backend_kind, &root)?;
    kill_child_at_checkpoint("child-publication", backend_kind, &root, point, watchdog)?;
    let observation = recover_publication(backend_kind, &root)?;
    verify_publication(point, &observation)?;
    Ok(serde_json::to_value(observation)?)
}

fn run_latest_concurrency_case(
    backend_kind: BackendKind,
    same_sequence: bool,
    watchdog: Duration,
    backend_root: &Path,
) -> Result<serde_json::Value> {
    let case_name = if same_sequence {
        "same-sequence-terminal-beats-running"
    } else {
        "reverse-sequence-independent-fields"
    };
    let root = case_root(backend_root, "publication-concurrency", case_name)?;
    initialize_repository(backend_kind, &root)?;
    let barrier = root.join("barrier");
    fs::create_dir_all(&barrier)?;

    let actors = if same_sequence {
        [
            ("terminal", 10_u64, "complete", true),
            ("running", 10_u64, "running", false),
        ]
    } else {
        [
            ("newer-failure", 11_u64, "interrupted", true),
            ("older-success", 10_u64, "complete", false),
        ]
    };
    let mut children = Vec::new();
    for (actor, sequence, phase, hold_guard) in actors {
        children.push(
            Command::new(env::current_exe()?)
                .arg("child-latest-publish")
                .arg("--backend")
                .arg(backend_kind.to_string())
                .arg("--root")
                .arg(&root)
                .arg("--sequence")
                .arg(sequence.to_string())
                .arg("--phase")
                .arg(phase)
                .arg("--barrier")
                .arg(&barrier)
                .arg("--actor")
                .arg(actor)
                .arg("--hold-guard")
                .arg(hold_guard.to_string())
                .arg("--result")
                .arg(root.join(format!("{actor}.json")))
                .arg("--watchdog-ms")
                .arg(watchdog.as_millis().to_string())
                .spawn()?,
        );
    }
    for (actor, _, _, _) in actors {
        wait_for_path(&barrier.join(format!("{actor}.ready")), watchdog)?;
    }
    let first_actor = actors[0].0;
    let second_actor = actors[1].0;
    create_durable_marker(&barrier.join(format!("{first_actor}.start")), b"start\n")?;
    wait_for_path(
        &barrier.join(format!("{first_actor}.guard-acquired")),
        watchdog,
    )?;
    create_durable_marker(&barrier.join(format!("{second_actor}.start")), b"start\n")?;
    wait_for_path(
        &barrier.join(format!("{second_actor}.lock-blocked")),
        watchdog,
    )?;
    create_durable_marker(
        &barrier.join(format!("{first_actor}.release")),
        b"release\n",
    )?;
    for child in &mut children {
        wait_for_clean_child(child, watchdog)?;
    }
    let results = actors
        .into_iter()
        .map(|(actor, _, _, _)| {
            let bytes = fs::read(root.join(format!("{actor}.json")))?;
            Ok(serde_json::from_slice::<LatestPublishResult>(&bytes)?)
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        results[0].actor == first_actor && results[0].commit_ordinal == 1,
        "first latest publisher did not commit first: {:?}",
        results[0]
    );
    ensure!(
        results[1].actor == second_actor && results[1].commit_ordinal == 2,
        "second latest publisher did not commit second: {:?}",
        results[1]
    );
    let latest = read_latest(&root)?;
    if same_sequence {
        let attempt = latest
            .latest_attempt
            .as_ref()
            .context("latest attempt missing")?;
        ensure!(attempt.sequence == 10);
        ensure!(attempt.phase == AttemptPhase::Complete);
        ensure!(latest.latest_completed == Some(10));
    } else {
        let attempt = latest
            .latest_attempt
            .as_ref()
            .context("latest attempt missing")?;
        ensure!(attempt.sequence == 11);
        ensure!(attempt.phase == AttemptPhase::Interrupted);
        ensure!(latest.latest_completed == Some(10));
    }
    Ok(serde_json::json!({
        "latest": latest,
        "forced_commit_order": results,
        "second_actor_observed_lock_contention": true
    }))
}

fn run_publication_retention_case(
    backend_kind: BackendKind,
    retention_first: bool,
    watchdog: Duration,
    backend_root: &Path,
) -> Result<serde_json::Value> {
    let case_name = if retention_first {
        "retention-first-blocks-publication"
    } else {
        "publication-first-makes-retention-stale"
    };
    let root = case_root(backend_root, "publication-retention-race", case_name)?;
    initialize_repository(backend_kind, &root)?;
    initialize_publication_retention_fixture(backend_kind, &root)?;
    let barrier = root.join("barrier");
    fs::create_dir_all(&barrier)?;

    let winner = if retention_first {
        "retention"
    } else {
        "publication"
    };
    let loser = if retention_first {
        "publication"
    } else {
        "retention"
    };
    let mut winner_child =
        spawn_publication_retention_actor(backend_kind, &root, &barrier, winner, true, watchdog)?;
    wait_for_path(&barrier.join(format!("{winner}.locked")), watchdog)?;
    let mut loser_child =
        spawn_publication_retention_actor(backend_kind, &root, &barrier, loser, false, watchdog)?;
    wait_for_path(&barrier.join(format!("{loser}.started")), watchdog)?;
    create_durable_marker(&barrier.join(format!("{winner}.release")), b"release\n")?;
    wait_for_clean_child(&mut winner_child, watchdog)?;
    wait_for_clean_child(&mut loser_child, watchdog)?;

    let winner_result: serde_json::Value =
        serde_json::from_slice(&fs::read(barrier.join(format!("{winner}.result.json")))?)?;
    let loser_result: serde_json::Value =
        serde_json::from_slice(&fs::read(barrier.join(format!("{loser}.result.json")))?)?;
    let catalog = read_catalog(backend_kind, &root)?;
    let latest = read_latest(&root)?;

    if retention_first {
        ensure!(winner_result["outcome"] == "entered_pruning");
        ensure!(loser_result["outcome"] == "retention_blocked");
        ensure!(
            catalog.retention_plans.get(RETENTION_PLAN_ID) == Some(&RetentionPlanState::Pruning)
        );
        ensure!(catalog.records.get(RACE_RUN_ID) == Some(&RecordState::Pruning));
        ensure!(latest.latest_attempt.is_none());
        ensure!(latest.latest_completed.is_none());
    } else {
        ensure!(winner_result["outcome"] == "published");
        ensure!(loser_result["outcome"] == "stale_latest_target");
        ensure!(
            catalog.retention_plans.get(RETENTION_PLAN_ID) == Some(&RetentionPlanState::Prepared)
        );
        ensure!(catalog.records.get(RACE_RUN_ID) == Some(&RecordState::Active));
        let latest_attempt = latest
            .latest_attempt
            .as_ref()
            .context("publication-first latest attempt is missing")?;
        ensure!(latest_attempt.sequence == 10);
        ensure!(latest_attempt.phase == AttemptPhase::Complete);
        ensure!(latest_attempt.run_id.as_deref() == Some(RACE_RUN_ID));
        ensure!(latest.latest_completed == Some(10));
    }

    Ok(serde_json::json!({
        "winner": winner_result,
        "loser": loser_result,
        "retention_plan": catalog.retention_plans.get(RETENTION_PLAN_ID),
        "record_state": catalog.records.get(RACE_RUN_ID),
        "latest": latest
    }))
}

fn spawn_publication_retention_actor(
    backend_kind: BackendKind,
    root: &Path,
    barrier: &Path,
    role: &str,
    hold_guard: bool,
    watchdog: Duration,
) -> Result<std::process::Child> {
    Command::new(env::current_exe()?)
        .arg("child-publication-retention")
        .arg("--backend")
        .arg(backend_kind.to_string())
        .arg("--root")
        .arg(root)
        .arg("--role")
        .arg(role)
        .arg("--barrier")
        .arg(barrier)
        .arg("--result")
        .arg(barrier.join(format!("{role}.result.json")))
        .arg("--hold-guard")
        .arg(hold_guard.to_string())
        .arg("--watchdog-ms")
        .arg(watchdog.as_millis().to_string())
        .spawn()
        .context("spawn publication-retention actor")
}

fn run_retention_case(
    backend_kind: BackendKind,
    point: &str,
    watchdog: Duration,
    backend_root: &Path,
) -> Result<serde_json::Value> {
    let root = case_root(backend_root, "retention", point)?;
    initialize_repository(backend_kind, &root)?;
    initialize_retention_fixture(backend_kind, &root)?;
    kill_child_at_checkpoint("child-retention", backend_kind, &root, point, watchdog)?;
    let observation = recover_retention(backend_kind, &root)?;
    verify_retention(point, &observation)?;
    Ok(serde_json::to_value(observation)?)
}

fn run_retention_integrity_case(
    backend_kind: BackendKind,
    fault: &str,
    backend_root: &Path,
) -> Result<serde_json::Value> {
    let root = case_root(backend_root, "retention-integrity", fault)?;
    initialize_repository(backend_kind, &root)?;
    initialize_retention_fixture(backend_kind, &root)?;
    update_catalog(backend_kind, &root, |catalog| {
        catalog
            .retention_plans
            .insert(RETENTION_PLAN_ID.to_owned(), RetentionPlanState::Pruning);
        catalog
            .records
            .insert(RETAINED_RUN_ID.to_owned(), RecordState::Pruning);
        Ok(())
    })?;
    let canonical = retention_canonical_payload(&root);
    let trash = retention_trash_payload(&root);
    match fault {
        "both-canonical-and-trash" => copy_directory(&canonical, &trash)?,
        "neither-canonical-nor-trash" => fs::remove_dir_all(&canonical)?,
        _ => bail!("unknown retention integrity fault"),
    }
    let error = match recover_retention(backend_kind, &root) {
        Ok(_) => bail!("both-or-neither retention state did not hard-stop"),
        Err(error) => error,
    };
    Ok(serde_json::json!({"hard_stop":true,"diagnostic":format!("{error:#}")}))
}

fn run_migration_case(
    backend_kind: BackendKind,
    point: &str,
    watchdog: Duration,
    backend_root: &Path,
) -> Result<serde_json::Value> {
    let root = case_root(backend_root, "migration", point)?;
    initialize_repository(backend_kind, &root)?;
    kill_child_at_checkpoint("child-migration", backend_kind, &root, point, watchdog)?;
    let observation = recover_migration(backend_kind, &root)?;
    if point == "before-migration-intent" {
        ensure!(observation.generation == 1);
    } else {
        ensure!(observation.generation == 2);
    }
    ensure!(!observation.intent_exists);
    Ok(serde_json::to_value(observation)?)
}

fn run_stale_writer_case(
    backend_kind: BackendKind,
    watchdog: Duration,
    backend_root: &Path,
) -> Result<serde_json::Value> {
    let root = case_root(backend_root, "migration", "stale-generation-writer")?;
    initialize_repository(backend_kind, &root)?;
    let coordination = root.join("stale-writer-coordination");
    fs::create_dir_all(&coordination)?;
    let ready = coordination.join("token.ready");
    let resume = coordination.join("migration.complete");
    let result_path = coordination.join("result.json");
    let mut stale_writer = Command::new(env::current_exe()?)
        .arg("child-stale-writer")
        .arg("--backend")
        .arg(backend_kind.to_string())
        .arg("--root")
        .arg(&root)
        .arg("--ready")
        .arg(&ready)
        .arg("--resume")
        .arg(&resume)
        .arg("--result")
        .arg(&result_path)
        .arg("--watchdog-ms")
        .arg(watchdog.as_millis().to_string())
        .spawn()
        .context("spawn stale-generation writer child")?;
    wait_for_path(&ready, watchdog)?;

    {
        let _guard = acquire_catalog_guard(&root)?;
        write_json_durable(
            &migration_intent_path(&root),
            &MigrationIntent {
                from_generation: 1,
                to_generation: 2,
            },
        )?;
        build_replacement(backend_kind, &root)?;
        replace_canonical_store(backend_kind, &root)?;
        fs::remove_file(migration_intent_path(&root))?;
    }
    create_durable_marker(&resume, b"migration-complete\n")?;
    wait_for_clean_child(&mut stale_writer, watchdog)?;
    let stale_writer: StaleWriterResult = serde_json::from_slice(&fs::read(&result_path)?)?;
    ensure!(stale_writer.token_generation == 1);
    ensure!(stale_writer.observed_generation == 2);
    ensure!(stale_writer.transaction_closed_before_analysis);
    ensure!(stale_writer.logical_catalog_preserved);
    ensure!(!stale_writer.mutation_attempted);
    ensure!(stale_writer.outcome == StaleWriterOutcome::GenerationChanged);
    let catalog = read_catalog(backend_kind, &root)?;
    ensure!(catalog.generation == 2);
    ensure!(catalog.sentinel == "phase0-canonical-sentinel");
    Ok(serde_json::to_value(MigrationObservation {
        generation: catalog.generation,
        intent_exists: false,
        replacement_exists: replacement_database_path(&root).exists(),
        stale_writer: Some(stale_writer),
    })?)
}

fn kill_child_at_checkpoint(
    command: &str,
    backend_kind: BackendKind,
    root: &Path,
    point: &str,
    watchdog: Duration,
) -> Result<()> {
    let ready = root.join("fault.ready");
    let mut child = Command::new(env::current_exe()?)
        .arg(command)
        .arg("--backend")
        .arg(backend_kind.to_string())
        .arg("--root")
        .arg(root)
        .arg("--point")
        .arg(point)
        .arg("--ready")
        .arg(&ready)
        .spawn()
        .with_context(|| format!("spawn {command}"))?;
    wait_for_path(&ready, watchdog)?;
    child.kill().with_context(|| format!("kill {command}"))?;
    child.wait().with_context(|| format!("reap {command}"))?;
    Ok(())
}

fn initialize_repository(backend_kind: BackendKind, root: &Path) -> Result<()> {
    fs::create_dir_all(root.join("attempts"))?;
    fs::create_dir_all(root.join("runs"))?;
    fs::create_dir_all(root.join("trash"))?;
    fs::write(root.join("lifecycle.lock"), b"phase0-catalog-lock\n")?;
    let database = database_path(root);
    backend::initialize(backend_kind, &database)?;
    let bytes = serde_json::to_vec(&Catalog::default())?;
    ensure!(backend::compare_exchange_catalog(
        backend_kind,
        &database,
        None,
        &bytes
    )?);
    Ok(())
}

fn initialize_retention_fixture(backend_kind: BackendKind, root: &Path) -> Result<()> {
    let payload = retention_canonical_payload(root);
    fs::create_dir_all(&payload)?;
    fs::write(payload.join("payload.bin"), b"retained-run-payload")?;
    update_catalog(backend_kind, root, |catalog| {
        catalog
            .records
            .insert(RETAINED_RUN_ID.to_owned(), RecordState::Active);
        Ok(())
    })
}

fn initialize_publication_retention_fixture(backend_kind: BackendKind, root: &Path) -> Result<()> {
    let run = root.join("runs").join(RACE_RUN_ID);
    fs::create_dir_all(&run)?;
    write_json_durable(
        &run.join("run.json"),
        &serde_json::json!({"run_id":RACE_RUN_ID,"sequence":10}),
    )?;
    update_catalog(backend_kind, root, |catalog| {
        catalog
            .retention_plans
            .insert(RETENTION_PLAN_ID.to_owned(), RetentionPlanState::Prepared);
        catalog
            .records
            .insert(RACE_RUN_ID.to_owned(), RecordState::Active);
        Ok(())
    })
}

fn update_catalog<F>(backend_kind: BackendKind, root: &Path, mutate: F) -> Result<()>
where
    F: Fn(&mut Catalog) -> Result<()>,
{
    loop {
        let current = read_catalog_bytes(backend_kind, root)?;
        let mut catalog: Catalog = serde_json::from_slice(&current)?;
        mutate(&mut catalog)?;
        let replacement = serde_json::to_vec(&catalog)?;
        if backend::compare_exchange_catalog(
            backend_kind,
            &database_path(root),
            Some(&current),
            &replacement,
        )? {
            return Ok(());
        }
        thread::yield_now();
    }
}

fn read_catalog(backend_kind: BackendKind, root: &Path) -> Result<Catalog> {
    Ok(serde_json::from_slice(&read_catalog_bytes(
        backend_kind,
        root,
    )?)?)
}

fn read_catalog_bytes(backend_kind: BackendKind, root: &Path) -> Result<Vec<u8>> {
    backend::read_catalog(backend_kind, &database_path(root))?
        .context("canonical catalog is missing")
}

fn recover_publication(backend_kind: BackendKind, root: &Path) -> Result<PublicationObservation> {
    let catalog = read_catalog(backend_kind, root)?;
    if let Some(attempt) = catalog.attempts.get(&ATTEMPT_SEQUENCE) {
        match attempt.phase {
            AttemptPhase::Running => {
                let interrupted = AttemptEnvelope {
                    sequence: ATTEMPT_SEQUENCE,
                    phase: AttemptPhase::Interrupted,
                    run_id: None,
                };
                write_json_durable(&attempt_path(root), &interrupted)?;
                update_catalog(backend_kind, root, |catalog| {
                    catalog.attempts.insert(
                        ATTEMPT_SEQUENCE,
                        AttemptCatalog {
                            phase: AttemptPhase::Interrupted,
                            run_id: None,
                        },
                    );
                    Ok(())
                })?;
                merge_latest(backend_kind, root, interrupted)?;
            }
            AttemptPhase::Complete => {
                ensure!(root.join("runs").join("run-1").join("run.json").is_file());
                merge_latest(
                    backend_kind,
                    root,
                    AttemptEnvelope {
                        sequence: ATTEMPT_SEQUENCE,
                        phase: AttemptPhase::Complete,
                        run_id: Some("run-1".to_owned()),
                    },
                )?;
            }
            AttemptPhase::Interrupted => {}
        }
    }
    if latest_temp_path(root).exists() {
        fs::remove_file(latest_temp_path(root))?;
    }
    publication_observation(backend_kind, root)
}

fn publication_observation(
    backend_kind: BackendKind,
    root: &Path,
) -> Result<PublicationObservation> {
    let catalog = read_catalog(backend_kind, root)?;
    let latest = read_latest(root)?;
    let attempt_phase = catalog
        .attempts
        .get(&ATTEMPT_SEQUENCE)
        .map(|attempt| attempt.phase);
    let run_directory_exists = root.join("runs").join("run-1").exists();
    Ok(PublicationObservation {
        next_sequence: catalog.next_sequence,
        attempt_phase,
        latest_attempt_phase: latest.latest_attempt.as_ref().map(|attempt| attempt.phase),
        latest_completed: latest.latest_completed,
        run_directory_exists,
        orphan_run: run_directory_exists && attempt_phase != Some(AttemptPhase::Complete),
    })
}

fn verify_publication(point: &str, observation: &PublicationObservation) -> Result<()> {
    match point {
        "before-attempt-catalog-allocation" => {
            ensure!(observation.next_sequence == 1);
            ensure!(observation.attempt_phase.is_none());
            ensure!(observation.latest_attempt_phase.is_none());
        }
        "after-catalog-allocation" => {
            ensure!(observation.next_sequence == 2);
            ensure!(observation.attempt_phase.is_none());
            ensure!(observation.latest_attempt_phase.is_none());
        }
        "after-running-envelope" | "after-latest-running" => {
            ensure!(observation.attempt_phase == Some(AttemptPhase::Interrupted));
            ensure!(observation.latest_attempt_phase == Some(AttemptPhase::Interrupted));
            ensure!(!observation.run_directory_exists);
        }
        "after-run-rename" => {
            ensure!(observation.attempt_phase == Some(AttemptPhase::Interrupted));
            ensure!(observation.latest_attempt_phase == Some(AttemptPhase::Interrupted));
            ensure!(observation.orphan_run);
            ensure!(observation.latest_completed.is_none());
        }
        "after-terminal-attempt" | "after-latest-temp" | "after-latest-replace" => {
            ensure!(observation.attempt_phase == Some(AttemptPhase::Complete));
            ensure!(observation.latest_attempt_phase == Some(AttemptPhase::Complete));
            ensure!(observation.latest_completed == Some(ATTEMPT_SEQUENCE));
            ensure!(!observation.orphan_run);
        }
        _ => bail!("unknown publication point"),
    }
    Ok(())
}

fn recover_retention(backend_kind: BackendKind, root: &Path) -> Result<RetentionObservation> {
    let catalog = read_catalog(backend_kind, root)?;
    match catalog.retention_plans.get(RETENTION_PLAN_ID).copied() {
        None | Some(RetentionPlanState::Prepared) => {}
        Some(RetentionPlanState::Pruning) => {
            let canonical = retention_canonical_payload(root);
            let trash = retention_trash_payload(root);
            match (canonical.exists(), trash.exists()) {
                (true, false) => move_payload_to_trash(root)?,
                (false, true) => {}
                (true, true) | (false, false) => {
                    bail!("retention item has both-or-neither canonical/trash identity")
                }
            }
            mark_pruned(backend_kind, root)?;
        }
        Some(RetentionPlanState::Pruned) => {
            ensure!(!retention_canonical_payload(root).exists());
            let trash = retention_trash_payload(root);
            if trash.exists() {
                fs::remove_dir_all(trash)?;
            }
        }
    }
    retention_observation(backend_kind, root)
}

fn retention_observation(backend_kind: BackendKind, root: &Path) -> Result<RetentionObservation> {
    let catalog = read_catalog(backend_kind, root)?;
    Ok(RetentionObservation {
        plan_state: catalog.retention_plans.get(RETENTION_PLAN_ID).copied(),
        record_state: *catalog
            .records
            .get(RETAINED_RUN_ID)
            .context("retention record is missing")?,
        canonical_payload_exists: retention_canonical_payload(root).exists(),
        trash_payload_exists: retention_trash_payload(root).exists(),
    })
}

fn verify_retention(point: &str, observation: &RetentionObservation) -> Result<()> {
    match point {
        "before-prepared-plan" => {
            ensure!(observation.plan_state.is_none());
            ensure!(observation.record_state == RecordState::Active);
            ensure!(observation.canonical_payload_exists);
        }
        "after-prepared-plan" => {
            ensure!(observation.plan_state == Some(RetentionPlanState::Prepared));
            ensure!(observation.record_state == RecordState::Active);
            ensure!(observation.canonical_payload_exists);
        }
        "after-pruning-commit"
        | "after-payload-move"
        | "after-pruned-commit"
        | "after-physical-reclamation" => {
            ensure!(observation.plan_state == Some(RetentionPlanState::Pruned));
            ensure!(observation.record_state == RecordState::Pruned);
            ensure!(!observation.canonical_payload_exists);
        }
        _ => bail!("unknown retention point"),
    }
    Ok(())
}

fn move_payload_to_trash(root: &Path) -> Result<()> {
    let canonical = retention_canonical_payload(root);
    let trash = retention_trash_payload(root);
    fs::create_dir_all(trash.parent().context("trash payload has no parent")?)?;
    fs::rename(canonical, trash)?;
    Ok(())
}

fn mark_pruned(backend_kind: BackendKind, root: &Path) -> Result<()> {
    update_catalog(backend_kind, root, |catalog| {
        catalog
            .retention_plans
            .insert(RETENTION_PLAN_ID.to_owned(), RetentionPlanState::Pruned);
        catalog
            .records
            .insert(RETAINED_RUN_ID.to_owned(), RecordState::Pruned);
        Ok(())
    })
}

fn recover_migration(backend_kind: BackendKind, root: &Path) -> Result<MigrationObservation> {
    let intent_path = migration_intent_path(root);
    if !intent_path.exists() {
        let catalog = read_catalog(backend_kind, root)?;
        return Ok(MigrationObservation {
            generation: catalog.generation,
            intent_exists: false,
            replacement_exists: replacement_database_path(root).exists(),
            stale_writer: None,
        });
    }
    let intent: MigrationIntent = serde_json::from_slice(&fs::read(&intent_path)?)?;
    ensure!(intent.from_generation == 1 && intent.to_generation == 2);
    let catalog = read_catalog(backend_kind, root)?;
    match catalog.generation {
        1 => {
            if !replacement_database_path(root).exists() {
                build_replacement(backend_kind, root)?;
            }
            let replacement = read_catalog_at(backend_kind, &replacement_database_path(root))?;
            ensure!(replacement.generation == 2);
            replace_canonical_store(backend_kind, root)?;
        }
        2 => {}
        generation => bail!("migration intent disagrees with generation {generation}"),
    }
    let final_catalog = read_catalog(backend_kind, root)?;
    ensure!(final_catalog.generation == 2);
    fs::remove_file(intent_path)?;
    Ok(MigrationObservation {
        generation: 2,
        intent_exists: false,
        replacement_exists: replacement_database_path(root).exists(),
        stale_writer: None,
    })
}

fn build_replacement(backend_kind: BackendKind, root: &Path) -> Result<()> {
    let replacement_path = replacement_database_path(root);
    remove_database_files(&replacement_path)?;
    let mut catalog = read_catalog(backend_kind, root)?;
    ensure!(catalog.generation == 1);
    catalog.generation = 2;
    backend::initialize(backend_kind, &replacement_path)?;
    let bytes = serde_json::to_vec(&catalog)?;
    ensure!(backend::compare_exchange_catalog(
        backend_kind,
        &replacement_path,
        None,
        &bytes
    )?);
    let read_back = read_catalog_at(backend_kind, &replacement_path)?;
    ensure!(read_back == catalog);
    backend::prepare_for_replace(backend_kind, &replacement_path)?;
    Ok(())
}

fn replace_canonical_store(backend_kind: BackendKind, root: &Path) -> Result<()> {
    let canonical = database_path(root);
    let replacement = replacement_database_path(root);
    backend::prepare_for_replace(backend_kind, &canonical)?;
    backend::prepare_for_replace(backend_kind, &replacement)?;
    atomic_replace_file(&replacement, &canonical)?;
    let catalog = read_catalog(backend_kind, root)?;
    ensure!(catalog.generation == 2);
    Ok(())
}

fn read_catalog_at(backend_kind: BackendKind, path: &Path) -> Result<Catalog> {
    let bytes = backend::read_catalog(backend_kind, path)?.context("catalog missing")?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn remove_database_files(path: &Path) -> Result<()> {
    for candidate in [
        path.to_path_buf(),
        path.with_file_name(format!("{}-wal", file_name(path))),
        path.with_file_name(format!("{}-shm", file_name(path))),
        path.with_file_name(format!("{}-journal", file_name(path))),
    ] {
        if candidate.exists() {
            fs::remove_file(candidate)?;
        }
    }
    Ok(())
}

fn merge_latest(backend_kind: BackendKind, root: &Path, candidate: AttemptEnvelope) -> Result<()> {
    let _guard = acquire_catalog_guard(root)?;
    let outcome = publish_latest_guarded(backend_kind, root, candidate)?;
    ensure!(
        outcome == "published",
        "ordinary publication was blocked by retention"
    );
    Ok(())
}

fn record_latest_commit(root: &Path, actor: &str) -> Result<usize> {
    let path = root.join("latest-commit-order.json");
    let mut actors = if path.exists() {
        serde_json::from_slice::<Vec<String>>(&fs::read(&path)?)?
    } else {
        Vec::new()
    };
    actors.push(actor.to_owned());
    write_json_durable(&path, &actors)?;
    Ok(actors.len())
}

fn publish_latest_guarded(
    backend_kind: BackendKind,
    root: &Path,
    candidate: AttemptEnvelope,
) -> Result<&'static str> {
    if let Some(run_id) = candidate.run_id.as_deref() {
        let catalog = read_catalog(backend_kind, root)?;
        if matches!(
            catalog.records.get(run_id),
            Some(RecordState::Pruning | RecordState::Pruned)
        ) {
            return Ok("retention_blocked");
        }
    }
    let latest = merged_latest(root, candidate)?;
    write_json_durable(&latest_path(root), &latest)?;
    Ok("published")
}

fn confirm_retention_guarded(backend_kind: BackendKind, root: &Path) -> Result<&'static str> {
    let latest = read_latest(root)?;
    if latest
        .latest_attempt
        .as_ref()
        .and_then(|attempt| attempt.run_id.as_deref())
        == Some(RACE_RUN_ID)
    {
        return Ok("stale_latest_target");
    }
    update_catalog(backend_kind, root, |catalog| {
        ensure!(
            catalog.retention_plans.get(RETENTION_PLAN_ID) == Some(&RetentionPlanState::Prepared),
            "retention plan is not prepared"
        );
        ensure!(
            catalog.records.get(RACE_RUN_ID) == Some(&RecordState::Active),
            "retention target is not active"
        );
        catalog
            .retention_plans
            .insert(RETENTION_PLAN_ID.to_owned(), RetentionPlanState::Pruning);
        catalog
            .records
            .insert(RACE_RUN_ID.to_owned(), RecordState::Pruning);
        Ok(())
    })?;
    Ok("entered_pruning")
}

fn merged_latest(root: &Path, candidate: AttemptEnvelope) -> Result<LatestPointer> {
    let mut latest = read_latest(root)?;
    let replace_attempt = latest.latest_attempt.as_ref().is_none_or(|current| {
        (candidate.sequence, candidate.phase.rank()) > (current.sequence, current.phase.rank())
    });
    if replace_attempt {
        latest.latest_attempt = Some(candidate.clone());
    }
    if candidate.phase == AttemptPhase::Complete
        && latest
            .latest_completed
            .is_none_or(|current| candidate.sequence > current)
    {
        latest.latest_completed = Some(candidate.sequence);
    }
    Ok(latest)
}

fn read_latest(root: &Path) -> Result<LatestPointer> {
    let path = latest_path(root);
    if path.exists() {
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    } else {
        Ok(LatestPointer::default())
    }
}

fn checkpoint(selected: &str, current: &str, ready: &Path) -> Result<()> {
    if selected == current {
        create_durable_marker(ready, current.as_bytes())?;
        wait_forever();
    }
    Ok(())
}

fn case_root(backend_root: &Path, domain: &str, point: &str) -> Result<PathBuf> {
    let root = backend_root.join(domain).join(point);
    if root.exists() {
        fs::remove_dir_all(&root)?;
    }
    fs::create_dir_all(&root)?;
    Ok(root)
}

fn database_path(root: &Path) -> PathBuf {
    root.join("lifecycle.store")
}

fn replacement_database_path(root: &Path) -> PathBuf {
    root.join("lifecycle.store.next")
}

fn attempt_path(root: &Path) -> PathBuf {
    root.join("attempts").join("1.json")
}

fn latest_path(root: &Path) -> PathBuf {
    root.join("latest.json")
}

fn latest_temp_path(root: &Path) -> PathBuf {
    root.join("latest.json.next")
}

fn migration_intent_path(root: &Path) -> PathBuf {
    root.join("lifecycle-migration.json")
}

fn retention_canonical_payload(root: &Path) -> PathBuf {
    root.join("runs").join(RETAINED_RUN_ID)
}

fn retention_trash_payload(root: &Path) -> PathBuf {
    root.join("trash")
        .join(RETENTION_PLAN_ID)
        .join(RETAINED_RUN_ID)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_owned()
}

fn acquire_catalog_guard(root: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join("lifecycle.lock"))?;
    file.lock()?;
    Ok(file)
}

fn acquire_catalog_read_guard(root: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join("lifecycle.lock"))?;
    file.lock_shared()?;
    Ok(file)
}

fn acquire_catalog_guard_after_contention(
    root: &Path,
    blocked_marker: &Path,
    watchdog: Duration,
) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join("lifecycle.lock"))?;
    let started = Instant::now();
    let mut blocked_recorded = false;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => {
                if !blocked_recorded {
                    create_durable_marker(blocked_marker, b"lock-blocked\n")?;
                    blocked_recorded = true;
                }
                ensure!(
                    started.elapsed() < watchdog,
                    "latest publisher lock-contention watchdog expired"
                );
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => {
                return Err(anyhow::Error::from(error)).context("try exclusive catalog guard");
            }
        }
    }
}

fn wait_for_clean_child(child: &mut std::process::Child, watchdog: Duration) -> Result<()> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            ensure!(status.success(), "child failed with {status}");
            return Ok(());
        }
        if started.elapsed() >= watchdog {
            child.kill()?;
            child.wait()?;
            bail!("concurrency child watchdog expired");
        }
        thread::sleep(Duration::from_millis(1));
    }
}
