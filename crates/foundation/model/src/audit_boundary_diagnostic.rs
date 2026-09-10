//! W9's bounded API-cost observations. No clocks, resources or durable evidence.
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

inventory!(AuditBoundaryContext, 9, {
    OpenRecoveryEnter => "open-recovery-enter",
    OpenRecoveryExit => "open-recovery-exit",
    AttemptEnter => "attempt-enter",
    AttemptExit => "attempt-exit",
    PublishPrepareEnter => "publish-prepare-enter",
    PublishPrepareExit => "publish-prepare-exit",
    PublishFinalizeEnter => "publish-finalize-enter",
    FinalizeRelease => "finalize-release",
    PublishFinalizeExit => "publish-finalize-exit",
});

impl AuditBoundaryContext {
    pub const fn phase(self) -> AuditStorePhase {
        match self {
            Self::OpenRecoveryEnter => AuditStorePhase::OpenRecoveryEnter,
            Self::OpenRecoveryExit => AuditStorePhase::OpenRecoveryExit,
            Self::AttemptEnter => AuditStorePhase::AttemptEnter,
            Self::AttemptExit => AuditStorePhase::AttemptExit,
            Self::PublishPrepareEnter => AuditStorePhase::PublishPrepareEnter,
            Self::PublishPrepareExit => AuditStorePhase::PublishPrepareExit,
            Self::PublishFinalizeEnter => AuditStorePhase::PublishFinalizeEnter,
            Self::FinalizeRelease => AuditStorePhase::FinalizeRelease,
            Self::PublishFinalizeExit => AuditStorePhase::PublishFinalizeExit,
        }
    }
}

