use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use lumin_evidence::{
    AnalysisSnapshot, GateAnalysisOptions, GateBaseline, GateOperationResult, GateRecord,
    GateSignal, OperationRecord, PathPrefixIdentity, PhysicalAliasClosureRecord,
    RepoPathProjection, SemanticInputRecord, SemanticInputState, SemanticReadReservationBinding,
    WorktreeTransition, WriteLease, WriteLeaseKind, gate_policy, seal_analysis_snapshot,
};
use lumin_inventory::{
    InventoryRequest, WriteTargetError, WriteTargetKind, WriteTargetObservation,
};
use lumin_model::{
    GateDeltaRecord, GateId, OperationId, PhysicalFileIdentity, RepoPath, ResolutionProfile,
    append_length_prefixed, digest_hex,
};
use lumin_store::{
    ActiveGateLease, PostWriteFinish, PostWriteStart, PreWriteFinish, PreWriteStart,
    RepositoryStore, SemanticReadReservation,
};

use super::{EngineError, RepositoryAnalysisSession, RepositoryAnalysisStep, RepositoryCapture};

const ANALYSIS_CONTRACT: &str = "lumin-analysis-contract.phase1-foundation.v1";

#[derive(Clone, Debug)]
pub struct PreWriteRequest {
    pub root: PathBuf,
    pub operation_id: OperationId,
    pub paths: Vec<RepoPath>,
    pub jobs: usize,
    pub resolution_profile: Option<ResolutionProfile>,
}

#[derive(Clone, Debug)]
pub struct PostWriteRequest {
    pub root: PathBuf,
    pub gate_id: GateId,
    pub operation_id: OperationId,
}

pub fn open_write_gate(request: &PreWriteRequest) -> Result<GateOperationResult, EngineError> {
    if request.jobs == 0 {
        return Err(EngineError::InvalidWorkerCount(0));
    }
    let mut paths = request.paths.clone();
    paths.sort();
    paths.dedup();
    if paths.is_empty() {
        return Err(EngineError::NoDeclaredPaths);
    }
    let declared_write_set = paths
        .iter()
        .map(RepoPathProjection::from)
        .collect::<Vec<_>>();
    let analysis_options = GateAnalysisOptions {
        jobs: request.jobs,
        resolution_profile: request.resolution_profile,
    };
    let request_digest = pre_write_digest(&paths, &analysis_options);
    let (observations, initial_leases, inspection_signals) =
        inspect_declared_paths(&request.root, &paths);
    let store = RepositoryStore::open(&request.root)?;
    let (gate_id, transition_sequence) = match store.reserve_pre_write(
        &request.operation_id,
        &request_digest,
        &declared_write_set,
        &initial_leases,
        &analysis_options,
    )? {
        PreWriteStart::Committed(result) => return Ok(result),
        PreWriteStart::Analyze {
            gate_id,
            transition_sequence,
        } => (gate_id, transition_sequence),
    };

    let finish = if inspection_signals.is_empty() {
        match analyze_pre_write(
            &store,
            request,
            &observations,
            initial_leases,
            transition_sequence,
            &request_digest,
            &gate_id,
        )? {
            PreWriteAnalysis::Finished(finish) => finish,
            PreWriteAnalysis::Committed(result) => return Ok(result),
        }
    } else {
        PreWriteFinish {
            baseline: None,
            leased_write_set: initial_leases,
            alias_closures: Vec::new(),
            signals: inspection_signals,
        }
    };
    store
        .finish_pre_write(&request.operation_id, &request_digest, &gate_id, finish)
        .map_err(Into::into)
}

enum PreWriteAnalysis {
    Finished(PreWriteFinish),
    Committed(GateOperationResult),
}

