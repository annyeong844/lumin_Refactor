use std::fs;
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};

use crate::backend;
use crate::model::{BackendKind, BenchmarkReport, LatencySummary};
use crate::util::{source_hashes, unix_millis};

const ARCHITECTURE_COMMIT: &str = "65e60216891bb3d826a4778f84cb8aaa377abe92";
const ARCHITECTURE_MANIFEST_SHA256: &str =
    "66925583362be22257dd7357072d176cd3f1ae3d4f99874a53f6689b7cf535e0";

pub fn run_benchmark(
    backend_kind: BackendKind,
    records: usize,
    record_bytes: usize,
    durable_transactions: usize,
    work_root: &Path,
) -> Result<BenchmarkReport> {
    ensure!(records > 0);
    ensure!(record_bytes > 0);
    ensure!(durable_transactions > 0);
    let started_unix_millis = unix_millis()?;
    let root = work_root.join(format!("benchmark-{backend_kind}"));
    if root.exists() {
        fs::remove_dir_all(&root)?;
    }
    fs::create_dir_all(&root)?;
    let database = root.join(backend_kind.file_name());

    let started = Instant::now();
    backend::initialize(backend_kind, &database)?;
    let initialize_micros = started.elapsed().as_micros();

    let fixture = (0..records)
        .map(|index| {
            let key = format!("finding-{index:012}");
            let digest = Sha256::digest(key.as_bytes());
            let value = digest.iter().copied().cycle().take(record_bytes).collect();
            (key, value)
        })
        .collect::<Vec<_>>();
    let started = Instant::now();
    backend::insert_records(backend_kind, &database, &fixture)?;
    let bulk_insert_micros = started.elapsed().as_micros();

    let started = Instant::now();
    let first_page = backend::query_records(backend_kind, &database, None, 100)?;
    let first_reopen_query_micros = started.elapsed().as_micros();
    ensure!(first_page == fixture[..first_page.len()]);

    let mut query_samples = Vec::with_capacity(100);
    for iteration in 0..100 {
        let offset = (iteration * 83) % records;
        let cursor = (offset > 0).then(|| fixture[offset - 1].0.as_str());
        let expected = &fixture[offset..records.min(offset + 100)];
        let started = Instant::now();
        let page = backend::query_records(backend_kind, &database, cursor, 100)?;
        query_samples.push(started.elapsed().as_micros());
        ensure!(page == expected);
    }

    let mut admission_samples = Vec::with_capacity(durable_transactions);
    for index in 0..durable_transactions {
        let key = format!("src/benchmark/{index:012}.ts");
        let gate = format!("gate-{index:012}");
        let started = Instant::now();
        let outcome = backend::admit(backend_kind, &database, &key, &gate)?;
        admission_samples.push(started.elapsed().as_micros());
        ensure!(
            outcome
                == crate::model::AdmissionOutcome::Admitted {
                    gate_id: gate.clone()
                }
        );
    }
    backend::prepare_for_replace(backend_kind, &database)?;

    let executable = std::env::current_exe()?;
    Ok(BenchmarkReport {
        probe_id: "lumin-store-backend-measurement-v1".to_owned(),
        architecture_commit: ARCHITECTURE_COMMIT.to_owned(),
        architecture_manifest_sha256: ARCHITECTURE_MANIFEST_SHA256.to_owned(),
        started_unix_millis,
        finished_unix_millis: unix_millis()?,
        host_os: std::env::consts::OS.to_owned(),
        host_arch: std::env::consts::ARCH.to_owned(),
        backend: backend_kind,
        executable_bytes: fs::metadata(&executable)?.len(),
        executable,
        command: std::env::args().collect(),
        records,
        record_bytes,
        durable_transactions,
        initialize_micros,
        bulk_insert_micros,
        first_reopen_query_micros,
        warm_reopen_query: summarize(query_samples)?,
        durable_admission: summarize(admission_samples)?,
        peak_working_set_bytes: peak_working_set_bytes()?,
        store_bytes: directory_size(&root)?,
        source_files: source_hashes()?,
        status: "PASS".to_owned(),
    })
}

fn summarize(mut samples: Vec<u128>) -> Result<LatencySummary> {
    ensure!(!samples.is_empty());
    samples.sort_unstable();
    let sum = samples.iter().sum::<u128>();
    Ok(LatencySummary {
        samples: samples.len(),
        min_micros: samples[0],
        p50_micros: percentile(&samples, 50),
        p95_micros: percentile(&samples, 95),
        p99_micros: percentile(&samples, 99),
        max_micros: *samples.last().context("latency samples are empty")?,
        mean_micros: sum / samples.len() as u128,
    })
}

fn percentile(samples: &[u128], percentile: usize) -> u128 {
    let index = (samples.len() * percentile).div_ceil(100).saturating_sub(1);
    samples[index]
}

fn directory_size(root: &Path) -> Result<u64> {
    let mut total = 0;
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path)? {
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

#[cfg(windows)]
fn peak_working_set_bytes() -> Result<Option<u64>> {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut counters = PROCESS_MEMORY_COUNTERS::default();
    // SAFETY: the pseudo-handle is valid for this process and counters has the advertised size.
    let result = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS>())?,
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error()).context("read process memory counters");
    }
    Ok(Some(counters.PeakWorkingSetSize as u64))
}

#[cfg(target_os = "linux")]
fn peak_working_set_bytes() -> Result<Option<u64>> {
    let status = fs::read_to_string("/proc/self/status")?;
    let value = status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|value| value.split_whitespace().next())
        .map(str::parse::<u64>)
        .transpose()?;
    Ok(value.map(|kilobytes| kilobytes * 1024))
}

#[cfg(not(any(windows, target_os = "linux")))]
fn peak_working_set_bytes() -> Result<Option<u64>> {
    Ok(None)
}
