// These macros erase every observation (including clock reads) in production builds.
macro_rules! audit_phase_begin {
    ($profile:expr, $phase:ident) => {
        #[cfg(feature = "audit-execution-test-profile")]
        if let Some(profile) = $profile.as_deref_mut() {
            profile.begin(lumin_model::audit_diagnostic::AuditPhase::$phase);
        }
    };
}

macro_rules! audit_phase_end {
    ($profile:expr, $phase:ident) => {
        #[cfg(feature = "audit-execution-test-profile")]
        if let Some(profile) = $profile.as_deref_mut() {
            profile.end(lumin_model::audit_diagnostic::AuditPhase::$phase);
        }
    };
}

#[cfg(feature = "audit-execution-test-profile")]
pub(super) use recorder::AuditProfiler;

#[cfg(feature = "audit-execution-test-profile")]
mod recorder {
    use lumin_model::audit_diagnostic::{AuditPhase, AuditPoolObservation};
    use std::time::Instant;

    pub(crate) trait Clock {
        fn now(&self) -> u128;
    }

    pub(crate) struct MonotonicClock(Instant);
    impl Default for MonotonicClock {
        fn default() -> Self {
            Self(Instant::now())
        }
    }
    impl Clock for MonotonicClock {
        fn now(&self) -> u128 {
            self.0.elapsed().as_nanos()
        }
    }

    pub(crate) struct AuditProfiler<C = MonotonicClock> {
        clock: C,
        stack: [Option<(AuditPhase, u128)>; 23],
        depth: usize,
        observation: AuditPoolObservation,
    }

    impl Default for AuditProfiler {
        fn default() -> Self {
            Self::with_clock(MonotonicClock::default())
        }
    }

    impl<C: Clock> AuditProfiler<C> {
        fn with_clock(clock: C) -> Self {
            Self {
                clock,
                stack: [None; 23],
                depth: 0,
                observation: AuditPoolObservation::default(),
            }
        }

        pub(crate) fn begin(&mut self, phase: AuditPhase) {
            let parent = self
                .depth
                .checked_sub(1)
                .and_then(|index| self.stack[index])
                .map(|(phase, _)| phase)
                .or(Some(AuditPhase::Command));
            if self.depth == self.stack.len() || phase.parent() != parent {
                self.observation
                    .timings
                    .invalidate(format!("invalid parent for {}", phase.name()));
                return;
            }
            self.stack[self.depth] = Some((phase, self.clock.now()));
            self.depth += 1;
        }

        pub(crate) fn end(&mut self, phase: AuditPhase) {
            let Some(index) = self.depth.checked_sub(1) else {
                self.observation
                    .timings
                    .invalidate("unopened diagnostic interval");
                return;
            };
            let Some((opened, start)) = self.stack[index].take() else {
                self.observation
                    .timings
                    .invalidate("missing diagnostic interval");
                return;
            };
            self.depth = index;
            if opened != phase {
                self.observation
                    .timings
                    .invalidate("out-of-order diagnostic interval");
            } else if let Some(elapsed) = self.clock.now().checked_sub(start) {
                self.observation.timings.record(phase, elapsed);
            } else {
                self.observation
                    .timings
                    .invalidate("diagnostic clock regressed");
            }
        }

        pub(crate) fn pool(&mut self, actual: usize, stack: usize) {
            self.observation.actual_jobs = Some(actual);
            self.observation.configured_worker_stack_bytes = Some(stack);
        }

        #[cfg(feature = "audit-store-test-profile")]
        pub(crate) fn store(
            &mut self,
            root: lumin_model::audit_store_diagnostic::AuditStorePhase,
            timings: lumin_model::audit_store_diagnostic::AuditStoreTimings,
        ) {
            self.observation.store_timings.merge_root(root, timings);
        }

