use std::collections::{BTreeMap, BTreeSet};

use lumin_evidence::{
    ActualWriteSet, AnalysisSnapshot, DEAD_CODE_CAPABILITY_ID, DEPENDENCY_OWNERSHIP_CAPABILITY_ID,
    GateBaseline, GateRecord, GateSignal, PhysicalAliasClosureRecord, RepoPathProjection,
    RunEvidence, SemanticInputRecord, WorktreeTransition, WriteLease, seal_analysis_snapshot,
};
use lumin_model::{CapabilityState, Limitation};
use lumin_store::ActiveGateLease;

pub(super) fn reconcile_transitions(
    gate: &GateRecord,
    baseline: &GateBaseline,
    transitions: &[WorktreeTransition],
) -> (AnalysisSnapshot, Vec<u64>, Vec<GateSignal>) {
    let protected = baseline
        .protected_semantic_inputs
        .iter()
        .map(|input| input.path.canonical.as_slice())
        .collect::<BTreeSet<_>>();
    let mut adjusted = baseline.snapshot.clone();
    let mut sequences = Vec::new();
    let mut signals = Vec::new();
    for transition in transitions {
        let touching_lease = transition
            .capsule
            .changed_paths
            .iter()
            .any(|path| gate.leased_write_set.iter().any(|lease| lease.covers(path)));
        if touching_lease {
            signals.push(GateSignal::TransitionChainBroken {
                sequence: transition.sequence,
            });
            sequences.push(transition.sequence);
            continue;
        }
        let protected_paths = transition
            .capsule
            .changed_paths
            .iter()
            .filter(|path| protected.contains(path.canonical.as_slice()))
            .cloned()
            .collect::<Vec<_>>();
        if !protected_paths.is_empty() {
            signals.push(GateSignal::ProtectedInputChanged {
                paths: protected_paths,
            });
            sequences.push(transition.sequence);
            continue;
        }
        if !apply_transition(&mut adjusted, transition) {
            signals.push(GateSignal::TransitionChainBroken {
                sequence: transition.sequence,
            });
        }
        sequences.push(transition.sequence);
    }
    (adjusted, sequences, signals)
}

fn apply_transition(adjusted: &mut AnalysisSnapshot, transition: &WorktreeTransition) -> bool {
    if *adjusted == transition.capsule.before_snapshot {
        *adjusted = transition.capsule.after_snapshot.clone();
        return true;
    }
    if !request_scopes_are_compatible(adjusted, &transition.capsule.before_snapshot) {
        return false;
    }

    let Some(topology_paths) = owned_topology_replay_paths(
        &transition.capsule.before_snapshot.inputs,
        &transition.capsule.after_snapshot.inputs,
        &transition.capsule.changed_paths,
        &transition.capsule.leased_write_set,
    ) else {
        return false;
    };
    let mut complete_replay_paths = transition.capsule.changed_paths.clone();
    complete_replay_paths.extend(topology_paths.iter().cloned());
    complete_replay_paths.sort();
    complete_replay_paths.dedup();

    let Some(transition_inputs) = apply_input_delta(
        &transition.capsule.before_snapshot.inputs,
        &transition.capsule.before_snapshot.inputs,
        &transition.capsule.after_snapshot.inputs,
        &complete_replay_paths,
    ) else {
        return false;
    };
    let verified_after = seal_analysis_snapshot(
        transition_inputs,
        transition.capsule.after_snapshot.evidence.clone(),
        transition.capsule.after_snapshot.scan_invocation.clone(),
        transition.capsule.after_snapshot.entry_selections.clone(),
    );
    if verified_after != transition.capsule.after_snapshot {
        return false;
    }

    let Some(inputs) = apply_rebased_input_delta(
        &adjusted.inputs,
        &transition.capsule.before_snapshot.inputs,
        &transition.capsule.after_snapshot.inputs,
        &transition.capsule.changed_paths,
        &topology_paths,
    ) else {
        return false;
    };
    let Some(evidence) = rebase_request_specific_evidence(
        &adjusted.evidence,
        &transition.capsule.before_snapshot.evidence,
        &transition.capsule.after_snapshot.evidence,
    ) else {
        return false;
    };
    *adjusted = seal_analysis_snapshot(
        inputs,
        evidence,
        adjusted.scan_invocation.clone(),
        transition.capsule.after_snapshot.entry_selections.clone(),
    );
    true
}

