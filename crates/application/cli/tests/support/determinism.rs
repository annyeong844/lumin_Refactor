use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use lumin_model::GateId;
use serde_json::Value;

const CAPTURE_ENV: &str = "LUMIN_CORPUS_DETERMINISM_CAPTURE";
const JOBS_POLICY_ENV: &str = "LUMIN_CORPUS_JOBS_POLICY";

static CAPTURE_WRITE: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Clone, Copy)]
enum JobsPolicy {
    Default,
    One,
}

pub(super) fn effective_arguments(
    arguments: &[&str],
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let capture = std::env::var(CAPTURE_ENV);
    let policy = std::env::var(JOBS_POLICY_ENV);
    let policy = match (capture.as_ref(), policy.as_deref()) {
        (Err(_), Err(_)) => return Ok(arguments.iter().map(|value| (*value).to_owned()).collect()),
        (Ok(_), Ok("default")) => JobsPolicy::Default,
        (Ok(_), Ok("one")) => JobsPolicy::One,
        (Ok(_), Ok(value)) => {
            return Err(std::io::Error::other(format!(
                "{JOBS_POLICY_ENV} has unsupported value {value:?}"
            ))
            .into());
        }
        _ => {
            return Err(std::io::Error::other(format!(
                "{CAPTURE_ENV} and {JOBS_POLICY_ENV} must both be set or both unset"
            ))
            .into());
        }
    };

    rewrite_arguments(arguments, policy)
}

fn rewrite_arguments(
    arguments: &[&str],
    policy: JobsPolicy,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    if !matches!(arguments.first().copied(), Some("audit" | "pre-write")) {
        return Ok(arguments.iter().map(|value| (*value).to_owned()).collect());
    }

    let mut effective = Vec::with_capacity(arguments.len() + 2);
    let mut index = 0;
    while index < arguments.len() {
        if arguments[index] == "--jobs" {
            if arguments.get(index + 1).is_none() {
                return Err(std::io::Error::other("--jobs is missing its value").into());
            }
            index += 2;
        } else {
            effective.push(arguments[index].to_owned());
            index += 1;
        }
    }
    if matches!(policy, JobsPolicy::One) {
        effective.push("--jobs".to_owned());
        effective.push("1".to_owned());
    }
    Ok(effective)
}

pub(super) fn record_semantic_evidence(
    root: &Path,
    arguments: &[String],
    command_succeeded: bool,
    stdout: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let capture = std::env::var(CAPTURE_ENV);
    let policy = std::env::var(JOBS_POLICY_ENV);
    let capture = match (capture, policy) {
        (Err(_), Err(_)) => return Ok(()),
        (Ok(capture), Ok(policy)) if policy == "default" || policy == "one" => capture,
        (Ok(_), Ok(policy)) => {
            return Err(std::io::Error::other(format!(
                "{JOBS_POLICY_ENV} has unsupported value {policy:?}"
            ))
            .into());
        }
        _ => {
            return Err(std::io::Error::other(format!(
                "{CAPTURE_ENV} and {JOBS_POLICY_ENV} must both be set or both unset"
            ))
            .into());
        }
    };

    let evidence = match arguments.first().map(String::as_str) {
        Some("audit") if command_succeeded => lumin_engine::load_latest_run(root)?
            .map(|(_, evidence)| serde_json::to_value(evidence.semantic_projection()))
            .transpose()?
            .into_iter()
            .collect(),
        Some("audit") => Vec::new(),
        Some("pre-write" | "post-write") => gate_evidence(root, stdout)?,
        Some("overview") => attempt_failure_evidence(root, stdout)?,
        _ => Vec::new(),
    };
    if evidence.is_empty() {
        return Ok(());
    }

    let encoded = serde_json::to_vec(&evidence)?;
    let _guard = CAPTURE_WRITE
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| std::io::Error::other("determinism capture mutex is poisoned"))?;
    let mut file = OpenOptions::new().create(true).append(true).open(capture)?;
    file.write_all(&encoded)?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
}

fn attempt_failure_evidence(
    root: &Path,
    stdout: &str,
) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let response: Value = match serde_json::from_str(stdout) {
        Ok(response) => response,
        Err(_) => return Ok(Vec::new()),
    };
    if response.get("schemaVersion").and_then(Value::as_str) != Some("lumin.attempt-overview.v1") {
        return Ok(Vec::new());
    }
    let Some(latest) = response.get("latestAttempt") else {
        return Ok(Vec::new());
    };
    let failure = latest
        .get("failure")
        .and_then(Value::as_str)
        .map(|detail| detail.replace(root.to_string_lossy().as_ref(), "<repository-root>"));
    Ok(vec![serde_json::json!({
        "schemaVersion": "lumin.attempt-semantic.v1",
        "status": latest.get("status").cloned().unwrap_or(Value::Null),
        "failure": failure,
    })])
}

fn gate_evidence(root: &Path, stdout: &str) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let response: Value = match serde_json::from_str(stdout) {
        Ok(response) => response,
        Err(_) => return Ok(Vec::new()),
    };
    let Some(gate_id) = response.get("gateId").and_then(Value::as_str) else {
        return Ok(Vec::new());
    };
    let gate = lumin_engine::load_gate(root, &GateId::from_string(gate_id.to_owned()))?;
    let mut evidence = Vec::new();
    if let Some(baseline) = gate.baseline {
        evidence.push(serde_json::to_value(
            baseline.snapshot.evidence.semantic_projection(),
        )?);
    }
    for snapshot in gate
        .revisions
        .into_iter()
        .filter_map(|revision| revision.snapshot)
    {
        evidence.push(serde_json::to_value(
            snapshot.evidence.semantic_projection(),
        )?);
    }
    Ok(evidence)
}
