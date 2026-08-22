use std::collections::{BTreeMap, BTreeSet};

use lumin_model::{
    AnalysisInputId, DeltaDimensionChange, DeltaFactFamily, DynamicImportTargetScope,
    GateBaselineObservationId, GateCloseObservationId, GateDeltaClassification, GateDeltaRecord,
    GateId, ImportMetaGlobTargetScope, Limitation, ObservationBinding, OperationId,
    PhysicalFileIdentity, ResolutionOutcome, ResolutionProfile, ResolutionProfileSource,
    SelectedResolutionProfile, UnsealedObservationReason, append_length_prefixed,
    classify_lifecycle_deltas, digest_hex,
};
use serde::{Deserialize, Deserializer, Serialize};

use crate::{RepoPathProjection, RunEvidence, delta::lifecycle_delta_input_for};

pub type GateObservationBinding = ObservationBinding<RepoPathProjection>;
pub const GATE_RECORD_SCHEMA_VERSION: &str = "lumin-gate.v2";
pub const GATE_OPERATION_SCHEMA_VERSION: &str = "lumin-operation.v1";

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
pub enum GateEffect {
    Warn,
    Incomplete,
    Block,
    Stale,
}

impl GateEffect {
    const fn precedence(self) -> u8 {
        match self {
            Self::Warn => 1,
            Self::Incomplete => 2,
            Self::Block => 3,
            Self::Stale => 4,
        }
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
    PathRedirect,
}

impl SemanticInputState {
    fn tag(self) -> u8 {
        match self {
            Self::Source => 1,
            Self::ConfigPresent => 2,
            Self::Missing => 3,
            Self::NonRegular => 4,
            Self::Unreadable => 5,
            Self::PathRedirect => 6,
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
    #[serde(default)]
    pub absence_parent: Option<PathPrefixIdentity>,
    #[serde(default)]
    pub physical_redirect_sha256: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisSnapshot {
    pub analysis_input_id: AnalysisInputId,
    pub inputs: Vec<SemanticInputRecord>,
    #[serde(default)]
    pub scan_invocation: ScanInvocationTier,
    #[serde(default)]
    pub entry_selections: Vec<EntrySelectionRecord>,
    pub evidence: RunEvidence,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyIntentRecord {
    pub path: RepoPathProjection,
    pub dependency: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanInvocationTier {
    #[serde(default)]
    pub includes: Vec<String>,
    #[serde(default)]
    pub excludes: Vec<String>,
    #[serde(default)]
    pub role_overrides: Vec<lumin_model::RoleOverride>,
    #[serde(default)]
    pub entries: Vec<RepoPathProjection>,
    #[serde(default)]
    pub dependency_intents: Vec<DependencyIntentRecord>,
    #[serde(default)]
    pub resolution_profile: Option<ResolutionProfile>,
}

impl ScanInvocationTier {
    pub fn validate_patterns(&self) -> Result<(), lumin_model::ScanPatternError> {
        for pattern in self
            .includes
            .iter()
            .chain(&self.excludes)
            .chain(self.role_overrides.iter().map(|rule| &rule.pattern))
        {
            lumin_model::validate_scan_pattern(pattern)?;
        }
        Ok(())
    }

    /// Append canonical length-prefixed framing of all tier fields for deterministic hashing.
    /// Uses exhaustive stable tags for each ScanRole variant.
    pub fn append_semantic_framing(&self, output: &mut Vec<u8>) {
        // Tag: struct present
        output.push(1);
        // Includes
        output.extend_from_slice(&(self.includes.len() as u64).to_be_bytes());
        for include in &self.includes {
            append_length_prefixed(output, include.as_bytes());
        }
        // Excludes
        output.extend_from_slice(&(self.excludes.len() as u64).to_be_bytes());
        for exclude in &self.excludes {
            append_length_prefixed(output, exclude.as_bytes());
        }
        // Role overrides
        output.extend_from_slice(&(self.role_overrides.len() as u64).to_be_bytes());
        for role_override in &self.role_overrides {
            append_length_prefixed(output, role_override.pattern.as_bytes());
            output.push(scan_role_tag(role_override.role));
        }
        // Entries (already sorted/deduped by construction)
        output.extend_from_slice(&(self.entries.len() as u64).to_be_bytes());
        for entry in &self.entries {
            append_length_prefixed(output, &entry.canonical);
        }
        // Dependency intents (already sorted/deduped by construction)
        output.extend_from_slice(&(self.dependency_intents.len() as u64).to_be_bytes());
        for intent in &self.dependency_intents {
            append_length_prefixed(output, &intent.path.canonical);
            append_length_prefixed(output, intent.dependency.as_bytes());
        }
        // Resolution profile
        match self.resolution_profile {
            Some(profile) => {
                output.push(1);
                append_length_prefixed(output, profile.as_str().as_bytes());
            }
            None => output.push(0),
        }
    }
}

/// Stable exhaustive tag for ScanRole used in framing. Must remain stable across versions.
fn scan_role_tag(role: lumin_model::ScanRole) -> u8 {
    match role {
        lumin_model::ScanRole::Test => 1,
        lumin_model::ScanRole::Production => 2,
        lumin_model::ScanRole::Generated => 3,
        lumin_model::ScanRole::Vendor => 4,
        lumin_model::ScanRole::Authored => 5,
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntrySelectionRecord {
    pub path: RepoPathProjection,
    pub source: lumin_model::EntrySource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<lumin_model::EntryUnavailableReason>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateAnalysisOptions {
    pub jobs: usize,
    pub resolution_profile: Option<ResolutionProfile>,
    #[serde(default)]
    pub scan_invocation: ScanInvocationTier,
}

pub fn pre_write_request_digest(
    declared_write_set: &[RepoPathProjection],
    scan_invocation: &ScanInvocationTier,
) -> String {
    let mut framed = Vec::new();
    append_length_prefixed(&mut framed, b"lumin-pre-write.v4");
    scan_invocation.append_semantic_framing(&mut framed);
    let mut paths = declared_write_set
        .iter()
        .map(|path| path.canonical.as_slice())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    framed.extend_from_slice(&(paths.len() as u64).to_be_bytes());
    for path in paths {
        append_length_prefixed(&mut framed, path);
    }
    digest_hex(&framed)
}

pub fn post_write_request_digest(gate_id: &GateId) -> String {
    let mut framed = Vec::new();
    append_length_prefixed(&mut framed, b"lumin-post-write.v2");
    append_length_prefixed(&mut framed, gate_id.as_str().as_bytes());
    digest_hex(&framed)
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

    pub fn conflicts_with_semantic_read(
        &self,
        path: &RepoPathProjection,
        physical_identity: Option<&PhysicalFileIdentity>,
        absence_parent: Option<&PathPrefixIdentity>,
    ) -> bool {
        self.covers(path)
            || (physical_identity.is_some() && self.physical_identity.as_ref() == physical_identity)
            || absence_parent.is_some_and(|parent| {
                self.physical_identity.as_ref() == Some(&parent.physical_identity)
                    || self.enters_missing_semantic_branch(path, parent)
            })
    }

    fn enters_missing_semantic_branch(
        &self,
        path: &RepoPathProjection,
        parent: &PathPrefixIdentity,
    ) -> bool {
        let parent_components = &parent.path.components;
        if !path.components.starts_with(parent_components)
            || !self.path.components.starts_with(parent_components)
            || !self.prefix_identities.iter().any(|prefix| prefix == parent)
        {
            return false;
        }
        let first_missing_component = path.components.get(parent_components.len());
        first_missing_component.is_some()
            && self.path.components.get(parent_components.len()) == first_missing_component
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticReadReservationBinding {
    pub path: RepoPathProjection,
    #[serde(default)]
    pub physical_identity: Option<PhysicalFileIdentity>,
    #[serde(default)]
    pub absence_parent: Option<PathPrefixIdentity>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhysicalAliasClosureRecord {
    pub physical_identity: PhysicalFileIdentity,
    pub members: Vec<RepoPathProjection>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActualWriteSet {
    pub paths: Vec<RepoPathProjection>,
    #[serde(default)]
    pub baseline_alias_closures: Vec<PhysicalAliasClosureRecord>,
    #[serde(default)]
    pub current_alias_closures: Vec<PhysicalAliasClosureRecord>,
}

pub fn derive_protected_semantic_inputs(
    snapshot: &AnalysisSnapshot,
    leases: &[WriteLease],
) -> Vec<SemanticInputRecord> {
    let source_paths = snapshot
        .inputs
        .iter()
        .filter(|input| input.state == SemanticInputState::Source)
        .map(|input| input.path.clone())
        .collect::<BTreeSet<_>>();
    let paths_by_id = snapshot
        .evidence
        .source_contexts
        .iter()
        .map(|source| (source.source_id.clone(), source.path.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut adjacency = snapshot
        .evidence
        .source_contexts
        .iter()
        .map(|source| (source.path.clone(), BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    for resolution in &snapshot.evidence.resolutions {
        let ResolutionOutcome::Internal { target } = &resolution.outcome else {
            continue;
        };
        let Some(importer) = paths_by_id.get(&resolution.source_use.importer) else {
            continue;
        };
        let Some(target) = paths_by_id.get(target) else {
            continue;
        };
        adjacency
            .entry(importer.clone())
            .or_default()
            .insert(target.clone());
        adjacency
            .entry(target.clone())
            .or_default()
            .insert(importer.clone());
    }
    for limitation in &snapshot.evidence.limitations {
        let (source_id, candidates) = match limitation {
            Limitation::DynamicImportNonLiteral {
                source_id,
                candidates,
                target_scope: DynamicImportTargetScope::ExplicitTargets,
                ..
            }
            | Limitation::ImportMetaGlobUnsupported {
                source_id,
                candidates,
                target_scope: ImportMetaGlobTargetScope::ExplicitTargets,
                ..
            } => (source_id, candidates),
            _ => continue,
        };
        let Some(importer) = paths_by_id.get(source_id) else {
            continue;
        };
        for candidate in candidates {
            let Some(target) = paths_by_id.get(candidate) else {
                continue;
            };
            adjacency
                .entry(importer.clone())
                .or_default()
                .insert(target.clone());
            adjacency
                .entry(target.clone())
                .or_default()
                .insert(importer.clone());
        }
    }

    let protect_all_sources = leases.iter().any(|lease| {
        matches!(
            lease.kind,
            WriteLeaseKind::NewFile | WriteLeaseKind::Directory
        )
    });
    let mut selected = if protect_all_sources {
        source_paths.clone()
    } else {
        leases
            .iter()
            .filter(|lease| lease.kind == WriteLeaseKind::ExistingFile)
            .filter_map(|lease| {
                source_paths
                    .iter()
                    .find(|path| path.canonical == lease.path.canonical)
                    .cloned()
            })
            .collect::<BTreeSet<_>>()
    };
    let mut frontier = selected.iter().cloned().collect::<Vec<_>>();
    while let Some(path) = frontier.pop() {
        let Some(neighbors) = adjacency.get(&path) else {
            continue;
        };
        for neighbor in neighbors {
            if selected.insert(neighbor.clone()) {
                frontier.push(neighbor.clone());
            }
        }
    }
    let selected_paths = selected
        .iter()
        .map(|path| path.canonical.as_slice())
        .collect::<BTreeSet<_>>();
    let mut protected = snapshot
        .inputs
        .iter()
        .filter(|input| {
            input.physical_redirect_sha256.is_some()
                || !source_paths.contains(&input.path)
                || selected_paths.contains(input.path.canonical.as_slice())
        })
        .cloned()
        .collect::<Vec<_>>();
    protected.sort();
    protected.dedup();
    protected
}

pub struct GateBaselineObservationInput<'a> {
    pub catalog_revision: u64,
    pub transition_sequence: u64,
    pub analysis_contract: &'a str,
    pub analysis_input_id: &'a AnalysisInputId,
    pub evidence_payload_sha256: &'a str,
    pub signals: &'a [GateSignal],
    pub declared_write_set: &'a [RepoPathProjection],
    pub leased_write_set: &'a [WriteLease],
    pub alias_closures: &'a [PhysicalAliasClosureRecord],
    pub protected_semantic_inputs: &'a [SemanticInputRecord],
}

pub fn derive_gate_baseline_observation_id(
    input: GateBaselineObservationInput<'_>,
) -> GateBaselineObservationId {
    let mut framed = Vec::new();
    append_length_prefixed(&mut framed, b"lumin-gate-baseline-observation.v3");
    framed.extend_from_slice(&input.catalog_revision.to_be_bytes());
    framed.extend_from_slice(&input.transition_sequence.to_be_bytes());
    append_length_prefixed(&mut framed, input.analysis_contract.as_bytes());
    append_length_prefixed(&mut framed, input.analysis_input_id.as_str().as_bytes());
    append_length_prefixed(&mut framed, input.evidence_payload_sha256.as_bytes());
    append_observation_signals(&mut framed, input.signals);
    append_observation_paths(&mut framed, input.declared_write_set);
    append_observation_write_leases(&mut framed, input.leased_write_set);
    append_observation_alias_closures(&mut framed, input.alias_closures);
    append_observation_semantic_inputs(&mut framed, input.protected_semantic_inputs);
    GateBaselineObservationId::from_string(format!(
        "gate_baseline_observation_{}",
        digest_hex(&framed)
    ))
}

pub struct GateCloseObservationInput<'a> {
    pub gate_id: &'a GateId,
    pub opening_observation_id: &'a GateBaselineObservationId,
    pub opening_analysis_contract: &'a str,
    pub prior_revision: u64,
    pub catalog_revision: u64,
    pub analysis_input_id: &'a AnalysisInputId,
    pub evidence_payload_sha256: &'a str,
    pub signals: &'a [GateSignal],
    pub leased_write_set: &'a [WriteLease],
    pub protected_semantic_inputs: &'a [SemanticInputRecord],
    pub changed_paths: &'a [RepoPathProjection],
    pub actual_write_set: &'a ActualWriteSet,
    pub alias_closures: &'a [PhysicalAliasClosureRecord],
    pub reconciled_transition_sequences: &'a [u64],
}

pub fn derive_gate_close_observation_id(
    input: GateCloseObservationInput<'_>,
) -> GateCloseObservationId {
    let mut framed = Vec::new();
    append_length_prefixed(&mut framed, b"lumin-gate-close-observation.v3");
    append_length_prefixed(&mut framed, input.gate_id.as_str().as_bytes());
    append_length_prefixed(
        &mut framed,
        input.opening_observation_id.as_str().as_bytes(),
    );
    append_length_prefixed(&mut framed, input.opening_analysis_contract.as_bytes());
    framed.extend_from_slice(&input.prior_revision.to_be_bytes());
    framed.extend_from_slice(&input.catalog_revision.to_be_bytes());
    append_length_prefixed(&mut framed, input.analysis_input_id.as_str().as_bytes());
    append_length_prefixed(&mut framed, input.evidence_payload_sha256.as_bytes());
    append_observation_signals(&mut framed, input.signals);
    append_observation_write_leases(&mut framed, input.leased_write_set);
    append_observation_semantic_inputs(&mut framed, input.protected_semantic_inputs);
    append_observation_paths(&mut framed, input.changed_paths);
    append_observation_actual_write_set(&mut framed, input.actual_write_set);
    append_observation_alias_closures(&mut framed, input.alias_closures);
    let mut sequences = input.reconciled_transition_sequences.to_vec();
    sequences.sort_unstable();
    sequences.dedup();
    framed.extend_from_slice(&(sequences.len() as u64).to_be_bytes());
    for sequence in sequences {
        framed.extend_from_slice(&sequence.to_be_bytes());
    }
    GateCloseObservationId::from_string(format!("gate_close_observation_{}", digest_hex(&framed)))
}

fn append_observation_actual_write_set(output: &mut Vec<u8>, actual: &ActualWriteSet) {
    append_observation_paths(output, &actual.paths);
    append_observation_alias_closures(output, &actual.baseline_alias_closures);
    append_observation_alias_closures(output, &actual.current_alias_closures);
}

fn append_observation_paths(output: &mut Vec<u8>, paths: &[RepoPathProjection]) {
    let mut paths = paths
        .iter()
        .map(|path| path.canonical.as_slice())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    output.extend_from_slice(&(paths.len() as u64).to_be_bytes());
    for path in paths {
        append_length_prefixed(output, path);
    }
}

fn append_observation_write_leases(output: &mut Vec<u8>, leases: &[WriteLease]) {
    let mut leases = leases.to_vec();
    leases.sort();
    leases.dedup();
    output.extend_from_slice(&(leases.len() as u64).to_be_bytes());
    for lease in leases {
        append_length_prefixed(output, &lease.path.canonical);
        output.push(match lease.kind {
            WriteLeaseKind::ExistingFile => 1,
            WriteLeaseKind::NewFile => 2,
            WriteLeaseKind::Directory => 3,
        });
        append_observation_physical_identity(output, lease.physical_identity.as_ref());
        match lease.nearest_existing_parent {
            Some(parent) => {
                output.push(1);
                append_length_prefixed(output, &parent.canonical);
            }
            None => output.push(0),
        }
        let mut prefix_identities = lease.prefix_identities;
        prefix_identities.sort();
        prefix_identities.dedup();
        output.extend_from_slice(&(prefix_identities.len() as u64).to_be_bytes());
        for prefix in prefix_identities {
            append_length_prefixed(output, &prefix.path.canonical);
            append_length_prefixed(output, &prefix.physical_identity.canonical_bytes());
        }
    }
}

fn append_observation_alias_closures(
    output: &mut Vec<u8>,
    closures: &[PhysicalAliasClosureRecord],
) {
    let mut closures = closures.to_vec();
    closures.sort();
    closures.dedup();
    output.extend_from_slice(&(closures.len() as u64).to_be_bytes());
    for closure in closures {
        append_length_prefixed(output, &closure.physical_identity.canonical_bytes());
        append_observation_paths(output, &closure.members);
    }
}

fn append_observation_semantic_inputs(output: &mut Vec<u8>, inputs: &[SemanticInputRecord]) {
    let mut inputs = inputs.to_vec();
    inputs.sort();
    inputs.dedup();
    output.extend_from_slice(&(inputs.len() as u64).to_be_bytes());
    for input in inputs {
        append_length_prefixed(output, &input.path.canonical);
        output.push(input.state.tag());
        append_observation_optional_bytes(
            output,
            input.payload_sha256.as_deref().map(str::as_bytes),
        );
        append_observation_physical_identity(output, input.physical_identity.as_ref());
        match input.absence_parent {
            Some(parent) => {
                output.push(1);
                append_length_prefixed(output, &parent.path.canonical);
                append_length_prefixed(output, &parent.physical_identity.canonical_bytes());
            }
            None => output.push(0),
        }
        append_observation_optional_bytes(
            output,
            input.physical_redirect_sha256.as_deref().map(str::as_bytes),
        );
    }
}

fn append_observation_signals(output: &mut Vec<u8>, signals: &[GateSignal]) {
    output.extend_from_slice(&(signals.len() as u64).to_be_bytes());
    for signal in signals {
        match signal {
            GateSignal::FindingWarnings { count } => {
                output.push(1);
                output.extend_from_slice(&(*count as u64).to_be_bytes());
            }
            GateSignal::PreExistingAdverseFacts { count } => {
                output.push(2);
                output.extend_from_slice(&(*count as u64).to_be_bytes());
            }
            GateSignal::RequiredEvidenceIncomplete { limitation_count } => {
                output.push(3);
                output.extend_from_slice(&(*limitation_count as u64).to_be_bytes());
            }
            GateSignal::AnalysisFailed { detail } => {
                output.push(4);
                append_length_prefixed(output, detail.as_bytes());
            }
            GateSignal::DeclaredPathUnsupported { path, reason } => {
                output.push(5);
                append_length_prefixed(output, &path.canonical);
                output.push(declared_path_unsupported_reason_tag(*reason));
            }
            GateSignal::WriteConflict { paths, gate_ids } => {
                output.push(6);
                append_observation_signal_paths(output, paths);
                append_observation_gate_ids(output, gate_ids);
            }
            GateSignal::SemanticInputConflict { paths, gate_ids } => {
                output.push(7);
                append_observation_signal_paths(output, paths);
                append_observation_gate_ids(output, gate_ids);
            }
            GateSignal::SemanticReadClosureIncomplete { paths } => {
                output.push(8);
                append_observation_signal_paths(output, paths);
            }
            GateSignal::ProtectedInputChanged { paths } => {
                output.push(9);
                append_observation_signal_paths(output, paths);
            }
            GateSignal::AnalysisContractChanged => output.push(10),
            GateSignal::UnplannedWrite { paths } => {
                output.push(11);
                append_observation_signal_paths(output, paths);
            }
            GateSignal::ActiveTransitionPending { paths, gate_ids } => {
                output.push(12);
                append_observation_signal_paths(output, paths);
                append_observation_gate_ids(output, gate_ids);
            }
            GateSignal::TransitionChainBroken { sequence } => {
                output.push(13);
                output.extend_from_slice(&sequence.to_be_bytes());
            }
            GateSignal::TransitionCatalogChanged => output.push(14),
            GateSignal::AdverseFactIntroduced { count } => {
                output.push(15);
                output.extend_from_slice(&(*count as u64).to_be_bytes());
            }
            GateSignal::AdverseFactRegressed { count } => {
                output.push(16);
                output.extend_from_slice(&(*count as u64).to_be_bytes());
            }
            GateSignal::OpacityIntroduced { count } => {
                output.push(17);
                output.extend_from_slice(&(*count as u64).to_be_bytes());
            }
            GateSignal::OpacityRegressed { count } => {
                output.push(18);
                output.extend_from_slice(&(*count as u64).to_be_bytes());
            }
            GateSignal::LifecycleEvidenceRegressed { count } => {
                output.push(19);
                output.extend_from_slice(&(*count as u64).to_be_bytes());
            }
            GateSignal::LifecycleDeltaIncomparable { count } => {
                output.push(20);
                output.extend_from_slice(&(*count as u64).to_be_bytes());
            }
            GateSignal::LifecycleBaselineUnavailable { count } => {
                output.push(21);
                output.extend_from_slice(&(*count as u64).to_be_bytes());
            }
        }
    }
}

fn append_observation_signal_paths(output: &mut Vec<u8>, paths: &[RepoPathProjection]) {
    output.extend_from_slice(&(paths.len() as u64).to_be_bytes());
    for path in paths {
        append_length_prefixed(output, &path.canonical);
    }
}

fn append_observation_gate_ids(output: &mut Vec<u8>, gate_ids: &[GateId]) {
    output.extend_from_slice(&(gate_ids.len() as u64).to_be_bytes());
    for gate_id in gate_ids {
        append_length_prefixed(output, gate_id.as_str().as_bytes());
    }
}

fn declared_path_unsupported_reason_tag(reason: DeclaredPathUnsupportedReason) -> u8 {
    match reason {
        DeclaredPathUnsupportedReason::ReservedState => 1,
        DeclaredPathUnsupportedReason::Missing => 2,
        DeclaredPathUnsupportedReason::NonRegular => 3,
        DeclaredPathUnsupportedReason::SymlinkOrAliasedPrefix => 4,
        DeclaredPathUnsupportedReason::MultiplyLinked => 5,
        DeclaredPathUnsupportedReason::NotAnalyzedSource => 6,
        DeclaredPathUnsupportedReason::MissingParent => 7,
        DeclaredPathUnsupportedReason::OutsideRoot => 8,
        DeclaredPathUnsupportedReason::UnboundedDirectory => 9,
    }
}

fn append_observation_physical_identity(
    output: &mut Vec<u8>,
    identity: Option<&PhysicalFileIdentity>,
) {
    match identity {
        Some(identity) => {
            output.push(1);
            append_length_prefixed(output, &identity.canonical_bytes());
        }
        None => output.push(0),
    }
}

fn append_observation_optional_bytes(output: &mut Vec<u8>, value: Option<&[u8]>) {
    match value {
        Some(value) => {
            output.push(1);
            append_length_prefixed(output, value);
        }
        None => output.push(0),
    }
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
    SemanticReadClosureIncomplete {
        paths: Vec<RepoPathProjection>,
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
pub struct UnsealedGateObservationInputs {
    #[serde(default)]
    pub attempted_write_leases: Vec<WriteLease>,
    #[serde(default)]
    pub attempted_semantic_inputs: Vec<SemanticReadReservationBinding>,
    #[serde(default)]
    pub last_complete_read_set: Vec<RepoPathProjection>,
}

impl UnsealedGateObservationInputs {
    pub fn new(
        mut attempted_write_leases: Vec<WriteLease>,
        mut attempted_semantic_inputs: Vec<SemanticReadReservationBinding>,
        mut last_complete_read_set: Vec<RepoPathProjection>,
    ) -> Self {
        attempted_write_leases.sort();
        attempted_write_leases.dedup();
        attempted_semantic_inputs.sort();
        attempted_semantic_inputs.dedup();
        last_complete_read_set.sort();
        last_complete_read_set.dedup();
        Self {
            attempted_write_leases,
            attempted_semantic_inputs,
            last_complete_read_set,
        }
    }

    pub fn is_canonical(&self) -> bool {
        Self::new(
            self.attempted_write_leases.clone(),
            self.attempted_semantic_inputs.clone(),
            self.last_complete_read_set.clone(),
        ) == *self
    }
}

pub fn derive_unsealed_gate_observation_binding(
    primary_paths: &[RepoPathProjection],
    inputs: &UnsealedGateObservationInputs,
    signals: &[GateSignal],
) -> GateObservationBinding {
    let reason = signals
        .iter()
        .find_map(unsealed_observation_reason)
        .unwrap_or(UnsealedObservationReason::ObservationDomainUnbounded);
    let mut attempted_domain = primary_paths.to_vec();
    attempted_domain.extend(
        inputs
            .attempted_write_leases
            .iter()
            .map(|lease| lease.path.clone()),
    );
    attempted_domain.extend(
        inputs
            .attempted_semantic_inputs
            .iter()
            .map(|input| input.path.clone()),
    );
    attempted_domain.sort();
    attempted_domain.dedup();
    let mut conflicting_or_unbounded_inputs = observation_signal_paths(signals);
    if conflicting_or_unbounded_inputs.is_empty() {
        conflicting_or_unbounded_inputs = attempted_domain.clone();
    }
    ObservationBinding::Unsealed {
        reason,
        attempted_domain,
        last_complete_read_set: inputs.last_complete_read_set.clone(),
        conflicting_or_unbounded_inputs,
    }
}

fn unsealed_observation_reason(signal: &GateSignal) -> Option<UnsealedObservationReason> {
    match signal {
        GateSignal::WriteConflict { .. } => Some(UnsealedObservationReason::AdmissionConflict),
        GateSignal::SemanticInputConflict { .. } => {
            Some(UnsealedObservationReason::SemanticReadConflict)
        }
        GateSignal::SemanticReadClosureIncomplete { .. } => {
            Some(UnsealedObservationReason::SemanticReadClosureIncomplete)
        }
        GateSignal::AnalysisFailed { .. } | GateSignal::AnalysisContractChanged => {
            Some(UnsealedObservationReason::AnalysisFailed)
        }
        GateSignal::DeclaredPathUnsupported { .. } => {
            Some(UnsealedObservationReason::DeclaredPathUnsupported)
        }
        GateSignal::ProtectedInputChanged { .. } => {
            Some(UnsealedObservationReason::ProtectedInputChanged)
        }
        GateSignal::TransitionCatalogChanged => {
            Some(UnsealedObservationReason::TransitionCatalogChanged)
        }
        GateSignal::UnplannedWrite { .. } => Some(UnsealedObservationReason::UnplannedWrite),
        GateSignal::RequiredEvidenceIncomplete { .. }
        | GateSignal::ActiveTransitionPending { .. }
        | GateSignal::TransitionChainBroken { .. }
        | GateSignal::LifecycleDeltaIncomparable { .. }
        | GateSignal::LifecycleBaselineUnavailable { .. } => {
            Some(UnsealedObservationReason::ObservationDomainUnbounded)
        }
        GateSignal::FindingWarnings { .. }
        | GateSignal::PreExistingAdverseFacts { .. }
        | GateSignal::AdverseFactIntroduced { .. }
        | GateSignal::AdverseFactRegressed { .. }
        | GateSignal::OpacityIntroduced { .. }
        | GateSignal::OpacityRegressed { .. }
        | GateSignal::LifecycleEvidenceRegressed { .. } => None,
    }
}

fn observation_signal_paths(signals: &[GateSignal]) -> Vec<RepoPathProjection> {
    let mut paths = Vec::new();
    for signal in signals {
        match signal {
            GateSignal::DeclaredPathUnsupported { path, .. } => paths.push(path.clone()),
            GateSignal::WriteConflict {
                paths: signal_paths,
                ..
            }
            | GateSignal::SemanticInputConflict {
                paths: signal_paths,
                ..
            }
            | GateSignal::SemanticReadClosureIncomplete {
                paths: signal_paths,
            }
            | GateSignal::ProtectedInputChanged {
                paths: signal_paths,
            }
            | GateSignal::UnplannedWrite {
                paths: signal_paths,
            }
            | GateSignal::ActiveTransitionPending {
                paths: signal_paths,
                ..
            } => paths.extend(signal_paths.iter().cloned()),
            _ => {}
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateBaseline {
    pub observation_id: GateBaselineObservationId,
    pub catalog_revision: u64,
    pub analysis_contract: String,
    pub snapshot: AnalysisSnapshot,
    pub leased_write_set: Vec<WriteLease>,
    pub alias_closures: Vec<PhysicalAliasClosureRecord>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub committed_unix_millis: Option<u64>,
    pub decision: GateDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation_binding: Option<GateObservationBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unsealed_observation_inputs: Option<UnsealedGateObservationInputs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub signals: Vec<GateSignal>,
    pub changed_paths: Vec<RepoPathProjection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_write_set: Option<ActualWriteSet>,
    pub snapshot: Option<AnalysisSnapshot>,
    #[serde(default)]
    pub protected_semantic_inputs: Vec<SemanticInputRecord>,
    #[serde(default)]
    pub alias_closures: Vec<PhysicalAliasClosureRecord>,
    #[serde(default)]
    pub reconciled_transition_sequences: Vec<u64>,
    #[serde(default)]
    pub deltas: Vec<GateDeltaRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GateRecordWire {
    schema_version: String,
    gate_id: GateId,
    lifecycle: GateLifecycle,
    current_revision: u64,
    declared_write_set: Vec<RepoPathProjection>,
    #[serde(default)]
    leased_write_set: Vec<WriteLease>,
    #[serde(default)]
    alias_closures: Vec<PhysicalAliasClosureRecord>,
    #[serde(default)]
    transition_refs: Vec<u64>,
    analysis_options: GateAnalysisOptions,
    baseline: Option<GateBaseline>,
    #[serde(default)]
    protected_semantic_inputs: Option<Vec<SemanticInputRecord>>,
    revisions: Vec<GateRevision>,
}

impl<'de> Deserialize<'de> for GateRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = GateRecordWire::deserialize(deserializer)?;
        let protected_semantic_inputs = wire.protected_semantic_inputs.unwrap_or_else(|| {
            wire.baseline.as_ref().map_or_else(Vec::new, |baseline| {
                baseline.protected_semantic_inputs.clone()
            })
        });
        Ok(Self {
            schema_version: wire.schema_version,
            gate_id: wire.gate_id,
            lifecycle: wire.lifecycle,
            current_revision: wire.current_revision,
            declared_write_set: wire.declared_write_set,
            leased_write_set: wire.leased_write_set,
            alias_closures: wire.alias_closures,
            transition_refs: wire.transition_refs,
            analysis_options: wire.analysis_options,
            baseline: wire.baseline,
            protected_semantic_inputs,
            revisions: wire.revisions,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GateOperationKind {
    PreWrite,
    PostWrite,
    GateAbandon,
}

pub fn gate_abandon_request_digest(gate_id: &GateId, target_revision: u64, reason: &str) -> String {
    let mut framed = Vec::new();
    append_length_prefixed(&mut framed, b"lumin-gate-abandon.v1");
    append_length_prefixed(&mut framed, gate_id.as_str().as_bytes());
    framed.extend_from_slice(&target_revision.to_be_bytes());
    append_length_prefixed(&mut framed, reason.as_bytes());
    digest_hex(&framed)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GateOperationStatus {
    Pending,
    Interrupted,
    Committed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationLivenessLease {
    pub lease_nonce: String,
    pub owner_process_id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_physical_identity: Option<PhysicalFileIdentity>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation_binding: Option<GateObservationBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub signals: Vec<GateSignal>,
    #[serde(default)]
    pub leased_write_set: Vec<WriteLease>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_write_set: Option<ActualWriteSet>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub transition_sequence: u64,
    pub declared_write_set: Vec<RepoPathProjection>,
    #[serde(default)]
    pub leased_write_set: Vec<WriteLease>,
    #[serde(default)]
    pub semantic_read_reservations: Vec<RepoPathProjection>,
    #[serde(default)]
    pub semantic_read_reservation_bindings: Vec<SemanticReadReservationBinding>,
    #[serde(default)]
    pub interruption_count: u64,
    #[serde(default)]
    pub operation_liveness: Option<OperationLivenessLease>,
    pub analysis_options: Option<GateAnalysisOptions>,
    pub result: Option<GateOperationResult>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitionCapsule {
    pub gate_id: GateId,
    pub revision: u64,
    pub baseline_observation_id: GateBaselineObservationId,
    pub close_observation_id: GateCloseObservationId,
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
    scan_invocation: ScanInvocationTier,
    mut entry_selections: Vec<EntrySelectionRecord>,
) -> AnalysisSnapshot {
    inputs.sort();
    inputs.dedup();
    entry_selections.sort();
    entry_selections.dedup();
    let mut framed = Vec::new();
    // Frame scan invocation tier using canonical append_semantic_framing
    scan_invocation.append_semantic_framing(&mut framed);
    append_effective_resolution_profiles(&mut framed, &evidence.resolution_profiles);
    // Frame entry selections (including unavailable ones)
    framed.extend_from_slice(&(entry_selections.len() as u64).to_be_bytes());
    for entry in &entry_selections {
        append_length_prefixed(&mut framed, &entry.path.canonical);
        match entry.source {
            lumin_model::EntrySource::Invocation => framed.push(1),
            lumin_model::EntrySource::Configuration => framed.push(2),
        }
        match entry.unavailable_reason {
            None => framed.push(0),
            Some(reason) => {
                framed.push(1);
                framed.push(entry_unavailable_reason_tag(reason));
            }
        }
    }
    // Frame semantic inputs
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
        match &input.absence_parent {
            Some(parent) => {
                framed.push(1);
                append_length_prefixed(&mut framed, &parent.path.canonical);
                append_length_prefixed(&mut framed, &parent.physical_identity.canonical_bytes());
            }
            None => framed.push(0),
        }
        match &input.physical_redirect_sha256 {
            Some(sha256) => {
                framed.push(1);
                append_length_prefixed(&mut framed, sha256.as_bytes());
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
        scan_invocation,
        entry_selections,
        evidence,
    }
}

fn append_effective_resolution_profiles(
    output: &mut Vec<u8>,
    profiles: &[SelectedResolutionProfile],
) {
    append_length_prefixed(output, b"effective-resolution-profiles.v1");
    let mut records = profiles
        .iter()
        .map(|selected| {
            let mut record = Vec::new();
            append_length_prefixed(&mut record, selected.source_id.as_str().as_bytes());
            append_length_prefixed(&mut record, selected.profile.as_str().as_bytes());
            match &selected.source {
                ResolutionProfileSource::Invocation => record.push(1),
                ResolutionProfileSource::Config { path_canonical, .. } => {
                    record.push(2);
                    append_length_prefixed(&mut record, path_canonical);
                }
                ResolutionProfileSource::ProductDefault => record.push(3),
            }
            record
        })
        .collect::<Vec<_>>();
    records.sort();
    output.extend_from_slice(&(records.len() as u64).to_be_bytes());
    for record in records {
        append_length_prefixed(output, &record);
    }
}

/// Stable tag for EntryUnavailableReason used in framing.
fn entry_unavailable_reason_tag(reason: lumin_model::EntryUnavailableReason) -> u8 {
    match reason {
        lumin_model::EntryUnavailableReason::Missing => 1,
        lumin_model::EntryUnavailableReason::Ignored => 2,
        lumin_model::EntryUnavailableReason::Excluded => 3,
        lumin_model::EntryUnavailableReason::OutOfDomain => 4,
        lumin_model::EntryUnavailableReason::HardExcluded => 5,
    }
}

pub mod gate_policy {
    use super::*;

    pub fn closure_expanded_actual_write_set(
        preliminary_paths: &[RepoPathProjection],
        baseline_alias_closures: &[PhysicalAliasClosureRecord],
        current_alias_closures: &[PhysicalAliasClosureRecord],
    ) -> ActualWriteSet {
        let mut paths = preliminary_paths.iter().cloned().collect::<BTreeSet<_>>();
        loop {
            let before = paths.len();
            for closure in baseline_alias_closures.iter().chain(current_alias_closures) {
                if closure.members.iter().any(|member| paths.contains(member)) {
                    paths.extend(closure.members.iter().cloned());
                }
            }
            if paths.len() == before {
                break;
            }
        }
        let baseline_alias_closures = baseline_alias_closures
            .iter()
            .filter(|closure| closure.members.iter().any(|member| paths.contains(member)))
            .cloned()
            .collect();
        let current_alias_closures = current_alias_closures
            .iter()
            .filter(|closure| closure.members.iter().any(|member| paths.contains(member)))
            .cloned()
            .collect();
        ActualWriteSet {
            paths: paths.into_iter().collect(),
            baseline_alias_closures,
            current_alias_closures,
        }
    }

    pub fn opening_signals(
        snapshot: &AnalysisSnapshot,
        leased_write_set: &[WriteLease],
    ) -> Vec<GateSignal> {
        let evidence = &snapshot.evidence;
        let delta_input = lifecycle_delta_input_for(
            evidence,
            &snapshot.scan_invocation.dependency_intents,
            leased_write_set,
        );
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
    ) -> (
        Vec<GateSignal>,
        Vec<RepoPathProjection>,
        Vec<GateDeltaRecord>,
    ) {
        let protected_by_path = protected_semantic_inputs
            .iter()
            .map(|input| (input.path.canonical.as_slice(), input))
            .collect::<BTreeMap<_, _>>();
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
            let current_input = current_by_path.get(path).copied();
            if current_input != Some(*baseline_input) {
                if current_input.is_some_and(|current_input| {
                    is_owned_missing_boundary_change(
                        baseline_input,
                        current_input,
                        leased_write_set,
                        &current.inputs,
                    )
                }) {
                    continue;
                }
                changed.push(baseline_input.path.clone());
                if !leased_write_set
                    .iter()
                    .any(|lease| lease.covers(&baseline_input.path))
                {
                    if let Some(reference) = protected_by_path.get(path) {
                        if current_by_path.get(path).copied() != Some(*reference) {
                            protected.push(baseline_input.path.clone());
                        }
                    } else {
                        unplanned.push(baseline_input.path.clone());
                    }
                }
            }
        }
        for (path, current_input) in &current_by_path {
            if !baseline_by_path.contains_key(path) {
                let leased = leased_write_set
                    .iter()
                    .any(|lease| lease.covers(&current_input.path));
                if protected_by_path.get(path).copied() == Some(*current_input) {
                    continue;
                }
                changed.push(current_input.path.clone());
                if !leased && protected_by_path.contains_key(path) {
                    protected.push(current_input.path.clone());
                } else if !leased {
                    unplanned.push(current_input.path.clone());
                }
            }
        }
        sort_paths(&mut changed);
        sort_paths(&mut protected);
        sort_paths(&mut unplanned);

        let baseline_delta_input = lifecycle_delta_input_for(
            &baseline.evidence,
            &baseline.scan_invocation.dependency_intents,
            leased_write_set,
        );
        let current_delta_input = lifecycle_delta_input_for(
            &current.evidence,
            &current.scan_invocation.dependency_intents,
            leased_write_set,
        );
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

    pub fn is_owned_missing_boundary_change(
        baseline: &SemanticInputRecord,
        current: &SemanticInputRecord,
        leased_write_set: &[WriteLease],
        current_inputs: &[SemanticInputRecord],
    ) -> bool {
        if baseline.state != SemanticInputState::Missing
            || current.state != SemanticInputState::Missing
            || baseline.path != current.path
            || baseline.payload_sha256 != current.payload_sha256
            || baseline.physical_identity != current.physical_identity
            || baseline.physical_redirect_sha256 != current.physical_redirect_sha256
        {
            return false;
        }
        let (Some(baseline_parent), Some(current_parent)) =
            (&baseline.absence_parent, &current.absence_parent)
        else {
            return false;
        };
        let Some((_, config_parent)) = baseline.path.components.split_last() else {
            return false;
        };
        if baseline_parent.path == current_parent.path {
            return leased_write_set.iter().any(|lease| {
                lease.kind == WriteLeaseKind::ExistingFile
                    && lease.path == baseline_parent.path
                    && lease.physical_identity.as_ref() == Some(&baseline_parent.physical_identity)
                    && current_inputs.iter().any(|input| {
                        input.path == lease.path
                            && input.state == SemanticInputState::Source
                            && input.physical_identity.as_ref()
                                == Some(&current_parent.physical_identity)
                    })
            });
        }
        if current_parent.path.components.len() <= baseline_parent.path.components.len()
            || !current_parent
                .path
                .components
                .starts_with(&baseline_parent.path.components)
            || !config_parent.starts_with(&current_parent.path.components)
        {
            return false;
        }
        leased_write_set.iter().any(|lease| match lease.kind {
            WriteLeaseKind::NewFile => {
                lease.path.components.starts_with(config_parent)
                    && current_inputs.iter().any(|input| {
                        input.path == lease.path
                            && matches!(
                                input.state,
                                SemanticInputState::Source | SemanticInputState::ConfigPresent
                            )
                    })
                    && lease.nearest_existing_parent.as_ref() == Some(&baseline_parent.path)
                    && lease.prefix_identities.last().is_some_and(|prefix| {
                        prefix.path == baseline_parent.path
                            && prefix.physical_identity == baseline_parent.physical_identity
                    })
            }
            WriteLeaseKind::Directory => {
                config_parent.starts_with(&lease.path.components)
                    && current_inputs.iter().any(|input| {
                        input.path.components.starts_with(config_parent)
                            && matches!(
                                input.state,
                                SemanticInputState::Source | SemanticInputState::ConfigPresent
                            )
                            && lease.covers(&input.path)
                    })
            }
            WriteLeaseKind::ExistingFile => false,
        })
    }

    pub fn actual_write_attribution_is_complete(signals: &[GateSignal]) -> bool {
        !signals.iter().any(|signal| {
            matches!(
                signal,
                GateSignal::AnalysisFailed { .. }
                    | GateSignal::RequiredEvidenceIncomplete { .. }
                    | GateSignal::SemanticInputConflict { .. }
                    | GateSignal::SemanticReadClosureIncomplete { .. }
                    | GateSignal::ProtectedInputChanged { .. }
                    | GateSignal::ActiveTransitionPending { .. }
                    | GateSignal::TransitionChainBroken { .. }
                    | GateSignal::TransitionCatalogChanged
            )
        })
    }

    fn requires_complete_evidence(evidence: &RunEvidence, required_gap_count: usize) -> bool {
        required_gap_count > 0
            || matches!(
                evidence.dead_code_state(),
                lumin_model::CapabilityState::Unavailable | lumin_model::CapabilityState::Failed
            )
    }

    pub fn decision(signals: &[GateSignal]) -> GateDecision {
        match signals
            .iter()
            .filter_map(effect)
            .max_by_key(|effect| effect.precedence())
        {
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
            | GateSignal::SemanticReadClosureIncomplete { .. }
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
                    if delta.key.family != DeltaFactFamily::DependencyOwnership {
                        counts.unchanged_facts += 1;
                    }
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

#[cfg(test)]
mod tests {
    use lumin_model::{CapabilityState, RepoPath};

    use super::*;
    use crate::{CapabilityRecord, DEAD_CODE_CAPABILITY_ID};

    #[test]
    fn gate_effect_precedence_is_explicit_and_order_independent() {
        let cases = [
            (Vec::new(), GateDecision::Allow),
            (
                vec![GateSignal::FindingWarnings { count: 1 }],
                GateDecision::AllowWithWarnings,
            ),
            (
                vec![
                    GateSignal::FindingWarnings { count: 1 },
                    GateSignal::RequiredEvidenceIncomplete {
                        limitation_count: 1,
                    },
                ],
                GateDecision::Incomplete,
            ),
            (
                vec![
                    GateSignal::FindingWarnings { count: 1 },
                    GateSignal::RequiredEvidenceIncomplete {
                        limitation_count: 1,
                    },
                    GateSignal::UnplannedWrite { paths: Vec::new() },
                ],
                GateDecision::Deny,
            ),
            (
                vec![
                    GateSignal::FindingWarnings { count: 1 },
                    GateSignal::RequiredEvidenceIncomplete {
                        limitation_count: 1,
                    },
                    GateSignal::UnplannedWrite { paths: Vec::new() },
                    GateSignal::AnalysisContractChanged,
                ],
                GateDecision::Stale,
            ),
        ];

        for (mut signals, expected) in cases {
            assert_eq!(gate_policy::decision(&signals), expected);
            signals.reverse();
            assert_eq!(gate_policy::decision(&signals), expected);
        }
    }

    #[test]
    fn required_evidence_incompleteness_withholds_actual_write_attribution() {
        let signals = vec![GateSignal::RequiredEvidenceIncomplete {
            limitation_count: 1,
        }];

        assert!(!gate_policy::actual_write_attribution_is_complete(&signals));
    }

    #[test]
    fn retry_revalidates_a_previously_protected_new_demand()
    -> Result<(), Box<dyn std::error::Error>> {
        let baseline = snapshot(Vec::new());
        let protected = input("config/base.json", "before")?;
        let current = snapshot(vec![input("config/base.json", "after")?]);

        let (signals, changed, _) = gate_policy::closing_signals(
            &baseline,
            &current,
            std::slice::from_ref(&protected),
            &[],
        );

        assert_eq!(changed, vec![protected.path.clone()]);
        assert!(matches!(
            signals.as_slice(),
            [GateSignal::ProtectedInputChanged { paths }] if paths == std::slice::from_ref(&protected.path)
        ));
        assert_eq!(gate_policy::decision(&signals), GateDecision::Stale);
        Ok(())
    }

    #[test]
    fn first_seen_input_outside_the_lease_is_not_assumed_read_only()
    -> Result<(), Box<dyn std::error::Error>> {
        let baseline = snapshot(Vec::new());
        let current_input = input("config/base.json", "current")?;
        let current = snapshot(vec![current_input.clone()]);

        let (signals, changed, _) = gate_policy::closing_signals(&baseline, &current, &[], &[]);

        assert_eq!(changed, vec![current_input.path.clone()]);
        assert!(matches!(
            signals.as_slice(),
            [GateSignal::UnplannedWrite { paths }] if paths == std::slice::from_ref(&current_input.path)
        ));
        assert_eq!(gate_policy::decision(&signals), GateDecision::Deny);
        Ok(())
    }

    #[test]
    fn new_file_lease_conflicts_only_with_its_reserved_missing_branch()
    -> Result<(), Box<dyn std::error::Error>> {
        let root_identity = PhysicalFileIdentity::Unix {
            device: 1,
            inode: 1,
        };
        let root_path = RepoPathProjection::from(&RepoPath::empty());
        let absence_parent = PathPrefixIdentity {
            path: root_path.clone(),
            physical_identity: root_identity.clone(),
        };
        let lease = WriteLease {
            path: path("generated/main.ts")?,
            kind: WriteLeaseKind::NewFile,
            physical_identity: None,
            nearest_existing_parent: Some(root_path),
            prefix_identities: vec![absence_parent.clone()],
        };

        assert!(lease.conflicts_with_semantic_read(
            &path("generated/deep/package.json")?,
            None,
            Some(&absence_parent),
        ));
        assert!(!lease.conflicts_with_semantic_read(
            &path("other/deep/package.json")?,
            None,
            Some(&absence_parent),
        ));
        Ok(())
    }

    #[test]
    fn new_file_parent_creation_preserves_missing_config_protection()
    -> Result<(), Box<dyn std::error::Error>> {
        let root_identity = PhysicalFileIdentity::Unix {
            device: 1,
            inode: 1,
        };
        let generated_identity = PhysicalFileIdentity::Unix {
            device: 1,
            inode: 2,
        };
        let baseline_input = missing_input("generated/package.json", "", root_identity.clone())?;
        let current_input =
            missing_input("generated/package.json", "generated", generated_identity)?;
        let root_path = RepoPathProjection::from(&RepoPath::empty());
        let lease = WriteLease {
            path: path("generated/deep/main.ts")?,
            kind: WriteLeaseKind::NewFile,
            physical_identity: None,
            nearest_existing_parent: Some(root_path.clone()),
            prefix_identities: vec![PathPrefixIdentity {
                path: root_path,
                physical_identity: root_identity,
            }],
        };

        let (signals, changed, _) = gate_policy::closing_signals(
            &snapshot(vec![baseline_input.clone()]),
            &snapshot(vec![current_input.clone()]),
            std::slice::from_ref(&baseline_input),
            std::slice::from_ref(&lease),
        );
        assert_eq!(changed, vec![baseline_input.path.clone()]);
        assert!(matches!(
            signals.as_slice(),
            [GateSignal::ProtectedInputChanged { paths }]
                if paths == std::slice::from_ref(&baseline_input.path)
        ));

        let created_source = input("generated/deep/main.ts", "new source")?;

        let (signals, changed, _) = gate_policy::closing_signals(
            &snapshot(vec![baseline_input.clone()]),
            &snapshot(vec![current_input, created_source.clone()]),
            std::slice::from_ref(&baseline_input),
            std::slice::from_ref(&lease),
        );
        assert!(signals.is_empty());
        assert_eq!(changed, vec![created_source.path.clone()]);

        let now_present = input("generated/package.json", "new manifest")?;
        let (signals, changed, _) = gate_policy::closing_signals(
            &snapshot(vec![baseline_input.clone()]),
            &snapshot(vec![now_present, created_source.clone()]),
            std::slice::from_ref(&baseline_input),
            &[lease],
        );
        assert_eq!(
            changed,
            vec![baseline_input.path.clone(), created_source.path]
        );
        assert!(matches!(
            signals.as_slice(),
            [GateSignal::ProtectedInputChanged { paths }]
                if paths == std::slice::from_ref(&baseline_input.path)
        ));
        Ok(())
    }

    #[test]
    fn directory_lease_attributes_a_created_descendant_parent_shift()
    -> Result<(), Box<dyn std::error::Error>> {
        let app_identity = PhysicalFileIdentity::Unix {
            device: 1,
            inode: 20,
        };
        let generated_identity = PhysicalFileIdentity::Unix {
            device: 1,
            inode: 21,
        };
        let baseline_input =
            missing_input("app/generated/package.json", "app", app_identity.clone())?;
        let current_input = missing_input(
            "app/generated/package.json",
            "app/generated",
            generated_identity,
        )?;
        let lease = WriteLease {
            path: path("app")?,
            kind: WriteLeaseKind::Directory,
            physical_identity: Some(app_identity),
            nearest_existing_parent: None,
            prefix_identities: Vec::new(),
        };

        let (signals, changed, _) = gate_policy::closing_signals(
            &snapshot(vec![baseline_input.clone()]),
            &snapshot(vec![current_input.clone()]),
            std::slice::from_ref(&baseline_input),
            std::slice::from_ref(&lease),
        );
        assert!(signals.is_empty());
        assert_eq!(changed, vec![baseline_input.path.clone()]);

        let mut created_source = input("app/generated/main.ts", "new source")?;
        created_source.state = SemanticInputState::Source;
        let (signals, changed, _) = gate_policy::closing_signals(
            &snapshot(vec![baseline_input.clone()]),
            &snapshot(vec![current_input, created_source.clone()]),
            std::slice::from_ref(&baseline_input),
            &[lease],
        );
        assert!(signals.is_empty());
        assert_eq!(changed, vec![created_source.path]);
        Ok(())
    }

    #[test]
    fn existing_file_replacement_preserves_impossible_child_guard()
    -> Result<(), Box<dyn std::error::Error>> {
        let baseline_identity = PhysicalFileIdentity::Unix {
            device: 1,
            inode: 10,
        };
        let current_identity = PhysicalFileIdentity::Unix {
            device: 1,
            inode: 11,
        };
        let baseline_guard = missing_input(
            "src/main.ts/package.json",
            "src/main.ts",
            baseline_identity.clone(),
        )?;
        let current_guard = missing_input(
            "src/main.ts/package.json",
            "src/main.ts",
            current_identity.clone(),
        )?;
        let source_path = path("src/main.ts")?;
        let current_source = SemanticInputRecord {
            path: source_path.clone(),
            state: SemanticInputState::Source,
            payload_sha256: Some("current".to_owned()),
            physical_identity: Some(current_identity),
            absence_parent: None,
            physical_redirect_sha256: None,
        };
        let lease = WriteLease {
            path: source_path,
            kind: WriteLeaseKind::ExistingFile,
            physical_identity: Some(baseline_identity),
            nearest_existing_parent: None,
            prefix_identities: Vec::new(),
        };

        let (signals, _, _) = gate_policy::closing_signals(
            &snapshot(vec![baseline_guard.clone()]),
            &snapshot(vec![current_guard.clone(), current_source.clone()]),
            std::slice::from_ref(&baseline_guard),
            &[],
        );
        assert!(signals.iter().any(|signal| matches!(
            signal,
            GateSignal::ProtectedInputChanged { paths }
                if paths == std::slice::from_ref(&baseline_guard.path)
        )));

        let (signals, changed, _) = gate_policy::closing_signals(
            &snapshot(vec![baseline_guard.clone()]),
            &snapshot(vec![current_guard, current_source.clone()]),
            std::slice::from_ref(&baseline_guard),
            std::slice::from_ref(&lease),
        );

        assert!(signals.is_empty());
        assert_eq!(changed, vec![current_source.path]);
        Ok(())
    }

    #[test]
    fn protected_semantic_inputs_include_non_source_configuration()
    -> Result<(), Box<dyn std::error::Error>> {
        let config = input("tsconfig.json", "config")?;

        assert_eq!(
            derive_protected_semantic_inputs(&snapshot(vec![config.clone()]), &[]),
            vec![config]
        );
        Ok(())
    }

    #[test]
    fn protected_semantic_inputs_include_source_backed_redirects_without_a_selected_lease()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut redirect = input("packages/lib/dist/index.js", "source")?;
        redirect.state = SemanticInputState::Source;
        redirect.physical_redirect_sha256 = Some("redirect".to_owned());

        assert_eq!(
            derive_protected_semantic_inputs(&snapshot(vec![redirect.clone()]), &[]),
            vec![redirect]
        );
        Ok(())
    }

    fn snapshot(inputs: Vec<SemanticInputRecord>) -> AnalysisSnapshot {
        seal_analysis_snapshot(
            inputs,
            RunEvidence {
                schema_version: "lumin-evidence.v1".to_owned(),
                capabilities: vec![CapabilityRecord {
                    capability_id: DEAD_CODE_CAPABILITY_ID.to_owned(),
                    state: CapabilityState::Complete,
                }],
                resolution_profiles: Vec::new(),
                source_classifications: Vec::new(),
                source_contexts: Vec::new(),
                source_observations: Vec::new(),
                dependency_owners: Vec::new(),
                resolutions: Vec::new(),
                metrics: Default::default(),
                findings: Vec::new(),
                limitations: Vec::new(),
            },
            ScanInvocationTier::default(),
            Vec::new(),
        )
    }

    fn input(
        value: &str,
        payload_sha256: &str,
    ) -> Result<SemanticInputRecord, Box<dyn std::error::Error>> {
        Ok(SemanticInputRecord {
            path: path(value)?,
            state: SemanticInputState::ConfigPresent,
            payload_sha256: Some(payload_sha256.to_owned()),
            physical_identity: None,
            absence_parent: None,
            physical_redirect_sha256: None,
        })
    }

    fn missing_input(
        value: &str,
        parent: &str,
        physical_identity: PhysicalFileIdentity,
    ) -> Result<SemanticInputRecord, Box<dyn std::error::Error>> {
        let parent = if parent.is_empty() {
            RepoPathProjection::from(&RepoPath::empty())
        } else {
            path(parent)?
        };
        Ok(SemanticInputRecord {
            path: path(value)?,
            state: SemanticInputState::Missing,
            payload_sha256: None,
            physical_identity: None,
            absence_parent: Some(PathPrefixIdentity {
                path: parent,
                physical_identity,
            }),
            physical_redirect_sha256: None,
        })
    }

    fn path(value: &str) -> Result<RepoPathProjection, Box<dyn std::error::Error>> {
        Ok(RepoPathProjection::from(&RepoPath::from_portable(value)?))
    }

    #[test]
    fn scan_invocation_changes_analysis_input_id() -> Result<(), Box<dyn std::error::Error>> {
        let evidence = RunEvidence {
            schema_version: "lumin-evidence.v1".to_owned(),
            capabilities: vec![CapabilityRecord {
                capability_id: DEAD_CODE_CAPABILITY_ID.to_owned(),
                state: CapabilityState::Complete,
            }],
            resolution_profiles: Vec::new(),
            source_classifications: Vec::new(),
            source_contexts: Vec::new(),
            source_observations: Vec::new(),
            dependency_owners: Vec::new(),
            resolutions: Vec::new(),
            metrics: Default::default(),
            findings: Vec::new(),
            limitations: Vec::new(),
        };
        let without_invocation = seal_analysis_snapshot(
            Vec::new(),
            evidence.clone(),
            ScanInvocationTier::default(),
            Vec::new(),
        );
        let with_includes = seal_analysis_snapshot(
            Vec::new(),
            evidence.clone(),
            ScanInvocationTier {
                includes: vec!["src/**".to_owned()],
                ..Default::default()
            },
            Vec::new(),
        );
        assert_ne!(
            without_invocation.analysis_input_id, with_includes.analysis_input_id,
            "scan invocation tier includes must affect the analysis input ID"
        );

        let with_dependency_intent = seal_analysis_snapshot(
            Vec::new(),
            evidence,
            ScanInvocationTier {
                dependency_intents: vec![DependencyIntentRecord {
                    path: path("packages/app/src/main.ts")?,
                    dependency: "zod".to_owned(),
                }],
                ..Default::default()
            },
            Vec::new(),
        );
        assert_ne!(
            without_invocation.analysis_input_id, with_dependency_intent.analysis_input_id,
            "dependency intent must affect the analysis input ID"
        );
        Ok(())
    }

    #[test]
    fn effective_resolution_profiles_change_analysis_input_id_in_canonical_order() {
        let evidence = RunEvidence {
            schema_version: "lumin-evidence.v1".to_owned(),
            capabilities: Vec::new(),
            resolution_profiles: Vec::new(),
            source_classifications: Vec::new(),
            source_contexts: Vec::new(),
            source_observations: Vec::new(),
            dependency_owners: Vec::new(),
            resolutions: Vec::new(),
            metrics: Default::default(),
            findings: Vec::new(),
            limitations: Vec::new(),
        };
        let profile = |source: &str, profile| SelectedResolutionProfile {
            source_id: lumin_model::LogicalSourceId::from_string(source.to_owned()),
            profile,
            source: ResolutionProfileSource::ProductDefault,
        };
        let node = profile("source-a", ResolutionProfile::Node16);
        let bundler = profile("source-a", ResolutionProfile::Bundler);
        let other = profile("source-b", ResolutionProfile::NodeNext);
        let seal = |profiles| {
            let mut evidence = evidence.clone();
            evidence.resolution_profiles = profiles;
            seal_analysis_snapshot(
                Vec::new(),
                evidence,
                ScanInvocationTier::default(),
                Vec::new(),
            )
        };

        let node_first = seal(vec![node.clone(), other.clone()]);
        let node_reordered = seal(vec![other.clone(), node]);
        let bundler_first = seal(vec![bundler, other]);
        assert_eq!(
            node_first.analysis_input_id, node_reordered.analysis_input_id,
            "effective profile record ordering changed AnalysisInputId",
        );
        assert_ne!(
            node_first.analysis_input_id, bundler_first.analysis_input_id,
            "effective importer profile records did not participate in AnalysisInputId",
        );
    }

    #[test]
    fn missing_input_parent_identity_changes_analysis_input_id()
    -> Result<(), Box<dyn std::error::Error>> {
        let missing = |inode| -> Result<SemanticInputRecord, Box<dyn std::error::Error>> {
            Ok(SemanticInputRecord {
                path: path("config/missing.json")?,
                state: SemanticInputState::Missing,
                payload_sha256: None,
                physical_identity: None,
                absence_parent: Some(PathPrefixIdentity {
                    path: path("config")?,
                    physical_identity: PhysicalFileIdentity::Unix { device: 7, inode },
                }),
                physical_redirect_sha256: None,
            })
        };
        let before = snapshot(vec![missing(11)?]);
        let replaced_parent = snapshot(vec![missing(12)?]);

        assert_ne!(before.analysis_input_id, replaced_parent.analysis_input_id);
        Ok(())
    }

    #[test]
    fn physical_redirect_changes_analysis_input_id() -> Result<(), Box<dyn std::error::Error>> {
        let without_redirect = input("packages/lib/escape", "payload")?;
        let mut with_redirect = without_redirect.clone();
        with_redirect.physical_redirect_sha256 = Some("redirect".to_owned());

        assert_ne!(
            snapshot(vec![without_redirect]).analysis_input_id,
            snapshot(vec![with_redirect]).analysis_input_id,
        );
        Ok(())
    }

    #[test]
    fn entry_source_changes_analysis_input_id() -> Result<(), Box<dyn std::error::Error>> {
        let evidence = RunEvidence {
            schema_version: "lumin-evidence.v1".to_owned(),
            capabilities: vec![CapabilityRecord {
                capability_id: DEAD_CODE_CAPABILITY_ID.to_owned(),
                state: CapabilityState::Complete,
            }],
            resolution_profiles: Vec::new(),
            source_classifications: Vec::new(),
            source_contexts: Vec::new(),
            source_observations: Vec::new(),
            dependency_owners: Vec::new(),
            resolutions: Vec::new(),
            metrics: Default::default(),
            findings: Vec::new(),
            limitations: Vec::new(),
        };
        let entry_path = RepoPathProjection::from(&RepoPath::from_portable("src/lib.ts")?);
        let invocation_entry = vec![EntrySelectionRecord {
            path: entry_path.clone(),
            source: lumin_model::EntrySource::Invocation,
            unavailable_reason: None,
        }];
        let config_entry = vec![EntrySelectionRecord {
            path: entry_path,
            source: lumin_model::EntrySource::Configuration,
            unavailable_reason: None,
        }];
        let snap_invocation = seal_analysis_snapshot(
            Vec::new(),
            evidence.clone(),
            ScanInvocationTier::default(),
            invocation_entry,
        );
        let snap_config = seal_analysis_snapshot(
            Vec::new(),
            evidence,
            ScanInvocationTier::default(),
            config_entry,
        );
        assert_ne!(
            snap_invocation.analysis_input_id, snap_config.analysis_input_id,
            "entry source (invocation vs config) must affect the analysis input ID"
        );
        Ok(())
    }
}
