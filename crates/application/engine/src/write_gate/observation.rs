use lumin_evidence::{
    ActualWriteSet, GateObservationBinding, GateSignal, PhysicalAliasClosureRecord,
    RepoPathProjection, SemanticInputRecord, SemanticInputState, WriteLease, WriteLeaseKind,
};
use lumin_model::{
    GateBaselineObservationId, GateCloseObservationId, GateId, ObservationBinding,
    SealedGateObservation, UnsealedObservationReason, append_length_prefixed, digest_hex,
};
use lumin_store::GateBaselineDraft;

#[derive(Clone)]
pub(super) struct BaselineObservationSeed {
    pub(super) declared_write_set: Vec<RepoPathProjection>,
    pub(super) leased_write_set: Vec<WriteLease>,
    pub(super) alias_closures: Vec<PhysicalAliasClosureRecord>,
    pub(super) baseline: Option<GateBaselineDraft>,
}

#[derive(Clone)]
pub(super) struct CloseObservationSeed {
    pub(super) gate_id: GateId,
    pub(super) opening_observation_id: Option<GateBaselineObservationId>,
    pub(super) opening_analysis_contract: Option<String>,
    pub(super) prior_revision: u64,
    pub(super) leased_write_set: Vec<WriteLease>,
    pub(super) snapshot: Option<lumin_evidence::AnalysisSnapshot>,
    pub(super) protected_semantic_inputs: Vec<SemanticInputRecord>,
    pub(super) changed_paths: Vec<RepoPathProjection>,
    pub(super) actual_write_set: Option<ActualWriteSet>,
    pub(super) alias_closures: Vec<PhysicalAliasClosureRecord>,
    pub(super) reconciled_transition_sequences: Vec<u64>,
}

pub(super) fn pre_write_observation_binding(
    seed: &BaselineObservationSeed,
    catalog_revision: u64,
    signals: &[GateSignal],
) -> GateObservationBinding {
    if let Some(baseline) = &seed.baseline
        && !pre_write_observation_is_unsealed(signals)
    {
        return ObservationBinding::Sealed {
            observation: SealedGateObservation::Baseline {
                observation_id: baseline_observation_id(seed, baseline, catalog_revision),
            },
        };
    }
    unsealed_pre_write_observation_binding(seed, signals)
}

fn pre_write_observation_is_unsealed(signals: &[GateSignal]) -> bool {
    signals.iter().any(|signal| {
        matches!(
            signal,
            GateSignal::AnalysisFailed { .. }
                | GateSignal::DeclaredPathUnsupported { .. }
                | GateSignal::WriteConflict { .. }
                | GateSignal::SemanticInputConflict { .. }
                | GateSignal::UnplannedWrite { .. }
                | GateSignal::ActiveTransitionPending { .. }
                | GateSignal::TransitionChainBroken { .. }
                | GateSignal::TransitionCatalogChanged
        )
    })
}

pub(super) fn unsealed_pre_write_observation_binding(
    seed: &BaselineObservationSeed,
    signals: &[GateSignal],
) -> GateObservationBinding {
    let reason = signals
        .iter()
        .find_map(unsealed_observation_reason)
        .unwrap_or(UnsealedObservationReason::ObservationDomainUnbounded);
    let mut attempted_domain = seed.declared_write_set.clone();
    attempted_domain.extend(seed.leased_write_set.iter().map(|lease| lease.path.clone()));
    attempted_domain.sort();
    attempted_domain.dedup();
    let mut last_complete_read_set = seed.baseline.as_ref().map_or_else(Vec::new, |baseline| {
        baseline
            .snapshot
            .inputs
            .iter()
            .map(|input| input.path.clone())
            .collect()
    });
    last_complete_read_set.sort();
    last_complete_read_set.dedup();
    let mut conflicting_or_unbounded_inputs = observation_signal_paths(signals);
    if conflicting_or_unbounded_inputs.is_empty() {
        conflicting_or_unbounded_inputs = attempted_domain.clone();
    }
    ObservationBinding::Unsealed {
        reason,
        attempted_domain,
        last_complete_read_set,
        conflicting_or_unbounded_inputs,
    }
}