inventory!(AuditBoundaryCost, 19, {
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
    GuardPrevalidation => "guard-prevalidation",
    LifecycleLockAcquire => "lifecycle-lock-acquire",
    GuardConstruction => "guard-construction",
    LifecycleLockRelease => "lifecycle-lock-release",
    NativeStoreVerification => "native-store-verification",
    AttemptLockValidation => "attempt-lock-validation",
    AttemptLockDrop => "attempt-lock-drop",
    AttemptLockRemove => "attempt-lock-remove",
    DirectorySync => "directory-sync",
});

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuditBoundaryCostObservation {
    pub cost: AuditBoundaryCost,
    pub calls: u64,
    pub elapsed_nanoseconds: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuditBoundaryContextObservation {
    pub context: AuditBoundaryContext,
    pub calls: u64,
    pub elapsed_nanoseconds: u64,
    pub self_nanoseconds: u64,
    pub costs: [AuditBoundaryCostObservation; 19],
}

impl AuditBoundaryContextObservation {
    pub fn validate(&self) -> Result<(), String> {
        if self.calls != 1 {
            return Err("boundary context must be entered exactly once".to_owned());
        }
        let mut sum = 0_u64;
        for (row, cost) in self.costs.iter().zip(AuditBoundaryCost::ALL) {
            if row.cost != cost || (row.calls == 0) != row.elapsed_nanoseconds.is_none() {
                return Err("invalid boundary cost inventory or absent timing".to_owned());
            }
            sum = sum
                .checked_add(row.elapsed_nanoseconds.unwrap_or(0))
                .ok_or("boundary cost sum overflow")?;
        }
        if self.elapsed_nanoseconds.checked_sub(sum) != Some(self.self_nanoseconds) {
            return Err("invalid boundary residual".to_owned());
        }
        use AuditBoundaryCost::*;
        let count = |cost: AuditBoundaryCost| self.costs[cost as usize].calls;
        if count(NamespaceValidation) == 0
            || count(StoreValidation) == 0
            || count(StoreHandleOpen) != count(BackendOpen)
            || count(BackendOpen) == 0
            || Some(count(BackendOpen))
                != count(DatabaseExplicitDrop).checked_add(count(DatabaseReturnTail))
        {
            return Err("invalid boundary open/release/validation counts".to_owned());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AuditBoundaryTimings {
    contexts: [Option<AuditBoundaryContextObservation>; 9],
    error: Option<String>,
}

impl AuditBoundaryTimings {
    pub fn invalidate(&mut self, reason: impl Into<String>) {
        if self.error.is_none() {
            self.error = Some(reason.into());
        }
    }

    pub fn record(
        &mut self,
        root: AuditStorePhase,
        incoming: Result<AuditBoundaryContextObservation, String>,
    ) {
        let row = match incoming {
            Ok(row) => row,
            Err(error) => {
                self.invalidate(error);
                return;
            }
        };
        if row.context.phase().root() != root {
            self.invalidate("foreign boundary context root");
            return;
        }
        if let Err(error) = row.validate() {
            self.invalidate(error);
            return;
        }
        let index = row.context as usize;
        if self.contexts[index].is_some() || self.contexts[index + 1..].iter().any(Option::is_some)
        {
            self.invalidate("duplicate or out-of-order boundary context");
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

    pub fn observations(&self) -> Result<[AuditBoundaryContextObservation; 9], String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        let rows = self
            .contexts
            .iter()
            .copied()
            .collect::<Option<Vec<_>>>()
            .ok_or("missing boundary context")?;
        rows.try_into()
            .map_err(|_| "invalid boundary inventory".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const CONTEXTS: [&str; 9] = [
        "open-recovery-enter",
        "open-recovery-exit",
        "attempt-enter",
        "attempt-exit",
        "publish-prepare-enter",
        "publish-prepare-exit",
        "publish-finalize-enter",
        "finalize-release",
        "publish-finalize-exit",
    ];
    const COSTS: [&str; 19] = [
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
        "guard-prevalidation",
        "lifecycle-lock-acquire",
        "guard-construction",
        "lifecycle-lock-release",
        "native-store-verification",
        "attempt-lock-validation",
        "attempt-lock-drop",
        "attempt-lock-remove",
        "directory-sync",
    ];
    fn row(context: AuditBoundaryContext) -> AuditBoundaryContextObservation {
        AuditBoundaryContextObservation {
            context,
            calls: 1,
            elapsed_nanoseconds: 0,
            self_nanoseconds: 0,
            costs: AuditBoundaryCost::ALL.map(|cost| {
                use AuditBoundaryCost::*;
                let calls = match cost {
                    NamespaceValidation | StoreHandleOpen | BackendOpen | StoreValidation
                    | DatabaseExplicitDrop => 1,
                    _ => 0,
                };
                AuditBoundaryCostObservation {
                    cost,
                    calls,
                    elapsed_nanoseconds: (calls != 0).then_some(0),
                }
            }),
        }
    }
    #[test]
    fn audit_boundary_closed_inventory_and_root_order() -> Result<(), String> {
        assert_eq!(
            AuditBoundaryContext::ALL.map(AuditBoundaryContext::name),
            CONTEXTS
        );
        assert_eq!(AuditBoundaryCost::ALL.map(AuditBoundaryCost::name), COSTS);
        let mut combined = AuditBoundaryTimings::default();
        for root in AuditStorePhase::ROOTS {
            let mut packet = AuditBoundaryTimings::default();
            for context in AuditBoundaryContext::ALL
                .into_iter()
                .filter(|c| c.phase().root() == root)
            {
                packet.record(root, Ok(row(context)));
            }
            combined.merge_root(root, packet);
        }
        assert_eq!(combined.observations()?.map(|r| r.context.name()), CONTEXTS);
        Ok(())
    }
    #[test]
    fn audit_boundary_rejects_missing_foreign_reordered_duplicate_and_invalid_rows() {
        let root = AuditStorePhase::StoreOpen;
        assert!(AuditBoundaryTimings::default().observations().is_err());
        for mutation in 0..7 {
            let mut value = row(AuditBoundaryContext::OpenRecoveryEnter);
            match mutation {
                0 => value.calls = 0,
                1 => value.costs.swap(0, 1),
                2 => value.costs[0].elapsed_nanoseconds = None,
                3 => value.costs[4].elapsed_nanoseconds = Some(0),
                4 => value.self_nanoseconds = 1,
                5 => {
                    value.costs[0].elapsed_nanoseconds = Some(u64::MAX);
                    value.costs[1].elapsed_nanoseconds = Some(1);
                }
                _ => value.costs[8].calls = 2,
            }
            assert!(value.validate().is_err());
        }
        for rows in [
            [
                AuditBoundaryContext::OpenRecoveryEnter,
                AuditBoundaryContext::OpenRecoveryEnter,
            ],
            [
                AuditBoundaryContext::OpenRecoveryExit,
                AuditBoundaryContext::OpenRecoveryEnter,
            ],
            [
                AuditBoundaryContext::AttemptEnter,
                AuditBoundaryContext::OpenRecoveryExit,
            ],
        ] {
            let mut packet = AuditBoundaryTimings::default();
            for context in rows {
                packet.record(root, Ok(row(context)));
            }
            assert!(packet.error.is_some());
        }
    }
}
