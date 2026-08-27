use std::ffi::{OsStr, OsString};
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

pub(crate) fn effective_arguments(
    arguments: &[OsString],
) -> Result<Vec<OsString>, Box<dyn std::error::Error>> {
    let capture = std::env::var(CAPTURE_ENV);
    let policy = std::env::var(JOBS_POLICY_ENV);
    let policy = match (capture.as_ref(), policy.as_deref()) {
        (Err(_), Err(_)) => return Ok(arguments.to_vec()),
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
    arguments: &[OsString],
    policy: JobsPolicy,
) -> Result<Vec<OsString>, Box<dyn std::error::Error>> {
    if !arguments
        .first()
        .is_some_and(|value| value == OsStr::new("audit") || value == OsStr::new("pre-write"))
    {
        return Ok(arguments.to_vec());
    }

    let mut effective = Vec::with_capacity(arguments.len() + 2);
    let mut index = 0;
    while index < arguments.len() {
        if arguments[index] == OsStr::new("--jobs") {
            if arguments.get(index + 1).is_none() {
                return Err(std::io::Error::other("--jobs is missing its value").into());
            }
            index += 2;
        } else {
            effective.push(arguments[index].clone());
            index += 1;
        }
    }
    if matches!(policy, JobsPolicy::One) {
        effective.push("--jobs".into());
        effective.push("1".into());
    }
    Ok(effective)
}

pub(super) fn record_semantic_evidence(
    root: &Path,
    arguments: &[OsString],
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

    let command = arguments.first().and_then(|value| value.to_str());
    let subcommand = arguments.get(1).and_then(|value| value.to_str());
    let evidence = match (command, subcommand) {
        (Some("audit"), _) if command_succeeded => lumin_engine::load_latest_run(root)?
            .map(|(_, evidence)| serde_json::to_value(evidence.semantic_projection()))
            .transpose()?
            .into_iter()
            .collect(),
        (Some("audit"), _) => Vec::new(),
        (Some("pre-write" | "post-write"), _) => gate_evidence(root, stdout)?,
        (Some("overview"), _) => attempt_failure_evidence(root, stdout)?,
        (Some("operation"), Some("show")) if command_succeeded => {
            cache_cleanup_operation_evidence(stdout)?
        }
        (Some("store"), Some("migrate")) if command_succeeded => {
            lifecycle_migration_evidence(stdout)?
        }
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

fn cache_cleanup_operation_evidence(
    stdout: &str,
) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let mut response: Value = match serde_json::from_str(stdout) {
        Ok(response) => response,
        Err(_) => return Ok(Vec::new()),
    };
    if response.get("schemaVersion").and_then(Value::as_str)
        != Some("lumin.cache-cleanup-operation.v2")
    {
        return Ok(Vec::new());
    }
    let operation_id = response
        .get("operationId")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("cleanup operation omitted its operation ID"))?
        .to_owned();
    let request_digest = response
        .get("requestDigest")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("cleanup operation omitted its request digest"))?
        .to_owned();
    let result_operation_matches = response
        .pointer("/result/operationId")
        .and_then(Value::as_str)
        .is_none_or(|observed| observed == operation_id);
    let result_digest_matches = response
        .pointer("/result/requestDigest")
        .and_then(Value::as_str)
        .is_none_or(|observed| observed == request_digest);
    response["operationId"] = Value::String("<operation-id>".to_owned());
    response["requestDigest"] = serde_json::json!({
        "formatValid": request_digest.len() == 64
            && request_digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "matchesResult": result_digest_matches,
    });
    if let Some(result) = response.get_mut("result").and_then(Value::as_object_mut) {
        result.insert(
            "operationId".to_owned(),
            serde_json::json!({ "matchesOwner": result_operation_matches }),
        );
        result.insert(
            "requestDigest".to_owned(),
            serde_json::json!({ "matchesOwner": result_digest_matches }),
        );
    }
    Ok(vec![serde_json::json!({
        "schemaVersion": "lumin.cache-cleanup-operation-semantic.v1",
        "operation": response,
    })])
}

