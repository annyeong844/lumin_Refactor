//! W7's bounded API-cost observations. No clocks, resources or durable evidence.
use crate::audit_store_diagnostic::AuditStorePhase;

macro_rules! inventory {
    ($name:ident, $count:literal, {$($variant:ident => $text:literal),+ $(,)?}) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #[repr(usize)]
        pub enum $name { $($variant),+ }
        impl $name {
            pub const ALL: [Self; $count] = [$(Self::$variant),+];
            pub const fn name(self) -> &'static str {
                match self { $(Self::$variant => $text),+ }
            }
        }
    };
}

inventory!(AuditLifecycleContext, 8, {
    OpenRecoveryLatest => "open-recovery-latest",
    AttemptRecoverLatest => "attempt-recover-latest",
    AttemptDirectory => "attempt-directory",
    AttemptLatest => "attempt-latest",
    StagingCreate => "staging-create",
    StagingMove => "staging-move",
    PublishTerminal => "publish-terminal",
    FinalizeLatest => "finalize-latest",
});

impl AuditLifecycleContext {
    pub const fn phase(self) -> AuditStorePhase {
        match self {
            Self::OpenRecoveryLatest => AuditStorePhase::OpenRecoveryLatest,
            Self::AttemptRecoverLatest => AuditStorePhase::AttemptRecoverLatest,
            Self::AttemptDirectory => AuditStorePhase::AttemptDirectory,
            Self::AttemptLatest => AuditStorePhase::AttemptLatest,
            Self::StagingCreate => AuditStorePhase::StagingCreate,
            Self::StagingMove => AuditStorePhase::StagingMove,
            Self::PublishTerminal => AuditStorePhase::PublishTerminal,
            Self::FinalizeLatest => AuditStorePhase::FinalizeLatest,
        }
    }
}

