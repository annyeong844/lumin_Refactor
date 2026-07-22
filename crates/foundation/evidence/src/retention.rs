use lumin_model::{
    GateId, OperationId, PinId, RepositoryId, RetentionContentIdentity, RetentionPlanId,
    RetentionTombstoneIdentity, RunId,
};
use serde::{Deserialize, Serialize};

use crate::OperationRecord;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RetentionDomain {
    Runs,
    Gates,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum RetentionPlanScope {
    Runs { before_unix_millis: u64 },
    Gates { terminal_before_unix_millis: u64 },
}

impl RetentionPlanScope {
    pub fn domain(&self) -> RetentionDomain {
        match self {
            Self::Runs { .. } => RetentionDomain::Runs,
            Self::Gates { .. } => RetentionDomain::Gates,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RetentionPlanState {
    Prepared,
    Pruning,
    Pruned,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RetentionItemKind {
    Attempt,
    Run,
    Gate,
    GateRevision,
    Finding,
    Evidence,
    Operation,
    Transition,
    PinOrReference,
    OrphanPayload,
    Tombstone,
}

impl RetentionItemKind {
    pub fn rank(self) -> u8 {
        match self {
            Self::Attempt => 0,
            Self::Run => 1,
            Self::Gate => 2,
            Self::GateRevision => 3,
            Self::Finding => 4,
            Self::Evidence => 5,
            Self::Operation => 6,
            Self::Transition => 7,
            Self::PinOrReference => 8,
            Self::OrphanPayload => 9,
            Self::Tombstone => 10,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionPlanItem {
    pub kind: RetentionItemKind,
    pub owning_sequence: u64,
    pub record_id: String,
    pub identity_sha256: String,
    pub byte_count: u64,
}

impl Ord for RetentionPlanItem {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.kind
            .rank()
            .cmp(&other.kind.rank())
            .then_with(|| self.owning_sequence.cmp(&other.owning_sequence))
            .then_with(|| self.record_id.cmp(&other.record_id))
    }
}

impl PartialOrd for RetentionPlanItem {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(
    tag = "reason",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum RetentionExclusionReason {
    LatestAttempt,
    LatestCompleted,
    ActivePin { pin_ids: Vec<PinId> },
    ActiveTransitionReference { gate_ids: Vec<GateId> },
    RetentionInProgress { plan_id: RetentionPlanId },
    ActiveGate,
    TerminalTimestampUnavailable,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionPlanExclusion {
    pub kind: RetentionItemKind,
    pub record_id: String,
    pub reason: RetentionExclusionReason,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RetentionRecoverableState {
    MovingPayloads,
    ReadyToCommit,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionPlanRecord {
    pub schema_version: String,
    pub repository_id: RepositoryId,
    pub plan_id: RetentionPlanId,
    pub content_identity: RetentionContentIdentity,
    pub scope: RetentionPlanScope,
    pub created_unix_millis: u128,
    pub catalog_revision: u64,
    pub state: RetentionPlanState,
    pub items: Vec<RetentionPlanItem>,
    pub exclusions: Vec<RetentionPlanExclusion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmation_operation_id: Option<OperationId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recoverable_state: Option<RetentionRecoverableState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tombstone_identity: Option<RetentionTombstoneIdentity>,
    #[serde(default)]
    pub physical_reclamation_pending: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum RetentionMutationResult {
    Prepared {
        plan_id: RetentionPlanId,
        content_identity: RetentionContentIdentity,
    },
    Pruning {
        plan_id: RetentionPlanId,
        recoverable_state: RetentionRecoverableState,
    },
    Pruned {
        plan_id: RetentionPlanId,
        tombstone_identity: RetentionTombstoneIdentity,
        physical_reclamation_pending: bool,
    },
    Stale {
        plan_id: RetentionPlanId,
        changed_inputs: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RetentionOperationKind {
    RunPin,
    RunUnpin,
    RunPrunePlan,
    RunPruneConfirm,
    GatePrunePlan,
    GatePruneConfirm,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RetentionOperationStatus {
    Pruning,
    Committed,
    Stale,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum RetentionOperationResult {
    PinCreated { pin: RunPinRecord },
    PinRemoved { pin_id: PinId, run_id: RunId },
    Retention { result: RetentionMutationResult },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionOperationRecord {
    pub schema_version: String,
    pub operation_id: OperationId,
    pub kind: RetentionOperationKind,
    pub request_digest: String,
    pub status: RetentionOperationStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_id: Option<RetentionPlanId>,
    pub result: RetentionOperationResult,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPinRecord {
    pub schema_version: String,
    pub pin_id: PinId,
    pub run_id: RunId,
    pub reason: String,
    pub created_unix_millis: u128,
    pub created_operation_id: OperationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_operation_id: Option<OperationId>,
}

impl RunPinRecord {
    pub fn is_active(&self) -> bool {
        self.removed_operation_id.is_none()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LifecycleOperationRecord {
    Gate(Box<OperationRecord>),
    Retention {
        operation: Box<RetentionOperationRecord>,
        current_physical_reclamation_pending: Option<bool>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionTombstoneEnvelope {
    pub record_kind: RetentionItemKind,
    pub record_id: String,
    pub plan_id: RetentionPlanId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recoverable_state: Option<RetentionRecoverableState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tombstone_identity: Option<RetentionTombstoneIdentity>,
    #[serde(default)]
    pub physical_reclamation_pending: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecordLookup<T> {
    Live(T),
    Pruning(RetentionTombstoneEnvelope),
    Pruned(RetentionTombstoneEnvelope),
}
