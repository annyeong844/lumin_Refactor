//! Diagnostic-only transport. No production DTO or durable evidence uses this module.
use lumin_model::audit_diagnostic::AuditExecutionDiagnostic;
use serde::{Deserialize, Serialize};

pub const SCHEMA: &str = "lumin.audit-execution-diagnostic.v1";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuditDiagnosticDto {
    pub schema_version: String,
    pub diagnostic_only: bool,
    pub build_id: String,
    pub process_id: u32,
    pub attempt_id: String,
    pub run_id: String,
    pub requested_jobs: Option<usize>,
    pub observed_available_parallelism: Option<usize>,
    pub parallelism_observation_error: Option<String>,
    pub actual_jobs: usize,
    pub configured_worker_stack_bytes: usize,
    pub phases: Vec<AuditPhaseDto>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuditPhaseDto {
    pub phase: String,
    pub calls: u64,
    pub elapsed_nanoseconds: Option<u64>,
    pub self_nanoseconds: Option<u64>,
}

pub fn encode(value: &AuditExecutionDiagnostic) -> Result<String, String> {
    let dto = project(value)?;
    let mut bytes = serde_json::to_string(&dto).map_err(|error| error.to_string())?;
    bytes.push('\n');
    Ok(bytes)
}

pub(super) fn project(value: &AuditExecutionDiagnostic) -> Result<AuditDiagnosticDto, String> {
    let observations = if value.parallelism_observation_error.is_some() {
        value.pool.timings.observations()?
    } else {
        value.validate()?
    };
    let phases = observations
        .into_iter()
        .map(|phase| AuditPhaseDto {
            phase: phase.phase.name().to_owned(),
            calls: phase.calls,
            elapsed_nanoseconds: phase.elapsed_nanoseconds,
            self_nanoseconds: phase.self_nanoseconds,
        })
        .collect();
    Ok(AuditDiagnosticDto {
        schema_version: SCHEMA.to_owned(),
        diagnostic_only: true,
        build_id: value.build_id.clone(),
        process_id: value.process_id,
        attempt_id: value.attempt_id.clone(),
        run_id: value.run_id.clone(),
        requested_jobs: value.requested_jobs,
        observed_available_parallelism: value.observed_available_parallelism,
        parallelism_observation_error: value.parallelism_observation_error.clone(),
        actual_jobs: value.pool.actual_jobs.ok_or("missing pool size")?,
        configured_worker_stack_bytes: value
            .pool
            .configured_worker_stack_bytes
            .ok_or("missing worker stack policy")?,
        phases,
    })
}

/// Struct decoding rejects duplicate keys at every level. Canonical re-encoding also
/// requires explicit nulls, exact field order, and precisely one transport newline.
pub fn decode(bytes: &[u8]) -> Result<AuditDiagnosticDto, String> {
    let dto: AuditDiagnosticDto =
        serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let mut canonical = serde_json::to_vec(&dto).map_err(|error| error.to_string())?;
    canonical.push(b'\n');
    if bytes != canonical || dto.schema_version != SCHEMA || !dto.diagnostic_only {
        return Err("noncanonical audit diagnostic frame".to_owned());
    }
    validate_phases(&dto)?;
    Ok(dto)
}

pub(super) fn validate_phases(dto: &AuditDiagnosticDto) -> Result<(), String> {
    use lumin_model::audit_diagnostic::{AuditPhase, AuditTimings};
    if dto.phases.len() != AuditPhase::ALL.len() {
        return Err("incomplete phase inventory".to_owned());
    }
    let mut timings = AuditTimings::default();
    for (row, phase) in dto.phases.iter().zip(AuditPhase::ALL) {
        if row.phase != phase.name() {
            return Err("unexpected phase order".to_owned());
        }
        if row.calls == 0 {
            if row.elapsed_nanoseconds.is_some() || row.self_nanoseconds.is_some() {
                return Err("absent phase has a fabricated timing".to_owned());
            }
        } else {
            timings.record(
                phase,
                u128::from(row.elapsed_nanoseconds.ok_or("missing elapsed time")?),
            );
        }
    }
    for (row, expected) in dto.phases.iter().zip(timings.observations()?) {
        if row.self_nanoseconds != expected.self_nanoseconds {
            return Err("phase residual disagrees with direct children".to_owned());
        }
    }
    let observed = dto
        .observed_available_parallelism
        .filter(|jobs| *jobs > 0)
        .ok_or("missing parallelism observation")?;
    let expected = dto.requested_jobs.unwrap_or(observed.min(8));
    if dto.parallelism_observation_error.is_some()
        || expected == 0
        || dto.actual_jobs != expected
        || dto.configured_worker_stack_bytes != 4_194_304
    {
        return Err("contradictory worker observation".to_owned());
    }
    Ok(())
}
