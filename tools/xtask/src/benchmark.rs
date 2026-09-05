use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use sha2::{Digest, Sha256};

mod fixture;
mod measurement;
mod truth;

const REPETITIONS: usize = 3;
const WORKER_STACK_BYTES: u64 = 4_194_304;
const MAX_BINARY_BYTES: u64 = 12_582_912;
const MAX_PEAK_RSS_BYTES: u64 = 536_870_912;

pub(crate) fn run(arguments: &[String]) -> ExitCode {
    if arguments != ["foundation"] {
        eprintln!("[TOOL ERROR] usage: lumin-xtask benchmark foundation");
        return ExitCode::from(2);
    }
    match run_foundation() {
        Ok((report, passed)) => {
            let encoded = match serde_json::to_string_pretty(&report) {
                Ok(encoded) => encoded,
                Err(error) => {
                    eprintln!("[FAIL] cannot render benchmark report: {error}");
                    return ExitCode::FAILURE;
                }
            };
            println!("{encoded}");
            if passed {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("[FAIL] {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_foundation() -> Result<(Value, bool), String> {
    let workspace = crate::metadata::find_workspace_root().map_err(|error| error.to_string())?;
    let python = measurement::require_python()?;
    let script = workspace.join("tools/xtask/benchmark/measure-process.py");
    let scratch = create_scratch(&workspace)?;
    let report_path = report_path(&workspace, &scratch)?;
    let host = measurement::inspect_host(&python, &script, &scratch)?;
    let package = crate::package_check::load_host_package()?;
    let binary_bytes = fs::read(&package.binary).map_err(|error| {
        format!(
            "cannot read packaged binary {}: {error}",
            package.binary.display()
        )
    })?;
    let fixture = fixture::prepare(&workspace, &scratch, &python)?;
    verify_execution_policy(&workspace)?;

    let available_parallelism = std::thread::available_parallelism()
        .map_err(|error| format!("cannot observe available parallelism: {error}"))?
        .get();
    let default_jobs = available_parallelism.clamp(1, 8);
    let mut runner = MatrixRunner {
        binary: &package.binary,
        python: &python,
        script: &script,
        scratch: &scratch,
        fixture: &fixture,
        default_jobs,
        samples: Vec::new(),
        times: BTreeMap::new(),
        peak_rss_bytes: 0,
        semantic_reference: None,
    };
    let cache_conditioning = runner.condition_os_cache()?;
    runner.run()?;
    if runner.samples.len() != 7 * REPETITIONS {
        return Err(format!(
            "benchmark matrix produced {} measured samples; expected {}",
            runner.samples.len(),
            7 * REPETITIONS
        ));
    }
    let semantic_reference = runner
        .semantic_reference
        .as_ref()
        .ok_or_else(|| "benchmark matrix produced no semantic evidence".to_owned())?
        .report_value()?;
    let summary = summarize(
        &runner.times,
        runner.peak_rss_bytes,
        binary_bytes.len() as u64,
        default_jobs,
        host.blocking,
    )?;
    let target_misses = summary
        .pointer("/targetMisses")
        .and_then(Value::as_array)
        .ok_or_else(|| "benchmark summary omitted targetMisses".to_owned())?;
    let passed = !host.blocking || target_misses.is_empty();
    let status = if host.blocking {
        if passed { "PASS" } else { "FAIL" }
    } else {
        "REPORT_ONLY"
    };
    let report = serde_json::json!({
        "schemaVersion": "lumin.phase1-foundation-benchmark.v1",
        "status": status,
        "blocking": host.blocking,
        "environment": host.classification,
        "capturedAtUnixNanoseconds": SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| format!("system clock precedes Unix epoch: {error}"))?
            .as_nanos()
            .to_string(),
        "host": host.details,
        "osCacheState": "machine-global caches were conditioned by one unmeasured fresh jobs=1 audit and not flushed; cold means fresh repository, state namespace, and process",
        "osCacheConditioning": cache_conditioning,
        "scanInvocation": {"include": ["**"], "findingFilters": {}},
        "toolchain": measurement::python_identity(&python, &script, &host.details)?,
        "package": {
            "target": host_target(),
            "root": package.root.to_string_lossy(),
            "buildId": package.build_id,
            "binary": package.binary.to_string_lossy(),
            "binaryBytes": binary_bytes.len(),
            "binarySha256": sha256_hex(&binary_bytes),
        },
        "fixture": fixture.identity,
        "observedAvailableParallelism": available_parallelism,
        "defaultJobs": default_jobs,
        "workerStackBytes": WORKER_STACK_BYTES,
        "repetitionsPerMode": REPETITIONS,
        "semanticTruth": semantic_reference,
        "samples": runner.samples,
        "summary": summary,
    });

    fs::remove_dir_all(&scratch).map_err(|error| {
        format!(
            "cannot remove completed benchmark scratch {}: {error}",
            scratch.display()
        )
    })?;
    write_report(&report_path, &report)?;
    Ok((report, passed))
}

struct MatrixRunner<'a> {
    binary: &'a Path,
    python: &'a Path,
    script: &'a Path,
    scratch: &'a Path,
    fixture: &'a fixture::Fixture,
    default_jobs: usize,
    samples: Vec<Value>,
    times: BTreeMap<&'static str, Vec<u64>>,
    peak_rss_bytes: u64,
    semantic_reference: Option<truth::SemanticDump>,
}

impl MatrixRunner<'_> {
    fn condition_os_cache(&mut self) -> Result<Value, String> {
        // The fixed measured order must not make jobs=1 inherit binary and fixture page-cache
        // warmth from the default-jobs sample. One disclosed serial audit exercises the complete
        // product path before either compared mode while each measured repository, namespace,
        // and process remains fresh as required by the frozen cold-sample definition.
        let mode = "os-cache-conditioning-jobs-1";
        let (repository, copy_ns) = self.copy_repository(mode, 0)?;
        let measured = self.capture(
            &repository,
            &measurement::arguments(&[
                "audit",
                "--include",
                "**",
                "--jobs",
                "1",
                "--format",
                "json",
            ]),
            mode,
            0,
            "unmeasured",
        )?;
        let (run_id, sequence) = truth::audit_scope(&measured.response)?;
        let validation = Instant::now();
        let dump = truth::validate_semantic_dump(
            self.binary,
            &repository,
            &self.fixture.truth,
            truth::Scope::Run { run_id: &run_id },
        )?;
        self.accept_semantics(&dump)?;
        let report = serde_json::json!({
            "mode": mode,
            "measured": false,
            "requestedJobs": 1,
            "actualJobs": 1,
            "cacheState": "fresh-state-preconditioning",
            "fixtureCopyNanoseconds": copy_ns,
            "truthValidationNanoseconds": elapsed_ns(validation)?,
            "scope": {"kind": "run", "id": run_id, "sequence": sequence},
            "semanticDumpSha256": dump.sha256()?,
            "productProcess": measured.raw,
        });
        self.remove_repository(&repository)?;
        Ok(report)
    }

    fn run(&mut self) -> Result<(), String> {
        for repetition in 1..=REPETITIONS {
            self.audit_sample("cold-audit-default", repetition, None, false)?;
            self.audit_sample("cold-audit-jobs-1", repetition, Some(1), false)?;
            self.audit_sample("warm-audit-default", repetition, None, true)?;
            self.pre_write_sample("cold-pre-write-default", repetition, false)?;
            self.pre_write_sample("warm-pre-write-default", repetition, true)?;
            self.post_write_sample("post-write-one-file-default", repetition, false)?;
            self.post_write_sample("post-write-32-files-default", repetition, true)?;
        }
        Ok(())
    }

    fn audit_sample(
        &mut self,
        mode: &'static str,
        repetition: usize,
        jobs: Option<usize>,
        warm: bool,
    ) -> Result<(), String> {
        let (repository, copy_ns) = self.copy_repository(mode, repetition)?;
        let mut stages = serde_json::Map::new();
        stages.insert("fixtureCopy".to_owned(), copy_ns.into());
        if warm {
            let seed = self.capture(
                &repository,
                &measurement::arguments(&["audit", "--include", "**", "--format", "json"]),
                mode,
                repetition,
                "seed",
            )?;
            let (run_id, _) = truth::audit_scope(&seed.response)?;
            let validation = Instant::now();
            let dump = truth::validate_semantic_dump(
                self.binary,
                &repository,
                &self.fixture.truth,
                truth::Scope::Run { run_id: &run_id },
            )?;
            self.accept_semantics(&dump)?;
            stages.insert("seedProcess".to_owned(), seed.elapsed_nanoseconds.into());
            stages.insert(
                "seedTruthValidation".to_owned(),
                elapsed_ns(validation)?.into(),
            );
        }
        let arguments = match jobs {
            Some(1) => measurement::arguments(&[
                "audit",
                "--include",
                "**",
                "--jobs",
                "1",
                "--format",
                "json",
            ]),
            Some(other) => {
                return Err(format!(
                    "unsupported explicit benchmark jobs value: {other}"
                ));
            }
            None => measurement::arguments(&["audit", "--include", "**", "--format", "json"]),
        };
        let measured = self.capture(&repository, &arguments, mode, repetition, "measured")?;
        let (run_id, sequence) = truth::audit_scope(&measured.response)?;
        let validation = Instant::now();
        let dump = truth::validate_semantic_dump(
            self.binary,
            &repository,
            &self.fixture.truth,
            truth::Scope::Run { run_id: &run_id },
        )?;
        self.accept_semantics(&dump)?;
        stages.insert(
            "measuredProcess".to_owned(),
            measured.elapsed_nanoseconds.into(),
        );
        stages.insert("truthValidation".to_owned(), elapsed_ns(validation)?.into());
        self.record(
            mode,
            repetition,
            jobs,
            warm,
            measured,
            Value::Object(stages),
            serde_json::json!({"kind": "run", "id": run_id, "sequence": sequence}),
            &dump,
        )?;
        self.remove_repository(&repository)
    }

    fn pre_write_sample(
        &mut self,
        mode: &'static str,
        repetition: usize,
        warm: bool,
    ) -> Result<(), String> {
        let (repository, copy_ns) = self.copy_repository(mode, repetition)?;
        let mut stages = serde_json::Map::new();
        stages.insert("fixtureCopy".to_owned(), copy_ns.into());
        if warm {
            let seed = self.capture(
                &repository,
                &measurement::arguments(&["audit", "--include", "**", "--format", "json"]),
                mode,
                repetition,
                "seed",
            )?;
            let (run_id, _) = truth::audit_scope(&seed.response)?;
            let validation = Instant::now();
            let dump = truth::validate_semantic_dump(
                self.binary,
                &repository,
                &self.fixture.truth,
                truth::Scope::Run { run_id: &run_id },
            )?;
            self.accept_semantics(&dump)?;
            stages.insert("seedProcess".to_owned(), seed.elapsed_nanoseconds.into());
            stages.insert(
                "seedTruthValidation".to_owned(),
                elapsed_ns(validation)?.into(),
            );
        }
        let operation = format!("bench-{mode}-{repetition}");
        let arguments = vec![
            "pre-write".into(),
            "--operation-id".into(),
            operation.into(),
            "--path".into(),
            "packages/pkg-00/src/live/live-000.ts".into(),
            "--include".into(),
            "**".into(),
            "--format".into(),
            "json".into(),
        ];
        let measured = self.capture(&repository, &arguments, mode, repetition, "measured")?;
        let (gate_id, revision) = truth::gate_scope(&measured.response, "baseline")?;
        let validation = Instant::now();
        let dump = truth::validate_semantic_dump(
            self.binary,
            &repository,
            &self.fixture.truth,
            truth::Scope::Gate {
                gate_id: &gate_id,
                revision,
            },
        )?;
        self.accept_semantics(&dump)?;
        stages.insert(
            "measuredProcess".to_owned(),
            measured.elapsed_nanoseconds.into(),
        );
        stages.insert("truthValidation".to_owned(), elapsed_ns(validation)?.into());
        self.record(
            mode,
            repetition,
            None,
            warm,
            measured,
            Value::Object(stages),
            serde_json::json!({"kind": "gate-attempt", "gateId": gate_id, "revision": revision}),
            &dump,
        )?;
        self.remove_repository(&repository)
    }

    fn post_write_sample(
        &mut self,
        mode: &'static str,
        repetition: usize,
        wave: bool,
    ) -> Result<(), String> {
        let (repository, copy_ns) = self.copy_repository(mode, repetition)?;
        let mut arguments = vec![
            "pre-write".into(),
            "--operation-id".into(),
            format!("bench-{mode}-open-{repetition}").into(),
        ];
        if wave {
            for package in 0..8 {
                for source in 0..4 {
                    arguments.push("--path".into());
                    arguments.push(
                        format!("packages/pkg-{package:02}/src/live/live-{source:03}.ts").into(),
                    );
                }
            }
        } else {
            arguments.push("--path".into());
            arguments.push("packages/pkg-00/src/live/live-000.ts".into());
        }
        arguments.push("--include".into());
        arguments.push("**".into());
        arguments.push("--format".into());
        arguments.push("json".into());
        let setup = self.capture(&repository, &arguments, mode, repetition, "setup")?;
        let (gate_id, baseline_revision) = truth::gate_scope(&setup.response, "baseline")?;
        let setup_validation = Instant::now();
        let setup_dump = truth::validate_semantic_dump(
            self.binary,
            &repository,
            &self.fixture.truth,
            truth::Scope::Gate {
                gate_id: &gate_id,
                revision: baseline_revision,
            },
        )?;
        self.accept_semantics(&setup_dump)?;
        let setup_validation_ns = elapsed_ns(setup_validation)?;
        let mutation = Instant::now();
        if wave {
            fixture::mutate_wave(&repository)?;
        } else {
            fixture::mutate_one(&repository)?;
        }
        let mutation_ns = elapsed_ns(mutation)?;
        let measured = self.capture(
            &repository,
            &[
                "post-write".into(),
                OsString::from(&gate_id),
                "--operation-id".into(),
                format!("bench-{mode}-close-{repetition}").into(),
                "--format".into(),
                "json".into(),
            ],
            mode,
            repetition,
            "measured",
        )?;
        let (closed_gate_id, revision) = truth::gate_scope(&measured.response, "close")?;
        if closed_gate_id != gate_id || revision <= baseline_revision {
            return Err("post-write returned the wrong gate revision".to_owned());
        }
        let validation = Instant::now();
        let dump = truth::validate_semantic_dump(
            self.binary,
            &repository,
            &self.fixture.truth,
            truth::Scope::Gate {
                gate_id: &closed_gate_id,
                revision,
            },
        )?;
        self.accept_semantics(&dump)?;
        let stages = serde_json::json!({
            "fixtureCopy": copy_ns,
            "setupProcess": setup.elapsed_nanoseconds,
            "setupTruthValidation": setup_validation_ns,
            "numericOnlyMutation": mutation_ns,
            "measuredProcess": measured.elapsed_nanoseconds,
            "truthValidation": elapsed_ns(validation)?,
        });
        self.record(
            mode,
            repetition,
            None,
            true,
            measured,
            stages,
            serde_json::json!({"kind": "gate-attempt", "gateId": closed_gate_id, "revision": revision}),
            &dump,
        )?;
        self.remove_repository(&repository)
    }

    fn copy_repository(&self, mode: &str, repetition: usize) -> Result<(PathBuf, u64), String> {
        let repository = self
            .scratch
            .join("repositories")
            .join(format!("{mode}-{repetition}"));
        let started = Instant::now();
        fixture::copy_repository(&self.fixture.root, &repository)?;
        Ok((repository, elapsed_ns(started)?))
    }

    fn capture(
        &self,
        repository: &Path,
        arguments: &[OsString],
        mode: &str,
        repetition: usize,
        role: &str,
    ) -> Result<measurement::ProcessMeasurement, String> {
        measurement::measure_product(
            self.python,
            self.script,
            self.binary,
            repository,
            arguments,
            &self
                .scratch
                .join("captures")
                .join(format!("{mode}-{repetition}-{role}")),
        )
    }

    fn accept_semantics(&mut self, dump: &truth::SemanticDump) -> Result<(), String> {
        if let Some(reference) = &self.semantic_reference {
            if reference != dump {
                return Err(
                    "finding IDs changed across cold, warm, jobs, or gate samples".to_owned(),
                );
            }
        } else {
            self.semantic_reference = Some(dump.clone());
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn record(
        &mut self,
        mode: &'static str,
        repetition: usize,
        requested_jobs: Option<usize>,
        warm: bool,
        measured: measurement::ProcessMeasurement,
        stage_timings: Value,
        scope: Value,
        dump: &truth::SemanticDump,
    ) -> Result<(), String> {
        self.peak_rss_bytes = self.peak_rss_bytes.max(measured.peak_rss_bytes);
        self.times
            .entry(mode)
            .or_default()
            .push(measured.elapsed_nanoseconds);
        self.samples.push(serde_json::json!({
            "mode": mode,
            "repetition": repetition,
            "cacheState": if warm { "same-state-after-unmeasured-seed" } else { "fresh-state" },
            "requestedJobs": requested_jobs,
            "actualJobs": requested_jobs.unwrap_or(self.default_jobs),
            "workerStackBytes": WORKER_STACK_BYTES,
            "scope": scope,
            "semanticDumpSha256": dump.sha256()?,
            "stageTimingsNanoseconds": stage_timings,
            "productProcess": measured.raw,
        }));
        Ok(())
    }

    fn remove_repository(&self, repository: &Path) -> Result<(), String> {
        if !repository.starts_with(self.scratch.join("repositories")) {
            return Err("refusing to remove a benchmark repository outside scratch".to_owned());
        }
        fs::remove_dir_all(repository).map_err(|error| {
            format!(
                "cannot remove benchmark repository {}: {error}",
                repository.display()
            )
        })
    }
}

fn summarize(
    times: &BTreeMap<&'static str, Vec<u64>>,
    peak_rss_bytes: u64,
    binary_bytes: u64,
    default_jobs: usize,
    blocking: bool,
) -> Result<Value, String> {
    let targets = [
        ("cold-audit-default", 30_000_u64),
        ("warm-audit-default", 8_000),
        ("cold-pre-write-default", 6_000),
        ("warm-pre-write-default", 4_000),
        ("post-write-one-file-default", 4_000),
        ("post-write-32-files-default", 8_000),
    ];
    let mut medians = serde_json::Map::new();
    let mut target_misses = Vec::new();
    for (mode, maximum_ms) in targets {
        let median_ns = median(
            times
                .get(mode)
                .ok_or_else(|| format!("missing mode {mode}"))?,
        )?;
        medians.insert(
            mode.to_owned(),
            serde_json::json!({
                "medianNanoseconds": median_ns,
                "targetMaximumMilliseconds": maximum_ms,
                "met": median_ns <= maximum_ms * 1_000_000,
            }),
        );
        if median_ns > maximum_ms * 1_000_000 {
            target_misses.push(format!(
                "{mode} median {:.3} ms exceeds {maximum_ms} ms",
                median_ns as f64 / 1_000_000.0
            ));
        }
    }
    let jobs1 = median(
        times
            .get("cold-audit-jobs-1")
            .ok_or_else(|| "missing cold-audit-jobs-1 mode".to_owned())?,
    )?;
    let default = median(
        times
            .get("cold-audit-default")
            .ok_or_else(|| "missing cold-audit-default mode".to_owned())?,
    )?;
    let scaling_ratio = default as f64 / jobs1 as f64;
    let scaling_applicable = default_jobs >= 4;
    let scaling_met = !scaling_applicable || default * 100 <= jobs1 * 75;
    if !scaling_met {
        target_misses.push(format!(
            "default/jobs=1 cold audit ratio {scaling_ratio:.4} exceeds 0.75"
        ));
    }
    if peak_rss_bytes > MAX_PEAK_RSS_BYTES {
        target_misses.push(format!(
            "peak RSS {peak_rss_bytes} exceeds {MAX_PEAK_RSS_BYTES} bytes"
        ));
    }
    if binary_bytes > MAX_BINARY_BYTES {
        target_misses.push(format!(
            "packaged binary {binary_bytes} exceeds {MAX_BINARY_BYTES} bytes"
        ));
    }
    Ok(serde_json::json!({
        "blockingTargetsApplied": blocking,
        "medians": medians,
        "peakRssBytes": peak_rss_bytes,
        "peakRssMaximumBytes": MAX_PEAK_RSS_BYTES,
        "peakRssMet": peak_rss_bytes <= MAX_PEAK_RSS_BYTES,
        "binaryBytes": binary_bytes,
        "binaryMaximumBytes": MAX_BINARY_BYTES,
        "binarySizeMet": binary_bytes <= MAX_BINARY_BYTES,
        "scaling": {
            "applicable": scaling_applicable,
            "defaultJobs": default_jobs,
            "jobs1MedianNanoseconds": jobs1,
            "defaultMedianNanoseconds": default,
            "ratio": scaling_ratio,
            "maximumRatio": 0.75,
            "met": scaling_met,
        },
        "targetMisses": target_misses,
    }))
}

fn median(values: &[u64]) -> Result<u64, String> {
    if values.len() != REPETITIONS {
        return Err(format!(
            "benchmark mode has {} repetitions; expected {REPETITIONS}",
            values.len()
        ));
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    Ok(sorted[REPETITIONS / 2])
}

fn create_scratch(workspace: &Path) -> Result<PathBuf, String> {
    let scratch = std::env::var_os("LUMIN_BENCHMARK_SCRATCH_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos());
            std::env::temp_dir().join(format!(
                "lumin-foundation-benchmark-{}-{nonce}",
                std::process::id()
            ))
        });
    require_external_absolute_path(workspace, &scratch, "benchmark scratch root")?;
    if scratch.exists() {
        return Err(format!(
            "benchmark scratch root must not already exist: {}",
            scratch.display()
        ));
    }
    let parent = scratch.parent().ok_or_else(|| {
        format!(
            "benchmark scratch root has no parent: {}",
            scratch.display()
        )
    })?;
    if !parent.is_dir() {
        return Err(format!(
            "benchmark scratch parent does not exist: {}",
            parent.display()
        ));
    }
    fs::create_dir(&scratch).map_err(|error| {
        format!(
            "cannot create benchmark scratch root {}: {error}",
            scratch.display()
        )
    })?;
    fs::create_dir(scratch.join("repositories"))
        .map_err(|error| format!("cannot create benchmark repository root: {error}"))?;
    fs::create_dir(scratch.join("captures"))
        .map_err(|error| format!("cannot create benchmark capture root: {error}"))?;
    scratch
        .canonicalize()
        .map_err(|error| format!("cannot resolve benchmark scratch root: {error}"))
}

fn report_path(workspace: &Path, scratch: &Path) -> Result<PathBuf, String> {
    let path = std::env::var_os("LUMIN_BENCHMARK_REPORT")
        .map(PathBuf::from)
        .ok_or_else(|| {
            "LUMIN_BENCHMARK_REPORT is required to retain benchmark evidence".to_owned()
        })?;
    require_external_absolute_path(workspace, &path, "benchmark report")?;
    if path.starts_with(scratch) {
        return Err("benchmark report must be outside the disposable scratch root".to_owned());
    }
    if path.exists() {
        return Err(format!(
            "benchmark report already exists: {}",
            path.display()
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| format!("benchmark report has no parent: {}", path.display()))?;
    if !parent.is_dir() {
        return Err(format!(
            "benchmark report parent does not exist: {}",
            parent.display()
        ));
    }
    Ok(path)
}

fn require_external_absolute_path(
    workspace: &Path,
    path: &Path,
    label: &str,
) -> Result<(), String> {
    if !path.is_absolute() {
        return Err(format!("{label} must be absolute"));
    }
    let workspace = workspace
        .canonicalize()
        .map_err(|error| format!("cannot resolve workspace root: {error}"))?;
    let parent = path
        .parent()
        .ok_or_else(|| format!("{label} has no parent"))?
        .canonicalize()
        .map_err(|error| format!("cannot resolve {label} parent: {error}"))?;
    let comparison = parent.join(
        path.file_name()
            .ok_or_else(|| format!("{label} has no final component"))?,
    );
    if comparison.starts_with(workspace) {
        return Err(format!("{label} must be outside the source checkout"));
    }
    Ok(())
}

fn write_report(path: &Path, report: &Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(report)
        .map_err(|error| format!("cannot encode benchmark report: {error}"))?;
    bytes.push(b'\n');
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options
        .open(path)
        .map_err(|error| format!("cannot create benchmark report {}: {error}", path.display()))?;
    file.write_all(&bytes)
        .map_err(|error| format!("cannot write benchmark report: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("cannot flush benchmark report: {error}"))
}

fn verify_execution_policy(workspace: &Path) -> Result<(), String> {
    let engine = fs::read_to_string(workspace.join("crates/application/engine/src/lib.rs"))
        .map_err(|error| format!("cannot read worker policy source: {error}"))?;
    if !engine.contains("const WORKER_STACK_BYTES: usize = 4_194_304;")
        || !engine.contains(".stack_size(WORKER_STACK_BYTES)")
    {
        return Err("packaged worker stack policy differs from 4,194,304 bytes".to_owned());
    }
    let cli = fs::read_to_string(workspace.join("crates/application/cli/src/lib.rs"))
        .map_err(|error| format!("cannot read default-jobs policy source: {error}"))?;
    if !cli.contains("available.map_or(1, |value| value.get().min(8))") {
        return Err("packaged default-jobs policy differs from max(1,min(8,available))".to_owned());
    }
    Ok(())
}

fn elapsed_ns(started: Instant) -> Result<u64, String> {
    u64::try_from(started.elapsed().as_nanos())
        .map_err(|_| "benchmark stage duration overflow".to_owned())
}

fn host_target() -> &'static str {
    if cfg!(windows) {
        "windows-x64"
    } else {
        "linux-x64"
    }
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benchmark_median_requires_the_frozen_three_samples() {
        assert_eq!(median(&[30, 10, 20]), Ok(20));
        assert!(median(&[10, 20]).is_err());
    }

    #[test]
    fn report_only_misses_remain_visible_without_becoming_blocking() -> Result<(), String> {
        let times = BTreeMap::from([
            ("cold-audit-default", vec![31_000_000_000; 3]),
            ("cold-audit-jobs-1", vec![40_000_000_000; 3]),
            ("warm-audit-default", vec![1; 3]),
            ("cold-pre-write-default", vec![1; 3]),
            ("warm-pre-write-default", vec![1; 3]),
            ("post-write-one-file-default", vec![1; 3]),
            ("post-write-32-files-default", vec![1; 3]),
        ]);
        let summary = summarize(&times, 1, 1, 8, false)?;
        assert_eq!(
            summary.pointer("/blockingTargetsApplied"),
            Some(&Value::Bool(false))
        );
        assert!(
            summary
                .pointer("/targetMisses")
                .and_then(Value::as_array)
                .is_some_and(|misses| !misses.is_empty())
        );
        Ok(())
    }
}
