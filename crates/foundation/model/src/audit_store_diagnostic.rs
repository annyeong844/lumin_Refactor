//! W3's bounded, ephemeral store observations. Never durable or product evidence.
macro_rules! phases {
    ($($variant:ident => ($name:literal, $parent:expr)),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #[repr(usize)]
        pub enum AuditStorePhase { $($variant),+ }
        impl AuditStorePhase {
            pub const ALL: [Self; 52] = [$(Self::$variant),+];
            pub const fn name(self) -> &'static str {
                match self { $(Self::$variant => $name),+ }
            }
            pub const fn parent(self) -> Option<Self> {
                match self { $(Self::$variant => $parent),+ }
            }
        }
    };
}
phases! {
    StoreOpen => ("store-open", None),
    NamespaceOpen => ("namespace-open", Some(Self::StoreOpen)),
    BootstrapSetup => ("bootstrap-setup", Some(Self::NamespaceOpen)),
    BootstrapParents => ("bootstrap-parents", Some(Self::NamespaceOpen)),
    BootstrapMarker => ("bootstrap-marker", Some(Self::NamespaceOpen)),
    BootstrapStore => ("bootstrap-store", Some(Self::NamespaceOpen)),
    BootstrapValidation => ("bootstrap-validation", Some(Self::NamespaceOpen)),
    OpenRecovery => ("open-recovery", Some(Self::StoreOpen)),
    OpenRecoveryEnter => ("open-recovery-enter", Some(Self::OpenRecovery)),
    OpenRecoveryLatest => ("open-recovery-latest", Some(Self::OpenRecovery)),
    OpenRecoveryLeases => ("open-recovery-leases", Some(Self::OpenRecovery)),
    OpenRecoveryExit => ("open-recovery-exit", Some(Self::OpenRecovery)),
    AttemptBegin => ("attempt-begin", None),
    AttemptEnter => ("attempt-enter", Some(Self::AttemptBegin)),
    AttemptRecoverLatest => ("attempt-recover-latest", Some(Self::AttemptBegin)),
    AttemptRecoverLeases => ("attempt-recover-leases", Some(Self::AttemptBegin)),
    AttemptReserve => ("attempt-reserve", Some(Self::AttemptBegin)),
    AttemptLock => ("attempt-lock", Some(Self::AttemptBegin)),
    AttemptActivate => ("attempt-activate", Some(Self::AttemptBegin)),
    AttemptDirectory => ("attempt-directory", Some(Self::AttemptBegin)),
    AttemptEnvelope => ("attempt-envelope", Some(Self::AttemptBegin)),
    AttemptLatest => ("attempt-latest", Some(Self::AttemptBegin)),
    AttemptExit => ("attempt-exit", Some(Self::AttemptBegin)),
    StorePublish => ("store-publish", None),
    PublishPrepare => ("publish-prepare", Some(Self::StorePublish)),
    PublishPrepareEnter => ("publish-prepare-enter", Some(Self::PublishPrepare)),
    PublishSession => ("publish-session", Some(Self::PublishPrepare)),
    PublishEnvelope => ("publish-envelope", Some(Self::PublishPrepare)),
    PublishIdentities => ("publish-identities", Some(Self::PublishPrepare)),
    PublishPreflight => ("publish-preflight", Some(Self::PublishPrepare)),
    PublishDirectory => ("publish-directory", Some(Self::PublishPrepare)),
    StagingCreate => ("staging-create", Some(Self::PublishDirectory)),
    EvidenceWrite => ("evidence-write", Some(Self::PublishDirectory)),
    EvidenceCreate => ("evidence-create", Some(Self::EvidenceWrite)),
    EvidenceBeginWrite => ("evidence-begin-write", Some(Self::EvidenceWrite)),
    EvidenceRows => ("evidence-rows", Some(Self::EvidenceWrite)),
    EvidenceCommit => ("evidence-commit", Some(Self::EvidenceWrite)),
    EvidenceClose => ("evidence-close", Some(Self::EvidenceWrite)),
    EvidenceBindFlushHash => ("evidence-bind-flush-hash", Some(Self::PublishDirectory)),
    RunEnvelope => ("run-envelope", Some(Self::PublishDirectory)),
    StagingFlush => ("staging-flush", Some(Self::PublishDirectory)),
    StagingMove => ("staging-move", Some(Self::PublishDirectory)),
    PublishedValidation => ("published-validation", Some(Self::PublishDirectory)),
    PublishTerminal => ("publish-terminal", Some(Self::PublishPrepare)),
    PublishPrepareExit => ("publish-prepare-exit", Some(Self::PublishPrepare)),
    PublishFinalize => ("publish-finalize", Some(Self::StorePublish)),
    PublishFinalizeEnter => ("publish-finalize-enter", Some(Self::PublishFinalize)),
    FinalizeCandidate => ("finalize-candidate", Some(Self::PublishFinalize)),
    FinalizeCatalog => ("finalize-catalog", Some(Self::PublishFinalize)),
    FinalizeLatest => ("finalize-latest", Some(Self::PublishFinalize)),
    FinalizeRelease => ("finalize-release", Some(Self::PublishFinalize)),
    PublishFinalizeExit => ("publish-finalize-exit", Some(Self::PublishFinalize)),
}

