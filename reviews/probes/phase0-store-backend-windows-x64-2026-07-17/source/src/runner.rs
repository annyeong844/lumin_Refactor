use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};

use crate::backend;
use crate::model::{
    AdmissionOutcome, BackendKind, BackendReport, ChildResult, ContentionRound, CrashCaseResult,
};
use crate::util::{create_durable_marker, wait_for_path, write_json_durable};

pub fn run_backend(
    backend_kind: BackendKind,
    rounds: u32,
    watchdog: Duration,
    work_root: &Path,
) -> BackendReport {
    match run_backend_inner(backend_kind, rounds, watchdog, work_root) {
        Ok(report) => report,
        Err(error) => BackendReport {
            backend: backend_kind,
            status: "FAIL".to_owned(),
            error: Some(format!("{error:#}")),
            contention_rounds: Vec::new(),
            disjoint_rounds: Vec::new(),
            crash_cases: Vec::new(),
            database_bytes: directory_size(work_root).unwrap_or(0),
        },
    }
}

fn run_backend_inner(
    backend_kind: BackendKind,
    rounds: u32,
    watchdog: Duration,
    work_root: &Path,
) -> Result<BackendReport> {
    let backend_root = work_root.join(backend_kind.to_string());
    if backend_root.exists() {
        fs::remove_dir_all(&backend_root)
            .with_context(|| format!("remove previous work {}", backend_root.display()))?;
    }
    fs::create_dir_all(&backend_root)
        .with_context(|| format!("create backend work {}", backend_root.display()))?;

    let mut contention_rounds = Vec::with_capacity(rounds as usize);
    let mut disjoint_rounds = Vec::with_capacity(rounds as usize);
    for round in 0..rounds {
        contention_rounds.push(run_contention_round(
            backend_kind,
            round,
            true,
            watchdog,
            &backend_root,
        )?);
        disjoint_rounds.push(run_contention_round(
            backend_kind,
            round,
            false,
            watchdog,
            &backend_root,
        )?);
    }

    let crash_cases = vec![
        run_crash_case(backend_kind, "uncommitted", watchdog, &backend_root)?,
        run_crash_case(backend_kind, "committed", watchdog, &backend_root)?,
    ];

    Ok(BackendReport {
        backend: backend_kind,
        status: "PASS".to_owned(),
        error: None,
        contention_rounds,
        disjoint_rounds,
        crash_cases,
        database_bytes: directory_size(&backend_root)?,
    })
}

fn run_contention_round(
    backend_kind: BackendKind,
    round: u32,
    conflicting: bool,
    watchdog: Duration,
    backend_root: &Path,
) -> Result<ContentionRound> {
    let mode = if conflicting { "conflict" } else { "disjoint" };
    let round_root = backend_root.join(format!("{mode}-{round:04}"));
    fs::create_dir_all(&round_root)?;
    let database_path = round_root.join(backend_kind.file_name());
    backend::initialize(backend_kind, &database_path)?;

    let barrier = round_root.join("barrier");
    fs::create_dir_all(&barrier)?;
    let go_path = barrier.join("go");
    let key_a = format!("src/{mode}/{round:04}/a.ts");
    let key_b = if conflicting {
        key_a.clone()
    } else {
        format!("src/{mode}/{round:04}/b.ts")
    };
    let actors = [("actor-a", key_a.as_str()), ("actor-b", key_b.as_str())];

    let started = Instant::now();
    let mut children = Vec::with_capacity(2);
    for (actor, key) in actors {
        children.push(spawn_admit_child(
            backend_kind,
            &database_path,
            &barrier,
            actor,
            key,
            &round_root.join(format!("{actor}.json")),
            watchdog,
        )?);
    }
    wait_for_path(&barrier.join("actor-a.ready"), watchdog)?;
    wait_for_path(&barrier.join("actor-b.ready"), watchdog)?;
    create_durable_marker(&go_path, b"go\n")?;
    for child in &mut children {
        let status = wait_for_child(child, watchdog)?;
        ensure!(status.success(), "admission child failed with {status}");
    }

    let child_results = ["actor-a", "actor-b"]
        .into_iter()
        .map(|actor| read_child_result(&round_root.join(format!("{actor}.json"))))
        .collect::<Result<Vec<_>>>()?;
    let canonical_holders = [key_a.clone(), key_b.clone()]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|key| {
            let holder = backend::read_holder(backend_kind, &database_path, &key)?;
            Ok((key, holder))
        })
        .collect::<Result<Vec<_>>>()?;

    if conflicting {
        verify_conflicting(&child_results, &canonical_holders)?;
    } else {
        verify_disjoint(&child_results, &canonical_holders)?;
    }

    Ok(ContentionRound {
        round,
        conflicting,
        child_results,
        canonical_holders,
        elapsed_micros: started.elapsed().as_micros(),
    })
}

