// Each invocation lends one local recorder along the synchronous coordinating call.
// These macros erase the recorder and every clock access when W3 is disabled.
macro_rules! store_phase_begin {
    ($profile:expr, $phase:ident) => {
        #[cfg(feature = "audit-store-test-profile")]
        if let Some(profile) = $profile.as_deref_mut() {
            profile.begin(lumin_model::audit_store_diagnostic::AuditStorePhase::$phase);
        }
    };
}
macro_rules! store_phase_end {
    ($profile:expr, $phase:ident) => {
        #[cfg(feature = "audit-store-test-profile")]
        if let Some(profile) = $profile.as_deref_mut() {
            profile.end(lumin_model::audit_store_diagnostic::AuditStorePhase::$phase);
        }
    };
}

// One authored closure body, with its observation argument erased in ordinary builds.
macro_rules! store_profile_lock {
    ($store:expr, $ordinary:ident, $exclusive:literal, $admission:literal,
        $profile:ident, $enter:ident, $exit:ident, |$guard:ident| $body:block) => {{
        #[cfg(feature = "audit-store-test-profile")]
        {
            use lumin_model::audit_store_diagnostic::AuditStorePhase;
            let operation =
                |$guard: &crate::namespace::NamespaceGuard,
                 mut $profile: Option<&mut crate::audit_profile::StoreProfiler>| {
                    $body
                };
            if $profile.is_some() {
                $store.namespace.with_profiled_lock(
                    $exclusive,
                    $admission,
                    $profile.as_deref_mut(),
                    (AuditStorePhase::$enter, AuditStorePhase::$exit),
                    operation,
                )
            } else {
                $store.$ordinary(|guard| crate::audit_profile::unobserved(guard, operation))
            }
        }
        #[cfg(not(feature = "audit-store-test-profile"))]
        {
            $store.$ordinary(|$guard| $body)
        }
    }};
}

#[cfg(feature = "audit-store-test-profile")]
pub(crate) use recorder::StoreProfiler;

#[cfg(feature = "audit-store-test-profile")]
pub(crate) fn unobserved<T>(
    guard: &crate::namespace::NamespaceGuard,
    operation: impl FnOnce(
        &crate::namespace::NamespaceGuard,
        Option<&mut StoreProfiler>,
    ) -> Result<T, crate::StoreError>,
) -> Result<T, crate::StoreError> {
    operation(guard, None)
}

#[cfg(feature = "audit-store-test-profile")]
mod recorder {
    use lumin_model::audit_store_diagnostic::{AuditStorePhase, AuditStoreTimings};
    use std::time::Instant;

    pub(crate) trait Clock {
        fn now(&self) -> u128;
    }
    pub(crate) struct MonotonicClock(Instant);
    impl Clock for MonotonicClock {
        fn now(&self) -> u128 {
            self.0.elapsed().as_nanos()
        }
    }

    pub(crate) struct StoreProfiler<C = MonotonicClock> {
        clock: C,
        root: AuditStorePhase,
        stack: [Option<(AuditStorePhase, u128)>; 52],
        depth: usize,
        timings: AuditStoreTimings,
    }

    impl StoreProfiler {
        pub(crate) fn new(root: AuditStorePhase) -> Self {
            Self::with_clock(root, MonotonicClock(Instant::now()))
        }
    }
    impl<C: Clock> StoreProfiler<C> {
        fn with_clock(root: AuditStorePhase, clock: C) -> Self {
            let mut timings = AuditStoreTimings::default();
            if root.parent().is_some() {
                timings.invalidate("store recorder requires a root");
            }
            Self {
                clock,
                root,
                stack: [None; 52],
                depth: 0,
                timings,
            }
        }

