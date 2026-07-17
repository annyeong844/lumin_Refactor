mod backend;
mod backend_contract;
mod benchmark;
mod lifecycle;
mod model;
mod namespace;
mod runner;
mod util;

use std::env;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};

use crate::model::{BackendKind, FaultMatrixReport, HoldPhase, IdentityReport, ProbeReport};
use crate::util::{
    executable_identity, source_hashes, source_manifest_sha256, unix_millis, write_json_durable,
};

const ARCHITECTURE_COMMIT: &str = "65e60216891bb3d826a4778f84cb8aaa377abe92";
const ARCHITECTURE_MANIFEST_SHA256: &str =
    "66925583362be22257dd7357072d176cd3f1ae3d4f99874a53f6689b7cf535e0";

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        for cause in error.chain().skip(1) {
            eprintln!("caused by: {cause}");
        }
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let arguments = env::args().collect::<Vec<_>>();
    let command = arguments.get(1).map(String::as_str).unwrap_or("help");
    match command {
        "run" => run_parent(&arguments[2..]),
        "identity" => run_identity(),
        "fault-matrix" => run_fault_matrix_parent(&arguments[2..]),
        "benchmark" => run_benchmark_parent(&arguments[2..]),
        "child-admit" => run_child_admit(&arguments[2..]),
        "child-hold" => run_child_hold(&arguments[2..]),
        "child-publication" => run_fault_child(&arguments[2..], "publication"),
        "child-retention" => run_fault_child(&arguments[2..], "retention"),
        "child-migration" => run_fault_child(&arguments[2..], "migration"),
        "child-stale-writer" => run_stale_writer_child(&arguments[2..]),
        "child-namespace" => run_namespace_child(&arguments[2..]),
        "child-latest-publish" => run_latest_publish_child(&arguments[2..]),
        "child-publication-retention" => run_publication_retention_child(&arguments[2..]),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        unknown => bail!("unknown command {unknown:?}"),
    }
}