fn owned_topology_replay_paths(
    before_inputs: &[SemanticInputRecord],
    after_inputs: &[SemanticInputRecord],
    changed_paths: &[RepoPathProjection],
    leased_write_set: &[WriteLease],
) -> Option<Vec<RepoPathProjection>> {
    let before = before_inputs
        .iter()
        .map(|input| (input.path.canonical.as_slice(), input))
        .collect::<BTreeMap<_, _>>();
    let after = after_inputs
        .iter()
        .map(|input| (input.path.canonical.as_slice(), input))
        .collect::<BTreeMap<_, _>>();
    let changed = changed_paths
        .iter()
        .map(|path| path.canonical.as_slice())
        .collect::<BTreeSet<_>>();
    let mut topology_paths = Vec::new();

    for (path, baseline) in &before {
        let current = after.get(path).copied();
        if current == Some(*baseline) || changed.contains(path) {
            continue;
        }
        let current = current?;
        if !lumin_evidence::gate_policy::is_owned_missing_boundary_change(
            baseline,
            current,
            leased_write_set,
            after_inputs,
        ) {
            return None;
        }
        topology_paths.push(current.path.clone());
    }
    if after
        .keys()
        .any(|path| !before.contains_key(path) && !changed.contains(path))
    {
        return None;
    }
    topology_paths.sort();
    topology_paths.dedup();
    Some(topology_paths)
}

fn request_scopes_are_compatible(
    adjusted: &AnalysisSnapshot,
    transition_before: &AnalysisSnapshot,
) -> bool {
    let mut adjusted_invocation = adjusted.scan_invocation.clone();
    adjusted_invocation.dependency_intents.clear();
    let mut transition_invocation = transition_before.scan_invocation.clone();
    transition_invocation.dependency_intents.clear();
    adjusted_invocation == transition_invocation
        && adjusted.entry_selections == transition_before.entry_selections
}

fn apply_input_delta(
    base: &[SemanticInputRecord],
    before_inputs: &[SemanticInputRecord],
    after_inputs: &[SemanticInputRecord],
    changed_paths: &[RepoPathProjection],
) -> Option<Vec<SemanticInputRecord>> {
    let mut inputs = base
        .iter()
        .map(|input| (input.path.canonical.clone(), input.clone()))
        .collect::<BTreeMap<_, _>>();
    let before = before_inputs
        .iter()
        .map(|input| (input.path.canonical.as_slice(), input))
        .collect::<BTreeMap<_, _>>();
    let after = after_inputs
        .iter()
        .map(|input| (input.path.canonical.as_slice(), input))
        .collect::<BTreeMap<_, _>>();
    for path in changed_paths {
        if inputs.get(&path.canonical) != before.get(path.canonical.as_slice()).copied() {
            return None;
        }
        match after.get(path.canonical.as_slice()) {
            Some(input) => {
                inputs.insert(path.canonical.clone(), (*input).clone());
            }
            None => {
                inputs.remove(&path.canonical);
            }
        }
    }
    Some(inputs.into_values().collect())
}

