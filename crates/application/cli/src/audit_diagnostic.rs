use std::io::{self, Write};
use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use lumin_model::audit_diagnostic::{AuditExecutionDiagnostic, AuditPhase};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingAuditDiagnostic {
    command_start: Instant,
    value: AuditExecutionDiagnostic,
}

impl PendingAuditDiagnostic {
    pub(crate) fn new(
        command_start: Instant,
        build: &lumin_model::BuildIdentity,
        result: &lumin_engine::AuditResult,
        requested_jobs: Option<usize>,
        parallelism: io::Result<NonZeroUsize>,
    ) -> Self {
        let (observed, error) = match parallelism {
            Ok(value) => (Some(value.get()), None),
            Err(error) => (None, Some(error.to_string())),
        };
        Self {
            command_start,
            value: AuditExecutionDiagnostic {
                build_id: build.as_str().to_owned(),
                process_id: std::process::id(),
                attempt_id: result.published.attempt_id.as_str().to_owned(),
                run_id: result.published.run_id.as_str().to_owned(),
                requested_jobs,
                observed_available_parallelism: observed,
                parallelism_observation_error: error,
                pool: result.audit_diagnostic.clone(),
            },
        }
    }

    pub(crate) fn response_elapsed(&mut self, elapsed: Duration) {
        self.value
            .pool
            .timings
            .record(AuditPhase::Response, elapsed.as_nanos());
    }

    /// Invoked only after normal result transport succeeds. There are no store handles here.
    pub fn emit(
        &self,
        stdout_elapsed: Duration,
        command_end: Instant,
        stderr: &mut dyn Write,
    ) -> io::Result<()> {
        let mut value = self.value.clone();
        value
            .pool
            .timings
            .record(AuditPhase::Stdout, stdout_elapsed.as_nanos());
        let elapsed = command_end
            .checked_duration_since(self.command_start)
            .ok_or_else(|| io::Error::other("audit diagnostic clock regressed"))?;
        value
            .pool
            .timings
            .record(AuditPhase::Command, elapsed.as_nanos());
        let frame = lumin_protocol::audit_diagnostic::encode(&value).map_err(io::Error::other)?;
        stderr.write_all(frame.as_bytes())?;
        stderr.flush()
    }
}
