use std::collections::{BTreeMap, BTreeSet};

use lumin_evidence::{
    AnalysisSnapshot, GateBaseline, GateRecord, GateSignal, RepoPathProjection,
    SemanticInputRecord, WorktreeTransition, seal_analysis_snapshot,
};
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
    if adjusted.evidence != transition.capsule.before_snapshot.evidence {
        return false;
    }
    let mut inputs = adjusted
        .inputs
        .iter()
        .map(|input| (input.path.canonical.clone(), input.clone()))
        .collect::<BTreeMap<_, _>>();
    let before = transition
        .capsule
        .before_snapshot
        .inputs
        .iter()
        .map(|input| (input.path.canonical.as_slice(), input))
        .collect::<BTreeMap<_, _>>();
    let after = transition
        .capsule
        .after_snapshot
        .inputs
        .iter()
        .map(|input| (input.path.canonical.as_slice(), input))
        .collect::<BTreeMap<_, _>>();
    for path in &transition.capsule.changed_paths {
        if inputs.get(&path.canonical) != before.get(path.canonical.as_slice()).copied() {
            return false;
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
    let candidate = seal_analysis_snapshot(
        inputs.into_values().collect(),
        transition.capsule.after_snapshot.evidence.clone(),
    );
    if candidate != transition.capsule.after_snapshot {
        return false;
    }
    *adjusted = candidate;
    true
}

pub(super) fn changed_paths(
    baseline: &AnalysisSnapshot,
    current: &AnalysisSnapshot,
    protected_semantic_inputs: &[SemanticInputRecord],
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
            current_by_path
                .get(input.path.canonical.as_slice())
                .copied()
                != Some(*input)
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