fn run_identity() -> Result<()> {
    let executable = executable_identity()?;
    let source_files = source_hashes();
    let report = IdentityReport {
        probe_id: "lumin-phase0-store-probe-identity-v1".to_owned(),
        architecture_commit: ARCHITECTURE_COMMIT.to_owned(),
        architecture_manifest_sha256: ARCHITECTURE_MANIFEST_SHA256.to_owned(),
        executable: executable.path,
        executable_bytes: executable.bytes,
        executable_sha256: executable.sha256,
        source_manifest_sha256: source_manifest_sha256(&source_files),
        source_files,
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn run_benchmark_parent(arguments: &[String]) -> Result<()> {
    let backend = BackendKind::from_str(&required(arguments, "--backend")?)?;
    let records = option(arguments, "--records")?
        .unwrap_or_else(|| "10000".to_owned())
        .parse::<usize>()?;
    let record_bytes = option(arguments, "--record-bytes")?
        .unwrap_or_else(|| "256".to_owned())
        .parse::<usize>()?;
    let durable_transactions = option(arguments, "--durable-transactions")?
        .unwrap_or_else(|| "200".to_owned())
        .parse::<usize>()?;
    let output = PathBuf::from(
        option(arguments, "--output")?
            .unwrap_or_else(|| format!("evidence/benchmark-{backend}.json")),
    );
    let work_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("work");
    fs::create_dir_all(&work_root)?;
    let report = benchmark::run_benchmark(
        backend,
        records,
        record_bytes,
        durable_transactions,
        &work_root,
    )?;
    write_json_durable(&output, &report)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn run_fault_matrix_parent(arguments: &[String]) -> Result<()> {
    let backend_selection = option(arguments, "--backend")?.unwrap_or_else(|| "all".to_owned());
    let watchdog_millis = option(arguments, "--watchdog-ms")?
        .unwrap_or_else(|| "30000".to_owned())
        .parse::<u64>()
        .context("parse --watchdog-ms")?;
    ensure!(watchdog_millis > 0, "--watchdog-ms must be positive");
    let output = PathBuf::from(
        option(arguments, "--output")?.unwrap_or_else(|| "evidence/fault-matrix.json".to_owned()),
    );
    let backends = match backend_selection.as_str() {
        "all" => BackendKind::ALL.to_vec(),
        value => vec![BackendKind::from_str(value)?],
    };
    let started_unix_millis = unix_millis()?;
    let work_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("work");
    fs::create_dir_all(&work_root)?;
    let watchdog = Duration::from_millis(watchdog_millis);
    let backend_reports = backends
        .into_iter()
        .map(|backend| lifecycle::run_fault_backend(backend, watchdog, &work_root))
        .collect::<Vec<_>>();
    let overall_status = if backend_reports.iter().all(|report| report.status == "PASS") {
        "PASS"
    } else {
        "FAIL"
    };
    let executable = executable_identity()?;
    let source_files = source_hashes();
    let report = FaultMatrixReport {
        probe_id: "lumin-store-publication-retention-migration-fault-v1".to_owned(),
        architecture_commit: ARCHITECTURE_COMMIT.to_owned(),
        architecture_manifest_sha256: ARCHITECTURE_MANIFEST_SHA256.to_owned(),
        started_unix_millis,
        finished_unix_millis: unix_millis()?,
        host_os: env::consts::OS.to_owned(),
        host_arch: env::consts::ARCH.to_owned(),
        executable: executable.path,
        executable_bytes: executable.bytes,
        executable_sha256: executable.sha256,
        command: env::args().collect(),
        watchdog_millis,
        source_manifest_sha256: source_manifest_sha256(&source_files),
        source_files,
        backends: backend_reports,
        overall_status: overall_status.to_owned(),
    };
    write_json_durable(&output, &report)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    ensure!(
        overall_status == "PASS",
        "one or more fault-matrix cases failed"
    );
    Ok(())
}

fn run_parent(arguments: &[String]) -> Result<()> {
    let backend_selection = option(arguments, "--backend")?.unwrap_or_else(|| "all".to_owned());
    let rounds = option(arguments, "--rounds")?
        .unwrap_or_else(|| "32".to_owned())
        .parse::<u32>()
        .context("parse --rounds")?;
    ensure!(rounds > 0, "--rounds must be positive");
    let watchdog_millis = option(arguments, "--watchdog-ms")?
        .unwrap_or_else(|| "30000".to_owned())
        .parse::<u64>()
        .context("parse --watchdog-ms")?;
    ensure!(watchdog_millis > 0, "--watchdog-ms must be positive");
    let output = PathBuf::from(
        option(arguments, "--output")?.unwrap_or_else(|| "evidence/admission.json".to_owned()),
    );
    let backends = match backend_selection.as_str() {
        "all" => BackendKind::ALL.to_vec(),
        value => vec![BackendKind::from_str(value)?],
    };

    let started_unix_millis = unix_millis()?;
    let work_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("work");
    fs::create_dir_all(&work_root)?;
    let watchdog = Duration::from_millis(watchdog_millis);
    let backend_reports = backends
        .into_iter()
        .map(|backend| runner::run_backend(backend, rounds, watchdog, &work_root))
        .collect::<Vec<_>>();
    let overall_status = if backend_reports.iter().all(|report| report.status == "PASS") {
        "PASS"
    } else {
        "FAIL"
    };
    let executable = executable_identity()?;
    let source_files = source_hashes();
    let report = ProbeReport {
        probe_id: "lumin-store-cross-process-admission-v1".to_owned(),
        architecture_commit: ARCHITECTURE_COMMIT.to_owned(),
        architecture_manifest_sha256: ARCHITECTURE_MANIFEST_SHA256.to_owned(),
        started_unix_millis,
        finished_unix_millis: unix_millis()?,
        host_os: env::consts::OS.to_owned(),
        host_arch: env::consts::ARCH.to_owned(),
        executable: executable.path,
        executable_bytes: executable.bytes,
        executable_sha256: executable.sha256,
        command: env::args().collect(),
        rounds,
        watchdog_millis,
        source_manifest_sha256: source_manifest_sha256(&source_files),
        source_files,
        backends: backend_reports,
        overall_status: overall_status.to_owned(),
    };
    write_json_durable(&output, &report)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    ensure!(overall_status == "PASS", "one or more backends failed");
    Ok(())
}

fn run_child_admit(arguments: &[String]) -> Result<()> {
    let backend = BackendKind::from_str(&required(arguments, "--backend")?)?;
    let database = PathBuf::from(required(arguments, "--database")?);
    let barrier = PathBuf::from(required(arguments, "--barrier")?);
    let actor = required(arguments, "--actor")?;
    let key = required(arguments, "--key")?;
    let result = PathBuf::from(required(arguments, "--result")?);
    let watchdog_millis = required(arguments, "--watchdog-ms")?
        .parse::<u64>()
        .context("parse child --watchdog-ms")?;
    runner::run_child_admit(
        backend,
        &database,
        &barrier,
        &actor,
        &key,
        &result,
        Duration::from_millis(watchdog_millis),
    )
}

fn run_child_hold(arguments: &[String]) -> Result<()> {
    let backend = BackendKind::from_str(&required(arguments, "--backend")?)?;
    let database = PathBuf::from(required(arguments, "--database")?);
    let key = required(arguments, "--key")?;
    let gate = required(arguments, "--gate")?;
    let phase = HoldPhase::from_str(&required(arguments, "--phase")?)?;
    let ready = PathBuf::from(required(arguments, "--ready")?);
    backend::hold(backend, &database, &key, &gate, phase, &ready)
}

fn run_fault_child(arguments: &[String], domain: &str) -> Result<()> {
    let backend = BackendKind::from_str(&required(arguments, "--backend")?)?;
    let root = PathBuf::from(required(arguments, "--root")?);
    let point = required(arguments, "--point")?;
    let ready = PathBuf::from(required(arguments, "--ready")?);
    match domain {
        "publication" => lifecycle::run_publication_child(backend, &root, &point, &ready),
        "retention" => lifecycle::run_retention_child(backend, &root, &point, &ready),
        "migration" => lifecycle::run_migration_child(backend, &root, &point, &ready),
        _ => bail!("unknown fault child domain {domain}"),
    }
}

fn run_namespace_child(arguments: &[String]) -> Result<()> {
    let backend = BackendKind::from_str(&required(arguments, "--backend")?)?;
    let repository_root = PathBuf::from(required(arguments, "--repository-root")?);
    let operation = required(arguments, "--operation")?;
    let checkpoint = required(arguments, "--checkpoint")?;
    let ready = PathBuf::from(required(arguments, "--ready")?);
    let go = PathBuf::from(required(arguments, "--go")?);
    let result = PathBuf::from(required(arguments, "--result")?);
    let watchdog_millis = required(arguments, "--watchdog-ms")?
        .parse::<u64>()
        .context("parse namespace child --watchdog-ms")?;
    namespace::run_namespace_child(namespace::NamespaceChildRequest {
        backend_kind: backend,
        repository_root: &repository_root,
        operation: &operation,
        checkpoint: &checkpoint,
        ready: &ready,
        go: &go,
        result: &result,
        watchdog: Duration::from_millis(watchdog_millis),
    })
}

fn run_stale_writer_child(arguments: &[String]) -> Result<()> {
    let backend = BackendKind::from_str(&required(arguments, "--backend")?)?;
    let root = PathBuf::from(required(arguments, "--root")?);
    let ready = PathBuf::from(required(arguments, "--ready")?);
    let resume = PathBuf::from(required(arguments, "--resume")?);
    let result = PathBuf::from(required(arguments, "--result")?);
    let watchdog_millis = required(arguments, "--watchdog-ms")?
        .parse::<u64>()
        .context("parse stale-writer --watchdog-ms")?;
    lifecycle::run_stale_writer_child(lifecycle::StaleWriterRequest {
        backend,
        root,
        ready,
        resume,
        result,
        watchdog: Duration::from_millis(watchdog_millis),
    })
}

fn run_latest_publish_child(arguments: &[String]) -> Result<()> {
    let backend = BackendKind::from_str(&required(arguments, "--backend")?)?;
    let root = PathBuf::from(required(arguments, "--root")?);
    let sequence = required(arguments, "--sequence")?.parse::<u64>()?;
    let phase = required(arguments, "--phase")?;
    let barrier = PathBuf::from(required(arguments, "--barrier")?);
    let actor = required(arguments, "--actor")?;
    let hold_guard = required(arguments, "--hold-guard")?
        .parse::<bool>()
        .context("parse latest-publish --hold-guard")?;
    let result = PathBuf::from(required(arguments, "--result")?);
    let watchdog_millis = required(arguments, "--watchdog-ms")?.parse::<u64>()?;
    lifecycle::run_latest_publish_child(lifecycle::LatestPublishRequest {
        backend,
        root,
        sequence,
        phase,
        barrier,
        actor,
        hold_guard,
        result,
        watchdog: Duration::from_millis(watchdog_millis),
    })
}

fn run_publication_retention_child(arguments: &[String]) -> Result<()> {
    let backend = BackendKind::from_str(&required(arguments, "--backend")?)?;
    let root = PathBuf::from(required(arguments, "--root")?);
    let role = required(arguments, "--role")?;
    let barrier = PathBuf::from(required(arguments, "--barrier")?);
    let result = PathBuf::from(required(arguments, "--result")?);
    let hold_guard = required(arguments, "--hold-guard")?
        .parse::<bool>()
        .context("parse publication-retention --hold-guard")?;
    let watchdog_millis = required(arguments, "--watchdog-ms")?
        .parse::<u64>()
        .context("parse publication-retention --watchdog-ms")?;
    lifecycle::run_publication_retention_child(lifecycle::PublicationRetentionRequest {
        backend,
        root,
        role,
        barrier,
        result,
        hold_guard,
        watchdog: Duration::from_millis(watchdog_millis),
    })
}

fn option(arguments: &[String], name: &str) -> Result<Option<String>> {
    let mut values = arguments.iter();
    while let Some(argument) = values.next() {
        if argument == name {
            return values
                .next()
                .cloned()
                .map(Some)
                .with_context(|| format!("missing value for {name}"));
        }
    }
    Ok(None)
}

fn required(arguments: &[String], name: &str) -> Result<String> {
    option(arguments, name)?.with_context(|| format!("missing required option {name}"))
}

fn print_help() {
    println!(
        "lumin-phase0-store-probe\n\n\
         run --backend all|redb|sqlite [--rounds N] [--watchdog-ms N] [--output PATH]\n\
         fault-matrix --backend all|redb|sqlite [--watchdog-ms N] [--output PATH]\n\
         benchmark --backend redb|sqlite [--records N] [--record-bytes N]\n\
           [--durable-transactions N] [--output PATH]"
    );
}
