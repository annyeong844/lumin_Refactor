use std::collections::{BTreeMap, BTreeSet};

use lumin_model::{
    AnalysisInputId, DeltaDimensionChange, DeltaFactFamily, GateDeltaClassification,
    GateDeltaRecord, GateId, OperationId, PhysicalFileIdentity, ResolutionProfile,
    append_length_prefixed, classify_lifecycle_deltas, digest_hex,
};
use serde::{Deserialize, Serialize};

use crate::{RepoPathProjection, RunEvidence, delta::lifecycle_delta_input};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GateDecision {
    Allow,
    AllowWithWarnings,
    Deny,
    Incomplete,
    Stale,
}

impl GateDecision {
    pub fn authorizes(self) -> bool {
        matches!(self, Self::Allow | Self::AllowWithWarnings)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GateEffect {
    Warn,
    Incomplete,
    Block,
    Stale,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GateLifecycle {
    Active,
    Rejected,
    Closed,
    Abandoned,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SemanticInputState {
    Source,
    ConfigPresent,
    Missing,
    NonRegular,
    Unreadable,
}

impl SemanticInputState {
    fn tag(self) -> u8 {
        match self {
            Self::Source => 1,
            Self::ConfigPresent => 2,
            Self::Missing => 3,
            Self::NonRegular => 4,
            Self::Unreadable => 5,
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticInputRecord {
    pub path: RepoPathProjection,
    pub state: SemanticInputState,
    pub payload_sha256: Option<String>,
    #[serde(default)]
    pub physical_identity: Option<PhysicalFileIdentity>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisSnapshot {
    pub analysis_input_id: AnalysisInputId,
    pub inputs: Vec<SemanticInputRecord>,
    pub evidence: RunEvidence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateAnalysisOptions {
    pub jobs: usize,
    pub resolution_profile: Option<ResolutionProfile>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WriteLeaseKind {
    ExistingFile,
    NewFile,
    Directory,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathPrefixIdentity {
    pub path: RepoPathProjection,
    pub physical_identity: PhysicalFileIdentity,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteLease {
    pub path: RepoPathProjection,
    pub kind: WriteLeaseKind,
    #[serde(default)]
    pub physical_identity: Option<PhysicalFileIdentity>,
    #[serde(default)]
    pub nearest_existing_parent: Option<RepoPathProjection>,
    #[serde(default)]
    pub prefix_identities: Vec<PathPrefixIdentity>,
}

impl WriteLease {
    pub fn covers(&self, candidate: &RepoPathProjection) -> bool {
        self.path.canonical == candidate.canonical
            || (self.kind == WriteLeaseKind::Directory
                && !self.path.components.is_empty()
                && candidate.components.starts_with(&self.path.components))
    }

    pub fn conflicts_with(&self, other: &Self) -> bool {
        let same_physical =
            self.physical_identity.is_some() && self.physical_identity == other.physical_identity;
        same_physical || self.covers(&other.path) || other.covers(&self.path)
    }

    pub fn conflicts_with_input(&self, input: &SemanticInputRecord) -> bool {
        self.covers(&input.path)
            || (self.physical_identity.is_some()
                && self.physical_identity == input.physical_identity)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhysicalAliasClosureRecord {
    pub physical_identity: PhysicalFileIdentity,
    pub members: Vec<RepoPathProjection>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeclaredPathUnsupportedReason {
    ReservedState,
    Missing,
    NonRegular,
    SymlinkOrAliasedPrefix,
    MultiplyLinked,
    NotAnalyzedSource,
    MissingParent,
    OutsideRoot,
    UnboundedDirectory,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum GateSignal {
    FindingWarnings {
        count: usize,
    },
    PreExistingAdverseFacts {
        count: usize,
    },
    RequiredEvidenceIncomplete {
        limitation_count: usize,
    },
    AnalysisFailed {
        detail: String,
    },
    DeclaredPathUnsupported {
        path: RepoPathProjection,
        reason: DeclaredPathUnsupportedReason,
    },
    WriteConflict {
        paths: Vec<RepoPathProjection>,
        gate_ids: Vec<GateId>,
    },
    SemanticInputConflict {
        paths: Vec<RepoPathProjection>,
        gate_ids: Vec<GateId>,
    },
    ProtectedInputChanged {
        paths: Vec<RepoPathProjection>,
    },
    AnalysisContractChanged,
    UnplannedWrite {
        paths: Vec<RepoPathProjection>,
    },
    ActiveTransitionPending {
        paths: Vec<RepoPathProjection>,
        gate_ids: Vec<GateId>,
    },
    TransitionChainBroken {
        sequence: u64,
    },
    TransitionCatalogChanged,
    AdverseFactIntroduced {
        count: usize,
    },
    AdverseFactRegressed {
        count: usize,
    },
    OpacityIntroduced {
        count: usize,
    },
    OpacityRegressed {
        count: usize,
    },
    LifecycleEvidenceRegressed {
        count: usize,
    },
    LifecycleDeltaIncomparable {
        count: usize,
    },
    LifecycleBaselineUnavailable {
        count: usize,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateBaseline {
    pub analysis_contract: String,
    pub snapshot: AnalysisSnapshot,
    #[serde(default)]
    pub protected_semantic_inputs: Vec<SemanticInputRecord>,
    #[serde(default)]
    pub transition_sequence: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateRevision {
    pub revision: u64,
    pub operation_id: OperationId,
    pub decision: GateDecision,
    pub signals: Vec<GateSignal>,
    pub changed_paths: Vec<RepoPathProjection>,
    pub snapshot: Option<AnalysisSnapshot>,
    pub protected_semantic_inputs: Vec<SemanticInputRecord>,
    #[serde(default)]
    pub alias_closures: Vec<PhysicalAliasClosureRecord>,
    #[serde(default)]
    pub reconciled_transition_sequences: Vec<u64>,
    #[serde(default)]
    pub deltas: Vec<GateDeltaRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateRecord {
    pub schema_version: String,
    pub gate_id: GateId,
    pub lifecycle: GateLifecycle,
    pub current_revision: u64,
    pub declared_write_set: Vec<RepoPathProjection>,
    #[serde(default)]
    pub leased_write_set: Vec<WriteLease>,
    #[serde(default)]
    pub alias_closures: Vec<PhysicalAliasClosureRecord>,
    #[serde(default)]
    pub transition_refs: Vec<u64>,
    pub analysis_options: GateAnalysisOptions,
    pub baseline: Option<GateBaseline>,
    pub protected_semantic_inputs: Vec<SemanticInputRecord>,
    pub revisions: Vec<GateRevision>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GateOperationKind {
    PreWrite,
    PostWrite,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GateOperationStatus {
    Pending,
    Committed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateOperationResult {
    pub operation_id: OperationId,
    pub request_digest: String,
    pub gate_id: GateId,
    pub revision: u64,
    pub lifecycle: GateLifecycle,
    pub decision: GateDecision,
    pub signals: Vec<GateSignal>,
    #[serde(default)]
    pub leased_write_set: Vec<WriteLease>,
    #[serde(default)]
    pub deltas: Vec<GateDeltaRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationRecord {
    pub schema_version: String,
    pub operation_id: OperationId,
    pub kind: GateOperationKind,
    pub request_digest: String,
    pub status: GateOperationStatus,
    pub gate_id: GateId,
    pub target_revision: u64,
    #[serde(default)]
    pub transition_sequence: u64,
    pub declared_write_set: Vec<RepoPathProjection>,
    #[serde(default)]
    pub leased_write_set: Vec<WriteLease>,
    pub semantic_read_reservations: Vec<RepoPathProjection>,
    pub analysis_options: Option<GateAnalysisOptions>,
    pub result: Option<GateOperationResult>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitionCapsule {
    pub gate_id: GateId,
    pub revision: u64,
    pub before_snapshot: AnalysisSnapshot,
    pub after_snapshot: AnalysisSnapshot,
    pub changed_paths: Vec<RepoPathProjection>,
    pub leased_write_set: Vec<WriteLease>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeTransition {
    pub sequence: u64,
    pub capsule: TransitionCapsule,
}

pub fn seal_analysis_snapshot(
    mut inputs: Vec<SemanticInputRecord>,
    evidence: RunEvidence,
) -> AnalysisSnapshot {
    inputs.sort();
    inputs.dedup();
    let mut framed = Vec::new();
    framed.extend_from_slice(&(inputs.len() as u64).to_be_bytes());
    for input in &inputs {
        append_length_prefixed(&mut framed, &input.path.canonical);
        framed.push(input.state.tag());
        match &input.payload_sha256 {
            Some(payload_sha256) => {
                framed.push(1);
                append_length_prefixed(&mut framed, payload_sha256.as_bytes());
            }
            None => framed.push(0),
        }
        match &input.physical_identity {
            Some(identity) => {
                framed.push(1);
                append_length_prefixed(&mut framed, &identity.canonical_bytes());
            }
            None => framed.push(0),
        }
    }
    AnalysisSnapshot {
        analysis_input_id: AnalysisInputId::from_string(format!(
            "analysis_input_{}",
            digest_hex(&framed)
        )),
        inputs,
        evidence,
    }
}

pub mod gate_policy {
    use super::*;

    pub fn opening_signals(evidence: &RunEvidence) -> Vec<GateSignal> {
        let delta_input = lifecycle_delta_input(evidence);
        let mut signals = Vec::new();
        if requires_complete_evidence(evidence, delta_input.required_evidence_gap_count) {
            signals.push(GateSignal::RequiredEvidenceIncomplete {
                limitation_count: delta_input.required_evidence_gap_count,
            });
        }
        if !evidence.findings.is_empty() {
            signals.push(GateSignal::FindingWarnings {
                count: evidence.findings.len(),
            });
        }
        if delta_input.advisory_limitation_count > 0 {
            signals.push(GateSignal::PreExistingAdverseFacts {
                count: delta_input.advisory_limitation_count,
            });
        }
        signals
    }

    pub fn closing_signals(
        baseline: &AnalysisSnapshot,
        current: &AnalysisSnapshot,
        protected_semantic_inputs: &[SemanticInputRecord],
        leased_write_set: &[WriteLease],
        newly_demanded_inputs: &[RepoPathProjection],
    ) -> (
        Vec<GateSignal>,
        Vec<RepoPathProjection>,
        Vec<GateDeltaRecord>,
    ) {
        let protected_set = protected_semantic_inputs
            .iter()
            .map(|input| input.path.canonical.as_slice())
            .collect::<BTreeSet<_>>();
        let baseline_by_path = baseline
            .inputs
            .iter()
            .map(|input| (input.path.canonical.as_slice(), input))
            .collect::<BTreeMap<_, _>>();
        let current_by_path = current
            .inputs
            .iter()
            .map(|input| (input.path.canonical.as_slice(), input))
            .collect::<BTreeMap<_, _>>();
        let newly_demanded_set = newly_demanded_inputs
            .iter()
            .map(|path| path.canonical.as_slice())
            .collect::<BTreeSet<_>>();
        let mut changed = Vec::new();
        let mut protected = Vec::new();
        let mut unplanned = Vec::new();

        for (path, baseline_input) in &baseline_by_path {
            if current_by_path.get(path).copied() != Some(*baseline_input) {
                changed.push(baseline_input.path.clone());
                if !leased_write_set
                    .iter()
                    .any(|lease| lease.covers(&baseline_input.path))
                    && protected_set.contains(path)
                {
                    protected.push(baseline_input.path.clone());
                } else if !leased_write_set
                    .iter()
                    .any(|lease| lease.covers(&baseline_input.path))
                {
                    unplanned.push(baseline_input.path.clone());
                }
            }
        }
        for (path, current_input) in &current_by_path {
            if !baseline_by_path.contains_key(path) {
                let demanded_read_only = newly_demanded_set.contains(path)
                    && !leased_write_set
                        .iter()
                        .any(|lease| lease.covers(&current_input.path));
                if demanded_read_only {
                    continue;
                }
                changed.push(current_input.path.clone());
                if !leased_write_set
                    .iter()
                    .any(|lease| lease.covers(&current_input.path))
                {
                    unplanned.push(current_input.path.clone());
                }
            }
        }
        sort_paths(&mut changed);
        sort_paths(&mut protected);
        sort_paths(&mut unplanned);

        let baseline_delta_input = lifecycle_delta_input(&baseline.evidence);
        let current_delta_input = lifecycle_delta_input(&current.evidence);
        let deltas = classify_lifecycle_deltas(
            Some(&baseline_delta_input.facts),
            &current_delta_input.facts,
        );
        let mut signals = lifecycle_delta_signals(&deltas);
        if requires_complete_evidence(
            &current.evidence,
            current_delta_input.required_evidence_gap_count,
        ) {
            signals.push(GateSignal::RequiredEvidenceIncomplete {
                limitation_count: current_delta_input.required_evidence_gap_count,
            });
        }
        if !protected.is_empty() {
            signals.push(GateSignal::ProtectedInputChanged { paths: protected });
        }
        if !unplanned.is_empty() {
            signals.push(GateSignal::UnplannedWrite { paths: unplanned });
        }
        (signals, changed, deltas)
    }

    fn requires_complete_evidence(evidence: &RunEvidence, required_gap_count: usize) -> bool {
        required_gap_count > 0
            || matches!(
                evidence.dead_code_state(),
                lumin_model::CapabilityState::Unavailable | lumin_model::CapabilityState::Failed
            )
    }

    pub fn decision(signals: &[GateSignal]) -> GateDecision {
        match signals.iter().filter_map(effect).max() {
            Some(GateEffect::Stale) => GateDecision::Stale,
            Some(GateEffect::Block) => GateDecision::Deny,
            Some(GateEffect::Incomplete) => GateDecision::Incomplete,
            Some(GateEffect::Warn) => GateDecision::AllowWithWarnings,
            None => GateDecision::Allow,
        }
    }

    pub fn effect(signal: &GateSignal) -> Option<GateEffect> {
        match signal {
            GateSignal::ProtectedInputChanged { .. }
            | GateSignal::AnalysisContractChanged
            | GateSignal::TransitionCatalogChanged => Some(GateEffect::Stale),
            GateSignal::UnplannedWrite { .. }
            | GateSignal::TransitionChainBroken { .. }
            | GateSignal::AdverseFactIntroduced { .. }
            | GateSignal::AdverseFactRegressed { .. } => Some(GateEffect::Block),
            GateSignal::RequiredEvidenceIncomplete { .. }
            | GateSignal::AnalysisFailed { .. }
            | GateSignal::DeclaredPathUnsupported { .. }
            | GateSignal::WriteConflict { .. }
            | GateSignal::SemanticInputConflict { .. }
            | GateSignal::ActiveTransitionPending { .. }
            | GateSignal::OpacityIntroduced { .. }
            | GateSignal::OpacityRegressed { .. }
            | GateSignal::LifecycleEvidenceRegressed { .. }
            | GateSignal::LifecycleDeltaIncomparable { .. }
            | GateSignal::LifecycleBaselineUnavailable { .. } => Some(GateEffect::Incomplete),
            GateSignal::FindingWarnings { .. } | GateSignal::PreExistingAdverseFacts { .. } => {
                Some(GateEffect::Warn)
            }
        }
    }

    fn lifecycle_delta_signals(deltas: &[GateDeltaRecord]) -> Vec<GateSignal> {
        let mut counts = DeltaSignalCounts::default();
        for delta in deltas {
            match &delta.classification {
                GateDeltaClassification::Introduced => {
                    if delta.key.family.blocks_when_adverse() {
                        counts.adverse_introduced += 1;
                    } else {
                        counts.opacity_introduced += 1;
                    }
                }
                GateDeltaClassification::Unchanged => {
                    counts.unchanged_facts += 1;
                }
                GateDeltaClassification::Regressed { changes } => {
                    classify_regressions(delta.key.family, changes, &mut counts);
                }
                GateDeltaClassification::ChangedIncomparable {
                    regressions,
                    incomparable_changes,
                    ..
                } => {
                    classify_regressions(delta.key.family, regressions, &mut counts);
                    if !incomparable_changes.is_empty() {
                        counts.incomparable += 1;
                    }
                }
                GateDeltaClassification::BaselineUnavailable => {
                    counts.baseline_unavailable += 1;
                }
                GateDeltaClassification::Improved { .. } | GateDeltaClassification::Resolved => {}
            }
        }
        counts.into_signals()
    }

    fn classify_regressions(
        family: DeltaFactFamily,
        changes: &[DeltaDimensionChange],
        counts: &mut DeltaSignalCounts,
    ) {
        let mut adverse = false;
        let mut opacity = false;
        let mut evidence = false;
        let mut unexpected = false;
        for change in changes {
            match change {
                DeltaDimensionChange::TargetAdded { .. }
                | DeltaDimensionChange::AffectedIdentityAdded { .. }
                | DeltaDimensionChange::OwnerPayloadRegressed { .. } => {
                    if family.blocks_when_adverse() {
                        adverse = true;
                    } else {
                        opacity = true;
                    }
                }
                DeltaDimensionChange::ConfidenceLowered { .. }
                | DeltaDimensionChange::GroundingLowered { .. } => evidence = true,
                DeltaDimensionChange::TargetRemoved { .. }
                | DeltaDimensionChange::AffectedIdentityRemoved { .. }
                | DeltaDimensionChange::ConfidenceRaised { .. }
                | DeltaDimensionChange::GroundingRaised { .. }
                | DeltaDimensionChange::EvidenceIdentityChanged { .. }
                | DeltaDimensionChange::OwnerPayloadImproved { .. }
                | DeltaDimensionChange::OwnerPayloadChanged { .. } => unexpected = true,
            }
        }
        counts.adverse_regressed += usize::from(adverse);
        counts.opacity_regressed += usize::from(opacity);
        counts.evidence_regressed += usize::from(evidence);
        counts.incomparable += usize::from(unexpected);
    }
}

#[derive(Default)]
struct DeltaSignalCounts {
    unchanged_facts: usize,
    adverse_introduced: usize,
    adverse_regressed: usize,
    opacity_introduced: usize,
    opacity_regressed: usize,
    evidence_regressed: usize,
    incomparable: usize,
    baseline_unavailable: usize,
}

impl DeltaSignalCounts {
    fn into_signals(self) -> Vec<GateSignal> {
        let mut signals = Vec::new();
        push_count(&mut signals, self.unchanged_facts, |count| {
            GateSignal::PreExistingAdverseFacts { count }
        });
        push_count(&mut signals, self.adverse_introduced, |count| {
            GateSignal::AdverseFactIntroduced { count }
        });
        push_count(&mut signals, self.adverse_regressed, |count| {
            GateSignal::AdverseFactRegressed { count }
        });
        push_count(&mut signals, self.opacity_introduced, |count| {
            GateSignal::OpacityIntroduced { count }
        });
        push_count(&mut signals, self.opacity_regressed, |count| {
            GateSignal::OpacityRegressed { count }
        });
        push_count(&mut signals, self.evidence_regressed, |count| {
            GateSignal::LifecycleEvidenceRegressed { count }
        });
        push_count(&mut signals, self.incomparable, |count| {
            GateSignal::LifecycleDeltaIncomparable { count }
        });
        push_count(&mut signals, self.baseline_unavailable, |count| {
            GateSignal::LifecycleBaselineUnavailable { count }
        });
        signals
    }
}

fn push_count(
    signals: &mut Vec<GateSignal>,
    count: usize,
    signal: impl FnOnce(usize) -> GateSignal,
) {
    if count > 0 {
        signals.push(signal(count));
    }
}

fn sort_paths(paths: &mut Vec<RepoPathProjection>) {
    paths.sort();
    paths.dedup();
}
