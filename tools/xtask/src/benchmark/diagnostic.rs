//! Versioned W2/W3 cold-only comparison; never a performance-budget verdict.
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use lumin_protocol::audit_diagnostic::{AuditDiagnosticDto, decode};
use serde_json::Value;

use super::{archive, fixture, measurement, truth};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Version {
    Execution,
    Store,
}

impl Version {
    fn feature(self) -> &'static str {
        match self {
            Self::Execution => "audit-execution-test-profile",
            Self::Store => "audit-store-test-profile",
        }
    }
    fn report_schema(self) -> &'static str {
        match self {
            Self::Execution => "lumin.phase1-cold-audit-diagnostic.v1",
            Self::Store => "lumin.phase1-cold-audit-diagnostic.v2",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Cell {
    name: String,
    diagnostic: bool,
    jobs: Option<usize>,
    round: usize,
}

fn cells() -> Vec<Cell> {
    let mut cells = Vec::new();
    for diagnostic in [false, true] {
        cells.push(Cell {
            name: format!(
                "conditioning-{}",
                if diagnostic { "diagnostic" } else { "control" }
            ),
            diagnostic,
            jobs: Some(1),
            round: 0,
        });
    }
    for round in 1..=3 {
        let mut order = [
            (false, Some(1)),
            (false, None),
            (true, Some(1)),
            (true, None),
        ];
        if round == 2 {
            order.reverse();
        }
        for (diagnostic, jobs) in order {
            cells.push(Cell {
                name: format!(
                    "round-{round}-{}-{}",
                    if diagnostic { "diagnostic" } else { "control" },
                    if jobs.is_some() { "1" } else { "default" }
                ),
                diagnostic,
                jobs,
                round,
            });
        }
    }
    cells
}

pub(super) fn run(version: Version) -> ExitCode {
    match diagnose_cold_audit(version) {
        Ok(report) => {
            match serde_json::to_string_pretty(&report) {
                Ok(report) => println!("{report}"),
                Err(error) => {
                    eprintln!("[FAIL] {error}");
                    return ExitCode::FAILURE;
                }
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("[FAIL] cold-audit diagnostic: {error}");
            ExitCode::FAILURE
        }
    }
}

fn diagnose_cold_audit(version: Version) -> Result<Value, String> {
    let workspace = crate::metadata::find_workspace_root().map_err(|error| error.to_string())?;
    let python = measurement::require_python()?;
    let script = workspace.join("tools/xtask/benchmark/measure-process.py");
    let scratch = super::create_scratch(&workspace)?;
    let report_path = super::report_path(&workspace, &scratch)?;
    let package = crate::package_check::load_host_package()?;
    let diagnostic = required_file("LUMIN_AUDIT_DIAGNOSTIC_BINARY")?;
    let schedule = cells();
    let expected = schedule
        .iter()
        .map(|cell| cell.name.clone())
        .collect::<Vec<_>>();
    let mut archive = archive::CaptureArchive::from_environment(
        &workspace,
        &package.root,
        &scratch,
        &expected,
        true,
    )?
    .ok_or("diagnostic archive unavailable")?;
    let control_before = hash_file(&package.binary)?;
    let diagnostic_before = hash_file(&diagnostic)?;
    let result = (|| {
        if diagnostic.starts_with(
            package
                .root
                .canonicalize()
                .map_err(|error| error.to_string())?,
        ) || diagnostic.starts_with(&scratch)
            || diagnostic.starts_with(&archive.root)
        {
            return Err(
                "diagnostic executable overlaps package, scratch, or capture archive".to_owned(),
            );
        }
        if control_before == diagnostic_before {
            return Err("control and diagnostic are the same payload".to_owned());
        }
        let provenance =
            validate_build_record(&workspace, &control_before, &diagnostic_before, version)?;
        archive::write_json(&archive.root.join("builds.json"), &provenance)?;
        archive::write_bytes(
            &archive.root.join("control-package.json"),
            &fs::read(package.root.join("lumin-package.json"))
                .map_err(|error| error.to_string())?,
        )?;
        let host = measurement::inspect_host(&python, &script, &scratch)?;
        archive::write_json(&archive.root.join("host.json"), &host.details)?;
        let fixture = fixture::prepare(&workspace, &scratch, &python)?;
        archive::write_json(&archive.root.join("fixture.json"), &fixture.identity)?;
        archive::write_json(&archive.root.join("truth.json"), &fixture.truth)?;
        let control_build = build_identity(
            &package.binary,
            &fixture.root,
            &archive.root.join("control-capabilities"),
        )?;
        let diagnostic_build = build_identity(
            &diagnostic,
            &fixture.root,
            &archive.root.join("diagnostic-capabilities"),
        )?;
        if control_build != package.build_id || control_build != diagnostic_build {
            return Err(
                "compiled build scopes differ from staged control/source identity".to_owned(),
            );
        }
        let mut samples = Vec::new();
        let mut reference = None;
        for cell in &schedule {
            let capture = archive.begin(&cell.name)?;
            let repository = scratch.join("repositories").join(&cell.name);
            fixture::copy_repository(&fixture.root, &repository)?;
            let binary = if cell.diagnostic {
                &diagnostic
            } else {
                &package.binary
            };
            let mut args =
                measurement::arguments(&["audit", "--include", "**", "--format", "json"]);
            if let Some(jobs) = cell.jobs {
                args.extend(["--jobs".into(), jobs.to_string().into()]);
            }
            let measured = if cell.diagnostic {
                measurement::measure_diagnostic(
                    &python,
                    &script,
                    binary,
                    &repository,
                    &args,
                    &capture,
                )?
            } else {
                measurement::measure_product(
                    &python,
                    &script,
                    binary,
                    &repository,
                    &args,
                    &capture,
                )?
            };
            archive::write_json(
                &capture.join("binding.json"),
                &serde_json::json!({
                    "binary":binary, "arguments":args, "root":repository, "cell":cell.name,
                    "rootIdentity":measured.response.get("repositoryRoot"),
                }),
            )?;
            let (run_id, _) = truth::audit_scope(&measured.response)?;
            let frame_bytes =
                fs::read(capture.join("stderr")).map_err(|error| error.to_string())?;
            let frame = if cell.diagnostic {
                Some(validate_versioned_frame(
                    version,
                    &frame_bytes,
                    &measured.raw,
                    &measured.response,
                    &diagnostic_build,
                    cell.jobs,
                )?)
            } else {
                None
            };
            let dump = truth::validate_semantic_dump(
                binary,
                &repository,
                &fixture.truth,
                truth::Scope::Run { run_id: &run_id },
                &capture,
            )?;
            let overview_bytes =
                fs::read(capture.join("overview/stdout")).map_err(|error| error.to_string())?;
            let overview: Value =
                serde_json::from_slice(&overview_bytes).map_err(|error| error.to_string())?;
            if overview.get("attemptId") != measured.response.get("attemptId")
                || overview.pointer("/scope/id") != measured.response.get("runId")
            {
                return Err("run-pinned overview contradicts measured audit scope".to_owned());
            }
            if reference
                .as_ref()
                .is_some_and(|reference| reference != &dump)
            {
                return Err(
                    "control/diagnostic/worker/round changed the authored semantic ID map"
                        .to_owned(),
                );
            }
            let semantic = dump.report_value()?;
            reference = Some(dump);
            archive::write_json(&capture.join("semantic-truth.json"), &semantic)?;
            let command_elapsed = frame
                .as_ref()
                .and_then(|frame| frame.pointer("/phases/0/elapsedNanoseconds"))
                .and_then(Value::as_u64);
            let residual = command_elapsed
                .map(|elapsed| {
                    measured
                        .elapsed_nanoseconds
                        .checked_sub(elapsed)
                        .ok_or_else(|| {
                            "command interval exceeds observer process interval".to_owned()
                        })
                })
                .transpose()?;
            let sample = serde_json::json!({
                "cell":cell.name, "round":cell.round, "measured":cell.round != 0,
                "binaryKind":if cell.diagnostic { "diagnostic" } else { "control" },
                "requestedJobs":cell.jobs, "process":measured.raw,
                "engineObservations":frame, "rawFrameSha256":if cell.diagnostic { Some(super::sha256_hex(&frame_bytes)) } else { None },
                "externalResidualNanoseconds":residual,
                "scope":measured.response, "semanticTruth":semantic,
            });
            archive::write_json(&capture.join("sample.json"), &sample)?;
            samples.push(sample);
            archive.complete()?;
        }
        let mut summary = summarize(&samples)?;
        if version == Version::Store {
            summary["roundDifferencesNanoseconds"] = round_differences(&samples)?;
        }
        Ok(serde_json::json!({
            "schemaVersion":version.report_schema(), "status":"DIAGNOSTIC_ONLY",
            "numericBudgetVerdict":null, "summary":summary,
            "builds":provenance, "host":host.details, "environment":host.classification,
            "toolchain":measurement::python_identity(&python, &script, &host.details)?,
            "packageRoot":package.root, "fixture":fixture.identity, "samples":samples,
            "archive":archive.root,
            "uncertainty":"cold means fresh repository/state/process, not flushed machine caches; opaque store residuals are not backend flush or lock-wait measurements",
        }))
    })();
    let payload_check = (|| {
        let control_after = hash_file(&package.binary)?;
        let diagnostic_after = hash_file(&diagnostic)?;
        archive::write_json(
            &archive.root.join("payloads-after.json"),
            &serde_json::json!({
                "controlSha256":control_after, "diagnosticSha256":diagnostic_after,
            }),
        )?;
        if control_before != control_after || diagnostic_before != diagnostic_after {
            return Err("a measured executable changed during the packet".to_owned());
        }
        Ok(())
    })();
    let result = match (result, payload_check) {
        (Ok(report), Ok(())) => Ok(report),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(payload)) => Err(format!("{error}; payload verification: {payload}")),
    };
    archive.finish(result.as_ref().err().map(String::as_str))?;
    fs::remove_dir_all(&scratch)
        .map_err(|error| format!("cannot remove owned diagnostic scratch: {error}"))?;
    let report = result?;
    super::write_report(&report_path, &report)?;
    Ok(report)
}

fn required_file(name: &str) -> Result<PathBuf, String> {
    let path = std::env::var_os(name).ok_or_else(|| format!("{name} is required"))?;
    let path = Path::new(&path)
        .canonicalize()
        .map_err(|error| format!("cannot resolve {name}: {error}"))?;
    if !path.is_file() {
        return Err(format!("{name} is not a regular file"));
    }
    Ok(path)
}

fn hash_file(path: &Path) -> Result<String, String> {
    Ok(super::sha256_hex(&fs::read(path).map_err(|error| {
        format!("cannot hash {}: {error}", path.display())
    })?))
}

fn validate_build_record(
    workspace: &Path,
    control: &str,
    diagnostic: &str,
    version: Version,
) -> Result<Value, String> {
    let status = Command::new("git")
        .args(["status", "--porcelain=v1", "-z"])
        .current_dir(workspace)
        .output()
        .map_err(|error| error.to_string())?;
    if !status.status.success() || !status.stderr.is_empty() || !status.stdout.is_empty() {
        return Err("diagnostic build binding requires an exact clean source checkout".to_owned());
    }
    let path = required_file("LUMIN_AUDIT_DIAGNOSTIC_BUILD_RECORD")?;
    let record: Value = serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
        .map_err(|error| format!("invalid diagnostic build record: {error}"))?;
    let revision = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(workspace)
        .output()
        .map_err(|error| error.to_string())?;
    if !revision.status.success() || !revision.stderr.is_empty() {
        return Err("cannot bind diagnostic checkout revision".to_owned());
    }
    let revision = std::str::from_utf8(&revision.stdout)
        .map_err(|error| error.to_string())?
        .trim();
    if record["sourceRevision"] != revision
        || record["lockfileSha256"] != hash_file(&workspace.join("Cargo.lock"))?
        || record["controlSha256"] != control
        || record["diagnosticSha256"] != diagnostic
        || record["controlFeatures"] != serde_json::json!([])
        || record["diagnosticFeatures"] != serde_json::json!([version.feature()])
    {
        return Err(
            "build record contradicts source, lockfile, payloads, or isolated features".to_owned(),
        );
    }
    for key in ["target", "toolchain", "controlCommand", "diagnosticCommand"] {
        if record[key].as_str().is_none_or(str::is_empty) {
            return Err(format!("build record omitted {key}"));
        }
    }
    let target = if cfg!(windows) {
        "x86_64-pc-windows-msvc"
    } else {
        "x86_64-unknown-linux-musl"
    };
    if record["target"] != target || record["toolchain"] != "1.96.0" {
        return Err(
            "diagnostic build record does not use the pinned host target/toolchain".to_owned(),
        );
    }
    if version == Version::Store {
        let policy: Value = serde_json::from_slice(
            &fs::read(workspace.join("tools/xtask/dependency-surface-policy.v2.json"))
                .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        let closure = store_feature_closure(&policy)?;
        if closure != expected_store_feature_closure()
            || record["diagnosticFeatureClosure"] != closure
        {
            return Err("unreviewed or unbound diagnostic feature closure".to_owned());
        }
        let prefix = if cfg!(windows) {
            "cargo build -p lumin-cli --release".to_owned()
        } else {
            format!("cargo build -p lumin-cli --release --target {target}")
        };
        let expected_diagnostic = format!("{prefix} --features {} --locked", version.feature());
        let expected_control = format!("{prefix} --locked");
        for (key, expected) in [
            ("diagnosticCommand", expected_diagnostic.as_str()),
            ("controlCommand", expected_control.as_str()),
        ] {
            let command = record[key].as_str().ok_or("missing build command")?;
            if command != expected && command != format!("{expected} -j1") {
                return Err("diagnostic build command enables an unreviewed argument".to_owned());
            }
        }
    }
    Ok(record)
}

fn expected_store_feature_closure() -> Value {
    serde_json::json!({
        "lumin-cli":["audit-execution-test-profile","audit-store-test-profile"],
        "lumin-engine":["audit-execution-test-profile","audit-store-test-profile"],
        "lumin-model":["audit-execution-test-profile","audit-store-test-profile"],
        "lumin-protocol":["audit-execution-test-profile","audit-store-test-profile"],
        "lumin-store":["audit-store-test-profile"],
    })
}

// The source-provenance guard binds this feature graph to Cargo's resolved
// workspace declarations before building. Resolve its exact requested closure;
// an extra edge is a failure, not permission granted by a build-record string.
fn store_feature_closure(policy: &Value) -> Result<Value, String> {
    let members = policy["members"]
        .as_array()
        .ok_or("missing policy members")?;
    let mut pending = vec![(
        "lumin-cli".to_owned(),
        "audit-store-test-profile".to_owned(),
    )];
    let mut closure = BTreeMap::<String, std::collections::BTreeSet<String>>::new();
    while let Some((package, feature)) = pending.pop() {
        if !closure
            .entry(package.clone())
            .or_default()
            .insert(feature.clone())
        {
            continue;
        }
        let member = members
            .iter()
            .find(|member| member["name"] == package)
            .ok_or("unknown diagnostic feature owner")?;
        let implications = member["features"][&feature]
            .as_array()
            .ok_or("unknown diagnostic feature")?;
        for edge in implications {
            let edge = edge.as_str().ok_or("invalid diagnostic feature edge")?;
            if let Some((alias, feature)) = edge.split_once('/') {
                let dependencies = member["dependencies"]
                    .as_array()
                    .ok_or("missing dependencies")?;
                let dependency = dependencies
                    .iter()
                    .find(|dep| dep["alias"] == alias && dep["kind"] == "normal")
                    .ok_or("feature does not belong to a normal dependency")?;
                let owner = dependency["package"]
                    .as_str()
                    .ok_or("missing feature dependency owner")?;
                pending.push((owner.to_owned(), feature.to_owned()));
            } else {
                pending.push((package.clone(), edge.to_owned()));
            }
        }
    }
    serde_json::to_value(closure).map_err(|error| error.to_string())
}

fn build_identity(binary: &Path, root: &Path, capture: &Path) -> Result<String, String> {
    let response = measurement::run_query(
        binary,
        root,
        &measurement::arguments(&["capabilities", "--format", "json"]),
        capture,
    )?;
    response
        .pointer("/scope/buildId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "same-binary capabilities omitted build identity".to_owned())
}

fn validate_versioned_frame(
    version: Version,
    bytes: &[u8],
    observer: &Value,
    stdout: &Value,
    build: &str,
    jobs: Option<usize>,
) -> Result<Value, String> {
    match version {
        Version::Execution => {
            serde_json::to_value(validate_frame(bytes, observer, stdout, build, jobs)?)
        }
        Version::Store => {
            serde_json::to_value(validate_store_frame(bytes, observer, stdout, build, jobs)?)
        }
    }
    .map_err(|error| error.to_string())
}

fn validate_frame(
    bytes: &[u8],
    observer: &Value,
    stdout: &Value,
    build: &str,
    jobs: Option<usize>,
) -> Result<AuditDiagnosticDto, String> {
    let frame = decode(bytes)?;
    validate_frame_binding(&frame, observer, stdout, build, jobs)?;
    Ok(frame)
}

fn validate_store_frame(
    bytes: &[u8],
    observer: &Value,
    stdout: &Value,
    build: &str,
    jobs: Option<usize>,
) -> Result<lumin_protocol::audit_store_diagnostic::AuditStoreDiagnosticDto, String> {
    let frame = lumin_protocol::audit_store_diagnostic::decode(bytes)?;
    validate_frame_binding(&frame.execution(), observer, stdout, build, jobs)?;
    if frame.store_phases.iter().any(|phase| phase.calls != 1) {
        return Err("fresh cold repository omitted store/bootstrap work".to_owned());
    }
    Ok(frame)
}

fn validate_frame_binding(
    frame: &AuditDiagnosticDto,
    observer: &Value,
    stdout: &Value,
    build: &str,
    jobs: Option<usize>,
) -> Result<(), String> {
    if observer["schemaVersion"] != "lumin.phase1-process-measurement.v2"
        || observer["exitCode"] != 0
        || observer["analysisChildPids"] != serde_json::json!([])
        || observer["processId"].as_u64() != Some(u64::from(frame.process_id))
        || frame.process_id == 0
        || frame.build_id != build
        || frame.requested_jobs != jobs
        || stdout["schemaVersion"] != "lumin.audit.v2"
        || stdout["attemptId"].as_str() != Some(&frame.attempt_id)
        || stdout["runId"].as_str() != Some(&frame.run_id)
    {
        return Err(
            "diagnostic frame disagrees with OS observer, public scope, or same-binary build"
                .to_owned(),
        );
    }
    Ok(())
}

fn summarize(samples: &[Value]) -> Result<Value, String> {
    if samples.len() != 14 {
        return Err("diagnostic packet omitted a conditioning/measured cell".to_owned());
    }
    let mut timings = BTreeMap::<String, Vec<u64>>::new();
    let mut scaling_authority = true;
    for sample in samples.iter().filter(|sample| sample["measured"] == true) {
        let kind = sample["binaryKind"].as_str().ok_or("missing binary kind")?;
        let policy = if sample["requestedJobs"].is_null() {
            "default"
        } else {
            "1"
        };
        let elapsed = sample
            .pointer("/process/elapsedNanoseconds")
            .and_then(Value::as_u64)
            .filter(|value| *value > 0)
            .ok_or("missing process timing")?;
        timings
            .entry(format!("{kind}-{policy}"))
            .or_default()
            .push(elapsed);
        if kind == "diagnostic" && policy == "default" {
            scaling_authority &= sample
                .pointer("/engineObservations/actualJobs")
                .and_then(Value::as_u64)
                .is_some_and(|jobs| jobs >= 4);
        }
    }
    let mut medians = BTreeMap::new();
    for key in [
        "control-1",
        "control-default",
        "diagnostic-1",
        "diagnostic-default",
    ] {
        let median = super::median(
            timings
                .get(key)
                .ok_or("missing worker/binary cell family")?,
        )?;
        medians.insert(key, median);
    }
    Ok(serde_json::json!({
        "scalingAuthority":scaling_authority, "mediansNanoseconds":medians,
        "controlDefaultOverOne":medians["control-default"] as f64 / medians["control-1"] as f64,
        "diagnosticDefaultOverOne":medians["diagnostic-default"] as f64 / medians["diagnostic-1"] as f64,
        "featureOverheadNanoseconds":{
            "jobs1":i128::from(medians["diagnostic-1"])-i128::from(medians["control-1"]),
            "default":i128::from(medians["diagnostic-default"])-i128::from(medians["control-default"]),
        },
    }))
}

fn round_differences(samples: &[Value]) -> Result<Value, String> {
    let mut rounds = Vec::new();
    for round in 1..=3 {
        for jobs in [Some(1), None] {
            let mut elapsed = [0; 2];
            for (index, kind) in ["control", "diagnostic"].into_iter().enumerate() {
                let mut matches = samples.iter().filter(|sample| {
                    sample["round"] == round
                        && sample["requestedJobs"] == serde_json::json!(jobs)
                        && sample["binaryKind"] == kind
                        && sample["measured"] == true
                });
                let sample = matches.next().ok_or("missing round comparison cell")?;
                if matches.next().is_some() {
                    return Err("duplicated round comparison cell".to_owned());
                }
                elapsed[index] = sample
                    .pointer("/process/elapsedNanoseconds")
                    .and_then(Value::as_u64)
                    .filter(|elapsed| *elapsed > 0)
                    .ok_or("invalid round comparison timing")?;
            }
            rounds.push(serde_json::json!({
                "round":round, "requestedJobs":jobs,
                "controlNanoseconds":elapsed[0], "diagnosticNanoseconds":elapsed[1],
                "diagnosticMinusControl":i128::from(elapsed[1]) - i128::from(elapsed[0]),
            }));
        }
    }
    Ok(Value::Array(rounds))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod store_tests;