fn run_crash_case(
    backend_kind: BackendKind,
    phase: &str,
    watchdog: Duration,
    backend_root: &Path,
) -> Result<CrashCaseResult> {
    let case_root = backend_root.join(format!("crash-{phase}"));
    fs::create_dir_all(&case_root)?;
    let database_path = case_root.join(backend_kind.file_name());
    backend::initialize(backend_kind, &database_path)?;
    let key = format!("src/crash/{phase}.ts");
    let holder_gate = format!("crash-{phase}-gate");
    let ready_path = case_root.join("holder.ready");

    let started = Instant::now();
    let mut holder = Command::new(env::current_exe()?)
        .arg("child-hold")
        .arg("--backend")
        .arg(backend_kind.to_string())
        .arg("--database")
        .arg(&database_path)
        .arg("--key")
        .arg(&key)
        .arg("--gate")
        .arg(&holder_gate)
        .arg("--phase")
        .arg(phase)
        .arg("--ready")
        .arg(&ready_path)
        .spawn()
        .context("spawn crash holder child")?;
    wait_for_path(&ready_path, watchdog)?;
    holder.kill().context("kill crash holder child")?;
    holder.wait().context("reap crash holder child")?;

    let barrier = case_root.join("recovery-barrier");
    fs::create_dir_all(&barrier)?;
    let result_path = case_root.join("recovery.json");
    let mut recovery = spawn_admit_child(
        backend_kind,
        &database_path,
        &barrier,
        "recovery",
        &key,
        &result_path,
        watchdog,
    )?;
    wait_for_path(&barrier.join("recovery.ready"), watchdog)?;
    create_durable_marker(&barrier.join("go"), b"go\n")?;
    let recovery_status = wait_for_child(&mut recovery, watchdog)?;
    ensure!(
        recovery_status.success(),
        "recovery child failed with {recovery_status}"
    );

    let recovery_outcome = read_child_result(&result_path)?;
    let canonical_holder = backend::read_holder(backend_kind, &database_path, &key)?;
    match phase {
        "uncommitted" => {
            ensure!(
                recovery_outcome.outcome
                    == AdmissionOutcome::Admitted {
                        gate_id: "recovery".to_owned()
                    },
                "uncommitted crash did not roll back: {:?}",
                recovery_outcome.outcome
            );
            ensure!(canonical_holder.as_deref() == Some("recovery"));
        }
        "committed" => {
            ensure!(
                recovery_outcome.outcome
                    == AdmissionOutcome::Conflict {
                        holder_gate_id: holder_gate.clone()
                    },
                "durable commit was not preserved: {:?}",
                recovery_outcome.outcome
            );
            ensure!(canonical_holder.as_deref() == Some(holder_gate.as_str()));
        }
        _ => bail!("unknown crash phase {phase}"),
    }

    Ok(CrashCaseResult {
        phase: phase.to_owned(),
        recovery_outcome,
        canonical_holder,
        elapsed_micros: started.elapsed().as_micros(),
    })
}