inventory!(AuditLifecycleCost, 13, {
    NamespaceValidation => "namespace-validation",
    StoreHandleOpen => "store-handle-open",
    BackendOpen => "backend-open",
    StoreValidation => "store-validation",
    ReadAdmission => "read-admission",
    WriteAdmission => "write-admission",
    BackendCommit => "backend-commit",
    BackendAbort => "backend-abort",
    DatabaseExplicitDrop => "database-explicit-drop",
    DatabaseReturnTail => "database-return-tail",
    JsonWriteFlush => "json-write-flush",
    PublicationMove => "publication-move",
    DirectorySync => "directory-sync",
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuditLifecycleCostObservation {
    pub cost: AuditLifecycleCost,
    pub calls: u64,
    pub elapsed_nanoseconds: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuditLifecycleContextObservation {
    pub context: AuditLifecycleContext,
    pub calls: u64,
    pub elapsed_nanoseconds: u64,
    pub self_nanoseconds: u64,
    pub costs: [AuditLifecycleCostObservation; 13],
}

impl AuditLifecycleContextObservation {
    pub fn validate(&self) -> Result<(), String> {
        if self.calls != 1 {
            return Err("lifecycle context must be entered exactly once".to_owned());
        }
        let mut sum = 0_u64;
        for (row, cost) in self.costs.iter().zip(AuditLifecycleCost::ALL) {
            if row.cost != cost || (row.calls == 0) != row.elapsed_nanoseconds.is_none() {
                return Err("invalid lifecycle cost inventory or absent timing".to_owned());
            }
            sum = sum
                .checked_add(row.elapsed_nanoseconds.unwrap_or(0))
                .ok_or("lifecycle cost sum overflow")?;
        }
        if self.elapsed_nanoseconds.checked_sub(sum) != Some(self.self_nanoseconds) {
            return Err("invalid lifecycle residual".to_owned());
        }
        use AuditLifecycleCost::*;
        let count = |cost: AuditLifecycleCost| self.costs[cost as usize].calls;
        if count(NamespaceValidation) == 0
            || count(StoreValidation) == 0
            || count(StoreHandleOpen) != count(BackendOpen)
            || count(BackendOpen) == 0
            || Some(count(BackendOpen))
                != count(DatabaseExplicitDrop).checked_add(count(DatabaseReturnTail))
        {
            return Err("invalid lifecycle open/release/validation counts".to_owned());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AuditLifecycleTimings {
    contexts: [Option<AuditLifecycleContextObservation>; 8],
    error: Option<String>,
}

impl AuditLifecycleTimings {
    pub fn invalidate(&mut self, reason: impl Into<String>) {
        if self.error.is_none() {
            self.error = Some(reason.into());
        }
    }

    pub fn record(
        &mut self,
        root: AuditStorePhase,
        incoming: Result<AuditLifecycleContextObservation, String>,
    ) {
        let row = match incoming {
            Ok(row) => row,
            Err(error) => {
                self.invalidate(error);
                return;
            }
        };
        if row.context.phase().root() != root {
            self.invalidate("foreign lifecycle context root");
            return;
        }
        if let Err(error) = row.validate() {
            self.invalidate(error);
            return;
        }
        let index = row.context as usize;
        if self.contexts[index].is_some() || self.contexts[index + 1..].iter().any(Option::is_some)
        {
            self.invalidate("duplicate or out-of-order lifecycle context");
            return;
        }
        self.contexts[index] = Some(row);
    }

    pub fn merge_root(&mut self, root: AuditStorePhase, incoming: Self) {
        if let Some(error) = incoming.error {
            self.invalidate(error);
        }
        for row in incoming.contexts.into_iter().flatten() {
            self.record(root, Ok(row));
        }
    }

    pub fn observations(&self) -> Result<[AuditLifecycleContextObservation; 8], String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let rows = self
            .contexts
            .iter()
            .copied()
            .collect::<Option<Vec<_>>>()
            .ok_or("missing lifecycle context")?;
        rows.try_into()
            .map_err(|_| "invalid lifecycle inventory".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Authored from the frozen W7 API inventory, not inferred from a recording.
    const CONTEXTS: [&str; 8] = [
        "open-recovery-latest",
        "attempt-recover-latest",
        "attempt-directory",
        "attempt-latest",
        "staging-create",
        "staging-move",
        "publish-terminal",
        "finalize-latest",
    ];
    const COSTS: [&str; 13] = [
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
        "directory-sync",
    ];
    // Validation counts are positive fixture values, not claimed production counts.
    const FRESH: [[u64; 13]; 8] = [
        [1, 2, 2, 1, 1, 1, 0, 1, 0, 2, 0, 0, 0],
        [1, 2, 2, 1, 1, 1, 0, 1, 0, 2, 0, 0, 0],
        [1, 2, 2, 1, 0, 0, 0, 0, 2, 0, 0, 0, 1],
        [1, 2, 2, 1, 1, 1, 1, 0, 0, 2, 1, 1, 1],
        [1, 2, 2, 1, 0, 0, 0, 0, 2, 0, 0, 0, 1],
        [1, 2, 2, 1, 0, 0, 0, 0, 2, 0, 0, 1, 1],
        [1, 2, 2, 1, 0, 0, 0, 0, 2, 0, 1, 1, 1],
        [1, 3, 3, 1, 2, 1, 1, 0, 0, 3, 1, 1, 1],
    ];
    fn row(context: AuditLifecycleContext) -> AuditLifecycleContextObservation {
        AuditLifecycleContextObservation {
            context,
            calls: 1,
            elapsed_nanoseconds: 7,
            self_nanoseconds: 7,
            costs: AuditLifecycleCost::ALL.map(|cost| {
                let calls = FRESH[context as usize][cost as usize];
                AuditLifecycleCostObservation {
                    cost,
                    calls,
                    elapsed_nanoseconds: (calls != 0).then_some(0),
                }
            }),
        }
    }
    #[test]
    fn audit_lifecycle_exact_inventory_counts_and_root_merge() -> Result<(), String> {
        assert_eq!(
            AuditLifecycleContext::ALL.map(AuditLifecycleContext::name),
            CONTEXTS
        );
        assert_eq!(AuditLifecycleCost::ALL.map(AuditLifecycleCost::name), COSTS);
        let mut combined = AuditLifecycleTimings::default();
        for root in AuditStorePhase::ROOTS {
            let mut packet = AuditLifecycleTimings::default();
            for context in AuditLifecycleContext::ALL
                .into_iter()
                .filter(|c| c.phase().root() == root)
            {
                packet.record(root, Ok(row(context)));
            }
            combined.merge_root(root, packet);
        }
        assert_eq!(
            combined.observations()?,
            AuditLifecycleContext::ALL.map(row)
        );
        let totals = FRESH.into_iter().fold([0; 13], |mut sum, counts| {
            for (sum, count) in sum.iter_mut().zip(counts) {
                *sum += count;
            }
            sum
        });
        assert_eq!(&totals[4..], &[5, 4, 2, 2, 8, 9, 3, 4, 6]);
        assert_eq!(totals[2], 17);
        Ok(())
    }
    #[test]
    fn audit_lifecycle_missing_duplicate_wrong_root_order_and_sticky_errors() {
        assert!(AuditLifecycleTimings::default().observations().is_err());
        for index in 0..8 {
            let mut missing = AuditLifecycleTimings::default();
            for (i, context) in AuditLifecycleContext::ALL.into_iter().enumerate() {
                if i != index {
                    missing.record(context.phase().root(), Ok(row(context)));
                }
            }
            assert!(missing.observations().is_err());
        }
        let context = AuditLifecycleContext::OpenRecoveryLatest;
        let mut duplicate = AuditLifecycleTimings::default();
        duplicate.record(AuditStorePhase::StoreOpen, Ok(row(context)));
        duplicate.record(AuditStorePhase::StoreOpen, Ok(row(context)));
        assert!(duplicate.observations().is_err());
        let mut foreign = AuditLifecycleTimings::default();
        foreign.record(AuditStorePhase::AttemptBegin, Ok(row(context)));
        assert_eq!(
            foreign.observations().err().as_deref(),
            Some("foreign lifecycle context root")
        );
        let mut reordered = AuditLifecycleTimings::default();
        reordered.record(
            AuditStorePhase::AttemptBegin,
            Ok(row(AuditLifecycleContext::AttemptLatest)),
        );
        reordered.record(AuditStorePhase::StoreOpen, Ok(row(context)));
        assert!(reordered.observations().is_err());
        reordered.invalidate("later error");
        assert_eq!(
            reordered.observations().err().as_deref(),
            Some("duplicate or out-of-order lifecycle context")
        );
    }
    #[test]
    fn audit_lifecycle_closed_cost_shapes_and_checked_residuals() {
        let valid = row(AuditLifecycleContext::AttemptLatest);
        for index in 0..13 {
            let mut bad = valid;
            bad.costs.swap(index, (index + 1) % 13);
            assert!(bad.validate().is_err());
            bad = valid;
            bad.costs[index].elapsed_nanoseconds = if bad.costs[index].calls == 0 {
                Some(0)
            } else {
                None
            };
            assert!(bad.validate().is_err());
        }
        for cost in [
            AuditLifecycleCost::NamespaceValidation,
            AuditLifecycleCost::StoreValidation,
            AuditLifecycleCost::StoreHandleOpen,
            AuditLifecycleCost::BackendOpen,
            AuditLifecycleCost::DatabaseReturnTail,
        ] {
            let mut bad = valid;
            bad.costs[cost as usize].calls = 0;
            bad.costs[cost as usize].elapsed_nanoseconds = None;
            assert!(bad.validate().is_err());
        }
        let mut bad = valid;
        bad.costs[0].elapsed_nanoseconds = Some(u64::MAX);
        bad.costs[1].elapsed_nanoseconds = Some(1);
        assert!(bad.validate().is_err());
        bad = valid;
        bad.costs[0].elapsed_nanoseconds = Some(8);
        assert!(bad.validate().is_err());
        bad = valid;
        bad.self_nanoseconds = 6;
        assert!(bad.validate().is_err());
        bad = valid;
        bad.calls = 2;
        assert!(bad.validate().is_err());
    }
}