fn analyze_pre_write(
    store: &RepositoryStore,
    request: &PreWriteRequest,
    observations: &[WriteTargetObservation],
    initial_leases: Vec<WriteLease>,
    transition_sequence: u64,
    request_digest: &str,
    gate_id: &GateId,
) -> Result<PreWriteAnalysis, EngineError> {
    let options = GateAnalysisOptions {
        jobs: request.jobs,
        resolution_profile: request.resolution_profile,
    };
    let capture = match capture_reserved_repository(&request.root, &options, |paths| {
        store
            .reserve_pre_write_semantic_inputs(
                &request.operation_id,
                request_digest,
                gate_id,
                paths,
            )
            .map_err(Into::into)
    }) {
        Ok(ReservedCapture::Finished { capture, .. }) => capture,
        Ok(ReservedCapture::Blocked(signal)) => {
            return Ok(PreWriteAnalysis::Finished(PreWriteFinish {
                baseline: None,
                leased_write_set: initial_leases,
                alias_closures: Vec::new(),
                signals: vec![signal],
            }));
        }
        Ok(ReservedCapture::Committed(result)) => {
            return Ok(PreWriteAnalysis::Committed(result));
        }
        Err(EngineError::Store(error)) => return Err(EngineError::Store(error)),
        Err(error) => {
            return Ok(PreWriteAnalysis::Finished(PreWriteFinish {
                baseline: None,
                leased_write_set: initial_leases,
                alias_closures: Vec::new(),
                signals: vec![GateSignal::AnalysisFailed {
                    detail: error.to_string(),
                }],
            }));
        }
    };
    let (leased_write_set, alias_closures, mut signals) =
        expand_write_domain(&request.root, observations, initial_leases, &capture);
    let protected_semantic_inputs = protected_semantic_inputs(&capture, &leased_write_set);
    signals.extend(gate_policy::opening_signals(&capture.snapshot.evidence));
    let baseline = GateBaseline {
        analysis_contract: ANALYSIS_CONTRACT.to_owned(),
        snapshot: capture.snapshot,
        protected_semantic_inputs,
        transition_sequence,
    };
    Ok(PreWriteAnalysis::Finished(PreWriteFinish {
        baseline: Some(baseline),
        leased_write_set,
        alias_closures,
        signals,
    }))
}

pub fn close_write_gate(request: &PostWriteRequest) -> Result<GateOperationResult, EngineError> {
    let request_digest = post_write_digest(&request.gate_id);
    let store = RepositoryStore::open(&request.root)?;
    let (gate, transitions, active_gates) =
        match store.begin_post_write(&request.operation_id, &request_digest, &request.gate_id)? {
            PostWriteStart::Committed(result) => return Ok(result),
            PostWriteStart::Analyze {
                gate,
                transitions,
                active_gates,
            } => (*gate, transitions, active_gates),
        };
    let baseline = gate
        .baseline
        .as_ref()
        .ok_or_else(|| EngineError::GateBaselineMissing(request.gate_id.as_str().to_owned()))?;
    if baseline.analysis_contract != ANALYSIS_CONTRACT {
        return finish_failed_close(
            &store,
            request,
            &request_digest,
            vec![GateSignal::AnalysisContractChanged],
        );
    }

    let capture =
        match capture_reserved_repository(&request.root, &gate.analysis_options, |paths| {
            store
                .reserve_post_write_semantic_inputs(
                    &request.operation_id,
                    &request_digest,
                    &request.gate_id,
                    paths,
                )
                .map_err(Into::into)
        }) {
            Ok(ReservedCapture::Finished { capture }) => capture,
            Ok(ReservedCapture::Blocked(signal)) => {
                return finish_failed_close(&store, request, &request_digest, vec![signal]);
            }
            Ok(ReservedCapture::Committed(result)) => return Ok(result),
            Err(EngineError::Store(error)) => return Err(EngineError::Store(error)),
            Err(error) => {
                return finish_failed_close(
                    &store,
                    request,
                    &request_digest,
                    vec![GateSignal::AnalysisFailed {
                        detail: error.to_string(),
                    }],
                );
            }
        };

    let (reconciled_baseline, reconciled_sequences, mut signals) =
        reconcile_transitions(&gate, baseline, &transitions);
    let protected_semantic_inputs = protected_semantic_inputs(&capture, &gate.leased_write_set);
    let changed_paths = changed_paths(
        &reconciled_baseline,
        &capture.snapshot,
        &gate.protected_semantic_inputs,
    );
    signals.extend(active_transition_signals(&changed_paths, &active_gates));
    let mut deltas = Vec::<GateDeltaRecord>::new();
    if !signals
        .iter()
        .any(|signal| matches!(signal, GateSignal::ActiveTransitionPending { .. }))
    {
        let (closing_signals, _, closing_deltas) = gate_policy::closing_signals(
            &reconciled_baseline,
            &capture.snapshot,
            &gate.protected_semantic_inputs,
            &gate.leased_write_set,
        );
        signals.extend(closing_signals);
        deltas = closing_deltas;
    }
    let (alias_closures, topology_signals) =
        close_alias_topology(&request.root, &gate, &capture.source_paths);
    signals.extend(topology_signals);

    store
        .finish_post_write(
            &request.operation_id,
            &request_digest,
            &request.gate_id,
            PostWriteFinish {
                snapshot: Some(capture.snapshot),
                protected_semantic_inputs,
                reconciled_baseline: Some(reconciled_baseline),
                changed_paths,
                alias_closures,
                reconciled_transition_sequences: reconciled_sequences,
                signals,
                deltas,
            },
        )
        .map_err(Into::into)
}

