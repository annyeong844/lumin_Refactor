//! W9's closed extension. Previous diagnostic envelopes retain their exact decoders.
use super::audit_diagnostic::AuditPhaseDto;
use super::audit_lifecycle_diagnostic::{
    self, AuditLifecycleDiagnosticDto, LifecycleContextDto, LifecycleCostDto,
};
use lumin_model::audit_boundary_diagnostic::{
    AuditBoundaryContext, AuditBoundaryContextObservation, AuditBoundaryCost,
    AuditBoundaryCostObservation,
};
use lumin_model::audit_diagnostic::AuditExecutionDiagnostic;
use serde::{Deserialize, Serialize};

pub const SCHEMA: &str = "lumin.audit-execution-diagnostic.v4";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuditBoundaryDiagnosticDto {
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
    pub lifecycle_contexts: Vec<LifecycleContextDto>,
    pub store_boundary_contexts: Vec<LifecycleContextDto>,
}
impl AuditBoundaryDiagnosticDto {
    pub fn lifecycle(&self) -> AuditLifecycleDiagnosticDto {
        AuditLifecycleDiagnosticDto {
            schema_version: audit_lifecycle_diagnostic::SCHEMA.to_owned(),
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
            store_phases: self.store_phases.clone(),
            lifecycle_contexts: self.lifecycle_contexts.clone(),
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        self.lifecycle().validate()?;
        self.validate_boundary()
    }
    fn validate_boundary(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA
            || !self.diagnostic_only
            || self.store_boundary_contexts.len() != 9
        {
            return Err("invalid boundary diagnostic inventory or version".to_owned());
        }
        for (row, context) in self
            .store_boundary_contexts
            .iter()
            .zip(AuditBoundaryContext::ALL)
        {
            if row.context != context.name() || row.costs.len() != 19 {
                return Err("invalid boundary context/cost inventory or order".to_owned());
            }
            let costs = row
                .costs
                .iter()
                .zip(AuditBoundaryCost::ALL)
                .map(|(row, cost)| {
                    if row.cost != cost.name() {
                        return Err("invalid boundary cost order".to_owned());
                    }
                    Ok(AuditBoundaryCostObservation {
                        cost,
                        calls: row.calls,
                        elapsed_nanoseconds: row.elapsed_nanoseconds,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            AuditBoundaryContextObservation {
                context,
                calls: row.calls,
                elapsed_nanoseconds: row.elapsed_nanoseconds,
                self_nanoseconds: row.self_nanoseconds,
                costs: costs
                    .try_into()
                    .map_err(|_| "invalid boundary cost count")?,
            }
            .validate()?;
            let outer = self
                .store_phases
                .get(context.phase() as usize)
                .and_then(|phase| phase.elapsed_nanoseconds)
                .ok_or("missing enclosing boundary phase")?;
            if row.elapsed_nanoseconds > outer {
                return Err("boundary context exceeds enclosing store phase".to_owned());
            }
        }
        Ok(())
    }
}
pub fn encode(value: &AuditExecutionDiagnostic) -> Result<String, String> {
    let base = audit_lifecycle_diagnostic::project(value)?;
    let store_boundary_contexts = value
        .pool
        .store_timings
        .boundary
        .observations()?
        .into_iter()
        .map(|row| LifecycleContextDto {
            context: row.context.name().to_owned(),
            calls: row.calls,
            elapsed_nanoseconds: row.elapsed_nanoseconds,
            self_nanoseconds: row.self_nanoseconds,
            costs: row
                .costs
                .into_iter()
                .map(|cost| LifecycleCostDto {
                    cost: cost.cost.name().to_owned(),
                    calls: cost.calls,
                    elapsed_nanoseconds: cost.elapsed_nanoseconds,
                })
                .collect(),
        })
        .collect();
    let dto = AuditBoundaryDiagnosticDto {
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
        store_phases: base.store_phases,
        lifecycle_contexts: base.lifecycle_contexts,
        store_boundary_contexts,
    };
    dto.validate_boundary()?;
    let mut bytes = serde_json::to_string(&dto).map_err(|error| error.to_string())?;
    bytes.push('\n');
    Ok(bytes)
}
pub fn decode(bytes: &[u8]) -> Result<AuditBoundaryDiagnosticDto, String> {
    let dto: AuditBoundaryDiagnosticDto =
        serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let mut canonical = serde_json::to_vec(&dto).map_err(|error| error.to_string())?;
    canonical.push(b'\n');
    if bytes != canonical {
        return Err("noncanonical boundary diagnostic frame".to_owned());
    }
    dto.validate()?;
    Ok(dto)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn observation() -> AuditExecutionDiagnostic {
        let mut value = crate::audit_lifecycle_diagnostic::tests::observation();
        // Positive validation placeholders; exact non-validation W9 fixture counts.
        let counts: [[u64; 19]; 9] = [
            [1, 1, 1, 1, 0, 0, 0, 0, 1, 0, 1, 1, 1, 0, 2, 0, 0, 0, 0],
            [1, 1, 1, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0],
            [1, 1, 1, 1, 0, 0, 0, 0, 1, 0, 1, 1, 1, 0, 0, 0, 0, 0, 0],
            [1, 1, 1, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0],
            [1, 1, 1, 1, 0, 0, 0, 0, 1, 0, 1, 1, 1, 0, 0, 0, 0, 0, 0],
            [1, 1, 1, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0],
            [1, 1, 1, 1, 0, 0, 0, 0, 1, 0, 1, 1, 1, 0, 0, 0, 0, 0, 0],
            [1, 3, 3, 1, 1, 2, 2, 0, 1, 2, 0, 0, 0, 0, 0, 3, 1, 1, 1],
            [1, 1, 1, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0],
        ];
        for context in AuditBoundaryContext::ALL {
            value.pool.store_timings.boundary.record(
                context.phase().root(),
                Ok(AuditBoundaryContextObservation {
                    context,
                    calls: 1,
                    elapsed_nanoseconds: 0,
                    self_nanoseconds: 0,
                    costs: AuditBoundaryCost::ALL.map(|cost| {
                        let calls = counts[context as usize][cost as usize];
                        AuditBoundaryCostObservation {
                            cost,
                            calls,
                            elapsed_nanoseconds: (calls > 0).then_some(0),
                        }
                    }),
                }),
            );
        }
        value
    }
    #[test]
    fn audit_boundary_encoder_preserves_versions_and_exact_inventory() -> Result<(), String> {
        let value = observation();
        let bytes = encode(&value)?;
        let frame = decode(bytes.as_bytes())?;
        assert_eq!(frame.store_boundary_contexts.len(), 9);
        assert_eq!(frame.lifecycle_contexts.len(), 8);
        assert_eq!(
            frame
                .store_boundary_contexts
                .iter()
                .map(|row| row.context.as_str())
                .collect::<Vec<_>>(),
            [
                "open-recovery-enter",
                "open-recovery-exit",
                "attempt-enter",
                "attempt-exit",
                "publish-prepare-enter",
                "publish-prepare-exit",
                "publish-finalize-enter",
                "finalize-release",
                "publish-finalize-exit"
            ]
        );
        assert!(bytes.starts_with(
            "{\"schemaVersion\":\"lumin.audit-execution-diagnostic.v4\",\"diagnosticOnly\":true,"
        ));
        assert!(bytes.ends_with("}\n"));
        assert_eq!(bytes.bytes().filter(|b| *b == b'\n').count(), 1);
        assert!(crate::audit_diagnostic::decode(bytes.as_bytes()).is_err());
        assert!(crate::audit_store_diagnostic::decode(bytes.as_bytes()).is_err());
        assert!(crate::audit_lifecycle_diagnostic::decode(bytes.as_bytes()).is_err());
        for old in [
            crate::audit_diagnostic::encode(&value)?,
            crate::audit_store_diagnostic::encode(&value)?,
            crate::audit_lifecycle_diagnostic::encode(&value)?,
        ] {
            assert!(decode(old.as_bytes()).is_err());
        }
        Ok(())
    }
    #[test]
    fn audit_boundary_observer_failure_is_not_a_valid_frame_or_erased_host_error()
    -> Result<(), String> {
        let mut value = observation();
        value.observed_available_parallelism = None;
        value.parallelism_observation_error = Some("retained host failure".to_owned());
        let bytes = encode(&value)?;
        assert!(bytes.contains("retained host failure"));
        assert!(decode(bytes.as_bytes()).is_err());
        value
            .pool
            .store_timings
            .boundary
            .invalidate("owned boundary clock failure");
        assert_eq!(
            encode(&value).err().as_deref(),
            Some("owned boundary clock failure")
        );
        Ok(())
    }
}