fn unsealed_observation_reason(signal: &GateSignal) -> Option<UnsealedObservationReason> {
    match signal {
        GateSignal::WriteConflict { .. } => Some(UnsealedObservationReason::AdmissionConflict),
        GateSignal::SemanticInputConflict { .. } => {
            Some(UnsealedObservationReason::SemanticReadConflict)
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

fn baseline_observation_id(
    seed: &BaselineObservationSeed,
    baseline: &GateBaselineDraft,
    catalog_revision: u64,
) -> GateBaselineObservationId {
    let mut framed = Vec::new();
    append_length_prefixed(&mut framed, b"lumin-gate-baseline-observation.v1");
    framed.extend_from_slice(&catalog_revision.to_be_bytes());
    framed.extend_from_slice(&baseline.transition_sequence.to_be_bytes());
    append_length_prefixed(&mut framed, baseline.analysis_contract.as_bytes());
    append_length_prefixed(
        &mut framed,
        baseline.snapshot.analysis_input_id.as_str().as_bytes(),
    );
    append_paths(&mut framed, &seed.declared_write_set);
    append_write_leases(&mut framed, &seed.leased_write_set);
    append_alias_closures(&mut framed, &seed.alias_closures);
    append_semantic_inputs(&mut framed, &baseline.protected_semantic_inputs);
    GateBaselineObservationId::from_string(format!(
        "gate_baseline_observation_{}",
        digest_hex(&framed)
    ))
}

pub(super) fn close_observation_binding(
    seed: &CloseObservationSeed,
    catalog_revision: u64,
    signals: &[GateSignal],
) -> GateObservationBinding {
    if !close_observation_is_unsealed(signals)
        && let Some(observation_id) = close_observation_id(seed, catalog_revision)
    {
        return ObservationBinding::Sealed {
            observation: SealedGateObservation::Close { observation_id },
        };
    }
    let reason = signals
        .iter()
        .find_map(unsealed_observation_reason)
        .unwrap_or(UnsealedObservationReason::ObservationDomainUnbounded);
    let mut attempted_domain = seed.changed_paths.clone();
    attempted_domain.extend(seed.leased_write_set.iter().map(|lease| lease.path.clone()));
    attempted_domain.sort();
    attempted_domain.dedup();
    let mut last_complete_read_set = seed.snapshot.as_ref().map_or_else(Vec::new, |snapshot| {
        snapshot
            .inputs
            .iter()
            .map(|input| input.path.clone())
            .collect()
    });
    last_complete_read_set.sort();
    last_complete_read_set.dedup();
    let mut conflicting_or_unbounded_inputs = observation_signal_paths(signals);
    if conflicting_or_unbounded_inputs.is_empty() {
        conflicting_or_unbounded_inputs = attempted_domain.clone();
    }
    ObservationBinding::Unsealed {
        reason,
        attempted_domain,
        last_complete_read_set,
        conflicting_or_unbounded_inputs,
    }
}

fn close_observation_is_unsealed(signals: &[GateSignal]) -> bool {
    signals.iter().any(|signal| {
        matches!(
            signal,
            GateSignal::AnalysisFailed { .. }
                | GateSignal::DeclaredPathUnsupported { .. }
                | GateSignal::WriteConflict { .. }
                | GateSignal::SemanticInputConflict { .. }
                | GateSignal::ActiveTransitionPending { .. }
                | GateSignal::TransitionChainBroken { .. }
                | GateSignal::TransitionCatalogChanged
                | GateSignal::LifecycleDeltaIncomparable { .. }
                | GateSignal::LifecycleBaselineUnavailable { .. }
        )
    })
}

fn close_observation_id(
    seed: &CloseObservationSeed,
    catalog_revision: u64,
) -> Option<GateCloseObservationId> {
    let snapshot = seed.snapshot.as_ref()?;
    let opening_observation_id = seed.opening_observation_id.as_ref()?;
    let opening_analysis_contract = seed.opening_analysis_contract.as_ref()?;
    let actual_write_set = seed.actual_write_set.as_ref()?;
    let mut framed = Vec::new();
    append_length_prefixed(&mut framed, b"lumin-gate-close-observation.v1");
    append_length_prefixed(&mut framed, seed.gate_id.as_str().as_bytes());
    append_length_prefixed(&mut framed, opening_observation_id.as_str().as_bytes());
    append_length_prefixed(&mut framed, opening_analysis_contract.as_bytes());
    framed.extend_from_slice(&seed.prior_revision.to_be_bytes());
    framed.extend_from_slice(&catalog_revision.to_be_bytes());
    append_length_prefixed(&mut framed, snapshot.analysis_input_id.as_str().as_bytes());
    append_write_leases(&mut framed, &seed.leased_write_set);
    append_semantic_inputs(&mut framed, &seed.protected_semantic_inputs);
    append_paths(&mut framed, &seed.changed_paths);
    append_actual_write_set(&mut framed, actual_write_set);
    append_alias_closures(&mut framed, &seed.alias_closures);
    let mut sequences = seed.reconciled_transition_sequences.clone();
    sequences.sort_unstable();
    sequences.dedup();
    framed.extend_from_slice(&(sequences.len() as u64).to_be_bytes());
    for sequence in sequences {
        framed.extend_from_slice(&sequence.to_be_bytes());
    }
    Some(GateCloseObservationId::from_string(format!(
        "gate_close_observation_{}",
        digest_hex(&framed)
    )))
}

fn append_actual_write_set(output: &mut Vec<u8>, actual: &ActualWriteSet) {
    append_paths(output, &actual.paths);
    append_alias_closures(output, &actual.baseline_alias_closures);
    append_alias_closures(output, &actual.current_alias_closures);
}

fn append_paths(output: &mut Vec<u8>, paths: &[RepoPathProjection]) {
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

fn append_write_leases(output: &mut Vec<u8>, leases: &[WriteLease]) {
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
        append_physical_identity(output, lease.physical_identity.as_ref());
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

fn append_alias_closures(output: &mut Vec<u8>, closures: &[PhysicalAliasClosureRecord]) {
    let mut closures = closures.to_vec();
    closures.sort();
    closures.dedup();
    output.extend_from_slice(&(closures.len() as u64).to_be_bytes());
    for closure in closures {
        append_length_prefixed(output, &closure.physical_identity.canonical_bytes());
        append_paths(output, &closure.members);
    }
}

fn append_semantic_inputs(output: &mut Vec<u8>, inputs: &[SemanticInputRecord]) {
    let mut inputs = inputs.to_vec();
    inputs.sort();
    inputs.dedup();
    output.extend_from_slice(&(inputs.len() as u64).to_be_bytes());
    for input in inputs {
        append_length_prefixed(output, &input.path.canonical);
        output.push(match input.state {
            SemanticInputState::Source => 1,
            SemanticInputState::ConfigPresent => 2,
            SemanticInputState::Missing => 3,
            SemanticInputState::NonRegular => 4,
            SemanticInputState::Unreadable => 5,
            SemanticInputState::PathRedirect => 6,
        });
        append_optional_bytes(output, input.payload_sha256.as_deref().map(str::as_bytes));
        append_physical_identity(output, input.physical_identity.as_ref());
        match input.absence_parent {
            Some(parent) => {
                output.push(1);
                append_length_prefixed(output, &parent.path.canonical);
                append_length_prefixed(output, &parent.physical_identity.canonical_bytes());
            }
            None => output.push(0),
        }
        append_optional_bytes(
            output,
            input.physical_redirect_sha256.as_deref().map(str::as_bytes),
        );
    }
}

fn append_physical_identity(
    output: &mut Vec<u8>,
    identity: Option<&lumin_model::PhysicalFileIdentity>,
) {
    match identity {
        Some(identity) => {
            output.push(1);
            append_length_prefixed(output, &identity.canonical_bytes());
        }
        None => output.push(0),
    }
}

fn append_optional_bytes(output: &mut Vec<u8>, value: Option<&[u8]>) {
    match value {
        Some(value) => {
            output.push(1);
            append_length_prefixed(output, value);
        }
        None => output.push(0),
    }
}

#[cfg(test)]
mod tests {
    use lumin_evidence::{AnalysisMetrics, AnalysisSnapshot, RunEvidence};
    use lumin_model::{AnalysisInputId, RepoPath};

    use super::*;

    #[test]
    fn baseline_identity_is_set_canonical_and_content_sensitive()
    -> Result<(), Box<dyn std::error::Error>> {
        let original = seed("payload-a")?;
        let mut repeated = original.clone();
        repeated
            .declared_write_set
            .push(repeated.declared_write_set[0].clone());
        repeated.leased_write_set.reverse();
        let repeated_input = original
            .baseline
            .as_ref()
            .and_then(|baseline| baseline.protected_semantic_inputs.first())
            .ok_or("original test baseline omitted its semantic input")?
            .clone();
        repeated
            .baseline
            .as_mut()
            .ok_or("repeated test seed omitted its baseline")?
            .protected_semantic_inputs
            .push(repeated_input);

        let original_id = id(&original, 7)?;
        assert_eq!(original_id, id(&repeated, 7)?);
        assert_ne!(original_id, id(&seed("payload-b")?, 7)?);
        assert_ne!(original_id, id(&original, 8)?);
        Ok(())
    }

    fn id(
        seed: &BaselineObservationSeed,
        catalog_revision: u64,
    ) -> Result<GateBaselineObservationId, &'static str> {
        let baseline = seed
            .baseline
            .as_ref()
            .ok_or("test seed omitted its baseline")?;
        Ok(baseline_observation_id(seed, baseline, catalog_revision))
    }

    fn seed(payload: &str) -> Result<BaselineObservationSeed, Box<dyn std::error::Error>> {
        let path = RepoPathProjection::from(&RepoPath::from_portable("src/lib.ts")?);
        let input = SemanticInputRecord {
            path: path.clone(),
            state: SemanticInputState::Source,
            payload_sha256: Some(payload.to_owned()),
            physical_identity: None,
            absence_parent: None,
            physical_redirect_sha256: None,
        };
        Ok(BaselineObservationSeed {
            declared_write_set: vec![path],
            leased_write_set: Vec::new(),
            alias_closures: Vec::new(),
            baseline: Some(GateBaselineDraft {
                analysis_contract: "contract".to_owned(),
                snapshot: AnalysisSnapshot {
                    analysis_input_id: AnalysisInputId::from_string("analysis-input".to_owned()),
                    inputs: vec![input.clone()],
                    scan_invocation: Default::default(),
                    entry_selections: Vec::new(),
                    evidence: RunEvidence {
                        schema_version: "lumin-evidence.v1".to_owned(),
                        capabilities: Vec::new(),
                        resolution_profiles: Vec::new(),
                        source_classifications: Vec::new(),
                        source_contexts: Vec::new(),
                        source_observations: Vec::new(),
                        dependency_owners: Vec::new(),
                        resolutions: Vec::new(),
                        metrics: AnalysisMetrics::default(),
                        findings: Vec::new(),
                        limitations: Vec::new(),
                    },
                },
                protected_semantic_inputs: vec![input],
                transition_sequence: 3,
            }),
        })
    }
}