fn finish_failed_close(
    store: &RepositoryStore,
    request: &PostWriteRequest,
    request_digest: &str,
    signals: Vec<GateSignal>,
) -> Result<GateOperationResult, EngineError> {
    store
        .finish_post_write(
            &request.operation_id,
            request_digest,
            &request.gate_id,
            PostWriteFinish {
                snapshot: None,
                protected_semantic_inputs: Vec::new(),
                reconciled_baseline: None,
                changed_paths: Vec::new(),
                alias_closures: Vec::new(),
                reconciled_transition_sequences: Vec::new(),
                signals,
                deltas: Vec::new(),
            },
        )
        .map_err(Into::into)
}

enum ReservedCapture {
    Finished { capture: RepositoryCapture },
    Blocked(GateSignal),
    Committed(GateOperationResult),
}

fn capture_reserved_repository(
    root: &Path,
    options: &GateAnalysisOptions,
    mut reserve: impl FnMut(
        &[SemanticReadReservationBinding],
    ) -> Result<SemanticReadReservation, EngineError>,
) -> Result<ReservedCapture, EngineError> {
    let mut session =
        RepositoryAnalysisSession::start(root, &InventoryRequest::default(), options.jobs)?;
    loop {
        match session.next_step(options.resolution_profile)? {
            RepositoryAnalysisStep::NeedsInputs(demands) => {
                let reservations = demands
                    .iter()
                    .map(|demand| {
                        Ok(SemanticReadReservationBinding {
                            path: RepoPathProjection::from(&demand.path),
                            physical_identity: lumin_inventory::observe_config_physical_identity(
                                root,
                                &demand.path,
                            )?,
                        })
                    })
                    .collect::<Result<Vec<_>, EngineError>>()?;
                match reserve(&reservations)? {
                    SemanticReadReservation::Reserved => {
                        session.capture_demands(root, demands)?;
                    }
                    SemanticReadReservation::Conflict { paths, gate_ids } => {
                        return Ok(ReservedCapture::Blocked(
                            GateSignal::SemanticInputConflict { paths, gate_ids },
                        ));
                    }
                    SemanticReadReservation::TransitionCatalogChanged => {
                        return Ok(ReservedCapture::Blocked(
                            GateSignal::TransitionCatalogChanged,
                        ));
                    }
                    SemanticReadReservation::Committed(result) => {
                        return Ok(ReservedCapture::Committed(result));
                    }
                }
            }
            RepositoryAnalysisStep::Finished(resolver) => {
                return session
                    .finish(root, resolver)
                    .map(|capture| ReservedCapture::Finished { capture });
            }
        }
    }
}

pub fn load_gate(root: &Path, gate_id: &GateId) -> Result<GateRecord, EngineError> {
    RepositoryStore::open(root)?
        .load_gate(gate_id)
        .map_err(Into::into)
}

pub fn load_operation(
    root: &Path,
    operation_id: &OperationId,
) -> Result<OperationRecord, EngineError> {
    RepositoryStore::open(root)?
        .load_operation(operation_id)
        .map_err(Into::into)
}

