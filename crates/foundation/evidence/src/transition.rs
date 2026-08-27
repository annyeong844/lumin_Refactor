use std::collections::{BTreeMap, BTreeSet};

use lumin_model::Limitation;

use crate::{
    AnalysisSnapshot, DEAD_CODE_CAPABILITY_ID, DEPENDENCY_OWNERSHIP_CAPABILITY_ID,
    RepoPathProjection, RunEvidence, SemanticInputRecord, WorktreeTransition, WriteLease,
    dead_code_capability_state, dependency_ownership_capability_state, seal_analysis_snapshot,
};

pub fn apply_worktree_transition(
    adjusted: &mut AnalysisSnapshot,
    transition: &WorktreeTransition,
) -> bool {
    apply_worktree_transition_inner(adjusted, transition, None)
}

pub fn apply_worktree_transition_for_domain(
    adjusted: &mut AnalysisSnapshot,
    transition: &WorktreeTransition,
    leased_write_set: &[WriteLease],
    protected_semantic_inputs: &[SemanticInputRecord],
) -> bool {
    apply_worktree_transition_inner(
        adjusted,
        transition,
        Some((leased_write_set, protected_semantic_inputs)),
    )
}

fn apply_worktree_transition_inner(
    adjusted: &mut AnalysisSnapshot,
    transition: &WorktreeTransition,
    domain: Option<(&[WriteLease], &[SemanticInputRecord])>,
) -> bool {
    if *adjusted == transition.capsule.before_snapshot {
        *adjusted = transition.capsule.after_snapshot.clone();
        return true;
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

    if !request_scopes_are_compatible(adjusted, &transition.capsule.before_snapshot) {
        return domain.is_some_and(|(leased_write_set, protected_semantic_inputs)| {
            transition_is_outside_domain(
                &complete_replay_paths,
                leased_write_set,
                protected_semantic_inputs,
            )
        });
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

fn transition_is_outside_domain(
    replay_paths: &[RepoPathProjection],
    leased_write_set: &[WriteLease],
    protected_semantic_inputs: &[SemanticInputRecord],
) -> bool {
    let protected = protected_semantic_inputs
        .iter()
        .map(|input| input.path.canonical.as_slice())
        .collect::<BTreeSet<_>>();
    replay_paths.iter().all(|path| {
        !protected.contains(path.canonical.as_slice())
            && !leased_write_set.iter().any(|lease| lease.covers(path))
    })
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
        if !crate::gate_policy::is_owned_missing_boundary_change(
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
    let dead_code_state = dead_code_capability_state(&evidence.limitations);
    let dependency_ownership_state = dependency_ownership_capability_state(&evidence.limitations);
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