fn spawn_admit_child(
    backend_kind: BackendKind,
    database_path: &Path,
    barrier: &Path,
    actor: &str,
    key: &str,
    result_path: &Path,
    watchdog: Duration,
) -> Result<Child> {
    Command::new(env::current_exe()?)
        .arg("child-admit")
        .arg("--backend")
        .arg(backend_kind.to_string())
        .arg("--database")
        .arg(database_path)
        .arg("--barrier")
        .arg(barrier)
        .arg("--actor")
        .arg(actor)
        .arg("--key")
        .arg(key)
        .arg("--result")
        .arg(result_path)
        .arg("--watchdog-ms")
        .arg(watchdog.as_millis().to_string())
        .spawn()
        .context("spawn admission child")
}

fn wait_for_child(child: &mut Child, timeout: Duration) -> Result<ExitStatus> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().context("poll child")? {
            return Ok(status);
        }
        if started.elapsed() >= timeout {
            child.kill().context("kill wedged child")?;
            child.wait().context("reap wedged child")?;
            bail!("watchdog expired waiting for child process");
        }
        thread::sleep(Duration::from_millis(1));
    }
}

fn read_child_result(path: &Path) -> Result<ChildResult> {
    let bytes = fs::read(path).with_context(|| format!("read child result {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse child result {}", path.display()))
}

fn verify_conflicting(
    child_results: &[ChildResult],
    canonical_holders: &[(String, Option<String>)],
) -> Result<()> {
    let admitted = child_results
        .iter()
        .filter_map(|result| match &result.outcome {
            AdmissionOutcome::Admitted { gate_id } => Some(gate_id.as_str()),
            AdmissionOutcome::Conflict { .. } => None,
        })
        .collect::<Vec<_>>();
    let conflicts = child_results
        .iter()
        .filter_map(|result| match &result.outcome {
            AdmissionOutcome::Conflict { holder_gate_id } => Some(holder_gate_id.as_str()),
            AdmissionOutcome::Admitted { .. } => None,
        })
        .collect::<Vec<_>>();
    ensure!(
        admitted.len() == 1,
        "expected one admission, got {admitted:?}"
    );
    ensure!(
        conflicts.len() == 1,
        "expected one conflict, got {conflicts:?}"
    );
    ensure!(
        conflicts[0] == admitted[0],
        "conflict holder differs from winner"
    );
    ensure!(canonical_holders.len() == 1);
    ensure!(canonical_holders[0].1.as_deref() == Some(admitted[0]));
    Ok(())
}

fn verify_disjoint(
    child_results: &[ChildResult],
    canonical_holders: &[(String, Option<String>)],
) -> Result<()> {
    ensure!(
        child_results
            .iter()
            .all(|result| matches!(result.outcome, AdmissionOutcome::Admitted { .. })),
        "disjoint writers did not both commit: {child_results:?}"
    );
    for result in child_results {
        let expected_gate = match &result.outcome {
            AdmissionOutcome::Admitted { gate_id } => gate_id,
            AdmissionOutcome::Conflict { .. } => unreachable!(),
        };
        let actual = canonical_holders
            .iter()
            .find(|(key, _)| key == &result.key)
            .and_then(|(_, holder)| holder.as_deref());
        ensure!(actual == Some(expected_gate.as_str()));
    }
    Ok(())
}

fn directory_size(root: &Path) -> Result<u64> {
    if !root.exists() {
        return Ok(0);
    }
    let mut total = 0;
    let mut pending = vec![PathBuf::from(root)];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(&path)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                total += metadata.len();
            }
        }
    }
    Ok(total)
}

pub fn run_child_admit(
    backend_kind: BackendKind,
    database_path: &Path,
    barrier: &Path,
    actor: &str,
    key: &str,
    result_path: &Path,
    watchdog: Duration,
) -> Result<()> {
    create_durable_marker(&barrier.join(format!("{actor}.ready")), b"ready\n")?;
    wait_for_path(&barrier.join("go"), watchdog)?;
    let started = Instant::now();
    let outcome = backend::admit(backend_kind, database_path, key, actor)?;
    write_json_durable(
        result_path,
        &ChildResult {
            backend: backend_kind,
            actor: actor.to_owned(),
            key: key.to_owned(),
            elapsed_micros: started.elapsed().as_micros(),
            outcome,
        },
    )
}