        pub(crate) fn begin(&mut self, phase: AuditStorePhase) {
            let parent = self
                .depth
                .checked_sub(1)
                .and_then(|index| self.stack[index])
                .map(|(phase, _)| phase);
            if phase.root() != self.root
                || phase.parent() != parent
                || self.depth == self.stack.len()
            {
                self.timings.invalidate("invalid store interval parent");
                return;
            }
            self.stack[self.depth] = Some((phase, self.clock.now()));
            self.depth += 1;
        }

        pub(crate) fn end(&mut self, phase: AuditStorePhase) {
            let Some(index) = self.depth.checked_sub(1) else {
                self.timings.invalidate("unopened store interval");
                return;
            };
            let Some((opened, start)) = self.stack[index].take() else {
                self.timings.invalidate("missing store interval");
                return;
            };
            self.depth = index;
            if opened != phase {
                self.timings.invalidate("out-of-order store interval");
            } else if let Some(elapsed) = self.clock.now().checked_sub(start) {
                self.timings.record(phase, elapsed);
            } else {
                self.timings.invalidate("store clock regressed");
            }
        }

        pub(crate) fn finish(mut self) -> AuditStoreTimings {
            if self.depth != 0 {
                self.timings.invalidate("unclosed store interval");
            }
            self.timings
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
        fn zero_other_roots(timings: AuditStoreTimings) -> AuditStoreTimings {
            let mut combined = AuditStoreTimings::default();
            combined.merge_root(AuditStorePhase::StoreOpen, timings);
            for root in [AuditStorePhase::AttemptBegin, AuditStorePhase::StorePublish] {
                let mut next = AuditStoreTimings::default();
                for phase in AuditStorePhase::ALL
                    .into_iter()
                    .filter(|phase| phase.root() == root)
                {
                    next.record(phase, 0);
                }
                combined.merge_root(root, next);
            }
            combined
        }
        fn complete_open(recorder: &mut StoreProfiler<&TestClock>) {
            recorder.begin(AuditStorePhase::OpenRecovery);
            for phase in [
                AuditStorePhase::OpenRecoveryEnter,
                AuditStorePhase::OpenRecoveryLatest,
                AuditStorePhase::OpenRecoveryLeases,
                AuditStorePhase::OpenRecoveryExit,
            ] {
                recorder.begin(phase);
                recorder.end(phase);
            }
            recorder.end(AuditStorePhase::OpenRecovery);
        }

        #[test]
        fn audit_store_every_boundary_is_lifo_and_checked() -> Result<(), String> {
            fn visit(
                recorder: &mut StoreProfiler<&TestClock>,
                clock: &TestClock,
                phase: AuditStorePhase,
            ) {
                recorder.begin(phase);
                clock.0.set(clock.0.get() + 3);
                for child in AuditStorePhase::ALL
                    .into_iter()
                    .filter(|child| child.parent() == Some(phase))
                {
                    visit(recorder, clock, child);
                }
                clock.0.set(clock.0.get() + 4);
                recorder.end(phase);
            }
            let clock = TestClock(Cell::new(0));
            let mut combined = AuditStoreTimings::default();
            for root in AuditStorePhase::ROOTS {
                let mut recorder = StoreProfiler::with_clock(root, &clock);
                visit(&mut recorder, &clock, root);
                combined.merge_root(root, recorder.finish());
            }
            for row in combined.observations()? {
                assert_eq!(row.calls, 1, "{}", row.phase.name());
                assert_eq!(row.self_nanoseconds, Some(7), "{}", row.phase.name());
            }
            Ok(())
        }

        #[test]
        fn audit_store_preflight_failure_keeps_original_error_and_allows_attempt_release()
        -> Result<(), Box<dyn std::error::Error>> {
            let fixture = tempfile::tempdir()?;
            let admission = lumin_inventory::repository_admission(fixture.path())?;
            let (store, opened) = crate::RepositoryStore::open_observed(
                &admission.canonical_root,
                &admission.binding,
            );
            let store = store?;
            let (attempt, begun) = store.begin_attempt_observed();
            let mut attempt = attempt?;
            let (result, published) = store.publish_run_observed(&mut attempt, |_| {
                Err(crate::StoreError::Integrity(
                    "owned preflight failure".to_owned(),
                ))
            });
            let error = result.err().ok_or("preflight unexpectedly succeeded")?;
            assert!(error.to_string().contains("owned preflight failure"));
            let mut combined = AuditStoreTimings::default();
            combined.merge_root(AuditStorePhase::StoreOpen, opened);
            combined.merge_root(AuditStorePhase::AttemptBegin, begun);
            combined.merge_root(AuditStorePhase::StorePublish, published);
            assert!(combined.observations().is_err());
            store.fail_attempt(&mut attempt, &error.to_string())?;
            drop(attempt);
            let latest = store.latest_snapshot()?;
            assert!(latest.completed.is_none());
            assert_eq!(
                latest.latest_attempt.ok_or("missing failed attempt")?.state,
                lumin_model::AttemptStatus::Failed
            );
            let mut next = store.begin_attempt()?;
            store.fail_attempt(&mut next, "finish independent retry")?;
            Ok(())
        }
        #[test]
        fn audit_store_clock_distinguishes_nested_repeated_zero_and_absent() -> Result<(), String> {
            let clock = TestClock(Cell::new(0));
            let mut recorder = StoreProfiler::with_clock(AuditStorePhase::StoreOpen, &clock);
            recorder.begin(AuditStorePhase::StoreOpen);
            recorder.begin(AuditStorePhase::NamespaceOpen);
            for start in [10, 30] {
                clock.0.set(start);
                recorder.begin(AuditStorePhase::BootstrapParents);
                clock.0.set(start + 5);
                recorder.end(AuditStorePhase::BootstrapParents);
            }
            clock.0.set(50);
            recorder.end(AuditStorePhase::NamespaceOpen);
            complete_open(&mut recorder);
            clock.0.set(70);
            recorder.end(AuditStorePhase::StoreOpen);
            let rows = zero_other_roots(recorder.finish()).observations()?;
            let parents = rows[AuditStorePhase::BootstrapParents as usize];
            assert_eq!((parents.calls, parents.elapsed_nanoseconds), (2, Some(10)));
            assert_eq!(
                rows[AuditStorePhase::NamespaceOpen as usize].self_nanoseconds,
                Some(40)
            );
            assert_eq!(rows[0].self_nanoseconds, Some(20));
            assert_eq!(
                rows[AuditStorePhase::BootstrapSetup as usize].elapsed_nanoseconds,
                None
            );
            assert_eq!(
                rows[AuditStorePhase::OpenRecovery as usize].elapsed_nanoseconds,
                Some(0)
            );
            Ok(())
        }
        #[test]
        fn audit_store_invalid_intervals_stay_invalid_without_short_circuiting_product_work() {
            for failure in 0..5 {
                let clock = TestClock(Cell::new(10));
                let mut recorder = StoreProfiler::with_clock(AuditStorePhase::StoreOpen, &clock);
                recorder.begin(AuditStorePhase::StoreOpen);
                recorder.begin(AuditStorePhase::NamespaceOpen);
                match failure {
                    0 => recorder.begin(AuditStorePhase::AttemptReserve),
                    1 => recorder.end(AuditStorePhase::BootstrapSetup),
                    2 => clock.0.set(9),
                    3 => clock.0.set(u128::MAX),
                    _ => recorder.begin(AuditStorePhase::BootstrapSetup),
                }
                recorder.end(AuditStorePhase::NamespaceOpen);
                // The product's subsequent recovery/release path still executes.
                complete_open(&mut recorder);
                recorder.end(AuditStorePhase::StoreOpen);
                let mut timings = recorder.finish();
                for phase in AuditStorePhase::ALL
                    .into_iter()
                    .filter(|phase| phase.root() == AuditStorePhase::StoreOpen)
                {
                    timings.record(phase, 0);
                }
                assert!(zero_other_roots(timings).observations().is_err());
            }
        }
    }
}