impl AuditStorePhase {
    pub const ROOTS: [Self; 3] = [Self::StoreOpen, Self::AttemptBegin, Self::StorePublish];

    pub fn root(self) -> Self {
        let mut root = self;
        while let Some(parent) = root.parent() {
            root = parent;
        }
        root
    }

    pub const fn optional(self) -> bool {
        matches!(
            self,
            Self::BootstrapSetup
                | Self::BootstrapParents
                | Self::BootstrapMarker
                | Self::BootstrapStore
                | Self::BootstrapValidation
        )
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Aggregate {
    calls: u64,
    nanoseconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuditStoreTimings {
    aggregates: [Aggregate; 52],
    merged_roots: usize,
    error: Option<String>,
}

impl Default for AuditStoreTimings {
    fn default() -> Self {
        Self {
            aggregates: [Aggregate::default(); 52],
            merged_roots: 0,
            error: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuditStorePhaseObservation {
    pub phase: AuditStorePhase,
    pub calls: u64,
    pub elapsed_nanoseconds: Option<u64>,
    pub self_nanoseconds: Option<u64>,
}

impl AuditStoreTimings {
    pub fn invalidate(&mut self, reason: impl Into<String>) {
        if self.error.is_none() {
            self.error = Some(reason.into());
        }
    }

    pub fn record(&mut self, phase: AuditStorePhase, nanoseconds: u128) {
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

    /// One result per root, in source-call order. Never sum duplicate root packets.
    pub fn merge_root(&mut self, root: AuditStorePhase, incoming: Self) {
        if AuditStorePhase::ROOTS.get(self.merged_roots) != Some(&root)
            || incoming.merged_roots != 0
        {
            self.invalidate("duplicate or out-of-order store root");
            return;
        }
        if let Some(error) = incoming.error {
            self.invalidate(error);
        }
        for phase in AuditStorePhase::ALL {
            let row = incoming.aggregates[phase as usize];
            if phase.root() != root {
                if row != Aggregate::default() {
                    self.invalidate("foreign store subtree");
                }
            } else {
                let current = &mut self.aggregates[phase as usize];
                match (
                    current.calls.checked_add(row.calls),
                    current.nanoseconds.checked_add(row.nanoseconds),
                ) {
                    (Some(calls), Some(nanoseconds)) => *current = Aggregate { calls, nanoseconds },
                    _ => self.invalidate("store merge overflow"),
                }
            }
        }
        self.merged_roots += 1;
    }

    pub fn observations(&self) -> Result<[AuditStorePhaseObservation; 52], String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        if self.merged_roots != 3 {
            return Err("missing store root packet".to_owned());
        }
        let mut rows = AuditStorePhase::ALL.map(|phase| AuditStorePhaseObservation {
            phase,
            calls: 0,
            elapsed_nanoseconds: None,
            self_nanoseconds: None,
        });
        for phase in AuditStorePhase::ALL {
            let aggregate = self.aggregates[phase as usize];
            if aggregate.calls == 0 {
                if !phase.optional() {
                    return Err(format!("{} was not measured", phase.name()));
                }
                continue;
            }
            let children = AuditStorePhase::ALL
                .into_iter()
                .filter(|child| child.parent() == Some(phase))
                .try_fold(0_u64, |sum, child| {
                    sum.checked_add(self.aggregates[child as usize].nanoseconds)
                        .ok_or("store child timing overflow")
                })?;
            let residual = aggregate
                .nanoseconds
                .checked_sub(children)
                .ok_or("store children exceed parent")?;
            rows[phase as usize] = AuditStorePhaseObservation {
                phase,
                calls: aggregate.calls,
                elapsed_nanoseconds: Some(aggregate.nanoseconds),
                self_nanoseconds: Some(residual),
            };
        }
        Ok(rows)
    }
}