        pub(crate) fn finish(mut self) -> AuditPoolObservation {
            if self.depth != 0 {
                self.observation
                    .timings
                    .invalidate("unclosed diagnostic interval");
            }
            self.observation
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::cell::Cell;

        struct TestClock(Cell<u128>);
        impl Clock for &TestClock {
            fn now(&self) -> u128 {
                self.0.get()
            }
        }

        #[test]
        fn nested_and_repeated_intervals_use_an_independent_clock() -> Result<(), String> {
            let clock = TestClock(Cell::new(0));
            let mut recorder = AuditProfiler::with_clock(&clock);
            recorder.begin(AuditPhase::AuditWork);
            recorder.begin(AuditPhase::Capture);
            for start in [10, 30] {
                clock.0.set(start);
                recorder.begin(AuditPhase::Profiles);
                clock.0.set(start + 5);
                recorder.end(AuditPhase::Profiles);
            }
            clock.0.set(40);
            recorder.begin(AuditPhase::Finish);
            recorder.begin(AuditPhase::Graph);
            recorder.end(AuditPhase::Graph);
            recorder.begin(AuditPhase::DeadCode);
            recorder.end(AuditPhase::DeadCode);
            clock.0.set(45);
            recorder.end(AuditPhase::Finish);
            clock.0.set(50);
            recorder.end(AuditPhase::Capture);
            clock.0.set(70);
            recorder.end(AuditPhase::AuditWork);
            let mut timings = recorder.finish().timings;
            for phase in AuditPhase::ALL {
                if !matches!(
                    phase,
                    AuditPhase::AuditWork
                        | AuditPhase::Capture
                        | AuditPhase::Profiles
                        | AuditPhase::Finish
                        | AuditPhase::Graph
                        | AuditPhase::DeadCode
                        | AuditPhase::DemandCapture
                        | AuditPhase::Command
                ) {
                    timings.record(phase, 0);
                }
            }
            timings.record(AuditPhase::Command, 100);
            let rows = timings.observations()?;
            assert_eq!(rows[AuditPhase::Profiles as usize].calls, 2);
            assert_eq!(
                rows[AuditPhase::Profiles as usize].elapsed_nanoseconds,
                Some(10)
            );
            assert_eq!(
                rows[AuditPhase::Capture as usize].self_nanoseconds,
                Some(35)
            );
            assert_eq!(rows[AuditPhase::Finish as usize].self_nanoseconds, Some(5));
            assert_eq!(
                rows[AuditPhase::Graph as usize].elapsed_nanoseconds,
                Some(0)
            );
            assert_eq!(rows[AuditPhase::Graph as usize].calls, 1);
            assert_eq!(
                rows[AuditPhase::AuditWork as usize].self_nanoseconds,
                Some(20)
            );
            assert_eq!(
                rows[AuditPhase::Command as usize].self_nanoseconds,
                Some(30)
            );
            Ok(())
        }

        #[test]
        fn invalid_parent_and_unclosed_work_are_sticky_failures() {
            for phase in [AuditPhase::Capture, AuditPhase::AuditWork] {
                let clock = TestClock(Cell::new(0));
                let mut recorder = AuditProfiler::with_clock(&clock);
                recorder.begin(phase);
                let mut timings = recorder.finish().timings;
                for phase in AuditPhase::ALL {
                    timings.record(phase, 0);
                }
                assert!(timings.observations().is_err());
            }
        }

        #[test]
        fn clock_regression_and_out_of_order_close_remain_invalid_with_complete_rows() {
            for wrong_phase in [false, true] {
                let clock = TestClock(Cell::new(10));
                let mut recorder = AuditProfiler::with_clock(&clock);
                recorder.begin(AuditPhase::AuditWork);
                clock.0.set(9);
                recorder.end(if wrong_phase {
                    AuditPhase::Capture
                } else {
                    AuditPhase::AuditWork
                });
                let mut timings = recorder.finish().timings;
                for phase in AuditPhase::ALL {
                    timings.record(phase, 0);
                }
                assert!(timings.observations().is_err());
            }
        }
    }
}