fn apply_rebased_input_delta(
    base: &[SemanticInputRecord],
    before_inputs: &[SemanticInputRecord],
    after_inputs: &[SemanticInputRecord],
    changed_paths: &[RepoPathProjection],
    topology_paths: &[RepoPathProjection],
) -> Option<Vec<SemanticInputRecord>> {
    let mut inputs = apply_input_delta(base, before_inputs, after_inputs, changed_paths)?
        .into_iter()
        .map(|input| (input.path.canonical.clone(), input))
        .collect::<BTreeMap<_, _>>();
    let before = before_inputs
        .iter()
        .map(|input| (input.path.canonical.as_slice(), input))
        .collect::<BTreeMap<_, _>>();
    let after = after_inputs
        .iter()
        .map(|input| (input.path.canonical.as_slice(), input))
        .collect::<BTreeMap<_, _>>();
    for path in topology_paths {
        let baseline = before.get(path.canonical.as_slice()).copied()?;
        let current = after.get(path.canonical.as_slice()).copied()?;
        match inputs.get(&path.canonical) {
            None => {}
            Some(input) if input == baseline => {
                inputs.insert(path.canonical.clone(), current.clone());
            }
            Some(_) => return None,
        }
    }
    Some(inputs.into_values().collect())
}

fn rebase_request_specific_evidence(
    adjusted: &RunEvidence,
    transition_before: &RunEvidence,
    transition_after: &RunEvidence,
) -> Option<RunEvidence> {
    if repository_evidence_projection(adjusted) != repository_evidence_projection(transition_before)
    {
        return None;
    }

    let mut evidence = transition_after.clone();
    evidence.dependency_owners = adjusted.dependency_owners.clone();
    evidence
        .limitations
        .retain(|limitation| !is_request_specific_dependency_limitation(limitation));
    evidence.limitations.extend(
        adjusted
            .limitations
            .iter()
            .filter(|limitation| is_request_specific_dependency_limitation(limitation))
            .cloned(),
    );
    evidence.limitations.sort_by(Limitation::canonical_cmp);
    evidence.limitations.dedup();
    refresh_request_sensitive_capabilities(&mut evidence).then_some(evidence)
}

fn repository_evidence_projection(evidence: &RunEvidence) -> RunEvidence {
    let mut projection = evidence.clone();
    projection.dependency_owners.clear();
    projection
        .limitations
        .retain(|limitation| !is_request_specific_dependency_limitation(limitation));
    projection.capabilities.retain(|capability| {
        capability.capability_id != DEAD_CODE_CAPABILITY_ID
            && capability.capability_id != DEPENDENCY_OWNERSHIP_CAPABILITY_ID
    });
    projection
}

// The architecture check must inspect Limitation variants outside macro token streams.
#[allow(clippy::match_like_matches_macro)]
fn is_request_specific_dependency_limitation(limitation: &Limitation) -> bool {
    match limitation {
        Limitation::DependencyOwnerAmbiguous {
            required_intent: Some(_),
            ..
        } => true,
        _ => false,
    }
}

fn refresh_request_sensitive_capabilities(evidence: &mut RunEvidence) -> bool {
    let dead_code_state = if evidence.limitations.is_empty() {
        CapabilityState::Complete
    } else {
        CapabilityState::Incomplete
    };
    let dependency_ownership_state =
        super::super::dependency_ownership_state(&evidence.limitations);
    let mut dead_code_found = false;
    let mut dependency_ownership_found = false;
    for capability in &mut evidence.capabilities {
        if capability.capability_id == DEAD_CODE_CAPABILITY_ID {
            capability.state = dead_code_state;
            dead_code_found = true;
        } else if capability.capability_id == DEPENDENCY_OWNERSHIP_CAPABILITY_ID {
            capability.state = dependency_ownership_state;
            dependency_ownership_found = true;
        }
    }
    dead_code_found && dependency_ownership_found
}

pub(super) fn changed_paths(
    baseline: &AnalysisSnapshot,
    current: &AnalysisSnapshot,
    protected_semantic_inputs: &[SemanticInputRecord],
    leased_write_set: &[lumin_evidence::WriteLease],
) -> Vec<RepoPathProjection> {
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
    let protected_by_path = protected_semantic_inputs
        .iter()
        .map(|input| (input.path.canonical.as_slice(), input))
        .collect::<BTreeMap<_, _>>();
    let mut changed = baseline
        .inputs
        .iter()
        .filter(|input| {
            let current_input = current_by_path
                .get(input.path.canonical.as_slice())
                .copied();
            current_input != Some(*input)
                && !current_input.is_some_and(|current_input| {
                    lumin_evidence::gate_policy::is_owned_missing_boundary_change(
                        input,
                        current_input,
                        leased_write_set,
                        &current.inputs,
                    )
                })
        })
        .map(|input| input.path.clone())
        .collect::<Vec<_>>();
    changed.extend(
        current
            .inputs
            .iter()
            .filter(|input| {
                let path = input.path.canonical.as_slice();
                !baseline_by_path.contains_key(path)
                    && protected_by_path.get(path).copied() != Some(*input)
            })
            .map(|input| input.path.clone()),
    );
    changed.sort();
    changed.dedup();
    changed
}

