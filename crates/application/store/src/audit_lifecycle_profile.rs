// Optional explicit borrowing only. None of these macros attaches an observer to
// a database, transaction, guard, session or file, and W7-off erases every clock.
macro_rules! lifecycle_begin {
    ($profile:expr, $cost:ident) => {
        #[cfg(feature = "audit-lifecycle-test-profile")]
        if let Some(profile) = $profile.as_deref_mut() {
            profile.begin(lumin_model::audit_lifecycle_diagnostic::AuditLifecycleCost::$cost);
        }
    };
}
macro_rules! lifecycle_end {
    ($profile:expr, $cost:ident) => {
        #[cfg(feature = "audit-lifecycle-test-profile")]
        if let Some(profile) = $profile.as_deref_mut() {
            profile.end(lumin_model::audit_lifecycle_diagnostic::AuditLifecycleCost::$cost);
        }
    };
}
macro_rules! lifecycle_cost {
    ($profile:expr, $cost:ident, $expression:expr) => {{
        lifecycle_begin!($profile, $cost);
        let result = $expression;
        lifecycle_end!($profile, $cost);
        result
    }};
}
macro_rules! lifecycle_context {
    ($profile:ident, $context:ident, |$lifecycle:ident| $body:expr) => {{
        #[cfg(feature = "audit-lifecycle-test-profile")]
        let mut lifecycle_recorder = $profile.as_ref().map(|_| {
            crate::audit_lifecycle_profile::LifecycleProfiler::new(
                lumin_model::audit_lifecycle_diagnostic::AuditLifecycleContext::$context,
            )
        });
        #[cfg(feature = "audit-lifecycle-test-profile")]
        let $lifecycle = lifecycle_recorder.as_mut();
        let result = $body;
        #[cfg(feature = "audit-lifecycle-test-profile")]
        if let (Some(profile), Some(recorder)) = ($profile.as_deref_mut(), lifecycle_recorder) {
            profile.record_lifecycle(recorder.finish());
        }
        result
    }};
}

#[cfg(feature = "audit-boundary-test-profile")]
pub(crate) use recorder::BoundaryProfiler;
#[cfg(feature = "audit-lifecycle-test-profile")]
pub(crate) use recorder::LifecycleProfiler;

#[cfg(feature = "audit-lifecycle-test-profile")]
mod recorder {
    use crate::audit_profile::{Clock, MonotonicClock};
    use lumin_model::audit_lifecycle_diagnostic::{
        AuditLifecycleContext, AuditLifecycleContextObservation, AuditLifecycleCost,
        AuditLifecycleCostObservation,
    };

    pub(crate) type LifecycleProfiler<C = MonotonicClock> =
        ContextProfiler<AuditLifecycleContextObservation, C>;
    #[cfg(feature = "audit-boundary-test-profile")]
    pub(crate) type BoundaryProfiler<C = MonotonicClock> =
        ContextProfiler<lumin_model::audit_boundary_diagnostic::AuditBoundaryContextObservation, C>;