fn inspect_declared_paths(
    root: &Path,
    paths: &[RepoPath],
) -> (
    Vec<WriteTargetObservation>,
    Vec<WriteLease>,
    Vec<GateSignal>,
) {
    let mut observations = Vec::new();
    let mut leases = Vec::new();
    let mut signals = Vec::new();
    for path in paths {
        let projection = RepoPathProjection::from(path);
        let Some(portable) = path.portable() else {
            signals.push(unsupported_path(
                projection,
                lumin_evidence::DeclaredPathUnsupportedReason::NotAnalyzedSource,
            ));
            continue;
        };
        if portable == ".lumin" || portable.starts_with(".lumin/") {
            signals.push(unsupported_path(
                projection,
                lumin_evidence::DeclaredPathUnsupportedReason::ReservedState,
            ));
            continue;
        }
        match lumin_inventory::inspect_write_target(root, path) {
            Ok(observation) => {
                if observation.kind == WriteTargetKind::NewFile
                    && !lumin_inventory::is_supported_source_path(path)
                {
                    signals.push(unsupported_path(
                        projection,
                        lumin_evidence::DeclaredPathUnsupportedReason::NotAnalyzedSource,
                    ));
                    continue;
                }
                leases.push(write_lease(&observation));
                observations.push(observation);
            }
            Err(error) => signals.push(write_target_signal(projection, error)),
        }
    }
    leases.sort();
    leases.dedup();
    (observations, leases, signals)
}

fn write_lease(observation: &WriteTargetObservation) -> WriteLease {
    let kind = match observation.kind {
        WriteTargetKind::ExistingFile => WriteLeaseKind::ExistingFile,
        WriteTargetKind::ExistingDirectory => WriteLeaseKind::Directory,
        WriteTargetKind::NewFile => WriteLeaseKind::NewFile,
    };
    WriteLease {
        path: RepoPathProjection::from(&observation.path),
        kind,
        physical_identity: observation.physical_identity.clone(),
        nearest_existing_parent: observation
            .nearest_existing_parent
            .as_ref()
            .map(RepoPathProjection::from),
        prefix_identities: observation
            .prefix_identities
            .iter()
            .map(|(path, physical_identity)| PathPrefixIdentity {
                path: RepoPathProjection::from(path),
                physical_identity: physical_identity.clone(),
            })
            .collect(),
    }
}

fn write_target_signal(path: RepoPathProjection, error: WriteTargetError) -> GateSignal {
    use lumin_evidence::DeclaredPathUnsupportedReason as Reason;
    let reason = match error {
        WriteTargetError::UnboundedDirectory => Reason::UnboundedDirectory,
        WriteTargetError::MissingParent(_) => Reason::MissingParent,
        WriteTargetError::OutsideRoot(_) => Reason::OutsideRoot,
        WriteTargetError::LinkedDirectory(_) => Reason::SymlinkOrAliasedPrefix,
        WriteTargetError::NonRegular(_) | WriteTargetError::Io { .. } => Reason::NonRegular,
        WriteTargetError::PhysicalIdentity(_) => {
            return GateSignal::AnalysisFailed {
                detail: error.to_string(),
            };
        }
    };
    unsupported_path(path, reason)
}

fn unsupported_path(
    path: RepoPathProjection,
    reason: lumin_evidence::DeclaredPathUnsupportedReason,
) -> GateSignal {
    GateSignal::DeclaredPathUnsupported { path, reason }
}

fn expand_write_domain(
    root: &Path,
    observations: &[WriteTargetObservation],
    mut leases: Vec<WriteLease>,
    capture: &RepositoryCapture,
) -> (
    Vec<WriteLease>,
    Vec<PhysicalAliasClosureRecord>,
    Vec<GateSignal>,
) {
    let mut seeds = BTreeSet::new();
    let mut signals = Vec::new();
    for observation in observations {
        match observation.kind {
            WriteTargetKind::ExistingFile => {
                if capture.source_paths.contains(&observation.path) {
                    seeds.insert(observation.path.clone());
                } else {
                    signals.push(unsupported_path(
                        RepoPathProjection::from(&observation.path),
                        lumin_evidence::DeclaredPathUnsupportedReason::NotAnalyzedSource,
                    ));
                }
            }
            WriteTargetKind::ExistingDirectory => {
                seeds.extend(
                    capture
                        .source_paths
                        .iter()
                        .filter(|path| path.is_within(&observation.path))
                        .cloned(),
                );
            }
            WriteTargetKind::NewFile => {}
        }
    }

    let mut groups = BTreeMap::<PhysicalFileIdentity, BTreeSet<RepoPath>>::new();
    for seed in seeds {
        match lumin_inventory::physical_alias_write_closure(root, &seed, &capture.source_paths) {
            Ok(closure) if closure.members.is_empty() => signals.push(unsupported_path(
                RepoPathProjection::from(&seed),
                lumin_evidence::DeclaredPathUnsupportedReason::NotAnalyzedSource,
            )),
            Ok(closure) => {
                groups
                    .entry(closure.physical_identity)
                    .or_default()
                    .extend(closure.members);
            }
            Err(error) => signals.push(GateSignal::AnalysisFailed {
                detail: error.to_string(),
            }),
        }
    }
    for member in groups.values().flatten() {
        match lumin_inventory::inspect_write_target(root, member) {
            Ok(observation) => leases.push(write_lease(&observation)),
            Err(error) => signals.push(GateSignal::AnalysisFailed {
                detail: error.to_string(),
            }),
        }
    }
    let alias_closures = alias_closure_records(groups);
    leases.sort();
    leases.dedup();
    (leases, alias_closures, signals)
}

