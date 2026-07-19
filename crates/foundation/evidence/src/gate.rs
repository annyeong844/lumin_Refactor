use std::collections::{BTreeMap, BTreeSet};

use lumin_model::{
    AnalysisInputId, GateId, OperationId, PhysicalFileIdentity, ResolutionProfile,
    append_length_prefixed, digest_hex,
};
use serde::{Deserialize, Serialize};

use crate::{RepoPathProjection, RunEvidence};

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
    SemanticDeltaUnsupported,
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
    #[serde(default)]
    pub alias_closures: Vec<PhysicalAliasClosureRecord>,
    #[serde(default)]
    pub reconciled_transition_sequences: Vec<u64>,
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
        let mut signals = Vec::new();
        if evidence.dead_code_state() != lumin_model::CapabilityState::Complete
            || !evidence.limitations.is_empty()
        {
            signals.push(GateSignal::RequiredEvidenceIncomplete {
                limitation_count: evidence.limitations.len(),
            });
        }
        if !evidence.findings.is_empty() {
            signals.push(GateSignal::FindingWarnings {
                count: evidence.findings.len(),
            });
        }
        signals
    }

    pub fn closing_signals(
        baseline: &AnalysisSnapshot,
        current: &AnalysisSnapshot,
        protected_semantic_inputs: &[SemanticInputRecord],
        leased_write_set: &[WriteLease],
    ) -> (Vec<GateSignal>, Vec<RepoPathProjection>) {
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

        let mut signals = opening_signals(&current.evidence);
        if !protected.is_empty() {
            signals.push(GateSignal::ProtectedInputChanged { paths: protected });
        }
        if !unplanned.is_empty() {
            signals.push(GateSignal::UnplannedWrite { paths: unplanned });
        }
        if baseline.evidence != current.evidence {
            signals.push(GateSignal::SemanticDeltaUnsupported);
        }
        (signals, changed)
    }

    pub fn decision(signals: &[GateSignal]) -> GateDecision {
        if signals.iter().any(|signal| {
            matches!(
                signal,
                GateSignal::ProtectedInputChanged { .. }
                    | GateSignal::AnalysisContractChanged
                    | GateSignal::TransitionCatalogChanged
            )
        }) {
            return GateDecision::Stale;
        }
        if signals.iter().any(|signal| {
            matches!(
                signal,
                GateSignal::UnplannedWrite { .. } | GateSignal::TransitionChainBroken { .. }
            )
        }) {
            return GateDecision::Deny;
        }
        if signals.iter().any(|signal| {
            matches!(
                signal,
                GateSignal::RequiredEvidenceIncomplete { .. }
                    | GateSignal::AnalysisFailed { .. }
                    | GateSignal::DeclaredPathUnsupported { .. }
                    | GateSignal::WriteConflict { .. }
                    | GateSignal::ActiveTransitionPending { .. }
                    | GateSignal::SemanticDeltaUnsupported
            )
        }) {
            return GateDecision::Incomplete;
        }
        if signals
            .iter()
            .any(|signal| matches!(signal, GateSignal::FindingWarnings { .. }))
        {
            return GateDecision::AllowWithWarnings;
        }
        GateDecision::Allow
    }
}

fn sort_paths(paths: &mut Vec<RepoPathProjection>) {
    paths.sort();
    paths.dedup();
}
