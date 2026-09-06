//! Strict W3 frame; v1 remains a separate closed decoder.
use super::audit_diagnostic::{self, AuditDiagnosticDto, AuditPhaseDto};
use lumin_model::audit_diagnostic::AuditExecutionDiagnostic;
use lumin_model::audit_store_diagnostic::AuditStorePhase;
use serde::{Deserialize, Serialize};

pub const SCHEMA: &str = "lumin.audit-execution-diagnostic.v2";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuditStoreDiagnosticDto {
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
    pub store_phases: Vec<AuditPhaseDto>,
}

impl AuditStoreDiagnosticDto {
    pub fn execution(&self) -> AuditDiagnosticDto {
        AuditDiagnosticDto {
            schema_version: audit_diagnostic::SCHEMA.to_owned(),
            diagnostic_only: self.diagnostic_only,
            build_id: self.build_id.clone(),
            process_id: self.process_id,
            attempt_id: self.attempt_id.clone(),
            run_id: self.run_id.clone(),
            requested_jobs: self.requested_jobs,
            observed_available_parallelism: self.observed_available_parallelism,
            parallelism_observation_error: self.parallelism_observation_error.clone(),
            actual_jobs: self.actual_jobs,
            configured_worker_stack_bytes: self.configured_worker_stack_bytes,
            phases: self.phases.clone(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        audit_diagnostic::validate_phases(&self.execution())?;
        self.validate_store_phases()
    }

    fn validate_store_phases(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA || !self.diagnostic_only || self.store_phases.len() != 52 {
            return Err("invalid store diagnostic inventory or version".to_owned());
        }
        for (row, phase) in self.store_phases.iter().zip(AuditStorePhase::ALL) {
            if row.phase != phase.name() || row.calls > 1 || (row.calls == 0 && !phase.optional()) {
                return Err("invalid store diagnostic phase/order/count".to_owned());
            }
            if row.calls == 0 {
                if row.elapsed_nanoseconds.is_some() || row.self_nanoseconds.is_some() {
                    return Err("absent store phase has fabricated timing".to_owned());
                }
                continue;
            }
            let children = AuditStorePhase::ALL
                .into_iter()
                .filter(|child| child.parent() == Some(phase))
                .try_fold(0_u64, |sum, child| {
                    sum.checked_add(
                        self.store_phases[child as usize]
                            .elapsed_nanoseconds
                            .unwrap_or(0),
                    )
                    .ok_or("store child timing overflow")
                })?;
            let residual = row
                .elapsed_nanoseconds
                .ok_or("missing store timing")?
                .checked_sub(children)
                .ok_or("store children exceed parent")?;
            if row.self_nanoseconds != Some(residual) {
                return Err("invalid store residual".to_owned());
            }
        }
        let bootstrap = &self.store_phases[2..7];
        let counts = bootstrap.iter().map(|row| row.calls).collect::<Vec<_>>();
        if counts != [0, 0, 0, 0, 0] && counts != [0, 1, 1, 1, 1] && counts != [1, 1, 1, 1, 1] {
            return Err("contradictory bootstrap observation".to_owned());
        }
        for root in AuditStorePhase::ROOTS {
            let outer = self
                .phases
                .iter()
                .find(|row| row.phase == root.name())
                .and_then(|row| row.elapsed_nanoseconds)
                .ok_or("missing enclosing execution phase")?;
            if self.store_phases[root as usize]
                .elapsed_nanoseconds
                .ok_or("missing store root")?
                > outer
            {
                return Err("store root exceeds enclosing execution phase".to_owned());
            }
        }
        let final_inputs = self
            .phases
            .iter()
            .find(|row| row.phase == "final-inputs")
            .and_then(|row| row.elapsed_nanoseconds)
            .ok_or("missing final inputs")?;
        if final_inputs
            > self.store_phases[AuditStorePhase::PublishPreflight as usize]
                .elapsed_nanoseconds
                .ok_or("missing store preflight")?
        {
            return Err("final inputs exceed store preflight".to_owned());
        }
        Ok(())
    }
}

pub fn encode(value: &AuditExecutionDiagnostic) -> Result<String, String> {
    let base = audit_diagnostic::project(value)?;
    let store_phases = value
        .pool
        .store_timings
        .observations()?
        .into_iter()
        .map(|row| AuditPhaseDto {
            phase: row.phase.name().to_owned(),
            calls: row.calls,
            elapsed_nanoseconds: row.elapsed_nanoseconds,
            self_nanoseconds: row.self_nanoseconds,
        })
        .collect();
    let dto = AuditStoreDiagnosticDto {
        schema_version: SCHEMA.to_owned(),
        diagnostic_only: base.diagnostic_only,
        build_id: base.build_id,
        process_id: base.process_id,
        attempt_id: base.attempt_id,
        run_id: base.run_id,
        requested_jobs: base.requested_jobs,
        observed_available_parallelism: base.observed_available_parallelism,
        parallelism_observation_error: base.parallelism_observation_error,
        actual_jobs: base.actual_jobs,
        configured_worker_stack_bytes: base.configured_worker_stack_bytes,
        phases: base.phases,
        store_phases,
    };
    // W2 projection retains a failed host parallelism observation as null/error.
    // Keep that raw evidence in v2 too; the runner's strict decoder rejects it.
    dto.validate_store_phases()?;
    let mut bytes = serde_json::to_string(&dto).map_err(|error| error.to_string())?;
    bytes.push('\n');
    Ok(bytes)
}

pub fn decode(bytes: &[u8]) -> Result<AuditStoreDiagnosticDto, String> {
    let dto: AuditStoreDiagnosticDto =
        serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let mut canonical = serde_json::to_vec(&dto).map_err(|error| error.to_string())?;
    canonical.push(b'\n');
    if bytes != canonical {
        return Err("noncanonical store diagnostic frame".to_owned());
    }
    dto.validate()?;
    Ok(dto)
}