fn alias_closure_records(
    groups: BTreeMap<PhysicalFileIdentity, BTreeSet<RepoPath>>,
) -> Vec<PhysicalAliasClosureRecord> {
    groups
        .into_iter()
        .map(|(physical_identity, members)| PhysicalAliasClosureRecord {
            physical_identity,
            members: members.iter().map(RepoPathProjection::from).collect(),
        })
        .collect()
}

fn protected_semantic_inputs(
    capture: &RepositoryCapture,
    leases: &[WriteLease],
) -> Vec<SemanticInputRecord> {
    let source_paths = capture
        .snapshot
        .inputs
        .iter()
        .filter(|input| input.state == SemanticInputState::Source)
        .map(|input| input.path.canonical.as_slice())
        .collect::<BTreeSet<_>>();
    let protect_all_sources = leases
        .iter()
        .any(|lease| lease.kind == WriteLeaseKind::NewFile)
        || leases
            .iter()
            .any(|lease| lease.kind == WriteLeaseKind::Directory);
    let mut selected = if protect_all_sources {
        capture
            .source_paths
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
    } else {
        leases
            .iter()
            .filter(|lease| lease.kind == WriteLeaseKind::ExistingFile)
            .filter_map(|lease| {
                capture
                    .source_paths
                    .iter()
                    .find(|path| path.canonical_bytes() == lease.path.canonical)
                    .cloned()
            })
            .collect::<BTreeSet<_>>()
    };
    let mut frontier = selected.iter().cloned().collect::<Vec<_>>();
    while let Some(path) = frontier.pop() {
        let Some(neighbors) = capture.source_adjacency.get(&path) else {
            continue;
        };
        for neighbor in neighbors {
            if selected.insert(neighbor.clone()) {
                frontier.push(neighbor.clone());
            }
        }
    }
    let selected_keys = selected
        .iter()
        .map(|path| path.canonical_bytes())
        .collect::<BTreeSet<_>>();
    let mut protected = capture
        .snapshot
        .inputs
        .iter()
        .filter(|input| {
            !source_paths.contains(input.path.canonical.as_slice())
                || selected_keys.contains(input.path.canonical.as_slice())
        })
        .cloned()
        .collect::<Vec<_>>();
    protected.sort();
    protected.dedup();
    protected
}