    pub(crate) trait ProfileRow: Sized {
        type Context;
        type Cost: Copy + Eq;
        fn empty(context: Self::Context) -> Self;
        fn cost_mut(&mut self, cost: Self::Cost) -> (&mut u64, &mut Option<u64>);
        fn complete(self, elapsed: u64) -> Result<Self, String>;
    }
    macro_rules! profile_row {
        ($row:ty, $context:ty, $cost:ty, $observation:path) => {
            impl ProfileRow for $row {
                type Context = $context;
                type Cost = $cost;
                fn empty(context: Self::Context) -> Self {
                    use $observation as CostObservation;
                    Self {
                        context,
                        calls: 1,
                        elapsed_nanoseconds: 0,
                        self_nanoseconds: 0,
                        costs: <$cost>::ALL.map(|cost| CostObservation {
                            cost,
                            calls: 0,
                            elapsed_nanoseconds: None,
                        }),
                    }
                }
                fn cost_mut(&mut self, cost: Self::Cost) -> (&mut u64, &mut Option<u64>) {
                    let row = &mut self.costs[cost as usize];
                    (&mut row.calls, &mut row.elapsed_nanoseconds)
                }
                fn complete(mut self, elapsed: u64) -> Result<Self, String> {
                    self.elapsed_nanoseconds = elapsed;
                    let sum = self.costs.iter().try_fold(0_u64, |sum, row| {
                        sum.checked_add(row.elapsed_nanoseconds.unwrap_or(0))
                            .ok_or("lifecycle cost sum overflow")
                    })?;
                    self.self_nanoseconds = elapsed
                        .checked_sub(sum)
                        .ok_or("lifecycle costs exceed context")?;
                    self.validate()?;
                    Ok(self)
                }
            }
        };
    }
    profile_row!(
        AuditLifecycleContextObservation,
        AuditLifecycleContext,
        AuditLifecycleCost,
        AuditLifecycleCostObservation
    );
    #[cfg(feature = "audit-boundary-test-profile")]
    profile_row!(
        lumin_model::audit_boundary_diagnostic::AuditBoundaryContextObservation,
        lumin_model::audit_boundary_diagnostic::AuditBoundaryContext,
        lumin_model::audit_boundary_diagnostic::AuditBoundaryCost,
        lumin_model::audit_boundary_diagnostic::AuditBoundaryCostObservation
    );

    pub(crate) struct ContextProfiler<R: ProfileRow, C = MonotonicClock> {
        clock: C,
        start: u128,
        last: u128,
        active: Option<(R::Cost, u128)>,
        row: R,
        error: Option<String>,
    }

    impl<R: ProfileRow> ContextProfiler<R> {
        pub(crate) fn new(context: R::Context) -> Self {
            Self::with_clock(context, MonotonicClock::new())
        }
    }

    impl<R: ProfileRow, C: Clock> ContextProfiler<R, C> {
        fn with_clock(context: R::Context, clock: C) -> Self {
            let start = clock.now();
            Self {
                clock,
                start,
                last: start,
                active: None,
                error: None,
                row: R::empty(context),
            }
        }

        pub(crate) fn invalidate(&mut self, reason: &str) {
            if self.error.is_none() {
                self.error = Some(reason.to_owned());
            }
        }

        fn now(&mut self) -> u128 {
            let now = self.clock.now();
            if now < self.last {
                self.invalidate("lifecycle clock regressed");
            }
            self.last = now;
            now
        }

        pub(crate) fn begin(&mut self, cost: R::Cost) {
            let now = self.now();
            if self.active.is_some() {
                self.invalidate("overlapping lifecycle costs");
            } else {
                self.active = Some((cost, now));
            }
        }

        pub(crate) fn end(&mut self, cost: R::Cost) {
            let now = self.now();
            let Some((opened, start)) = self.active.take() else {
                self.invalidate("unopened lifecycle cost");
                return;
            };
            if opened != cost {
                self.invalidate("out-of-order lifecycle cost");
                return;
            }
            let (calls, elapsed_nanoseconds) = self.row.cost_mut(cost);
            let next = now
                .checked_sub(start)
                .and_then(|elapsed| u64::try_from(elapsed).ok())
                .and_then(|elapsed| {
                    Some((
                        calls.checked_add(1)?,
                        elapsed_nanoseconds.unwrap_or(0).checked_add(elapsed)?,
                    ))
                });
            match next {
                Some((count, elapsed)) => {
                    *calls = count;
                    *elapsed_nanoseconds = Some(elapsed);
                }
                None => self.invalidate("lifecycle cost overflow"),
            }
        }

