//! Closed W7 transport. It extends neither the v1 nor the v2 decoder's envelope.
use super::audit_diagnostic::AuditPhaseDto;
use super::audit_store_diagnostic::{self, AuditStoreDiagnosticDto};
use lumin_model::audit_diagnostic::AuditExecutionDiagnostic;
use lumin_model::audit_lifecycle_diagnostic::{
    AuditLifecycleContext, AuditLifecycleContextObservation, AuditLifecycleCost,
    AuditLifecycleCostObservation,
};
use serde::{Deserialize, Serialize};

pub const SCHEMA: &str = "lumin.audit-execution-diagnostic.v3";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuditLifecycleDiagnosticDto {
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
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LifecycleContextDto {
    pub context: String,
    pub calls: u64,
    pub elapsed_nanoseconds: u64,
    pub self_nanoseconds: u64,
    pub costs: Vec<LifecycleCostDto>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LifecycleCostDto {
    pub cost: String,
    pub calls: u64,
    pub elapsed_nanoseconds: Option<u64>,
}

impl AuditLifecycleDiagnosticDto {
    pub fn store(&self) -> AuditStoreDiagnosticDto {
        AuditStoreDiagnosticDto {
            schema_version: audit_store_diagnostic::SCHEMA.to_owned(),
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
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        self.store().validate()?;
        self.validate_lifecycle()
    }

    fn validate_lifecycle(&self) -> Result<(), String> {
        if self.schema_version != SCHEMA
            || !self.diagnostic_only
            || self.lifecycle_contexts.len() != 8
        {
            return Err("invalid lifecycle diagnostic inventory or version".to_owned());
        }
        for (row, context) in self
            .lifecycle_contexts
            .iter()
            .zip(AuditLifecycleContext::ALL)
        {
            if row.context != context.name() || row.costs.len() != 13 {
                return Err("invalid lifecycle context/cost inventory or order".to_owned());
            }
            let costs = row
                .costs
                .iter()
                .zip(AuditLifecycleCost::ALL)
                .map(|(row, cost)| {
                    if row.cost != cost.name() {
                        return Err("invalid lifecycle cost order".to_owned());
                    }
                    Ok(AuditLifecycleCostObservation {
                        cost,
                        calls: row.calls,
                        elapsed_nanoseconds: row.elapsed_nanoseconds,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            AuditLifecycleContextObservation {
                context,
                calls: row.calls,
                elapsed_nanoseconds: row.elapsed_nanoseconds,
                self_nanoseconds: row.self_nanoseconds,
                costs: costs
                    .try_into()
                    .map_err(|_| "invalid lifecycle cost count")?,
            }
            .validate()?;
            let outer = self
                .store_phases
                .get(context.phase() as usize)
                .and_then(|phase| phase.elapsed_nanoseconds)
                .ok_or("missing enclosing lifecycle phase")?;
            if row.elapsed_nanoseconds > outer {
                return Err("lifecycle context exceeds enclosing store phase".to_owned());
            }
        }
        Ok(())
    }
}

pub fn encode(value: &AuditExecutionDiagnostic) -> Result<String, String> {
    let base = audit_store_diagnostic::project(value)?;
    let lifecycle_contexts = value
        .pool
        .store_timings
        .lifecycle
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
    let dto = AuditLifecycleDiagnosticDto {
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
        lifecycle_contexts,
    };
    // As in v1/v2, preserve a failed host observation in the raw frame; decoding
    // refuses to accept it as a completed measurement.
    dto.validate_lifecycle()?;
    let mut bytes = serde_json::to_string(&dto).map_err(|error| error.to_string())?;
    bytes.push('\n');
    Ok(bytes)
}

pub fn decode(bytes: &[u8]) -> Result<AuditLifecycleDiagnosticDto, String> {
    let dto: AuditLifecycleDiagnosticDto =
        serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let mut canonical = serde_json::to_vec(&dto).map_err(|error| error.to_string())?;
    canonical.push(b'\n');
    if bytes != canonical {
        return Err("noncanonical lifecycle diagnostic frame".to_owned());
    }
    dto.validate()?;
    Ok(dto)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumin_model::audit_diagnostic::{AuditPhase, AuditPoolObservation};
    use lumin_model::audit_store_diagnostic::{AuditStorePhase, AuditStoreTimings};

    fn observation() -> AuditExecutionDiagnostic {
        let mut pool = AuditPoolObservation {
            actual_jobs: Some(1),
            configured_worker_stack_bytes: Some(4_194_304),
            ..Default::default()
        };
        for phase in AuditPhase::ALL {
            if phase != AuditPhase::DemandCapture {
                pool.timings.record(phase, 0);
            }
        }
        // Authored API vectors; no producer or decoder supplies expected counts.
        let counts = [
            [1, 1, 1, 1, 1, 0, 0, 0, 0, 1, 0, 0, 0],
            [1, 1, 1, 1, 1, 0, 0, 0, 0, 1, 0, 0, 0],
            [1, 2, 2, 1, 0, 0, 0, 0, 2, 0, 0, 0, 1],
            [1, 2, 2, 1, 1, 1, 1, 0, 0, 2, 1, 1, 1],
            [1, 2, 2, 1, 0, 0, 0, 0, 2, 0, 0, 0, 1],
            [1, 2, 2, 1, 0, 0, 0, 0, 2, 0, 0, 1, 1],
            [1, 2, 2, 1, 0, 0, 0, 0, 2, 0, 1, 1, 1],
            [1, 3, 3, 1, 2, 1, 1, 0, 0, 3, 1, 1, 1],
        ];
        for root in AuditStorePhase::ROOTS {
            let mut packet = AuditStoreTimings::default();
            for phase in AuditStorePhase::ALL
                .into_iter()
                .filter(|p| p.root() == root)
            {
                packet.record(phase, 0);
            }
            for context in AuditLifecycleContext::ALL
                .into_iter()
                .filter(|c| c.phase().root() == root)
            {
                packet.lifecycle.record(
                    root,
                    Ok(AuditLifecycleContextObservation {
                        context,
                        calls: 1,
                        elapsed_nanoseconds: 0,
                        self_nanoseconds: 0,
                        costs: AuditLifecycleCost::ALL.map(|cost| {
                            let calls = counts[context as usize][cost as usize];
                            AuditLifecycleCostObservation {
                                cost,
                                calls,
                                elapsed_nanoseconds: (calls > 0).then_some(0),
                            }
                        }),
                    }),
                );
            }
            pool.store_timings.merge_root(root, packet);
        }
        AuditExecutionDiagnostic {
            build_id: "build_fixture".to_owned(),
            process_id: 1,
            attempt_id: "attempt_fixture".to_owned(),
            run_id: "run_fixture".to_owned(),
            requested_jobs: Some(1),
            observed_available_parallelism: Some(8),
            parallelism_observation_error: None,
            pool,
        }
    }

    #[test]
    fn audit_lifecycle_encoder_keeps_exact_version_and_api_order() -> Result<(), String> {
        let frame = encode(&observation())?;
        let dto = decode(frame.as_bytes())?;
        assert_eq!(
            dto.lifecycle_contexts
                .iter()
                .map(|c| c.context.as_str())
                .collect::<Vec<_>>(),
            [
                "open-recovery-latest",
                "attempt-recover-latest",
                "attempt-directory",
                "attempt-latest",
                "staging-create",
                "staging-move",
                "publish-terminal",
                "finalize-latest"
            ]
        );
        for context in &dto.lifecycle_contexts {
            assert_eq!(
                context
                    .costs
                    .iter()
                    .map(|c| c.cost.as_str())
                    .collect::<Vec<_>>(),
                [
                    "namespace-validation",
                    "store-handle-open",
                    "backend-open",
                    "store-validation",
                    "read-admission",
                    "write-admission",
                    "backend-commit",
                    "backend-abort",
                    "database-explicit-drop",
                    "database-return-tail",
                    "json-write-flush",
                    "publication-move",
                    "directory-sync"
                ]
            );
        }
        assert!(crate::audit_diagnostic::decode(frame.as_bytes()).is_err());
        assert!(audit_store_diagnostic::decode(frame.as_bytes()).is_err());
        assert!(frame.starts_with("{\"schemaVersion\":\"lumin.audit-execution-diagnostic.v3\",\"diagnosticOnly\":true,\"buildId\":"));
        assert_eq!(frame.bytes().filter(|b| *b == b'\n').count(), 1);
        assert!(frame.ends_with("}\n"));
        Ok(())
    }
    #[test]
    fn audit_lifecycle_invalid_observer_refuses_success_without_erasing_host_error()
    -> Result<(), String> {
        let mut evidence = observation();
        evidence.observed_available_parallelism = None;
        evidence.parallelism_observation_error = Some("owned host failure".to_owned());
        let bytes = encode(&evidence)?;
        assert!(bytes.contains("\"parallelismObservationError\":\"owned host failure\""));
        assert!(decode(bytes.as_bytes()).is_err());
        evidence
            .pool
            .store_timings
            .lifecycle
            .invalidate("owned clock failure");
        assert_eq!(
            encode(&evidence).err().as_deref(),
            Some("owned clock failure")
        );
        evidence.pool.store_timings.lifecycle = Default::default();
        assert!(encode(&evidence).is_err());
        Ok(())
    }
}