fn lifecycle_migration_evidence(stdout: &str) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let response: Value = serde_json::from_str(stdout).map_err(|error| {
        std::io::Error::other(format!(
            "successful lifecycle migration emitted malformed JSON: {error}"
        ))
    })?;
    if response.get("schemaVersion").and_then(Value::as_str)
        != Some("lumin.lifecycle-store-migration.v1")
    {
        return Err(std::io::Error::other(
            "successful lifecycle migration emitted an unsupported response schema",
        )
        .into());
    }
    Ok(vec![serde_json::json!({
        "schemaVersion": "lumin.lifecycle-store-migration-semantic.v1",
        "response": response,
    })])
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
    if let Some(baseline) = gate.baseline.as_ref() {
        let baseline_matches_owner = gate
            .revisions
            .first()
            .map(|revision| lumin_engine::gate_observation_binding_matches_owner(&gate, revision))
            .transpose()?
            .unwrap_or(false);
        if !baseline_matches_owner {
            return Err(std::io::Error::other(
                "sealed baseline observation ID disagrees with its owner inputs",
            )
            .into());
        }
        let mut baseline_alias_closures = baseline.alias_closures.iter().collect::<Vec<_>>();
        baseline_alias_closures.sort_by(|left, right| left.members.cmp(&right.members));
        let baseline_alias_closures = baseline_alias_closures
            .into_iter()
            .map(|closure| serde_json::json!({ "members": closure.members }))
            .collect::<Vec<_>>();
        evidence.push(serde_json::json!({
            "schemaVersion": "lumin.gate-baseline-semantic.v1",
            "gateSchemaVersion": gate.schema_version,
            "observationId": observation_id_projection(
                baseline.observation_id.as_str(),
                "baseline",
                baseline_matches_owner,
            ),
            "analysisContract": baseline.analysis_contract,
            "catalogRevision": baseline.catalog_revision,
            "transitionSequence": baseline.transition_sequence,
            "leasedWriteSet": baseline.leased_write_set.iter().map(|lease| serde_json::json!({
                "path": lease.path,
                "kind": lease.kind,
                "physicalIdentityPresent": lease.physical_identity.is_some(),
                "nearestExistingParent": lease.nearest_existing_parent,
                "prefixPaths": lease.prefix_identities.iter().map(|prefix| &prefix.path).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "aliasClosures": baseline_alias_closures,
            "protectedSemanticInputs": baseline.protected_semantic_inputs.iter().map(|input| serde_json::json!({
                "path": input.path,
                "state": input.state,
                "payloadSha256": input.payload_sha256,
                "physicalIdentityPresent": input.physical_identity.is_some(),
                "absenceParentPath": input.absence_parent.as_ref().map(|parent| &parent.path),
                // Redirect digests intentionally bind repository-instance physical identities.
                // Fresh fixtures compare that the binding exists; the sealed observation ID
                // still proves the persisted owner consumed the exact instance-specific digest.
                "physicalRedirectPresent": input.physical_redirect_sha256.is_some(),
            })).collect::<Vec<_>>(),
            "snapshot": {
                "inputs": baseline.snapshot.inputs.iter().map(|input| serde_json::json!({
                    "path": input.path,
                    "state": input.state,
                    "payloadSha256": input.payload_sha256,
                    "physicalIdentityPresent": input.physical_identity.is_some(),
                    "absenceParentPath": input.absence_parent.as_ref().map(|parent| &parent.path),
                    "physicalRedirectPresent": input.physical_redirect_sha256.is_some(),
                })).collect::<Vec<_>>(),
                "scanInvocation": baseline.snapshot.scan_invocation,
                "entrySelections": baseline.snapshot.entry_selections,
                "evidence": baseline.snapshot.evidence.semantic_projection(),
            },
        }));
    }
    for revision in &gate.revisions {
        let snapshot = revision.snapshot.as_ref().map(|snapshot| {
            serde_json::json!({
                "inputs": snapshot.inputs.iter().map(|input| serde_json::json!({
                    "path": input.path,
                    "state": input.state,
                    "payloadSha256": input.payload_sha256,
                    "physicalIdentityPresent": input.physical_identity.is_some(),
                    "absenceParentPath": input.absence_parent.as_ref().map(|parent| &parent.path),
                    "physicalRedirectPresent": input.physical_redirect_sha256.is_some(),
                })).collect::<Vec<_>>(),
                "scanInvocation": snapshot.scan_invocation,
                "entrySelections": snapshot.entry_selections,
                "evidence": snapshot.evidence.semantic_projection(),
            })
        });
        let observation_binding = revision
            .observation_binding
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?;
        let sealed_matches_owner = if matches!(
            revision.observation_binding.as_ref(),
            Some(lumin_model::ObservationBinding::Sealed { .. })
        ) {
            lumin_engine::gate_observation_binding_matches_owner(&gate, revision)?
        } else {
            false
        };
        let mut revision_alias_closures = revision.alias_closures.iter().collect::<Vec<_>>();
        revision_alias_closures.sort_by(|left, right| left.members.cmp(&right.members));
        let revision_alias_closures = revision_alias_closures
            .into_iter()
            .map(|closure| serde_json::json!({ "members": closure.members }))
            .collect::<Vec<_>>();
        let actual_write_set = revision.actual_write_set.as_ref().map(|actual| {
            let mut baseline_alias_closures =
                actual.baseline_alias_closures.iter().collect::<Vec<_>>();
            baseline_alias_closures.sort_by(|left, right| left.members.cmp(&right.members));
            let baseline_alias_closures = baseline_alias_closures
                .into_iter()
                .map(|closure| serde_json::json!({ "members": closure.members }))
                .collect::<Vec<_>>();
            let mut current_alias_closures =
                actual.current_alias_closures.iter().collect::<Vec<_>>();
            current_alias_closures.sort_by(|left, right| left.members.cmp(&right.members));
            let current_alias_closures = current_alias_closures
                .into_iter()
                .map(|closure| serde_json::json!({ "members": closure.members }))
                .collect::<Vec<_>>();
            serde_json::json!({
                "paths": actual.paths,
                "baselineAliasClosures": baseline_alias_closures,
                "currentAliasClosures": current_alias_closures,
            })
        });
        evidence.push(serde_json::json!({
            "schemaVersion": "lumin.gate-revision-semantic.v1",
            "gateId": gate.gate_id,
            "revision": revision.revision,
            "priorRevision": revision.revision.saturating_sub(1),
            "openingAnalysisContract": gate.baseline.as_ref().map(|baseline| &baseline.analysis_contract),
            "decision": revision.decision,
            "signals": revision.signals,
            "catalogRevision": revision.catalog_revision,
            "observationBinding": observation_binding_projection(
                observation_binding,
                revision.revision,
                sealed_matches_owner,
            )?,
            "leasedWriteSet": gate.leased_write_set.iter().map(|lease| serde_json::json!({
                "path": lease.path,
                "kind": lease.kind,
                "physicalIdentityPresent": lease.physical_identity.is_some(),
                "nearestExistingParent": lease.nearest_existing_parent,
                "prefixPaths": lease.prefix_identities.iter().map(|prefix| &prefix.path).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "protectedSemanticInputs": revision.protected_semantic_inputs.iter().map(|input| serde_json::json!({
                "path": input.path,
                "state": input.state,
                "payloadSha256": input.payload_sha256,
                "physicalIdentityPresent": input.physical_identity.is_some(),
                "absenceParentPath": input.absence_parent.as_ref().map(|parent| &parent.path),
                "physicalRedirectPresent": input.physical_redirect_sha256.is_some(),
            })).collect::<Vec<_>>(),
            "changedPaths": revision.changed_paths,
            "actualWriteSet": actual_write_set,
            "aliasClosures": revision_alias_closures,
            "reconciledTransitionSequences": revision.reconciled_transition_sequences,
            "snapshot": snapshot,
        }));
    }
    Ok(evidence)
}

fn observation_binding_projection(
    binding: Option<Value>,
    revision: u64,
    matches_owner: bool,
) -> Result<Value, Box<dyn std::error::Error>> {
    let Some(mut binding) = binding else {
        return Ok(Value::Null);
    };
    if binding.get("state").and_then(Value::as_str) != Some("sealed") {
        return Ok(binding);
    }
    let kind = binding
        .pointer("/observation/kind")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("sealed observation omitted its kind"))?;
    let observation_id = binding
        .pointer("/observation/observationId")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("sealed observation omitted its ID"))?
        .to_owned();
    if !matches_owner {
        return Err(std::io::Error::other(format!(
            "sealed {kind} observation ID disagrees with revision {revision} owner inputs"
        ))
        .into());
    }
    let equality_class = match kind {
        "baseline" => "baseline".to_owned(),
        "close" => format!("close-revision-{revision}"),
        other => {
            return Err(std::io::Error::other(format!(
                "sealed observation has unsupported kind {other:?}"
            ))
            .into());
        }
    };
    *binding
        .pointer_mut("/observation/observationId")
        .ok_or_else(|| std::io::Error::other("sealed observation omitted its ID"))? =
        observation_id_projection(&observation_id, &equality_class, matches_owner);
    Ok(binding)
}

fn observation_id_projection(value: &str, equality_class: &str, matches_owner: bool) -> Value {
    let expected_prefix = if equality_class == "baseline" {
        "gate_baseline_observation_"
    } else {
        "gate_close_observation_"
    };
    // Observation IDs intentionally bind repository-instance physical identities and the
    // active catalog revision. Determinism variants use fresh repositories, so compare the
    // exact persisted equality relation and every semantic owner input rather than erasing
    // the binding or comparing unrelated inode-derived digest bytes.
    serde_json::json!({
        "equalityClass": equality_class,
        "matchesOwner": matches_owner,
        "formatValid": value.starts_with(expected_prefix),
    })
}