        pub(crate) fn finish(mut self) -> Result<R, String> {
            let end = self.now();
            if self.active.is_some() {
                self.invalidate("unclosed lifecycle cost or return tail");
            }
            if let Some(error) = self.error {
                return Err(error);
            }
            let elapsed = end
                .checked_sub(self.start)
                .and_then(|elapsed| u64::try_from(elapsed).ok())
                .ok_or("lifecycle context overflow")?;
            self.row.complete(elapsed)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use AuditLifecycleCost::*;
        use std::cell::{Cell, RefCell};

        struct TestClock(Cell<u128>);
        impl Clock for &TestClock {
            fn now(&self) -> u128 {
                self.0.get()
            }
        }
        fn required_costs(profile: &mut LifecycleProfiler<&TestClock>) {
            for cost in [
                NamespaceValidation,
                StoreHandleOpen,
                BackendOpen,
                StoreValidation,
            ] {
                profile.begin(cost);
                profile.end(cost);
            }
        }
        #[test]
        fn audit_lifecycle_unclosed_return_and_cost_sum_overflow_stay_invalid() {
            let clock = TestClock(Cell::new(0));
            let mut profile =
                LifecycleProfiler::with_clock(AuditLifecycleContext::OpenRecoveryLatest, &clock);
            required_costs(&mut profile);
            profile.begin(DatabaseReturnTail);
            assert_eq!(
                profile.finish().err().as_deref(),
                Some("unclosed lifecycle cost or return tail")
            );
            let mut profile =
                LifecycleProfiler::with_clock(AuditLifecycleContext::AttemptDirectory, &clock);
            required_costs(&mut profile);
            profile.begin(DatabaseExplicitDrop);
            profile.end(DatabaseExplicitDrop);
            profile.row.costs[0].elapsed_nanoseconds = Some(u64::MAX);
            profile.row.costs[1].elapsed_nanoseconds = Some(1);
            assert_eq!(
                profile.finish().err().as_deref(),
                Some("lifecycle cost sum overflow")
            );
        }

        #[test]
        fn audit_lifecycle_clock_repeated_zero_absent_and_residual() -> Result<(), String> {
            let clock = TestClock(Cell::new(0));
            let mut profile =
                LifecycleProfiler::with_clock(AuditLifecycleContext::AttemptDirectory, &clock);
            required_costs(&mut profile);
            for start in [10, 30] {
                clock.0.set(start);
                profile.begin(NamespaceValidation);
                clock.0.set(start + 5);
                profile.end(NamespaceValidation);
            }
            clock.0.set(50);
            profile.begin(DatabaseExplicitDrop);
            clock.0.set(70);
            profile.end(DatabaseExplicitDrop);
            clock.0.set(100);
            let row = profile.finish()?;
            assert_eq!((row.elapsed_nanoseconds, row.self_nanoseconds), (100, 70));
            assert_eq!(
                (row.costs[0].calls, row.costs[0].elapsed_nanoseconds),
                (3, Some(10))
            );
            assert_eq!(row.costs[BackendOpen as usize].elapsed_nanoseconds, Some(0));
            assert_eq!(row.costs[ReadAdmission as usize].elapsed_nanoseconds, None);
            Ok(())
        }
        #[test]
        fn audit_lifecycle_invalid_intervals_cannot_skip_product_work_or_cleanup() {
            for failure in 0..8 {
                let clock = TestClock(Cell::new(10));
                let mut recorder =
                    LifecycleProfiler::with_clock(AuditLifecycleContext::AttemptLatest, &clock);
                match failure {
                    0 => {
                        recorder.begin(BackendOpen);
                        recorder.begin(StoreValidation);
                    }
                    1 => recorder.end(BackendOpen),
                    2 => {
                        recorder.begin(BackendOpen);
                        recorder.end(StoreValidation);
                    }
                    3 => {
                        clock.0.set(9);
                        recorder.begin(BackendOpen);
                    }
                    4 => {
                        recorder.begin(BackendOpen);
                        clock.0.set(u128::MAX);
                        recorder.end(BackendOpen);
                    }
                    5 => recorder.begin(DatabaseReturnTail),
                    6 => {
                        recorder.row.costs[BackendOpen as usize].calls = u64::MAX;
                        recorder.begin(BackendOpen);
                        recorder.end(BackendOpen);
                    }
                    _ => {
                        recorder.row.costs[BackendOpen as usize].elapsed_nanoseconds =
                            Some(u64::MAX);
                        recorder.begin(BackendOpen);
                        clock.0.set(11);
                        recorder.end(BackendOpen);
                    }
                }
                let events = RefCell::new(Vec::new());
                let mut profile = Some(&mut recorder);
                let result: Result<(), &str> = lifecycle_cost!(profile, JsonWriteFlush, {
                    let _cleanup = Canary {
                        name: "cleanup",
                        events: &events,
                        clock: &clock,
                    };
                    events.borrow_mut().push("product");
                    Err("original product error")
                });
                assert_eq!(result, Err("original product error"));
                assert_eq!(*events.borrow(), ["product", "cleanup"]);
                assert!(recorder.finish().is_err());
            }
        }

        // Supporting recorder mechanics only. These are NOT redb/HeldEntry
        // destructors; the concrete ownership comparison is a separate review.
        struct Canary<'a> {
            name: &'static str,
            events: &'a RefCell<Vec<&'static str>>,
            clock: &'a TestClock,
        }
        impl Drop for Canary<'_> {
            fn drop(&mut self) {
                self.events.borrow_mut().push(self.name);
                self.clock.0.set(self.clock.0.get().saturating_add(5));
            }
        }
        struct Constructed<'a> {
            _entry: Canary<'a>,
            _database: Canary<'a>,
        }
        fn return_owned(
            profile: &mut LifecycleProfiler<&TestClock>,
            clock: &TestClock,
            events: &RefCell<Vec<&'static str>>,
            early: bool,
        ) -> Result<u64, &'static str> {
            let _database = Constructed {
                _entry: Canary {
                    name: "entry",
                    events,
                    clock,
                },
                _database: Canary {
                    name: "database",
                    events,
                    clock,
                },
            };
            let mut profile = Some(profile);
            if early {
                let result = Ok(1);
                lifecycle_begin!(profile, DatabaseReturnTail);
                return result;
            }
            let result = Ok(2);
            lifecycle_begin!(profile, DatabaseReturnTail);
            result
        }
        #[test]
        fn audit_lifecycle_return_tail_closes_before_the_next_caller_action() -> Result<(), String>
        {
            for early in [false, true] {
                let clock = TestClock(Cell::new(0));
                let events = RefCell::new(Vec::new());
                let mut recorder = LifecycleProfiler::with_clock(
                    AuditLifecycleContext::OpenRecoveryLatest,
                    &clock,
                );
                required_costs(&mut recorder);
                let result = return_owned(&mut recorder, &clock, &events, early);
                let mut profile = Some(&mut recorder);
                lifecycle_end!(profile, DatabaseReturnTail);
                assert_eq!(result, Ok(if early { 1 } else { 2 }));
                events.borrow_mut().push("caller");
                clock.0.set(100);
                let row = recorder.finish()?;
                assert_eq!(*events.borrow(), ["entry", "database", "caller"]);
                assert_eq!(
                    row.costs[DatabaseReturnTail as usize].elapsed_nanoseconds,
                    Some(10)
                );
                assert_eq!(row.self_nanoseconds, 90);
            }
            Ok(())
        }
        #[test]
        fn audit_lifecycle_failed_preconstruction_return_preserves_local_teardown() {
            fn fail(
                clock: &TestClock,
                events: &RefCell<Vec<&'static str>>,
            ) -> Result<(), &'static str> {
                let _entry = Canary {
                    name: "entry",
                    events,
                    clock,
                };
                let _backend = Canary {
                    name: "database",
                    events,
                    clock,
                };
                Err("preconstruction error")
            }
            let clock = TestClock(Cell::new(0));
            let events = RefCell::new(Vec::new());
            let mut recorder =
                LifecycleProfiler::with_clock(AuditLifecycleContext::OpenRecoveryLatest, &clock);
            let mut profile = Some(&mut recorder);
            let result = lifecycle_cost!(profile, BackendOpen, fail(&clock, &events));
            assert_eq!(result, Err("preconstruction error"));
            assert_eq!(*events.borrow(), ["database", "entry"]);
            assert!(recorder.finish().is_err());
        }

        #[cfg(feature = "audit-boundary-test-profile")]
        #[test]
        fn audit_boundary_shared_recorder_counts_absence_and_residual() -> Result<(), String> {
            use lumin_model::audit_boundary_diagnostic::{
                AuditBoundaryContext as Context, AuditBoundaryCost as Cost,
            };
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
            for (context, expected) in Context::ALL.into_iter().zip(counts) {
                let clock = TestClock(Cell::new(0));
                let mut recorder = BoundaryProfiler::with_clock(context, &clock);
                for (cost, calls) in Cost::ALL.into_iter().zip(expected) {
                    for _ in 0..calls {
                        clock.0.set(clock.0.get() + 1);
                        recorder.begin(cost);
                        clock.0.set(clock.0.get() + 1);
                        recorder.end(cost);
                    }
                }
                clock.0.set(clock.0.get() + 7);
                let row = recorder.finish()?;
                let count: u64 = expected.into_iter().sum();
                assert_eq!(row.costs.map(|row| row.calls), expected);
                assert_eq!(row.elapsed_nanoseconds, count * 2 + 7);
                assert_eq!(row.self_nanoseconds, count + 7);
                for cost in row.costs {
                    assert_eq!(
                        cost.elapsed_nanoseconds,
                        (cost.calls > 0).then_some(cost.calls)
                    );
                }
            }
            Ok(())
        }
        #[cfg(feature = "audit-boundary-test-profile")]
        #[test]
        fn audit_boundary_invalid_observation_never_skips_product_cleanup() {
            use lumin_model::audit_boundary_diagnostic::{
                AuditBoundaryContext as Context, AuditBoundaryCost as Cost,
            };
            for failure in 0..8 {
                let clock = TestClock(Cell::new(10));
                let events = RefCell::new(Vec::new());
                let mut recorder = BoundaryProfiler::with_clock(Context::FinalizeRelease, &clock);
                match failure {
                    0 => {
                        recorder.begin(Cost::BackendOpen);
                        recorder.begin(Cost::StoreValidation);
                    }
                    1 => recorder.end(Cost::BackendOpen),
                    2 => {
                        recorder.begin(Cost::BackendOpen);
                        recorder.end(Cost::StoreValidation);
                    }
                    3 => {
                        recorder.begin(Cost::BackendOpen);
                        clock.0.set(0);
                        recorder.end(Cost::BackendOpen);
                    }
                    4 => {
                        recorder.begin(Cost::BackendOpen);
                        clock.0.set(u128::MAX);
                        recorder.end(Cost::BackendOpen);
                    }
                    5 => recorder.begin(Cost::DatabaseReturnTail),
                    6 => {
                        recorder.row.costs[Cost::BackendOpen as usize].calls = u64::MAX;
                        recorder.begin(Cost::BackendOpen);
                        recorder.end(Cost::BackendOpen);
                    }
                    _ => {
                        recorder.row.costs[Cost::BackendOpen as usize].elapsed_nanoseconds =
                            Some(u64::MAX);
                        recorder.begin(Cost::BackendOpen);
                        clock.0.set(11);
                        recorder.end(Cost::BackendOpen);
                    }
                }
                let mut profile = Some(&mut recorder);
                let result: Result<(), &str> = boundary_cost!(profile, DirectorySync, {
                    let _cleanup = Canary {
                        name: "cleanup",
                        events: &events,
                        clock: &clock,
                    };
                    events.borrow_mut().push("product");
                    Err("original product failure")
                });
                assert_eq!(result, Err("original product failure"));
                assert_eq!(*events.borrow(), ["product", "cleanup"]);
                assert!(recorder.finish().is_err());
            }
        }
    }
}