fn reconcile_transitions(
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

fn changed_paths(
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

fn active_transition_signals(
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

fn close_alias_topology(
    root: &Path,
    gate: &GateRecord,
    current_sources: &[RepoPath],
) -> (Vec<PhysicalAliasClosureRecord>, Vec<GateSignal>) {
    let mut signals = validate_stable_lease_parents(root, &gate.leased_write_set);
    let seeds = current_sources
        .iter()
        .filter(|path| {
            let projection = RepoPathProjection::from(*path);
            gate.leased_write_set
                .iter()
                .any(|lease| lease.covers(&projection))
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut groups = BTreeMap::<PhysicalFileIdentity, BTreeSet<RepoPath>>::new();
    for seed in seeds {
        match lumin_inventory::physical_alias_write_closure(root, &seed, current_sources) {
            Ok(closure) => {
                for member in &closure.members {
                    let projection = RepoPathProjection::from(member);
                    if !gate
                        .leased_write_set
                        .iter()
                        .any(|lease| lease.covers(&projection))
                    {
                        signals.push(GateSignal::UnplannedWrite {
                            paths: vec![projection],
                        });
                    }
                }
                groups
                    .entry(closure.physical_identity)
                    .or_default()
                    .extend(closure.members);
            }
            Err(error) => signals.push(GateSignal::AnalysisFailed {
                detail: error.to_string(),
            }),
        }
    }
    (alias_closure_records(groups), signals)
}

fn validate_stable_lease_parents(root: &Path, leases: &[WriteLease]) -> Vec<GateSignal> {
    let mut stale = Vec::new();
    let mut incomplete = Vec::new();
    for lease in leases {
        for prefix in &lease.prefix_identities {
            let prefix_path = match RepoPath::from_canonical_bytes(&prefix.path.canonical) {
                Ok(path) => path,
                Err(error) => {
                    incomplete.push(format!(
                        "stored write lease prefix is not canonical: {} ({error})",
                        prefix.path.display
                    ));
                    continue;
                }
            };
            match lumin_inventory::directory_physical_identity(root, &prefix_path) {
                Ok(identity) if identity == prefix.physical_identity => {}
                Ok(_) | Err(WriteTargetError::OutsideRoot(_)) => {
                    stale.push(prefix.path.clone());
                }
                Err(error) => incomplete.push(error.to_string()),
            }
        }
        let path = match RepoPath::from_canonical_bytes(&lease.path.canonical) {
            Ok(path) => path,
            Err(error) => {
                incomplete.push(format!(
                    "stored write lease path is not canonical: {} ({error})",
                    lease.path.display
                ));
                continue;
            }
        };
        match lease.kind {
            WriteLeaseKind::Directory => match lumin_inventory::inspect_write_target(root, &path) {
                Ok(observation)
                    if observation.kind == WriteTargetKind::ExistingDirectory
                        && observation.physical_identity == lease.physical_identity => {}
                Ok(_) | Err(WriteTargetError::OutsideRoot(_)) => stale.push(lease.path.clone()),
                Err(error) => incomplete.push(error.to_string()),
            },
            WriteLeaseKind::NewFile => {
                let nearest_matches =
                    lease
                        .nearest_existing_parent
                        .as_ref()
                        .is_some_and(|nearest| {
                            lease
                                .prefix_identities
                                .last()
                                .is_some_and(|prefix| prefix.path.canonical == nearest.canonical)
                        });
                if !nearest_matches {
                    incomplete.push(format!(
                        "new path omitted its nearest existing parent binding: {}",
                        lease.path.display
                    ));
                    continue;
                }
                match lumin_inventory::inspect_write_target(root, &path) {
                    Ok(observation)
                        if matches!(
                            observation.kind,
                            WriteTargetKind::ExistingFile | WriteTargetKind::NewFile
                        ) => {}
                    Ok(_) | Err(WriteTargetError::OutsideRoot(_)) => {
                        stale.push(lease.path.clone());
                    }
                    Err(error) => incomplete.push(error.to_string()),
                }
            }
            WriteLeaseKind::ExistingFile => {}
        }
    }
    let mut signals = Vec::new();
    if !stale.is_empty() {
        stale.sort();
        stale.dedup();
        signals.push(GateSignal::ProtectedInputChanged { paths: stale });
    }
    signals.extend(
        incomplete
            .into_iter()
            .map(|detail| GateSignal::AnalysisFailed { detail }),
    );
    signals
}

fn pre_write_digest(paths: &[RepoPath], options: &GateAnalysisOptions) -> String {
    let mut bytes = Vec::new();
    append_length_prefixed(&mut bytes, b"lumin-pre-write.v2");
    bytes.extend_from_slice(&(options.jobs as u64).to_be_bytes());
    append_length_prefixed(
        &mut bytes,
        options
            .resolution_profile
            .map_or(b"default".as_slice(), |profile| profile.as_str().as_bytes()),
    );
    bytes.extend_from_slice(&(paths.len() as u64).to_be_bytes());
    for path in paths {
        append_length_prefixed(&mut bytes, path.canonical_bytes());
    }
    digest_hex(&bytes)
}

fn post_write_digest(gate_id: &GateId) -> String {
    let mut bytes = Vec::new();
    append_length_prefixed(&mut bytes, b"lumin-post-write.v2");
    append_length_prefixed(&mut bytes, gate_id.as_str().as_bytes());
    digest_hex(&bytes)
}