pub(super) fn active_transition_signals(
    changed_paths: &[RepoPathProjection],
    active_gates: &[ActiveGateLease],
) -> Vec<GateSignal> {
    let mut paths = Vec::new();
    let mut gate_ids = Vec::new();
    for path in changed_paths {
        for active in active_gates {
            if active
                .leased_write_set
                .iter()
                .any(|lease| lease.covers(path))
            {
                paths.push(path.clone());
                gate_ids.push(active.gate_id.clone());
            }
        }
    }
    paths.sort();
    paths.dedup();
    gate_ids.sort();
    gate_ids.dedup();
    if paths.is_empty() {
        Vec::new()
    } else {
        vec![GateSignal::ActiveTransitionPending { paths, gate_ids }]
    }
}

pub(super) fn closure_expanded_actual_write_set(
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

#[cfg(test)]
mod tests {
    use lumin_evidence::{
        AnalysisMetrics, CapabilityRecord, DependencyIntentRecord, DependencyOwnerRecord,
        PathPrefixIdentity, ScanInvocationTier, SemanticInputState, TransitionCapsule,
        WriteLeaseKind,
    };
    use lumin_model::{
        GateBaselineObservationId, GateCloseObservationId, GateId, LogicalSourceId,
        PhysicalFileIdentity, RepoPath,
    };

    use super::*;

    #[test]
    fn request_specific_dependency_evidence_rebases_disjoint_transition()
    -> Result<(), Box<dyn std::error::Error>> {
        let changed_path = path("packages/b/src/main.ts")?;
        let mut adjusted = snapshot(
            vec![input("packages/b/src/main.ts", "before")?],
            owner("packages/a/src/main.ts", "left-pad", "packages/a")?,
            intent("packages/a/src/main.ts", "left-pad")?,
        );
        let transition_before = snapshot(
            vec![input("packages/b/src/main.ts", "before")?],
            owner("packages/b/src/main.ts", "is-odd", "packages/b")?,
            intent("packages/b/src/main.ts", "is-odd")?,
        );
        let transition_after = snapshot(
            vec![input("packages/b/src/main.ts", "after")?],
            owner("packages/b/src/main.ts", "is-odd", "packages/b")?,
            intent("packages/b/src/main.ts", "is-odd")?,
        );
        let transition = WorktreeTransition {
            sequence: 1,
            capsule: TransitionCapsule {
                gate_id: GateId::from_string("gate-b".to_owned()),
                revision: 1,
                baseline_observation_id: baseline_observation_id(),
                close_observation_id: close_observation_id(),
                before_snapshot: transition_before,
                after_snapshot: transition_after,
                changed_paths: vec![changed_path],
                leased_write_set: Vec::new(),
            },
        };
        let adjusted_owner = adjusted.evidence.dependency_owners.clone();
        let adjusted_invocation = adjusted.scan_invocation.clone();

        assert!(apply_transition(&mut adjusted, &transition));
        assert_eq!(adjusted.evidence.dependency_owners, adjusted_owner);
        assert_eq!(adjusted.scan_invocation, adjusted_invocation);
        assert_eq!(adjusted.inputs[0].payload_sha256.as_deref(), Some("after"));
        Ok(())
    }

    #[test]
    fn request_specific_rebase_replays_owned_missing_topology()
    -> Result<(), Box<dyn std::error::Error>> {
        let source_path = path("packages/b/generated/main.ts")?;
        let candidate_path = path("packages/b/generated/package.json")?;
        let package_path = path("packages/b")?;
        let generated_path = path("packages/b/generated")?;
        let package_identity = PhysicalFileIdentity::Unix {
            device: 1,
            inode: 10,
        };
        let generated_identity = PhysicalFileIdentity::Unix {
            device: 1,
            inode: 11,
        };
        let source_before = missing_input(
            source_path.clone(),
            package_path.clone(),
            package_identity.clone(),
        );
        let candidate_before = missing_input(
            candidate_path.clone(),
            package_path.clone(),
            package_identity.clone(),
        );
        let source_after = input("packages/b/generated/main.ts", "after")?;
        let candidate_after =
            missing_input(candidate_path.clone(), generated_path, generated_identity);

        let mut adjusted = snapshot(
            vec![source_before.clone()],
            owner("packages/a/src/main.ts", "left-pad", "packages/a")?,
            intent("packages/a/src/main.ts", "left-pad")?,
        );
        let transition = WorktreeTransition {
            sequence: 2,
            capsule: TransitionCapsule {
                gate_id: GateId::from_string("gate-b-topology".to_owned()),
                revision: 1,
                baseline_observation_id: baseline_observation_id(),
                close_observation_id: close_observation_id(),
                before_snapshot: snapshot(
                    vec![source_before, candidate_before],
                    owner("packages/b/generated/main.ts", "is-odd", "packages/b")?,
                    intent("packages/b/generated/main.ts", "is-odd")?,
                ),
                after_snapshot: snapshot(
                    vec![source_after, candidate_after],
                    owner("packages/b/generated/main.ts", "is-odd", "packages/b")?,
                    intent("packages/b/generated/main.ts", "is-odd")?,
                ),
                changed_paths: vec![source_path.clone()],
                leased_write_set: vec![WriteLease {
                    path: source_path.clone(),
                    kind: WriteLeaseKind::NewFile,
                    physical_identity: None,
                    nearest_existing_parent: Some(package_path.clone()),
                    prefix_identities: vec![PathPrefixIdentity {
                        path: package_path,
                        physical_identity: package_identity,
                    }],
                }],
            },
        };

        assert!(apply_transition(&mut adjusted, &transition));
        assert_eq!(adjusted.inputs.len(), 1);
        assert_eq!(adjusted.inputs[0].path, source_path);
        assert_eq!(adjusted.inputs[0].state, SemanticInputState::Source);
        assert!(
            adjusted
                .inputs
                .iter()
                .all(|input| input.path != candidate_path),
            "request-specific dependency topology leaked into the surviving gate",
        );
        Ok(())
    }

    #[test]
    fn request_specific_rebase_rejects_repository_evidence_drift()
    -> Result<(), Box<dyn std::error::Error>> {
        let changed_path = path("packages/b/src/main.ts")?;
        let mut adjusted = snapshot(
            vec![input("packages/b/src/main.ts", "before")?],
            owner("packages/a/src/main.ts", "left-pad", "packages/a")?,
            intent("packages/a/src/main.ts", "left-pad")?,
        );
        let mut drifted_evidence = adjusted.evidence.clone();
        drifted_evidence.metrics.logical_source_count = 1;
        adjusted = seal_analysis_snapshot(
            adjusted.inputs,
            drifted_evidence,
            adjusted.scan_invocation,
            adjusted.entry_selections,
        );
        let transition_before = snapshot(
            vec![input("packages/b/src/main.ts", "before")?],
            owner("packages/b/src/main.ts", "is-odd", "packages/b")?,
            intent("packages/b/src/main.ts", "is-odd")?,
        );
        let transition_after = snapshot(
            vec![input("packages/b/src/main.ts", "after")?],
            owner("packages/b/src/main.ts", "is-odd", "packages/b")?,
            intent("packages/b/src/main.ts", "is-odd")?,
        );
        let transition = WorktreeTransition {
            sequence: 1,
            capsule: TransitionCapsule {
                gate_id: GateId::from_string("gate-b".to_owned()),
                revision: 1,
                baseline_observation_id: baseline_observation_id(),
                close_observation_id: close_observation_id(),
                before_snapshot: transition_before,
                after_snapshot: transition_after,
                changed_paths: vec![changed_path],
                leased_write_set: Vec::new(),
            },
        };

        assert!(!apply_transition(&mut adjusted, &transition));
        assert_eq!(adjusted.inputs[0].payload_sha256.as_deref(), Some("before"));
        Ok(())
    }

    fn baseline_observation_id() -> GateBaselineObservationId {
        GateBaselineObservationId::from_string("gate_baseline_observation_test".to_owned())
    }

    fn close_observation_id() -> GateCloseObservationId {
        GateCloseObservationId::from_string("gate_close_observation_test".to_owned())
    }

    fn snapshot(
        inputs: Vec<SemanticInputRecord>,
        dependency_owner: DependencyOwnerRecord,
        dependency_intent: DependencyIntentRecord,
    ) -> AnalysisSnapshot {
        seal_analysis_snapshot(
            inputs,
            RunEvidence {
                schema_version: "lumin-evidence.v1".to_owned(),
                capabilities: vec![
                    CapabilityRecord {
                        capability_id: DEAD_CODE_CAPABILITY_ID.to_owned(),
                        state: CapabilityState::Complete,
                    },
                    CapabilityRecord {
                        capability_id: DEPENDENCY_OWNERSHIP_CAPABILITY_ID.to_owned(),
                        state: CapabilityState::Complete,
                    },
                ],
                resolution_profiles: Vec::new(),
                source_classifications: Vec::new(),
                source_contexts: Vec::new(),
                source_observations: Vec::new(),
                dependency_owners: vec![dependency_owner],
                resolutions: Vec::new(),
                metrics: AnalysisMetrics::default(),
                findings: Vec::new(),
                limitations: Vec::new(),
            },
            ScanInvocationTier {
                dependency_intents: vec![dependency_intent],
                ..ScanInvocationTier::default()
            },
            Vec::new(),
        )
    }

    fn input(
        value: &str,
        payload_sha256: &str,
    ) -> Result<SemanticInputRecord, Box<dyn std::error::Error>> {
        Ok(SemanticInputRecord {
            path: path(value)?,
            state: SemanticInputState::Source,
            payload_sha256: Some(payload_sha256.to_owned()),
            physical_identity: None,
            absence_parent: None,
            physical_redirect_sha256: None,
        })
    }

    fn missing_input(
        path: RepoPathProjection,
        parent: RepoPathProjection,
        parent_identity: PhysicalFileIdentity,
    ) -> SemanticInputRecord {
        SemanticInputRecord {
            path,
            state: SemanticInputState::Missing,
            payload_sha256: None,
            physical_identity: None,
            absence_parent: Some(PathPrefixIdentity {
                path: parent,
                physical_identity: parent_identity,
            }),
            physical_redirect_sha256: None,
        }
    }

    fn owner(
        consumer: &str,
        dependency: &str,
        package_root: &str,
    ) -> Result<DependencyOwnerRecord, Box<dyn std::error::Error>> {
        let consumer_path = RepoPath::from_portable(consumer)?;
        Ok(DependencyOwnerRecord {
            consumer: LogicalSourceId::from_path(&consumer_path),
            consumer_path: RepoPathProjection::from(&consumer_path),
            dependency: dependency.to_owned(),
            package_root: path(package_root)?,
            manifest_path: path(&format!("{package_root}/package.json"))?,
            manifest_payload_sha256: "manifest-hash".to_owned(),
            lockfile_path: None,
        })
    }

    fn intent(
        consumer: &str,
        dependency: &str,
    ) -> Result<DependencyIntentRecord, Box<dyn std::error::Error>> {
        Ok(DependencyIntentRecord {
            path: path(consumer)?,
            dependency: dependency.to_owned(),
        })
    }

    fn path(value: &str) -> Result<RepoPathProjection, Box<dyn std::error::Error>> {
        Ok(RepoPathProjection::from(&RepoPath::from_portable(value)?))
    }
}
