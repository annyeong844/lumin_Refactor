//! Ephemeral values for the unshippable W2 audit diagnostic. Never evidence or store state.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
pub enum AuditPhase {
    Command,
    PoolCreate,
    AuditWork,
    PoolRelease,
    Admission,
    StoreOpen,
    EntryIdentities,
    AttemptBegin,
    Capture,
    Inventory,
    Profiles,
    Extraction,
    Resolution,
    DemandCapture,
    Finish,
    Graph,
    DeadCode,
    Publication,
    EvidencePrepare,
    StorePublish,
    FinalInputs,
    Response,
    Stdout,
}

impl AuditPhase {
    pub const ALL: [Self; 23] = [
        Self::Command,
        Self::PoolCreate,
        Self::AuditWork,
        Self::PoolRelease,
        Self::Admission,
        Self::StoreOpen,
        Self::EntryIdentities,
        Self::AttemptBegin,
        Self::Capture,
        Self::Inventory,
        Self::Profiles,
        Self::Extraction,
        Self::Resolution,
        Self::DemandCapture,
        Self::Finish,
        Self::Graph,
        Self::DeadCode,
        Self::Publication,
        Self::EvidencePrepare,
        Self::StorePublish,
        Self::FinalInputs,
        Self::Response,
        Self::Stdout,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::PoolCreate => "pool-create",
            Self::AuditWork => "audit-work",
            Self::PoolRelease => "pool-release",
            Self::Admission => "admission",
            Self::StoreOpen => "store-open",
            Self::EntryIdentities => "entry-identities",
            Self::AttemptBegin => "attempt-begin",
            Self::Capture => "capture",
            Self::Inventory => "inventory",
            Self::Profiles => "profiles",
            Self::Extraction => "extraction",
            Self::Resolution => "resolution",
            Self::DemandCapture => "demand-capture",
            Self::Finish => "finish",
            Self::Graph => "graph",
            Self::DeadCode => "dead-code",
            Self::Publication => "publication",
            Self::EvidencePrepare => "evidence-prepare",
            Self::StorePublish => "store-publish",
            Self::FinalInputs => "final-inputs",
            Self::Response => "response",
            Self::Stdout => "stdout",
        }
    }

    pub const fn parent(self) -> Option<Self> {
        match self {
            Self::Command => None,
            Self::PoolCreate
            | Self::AuditWork
            | Self::PoolRelease
            | Self::Response
            | Self::Stdout => Some(Self::Command),
            Self::Admission
            | Self::StoreOpen
            | Self::EntryIdentities
            | Self::AttemptBegin
            | Self::Capture
            | Self::Publication => Some(Self::AuditWork),
            Self::Inventory
            | Self::Profiles
            | Self::Extraction
            | Self::Resolution
            | Self::DemandCapture
            | Self::Finish => Some(Self::Capture),
            Self::Graph | Self::DeadCode => Some(Self::Finish),
            Self::EvidencePrepare | Self::StorePublish => Some(Self::Publication),
            Self::FinalInputs => Some(Self::StorePublish),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Aggregate {
    calls: u64,
    nanoseconds: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AuditTimings {
    aggregates: [Aggregate; 23],
    error: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuditPhaseObservation {
    pub phase: AuditPhase,
    pub calls: u64,
    pub elapsed_nanoseconds: Option<u64>,
    pub self_nanoseconds: Option<u64>,
}

impl AuditTimings {
    pub fn merge(&mut self, other: Self) {
        if let Some(error) = other.error {
            self.invalidate(error);
        }
        for phase in AuditPhase::ALL {
            let incoming = other.aggregates[phase as usize];
            let current = &mut self.aggregates[phase as usize];
            match (
                current.calls.checked_add(incoming.calls),
                current.nanoseconds.checked_add(incoming.nanoseconds),
            ) {
                (Some(calls), Some(nanoseconds)) => *current = Aggregate { calls, nanoseconds },
                _ => self.invalidate(format!("{} merge overflow", phase.name())),
            }
        }
    }

    /// A measurement failure is sticky, but cannot interrupt the product's lifecycle.
    pub fn invalidate(&mut self, reason: impl Into<String>) {
        if self.error.is_none() {
            self.error = Some(reason.into());
        }
    }

    pub fn record(&mut self, phase: AuditPhase, nanoseconds: u128) {
        let row = &mut self.aggregates[phase as usize];
        let next = u64::try_from(nanoseconds).ok().and_then(|elapsed| {
            Some((
                row.calls.checked_add(1)?,
                row.nanoseconds.checked_add(elapsed)?,
            ))
        });
        match next {
            Some((calls, nanoseconds)) => *row = Aggregate { calls, nanoseconds },
            None => self.invalidate(format!("{} timing overflow", phase.name())),
        }
    }

    pub fn observations(&self) -> Result<[AuditPhaseObservation; 23], String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let mut rows = AuditPhase::ALL.map(|phase| AuditPhaseObservation {
            phase,
            calls: 0,
            elapsed_nanoseconds: None,
            self_nanoseconds: None,
        });
        for phase in AuditPhase::ALL {
            let aggregate = self.aggregates[phase as usize];
            if aggregate.calls == 0 {
                if phase != AuditPhase::DemandCapture {
                    return Err(format!("{} was not measured", phase.name()));
                }
                continue;
            }
            let children = AuditPhase::ALL
                .into_iter()
                .filter(|child| child.parent() == Some(phase))
                .try_fold(0_u64, |sum, child| {
                    sum.checked_add(self.aggregates[child as usize].nanoseconds)
                        .ok_or_else(|| format!("{} child timing overflow", phase.name()))
                })?;
            let residual = aggregate
                .nanoseconds
                .checked_sub(children)
                .ok_or_else(|| format!("{} children exceed parent timing", phase.name()))?;
            rows[phase as usize] = AuditPhaseObservation {
                phase,
                calls: aggregate.calls,
                elapsed_nanoseconds: Some(aggregate.nanoseconds),
                self_nanoseconds: Some(residual),
            };
        }
        Ok(rows)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AuditPoolObservation {
    pub timings: AuditTimings,
    pub actual_jobs: Option<usize>,
    pub configured_worker_stack_bytes: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditExecutionDiagnostic {
    pub build_id: String,
    pub process_id: u32,
    pub attempt_id: String,
    pub run_id: String,
    pub requested_jobs: Option<usize>,
    pub observed_available_parallelism: Option<usize>,
    pub parallelism_observation_error: Option<String>,
    pub pool: AuditPoolObservation,
}

impl AuditExecutionDiagnostic {
    pub fn validate(&self) -> Result<[AuditPhaseObservation; 23], String> {
        let observed = self
            .observed_available_parallelism
            .filter(|value| *value > 0)
            .ok_or("parallelism observation is unavailable")?;
        if self.parallelism_observation_error.is_some() {
            return Err("parallelism observation failed".to_owned());
        }
        let selected = self.requested_jobs.unwrap_or(observed.min(8));
        if selected == 0
            || self.pool.actual_jobs != Some(selected)
            || self.pool.configured_worker_stack_bytes != Some(4_194_304)
        {
            return Err("pool observation contradicts selected worker policy".to_owned());
        }
        self.pool.timings.observations()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zero_timings() -> AuditTimings {
        let mut timings = AuditTimings::default();
        for phase in AuditPhase::ALL {
            if phase != AuditPhase::DemandCapture {
                timings.record(phase, 0);
            }
        }
        timings
    }

    #[test]
    fn absence_is_not_zero_and_every_other_phase_is_required() -> Result<(), String> {
        let timings = zero_timings();
        let rows = timings.observations()?;
        assert_eq!(
            rows[AuditPhase::DemandCapture as usize].elapsed_nanoseconds,
            None
        );
        assert_eq!(
            rows[AuditPhase::Graph as usize].elapsed_nanoseconds,
            Some(0)
        );
        for phase in AuditPhase::ALL {
            if phase == AuditPhase::DemandCapture {
                continue;
            }
            let mut missing = timings.clone();
            missing.aggregates[phase as usize] = Aggregate::default();
            assert!(missing.observations().is_err(), "{}", phase.name());
        }
        Ok(())
    }

    #[test]
    fn overflow_underflow_and_invalid_intervals_fail_closed() {
        let mut timings = zero_timings();
        timings.record(AuditPhase::Command, u128::MAX);
        assert!(timings.observations().is_err());
        let mut timings = zero_timings();
        timings.record(AuditPhase::Command, u128::from(u64::MAX));
        timings.record(AuditPhase::Command, 1);
        assert!(timings.observations().is_err());
        let mut timings = zero_timings();
        timings.record(AuditPhase::Graph, 1);
        assert!(timings.observations().is_err());
        let mut timings = zero_timings();
        timings.aggregates[AuditPhase::Command as usize].calls = u64::MAX;
        timings.record(AuditPhase::Command, 0);
        assert!(timings.observations().is_err());
    }
}
