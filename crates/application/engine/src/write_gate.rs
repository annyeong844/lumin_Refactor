use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use lumin_evidence::{
    DependencyIntentRecord, GateAnalysisOptions, GateBaseline, GateOperationResult, GateRecord,
    GateSignal, OperationRecord, PathPrefixIdentity, RepoPathProjection, ScanInvocationTier,
    SemanticInputRecord, SemanticInputState, SemanticReadReservationBinding, gate_policy,
    seal_analysis_snapshot,
};
use lumin_inventory::InventoryRequest;
use lumin_model::{
    DependencyIntent, GateDeltaRecord, GateId, OperationId, PhysicalFileIdentity, RepoPath,
    RepositoryRootIdentity, ResolutionProfile, append_length_prefixed, digest_hex,
};
use lumin_store::{
    OperationSession, PostWriteFinish, PostWriteStart, PreWriteFinish, PreWriteStart,
    SemanticReadReservation,
};

use super::{
    EngineError, RepositoryAnalysisSession, RepositoryAnalysisStep, RepositoryCapture,
    RepositoryContext, open_repository_context, repository_context_from_admission,
};

mod domain;
mod transitions;

use domain::{
    DeclaredPathInspection, close_alias_topology, expand_write_domain, inspect_declared_paths,
    protected_semantic_inputs,
};
use transitions::{
    active_transition_signals, changed_paths, closure_expanded_actual_write_set,
    reconcile_transitions,
};

const ANALYSIS_CONTRACT_VERSION: &[u8] = b"lumin-analysis-contract.phase1-foundation.v26";

fn analysis_contract_id() -> String {
    let inputs = [
        ANALYSIS_CONTRACT_VERSION,
        lumin_model::PATH_CODEC_ARTIFACT_SHA256.as_bytes(),
        lumin_model::PATH_CODEC_TABLE_SHA256.as_bytes(),
        lumin_model::SOURCE_CLASSIFICATION_RULE_VERSION.as_bytes(),
        lumin_inventory::INVENTORY_CONFIG_ARTIFACT_SHA256.as_bytes(),
        lumin_inventory::INVENTORY_CONFIG_TABLE_SHA256.as_bytes(),
        lumin_js::EXTRACTOR_SEMANTICS_VERSION.as_bytes(),
        lumin_graph::SYMBOL_GRAPH_SEMANTICS_VERSION.as_bytes(),
        lumin_resolve::RESOLVER_VERSION.as_bytes(),
        lumin_resolve::RESOLVER_CONFIG_ARTIFACT_SHA256.as_bytes(),
        lumin_resolve::RESOLVER_CONFIG_TABLE_SHA256.as_bytes(),
    ];
    let mut framed = Vec::new();
    for input in inputs {
        append_length_prefixed(&mut framed, input);
    }
    digest_hex(&framed)
}

#[derive(Clone, Debug)]
pub struct PreWriteRequest {
    pub root: PathBuf,
    pub operation_id: OperationId,
    pub paths: Vec<RepoPath>,
    pub includes: Vec<String>,
    pub excludes: Vec<String>,
    pub role_overrides: Vec<lumin_model::RoleOverride>,
    pub entries: Vec<RepoPath>,
    pub dependency_intents: Vec<DependencyIntent>,
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
    lumin_inventory::validate_caller_paths_lexically(&request.paths)?;
    validate_analysis_path_names(&request.entries, &request.dependency_intents)?;
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
    // Build the exact tier from the request
    let scan_invocation = build_gate_scan_invocation_tier(request);
    let analysis_options = GateAnalysisOptions {
        jobs: request.jobs,
        resolution_profile: request.resolution_profile,
        scan_invocation: scan_invocation.clone(),
    };
    let request_digest = pre_write_digest(&paths, &analysis_options);
    let admission = lumin_inventory::repository_admission(&request.root)?;
    let store = match lumin_store::RepositoryStore::open_if_bound(
        &admission.canonical_root,
        &admission.binding,
    )? {
        Some(store) => store,
        None => {
            lumin_inventory::validate_caller_entries(&admission.canonical_root, &paths)?;
            validate_analysis_paths(
                &admission.canonical_root,
                &request.entries,
                &request.dependency_intents,
            )?;
            lumin_store::RepositoryStore::open(&admission.canonical_root, &admission.binding)?
        }
    };
    let context = repository_context_from_admission(admission, store);
    if let Some(result) = context
        .store
        .replay_pre_write_result(&request.operation_id, &request_digest)?
    {
        return Ok(result);
    }
    lumin_inventory::validate_caller_entries(&context.root, &paths)?;
    validate_analysis_paths(&context.root, &request.entries, &request.dependency_intents)?;
    let reserved_state_identities = context.store.reserved_state_identities()?;
    lumin_inventory::validate_caller_entry_identities(
        &context.root,
        &paths,
        &reserved_state_identities,
    )?;
    validate_analysis_path_identities(
        &context.root,
        &request.entries,
        &request.dependency_intents,
        &reserved_state_identities,
    )?;
    let inspection = inspect_declared_paths(&context.root, &paths);
    let operation = context.store.begin_operation(&request.operation_id)?;
    let (gate_id, transition_sequence) = match operation.reserve_pre_write(
        &request_digest,
        &declared_write_set,
        &inspection.leases,
        &analysis_options,
    )? {
        PreWriteStart::Committed(result) => return Ok(*result),
        PreWriteStart::Analyze {
            gate_id,
            transition_sequence,
        } => (gate_id, transition_sequence),
    };

    let promotion = if inspection.signals.is_empty() {
        match analyze_pre_write(
            &operation,
            &context,
            request,
            inspection,
            transition_sequence,
            &request_digest,
            &gate_id,
        )? {
            PreWriteAnalysis::Finished(promotion) => promotion,
            PreWriteAnalysis::Committed(result) => return Ok(result),
        }
    } else {
        PreWritePromotion::without_validation(PreWriteFinish {
            baseline: None,
            leased_write_set: inspection.leases,
            alias_closures: Vec::new(),
            signals: inspection.signals,
        })
    };
    let PreWritePromotion {
        finish,
        final_validation,
    } = promotion;
    operation
        .finish_pre_write(&request_digest, &gate_id, finish, || {
            final_validation.map_or_else(Vec::new, |validation| {
                final_freshness_validation_signals(&context.root, &validation)
            })
        })
        .map_err(Into::into)
}

/// Build the exact ScanInvocationTier from a PreWriteRequest with normalized entries.
fn build_gate_scan_invocation_tier(request: &PreWriteRequest) -> ScanInvocationTier {
    let mut entries: Vec<RepoPathProjection> = request
        .entries
        .iter()
        .map(RepoPathProjection::from)
        .collect();
    entries.sort();
    entries.dedup();
    let mut dependency_intents = request
        .dependency_intents
        .iter()
        .map(|intent| DependencyIntentRecord {
            path: RepoPathProjection::from(&intent.path),
            dependency: intent.dependency.clone(),
        })
        .collect::<Vec<_>>();
    dependency_intents.sort();
    dependency_intents.dedup();
    ScanInvocationTier {
        includes: request.includes.clone(),
        excludes: request.excludes.clone(),
        role_overrides: request.role_overrides.clone(),
        entries,
        dependency_intents,
        resolution_profile: request.resolution_profile,
    }
}

#[allow(clippy::large_enum_variant)]
enum PreWriteAnalysis {
    Finished(PreWritePromotion),
    Committed(GateOperationResult),
}

struct PreWritePromotion {
    finish: PreWriteFinish,
    final_validation: Option<FinalFreshnessValidation>,
}

impl PreWritePromotion {
    fn without_validation(finish: PreWriteFinish) -> Self {
        Self {
            finish,
            final_validation: None,
        }
    }
}

struct FinalFreshnessValidation {
    bindings: Vec<(RepoPath, SemanticReadReservationBinding)>,
    captured_inputs: Vec<SemanticInputRecord>,
    reserved_state_identities: BTreeSet<PhysicalFileIdentity>,
}

fn analyze_pre_write(
    operation: &OperationSession<'_>,
    context: &RepositoryContext,
    request: &PreWriteRequest,
    inspection: DeclaredPathInspection,
    transition_sequence: u64,
    request_digest: &str,
    gate_id: &GateId,
) -> Result<PreWriteAnalysis, EngineError> {
    let options = GateAnalysisOptions {
        jobs: request.jobs,
        resolution_profile: request.resolution_profile,
        scan_invocation: build_gate_scan_invocation_tier(request),
    };
    let inventory_request = inventory_request_from_tier(&options.scan_invocation)?;
    let capture = match capture_reserved_repository(
        &context.store,
        &context.root,
        &context.repository_root,
        &options,
        &inventory_request,
        |paths| {
            operation
                .reserve_pre_write_semantic_inputs(request_digest, gate_id, paths)
                .map_err(Into::into)
        },
    ) {
        Ok(ReservedCapture::Finished {
            capture,
            reserved_semantic_bindings,
        }) => (capture, reserved_semantic_bindings),
        Ok(ReservedCapture::Blocked(signal)) => {
            return Ok(PreWriteAnalysis::Finished(
                PreWritePromotion::without_validation(PreWriteFinish {
                    baseline: None,
                    leased_write_set: inspection.leases,
                    alias_closures: Vec::new(),
                    signals: vec![signal],
                }),
            ));
        }
        Ok(ReservedCapture::Committed(result)) => {
            return Ok(PreWriteAnalysis::Committed(result));
        }
        Err(EngineError::Store(error)) => return Err(EngineError::Store(error)),
        Err(error) => {
            return Ok(PreWriteAnalysis::Finished(
                PreWritePromotion::without_validation(PreWriteFinish {
                    baseline: None,
                    leased_write_set: inspection.leases,
                    alias_closures: Vec::new(),
                    signals: vec![GateSignal::AnalysisFailed {
                        detail: error.to_string(),
                    }],
                }),
            ));
        }
    };
    let (capture, reserved_semantic_bindings) = capture;
    let (leased_write_set, alias_closures, mut signals) = expand_write_domain(
        &context.root,
        &inspection.observations,
        inspection.leases,
        &capture,
    );
    let protected_semantic_inputs = protected_semantic_inputs(&capture, &leased_write_set);
    signals.extend(gate_policy::opening_signals(
        &capture.snapshot,
        &leased_write_set,
    ));
    let final_validation = FinalFreshnessValidation {
        bindings: reserved_semantic_bindings,
        captured_inputs: capture.snapshot.inputs.clone(),
        reserved_state_identities: context.store.reserved_state_identities()?,
    };
    let baseline = GateBaseline {
        analysis_contract: analysis_contract_id(),
        snapshot: capture.snapshot,
        protected_semantic_inputs,
        transition_sequence,
    };
    Ok(PreWriteAnalysis::Finished(PreWritePromotion {
        finish: PreWriteFinish {
            baseline: Some(baseline),
            leased_write_set,
            alias_closures,
            signals,
        },
        final_validation: Some(final_validation),
    }))
}

pub fn close_write_gate(request: &PostWriteRequest) -> Result<GateOperationResult, EngineError> {
    let request_digest = post_write_digest(&request.gate_id);
    let context = open_repository_context(&request.root)?;
    let operation = context.store.begin_operation(&request.operation_id)?;
    let (gate, transitions, active_gates) =
        match operation.begin_post_write(&request_digest, &request.gate_id)? {
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
    if baseline.analysis_contract != analysis_contract_id() {
        return finish_failed_close(
            &operation,
            request,
            &request_digest,
            vec![GateSignal::AnalysisContractChanged],
        );
    }

    // Reconstruct InventoryRequest from persisted tier (not default)
    let inventory_request = inventory_request_from_tier(&gate.analysis_options.scan_invocation)?;
    if let Err(error) = validate_analysis_paths(
        &context.root,
        &inventory_request.entries,
        &inventory_request.dependency_intents,
    ) {
        return finish_failed_close(
            &operation,
            request,
            &request_digest,
            vec![GateSignal::AnalysisFailed {
                detail: error.to_string(),
            }],
        );
    }

    // Validate tier resolution_profile agrees with legacy options.resolution_profile
    if gate.analysis_options.scan_invocation.resolution_profile
        != gate.analysis_options.resolution_profile
    {
        return Err(EngineError::TierProfileInconsistency(format!(
            "tier profile {:?} != options profile {:?}",
            gate.analysis_options.scan_invocation.resolution_profile,
            gate.analysis_options.resolution_profile
        )));
    }

    let (capture, reserved_semantic_bindings) = match capture_reserved_repository(
        &context.store,
        &context.root,
        &context.repository_root,
        &gate.analysis_options,
        &inventory_request,
        |paths| {
            operation
                .reserve_post_write_semantic_inputs(&request_digest, &request.gate_id, paths)
                .map_err(Into::into)
        },
    ) {
        Ok(ReservedCapture::Finished {
            capture,
            reserved_semantic_bindings,
        }) => (capture, reserved_semantic_bindings),
        Ok(ReservedCapture::Blocked(signal)) => {
            return finish_failed_close(&operation, request, &request_digest, vec![signal]);
        }
        Ok(ReservedCapture::Committed(result)) => return Ok(result),
        Err(EngineError::Store(error)) => return Err(EngineError::Store(error)),
        Err(error) => {
            return finish_failed_close(
                &operation,
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
    let preliminary_changed_paths = changed_paths(
        &reconciled_baseline,
        &capture.snapshot,
        &gate.protected_semantic_inputs,
        &gate.leased_write_set,
    );
    signals.extend(active_transition_signals(
        &preliminary_changed_paths,
        &active_gates,
    ));
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
    let (alias_closures, topology_signals) = close_alias_topology(&context.root, &gate, &capture);
    let actual_write_set = if gate_policy::actual_write_attribution_is_complete(&signals)
        && gate_policy::actual_write_attribution_is_complete(&topology_signals)
    {
        Some(closure_expanded_actual_write_set(
            &preliminary_changed_paths,
            &gate.alias_closures,
            &alias_closures,
        ))
    } else {
        None
    };
    signals.extend(topology_signals);
    let changed_paths = actual_write_set
        .as_ref()
        .map_or(preliminary_changed_paths, |actual| actual.paths.clone());

    let final_validation = FinalFreshnessValidation {
        bindings: reserved_semantic_bindings,
        captured_inputs: capture.snapshot.inputs.clone(),
        reserved_state_identities: context.store.reserved_state_identities()?,
    };
    operation
        .finish_post_write(
            &request_digest,
            &request.gate_id,
            PostWriteFinish {
                snapshot: Some(capture.snapshot),
                protected_semantic_inputs,
                reconciled_baseline: Some(reconciled_baseline),
                changed_paths,
                actual_write_set,
                alias_closures,
                reconciled_transition_sequences: reconciled_sequences,
                signals,
                deltas,
            },
            || final_freshness_validation_signals(&context.root, &final_validation),
        )
        .map_err(Into::into)
}

fn validate_analysis_paths(
    root: &Path,
    entries: &[RepoPath],
    dependency_intents: &[DependencyIntent],
) -> Result<(), EngineError> {
    lumin_inventory::validate_caller_entries(root, entries)?;
    let dependency_paths = dependency_intents
        .iter()
        .map(|intent| intent.path.clone())
        .collect::<Vec<_>>();
    lumin_inventory::validate_caller_entries(root, &dependency_paths)?;
    Ok(())
}

fn validate_analysis_path_names(
    entries: &[RepoPath],
    dependency_intents: &[DependencyIntent],
) -> Result<(), EngineError> {
    lumin_inventory::validate_caller_paths_lexically(entries)?;
    let dependency_paths = dependency_intents
        .iter()
        .map(|intent| intent.path.clone())
        .collect::<Vec<_>>();
    lumin_inventory::validate_caller_paths_lexically(&dependency_paths)?;
    Ok(())
}

fn validate_analysis_path_identities(
    root: &Path,
    entries: &[RepoPath],
    dependency_intents: &[DependencyIntent],
    reserved_state_identities: &std::collections::BTreeSet<lumin_model::PhysicalFileIdentity>,
) -> Result<(), EngineError> {
    lumin_inventory::validate_caller_entry_identities(root, entries, reserved_state_identities)?;
    let dependency_paths = dependency_intents
        .iter()
        .map(|intent| intent.path.clone())
        .collect::<Vec<_>>();
    lumin_inventory::validate_caller_entry_identities(
        root,
        &dependency_paths,
        reserved_state_identities,
    )?;
    Ok(())
}

fn finish_failed_close(
    operation: &OperationSession<'_>,
    request: &PostWriteRequest,
    request_digest: &str,
    signals: Vec<GateSignal>,
) -> Result<GateOperationResult, EngineError> {
    operation
        .finish_post_write(
            request_digest,
            &request.gate_id,
            PostWriteFinish {
                snapshot: None,
                protected_semantic_inputs: Vec::new(),
                reconciled_baseline: None,
                changed_paths: Vec::new(),
                actual_write_set: None,
                alias_closures: Vec::new(),
                reconciled_transition_sequences: Vec::new(),
                signals,
                deltas: Vec::new(),
            },
            Vec::new,
        )
        .map_err(Into::into)
}

enum ReservedCapture {
    Finished {
        capture: Box<RepositoryCapture>,
        reserved_semantic_bindings: Vec<(RepoPath, SemanticReadReservationBinding)>,
    },
    Blocked(GateSignal),
    Committed(GateOperationResult),
}

fn capture_reserved_repository(
    store: &lumin_store::RepositoryStore,
    root: &Path,
    repository_root: &RepositoryRootIdentity,
    options: &GateAnalysisOptions,
    inventory_request: &InventoryRequest,
    mut reserve: impl FnMut(
        &[SemanticReadReservationBinding],
    ) -> Result<SemanticReadReservation, EngineError>,
) -> Result<ReservedCapture, EngineError> {
    let mut reserved_semantic_bindings = Vec::new();
    let dependency_candidates =
        lumin_inventory::dependency_owner_candidate_paths(&inventory_request.dependency_intents)?;
    if let Some(outcome) = reserve_semantic_paths(
        root,
        &dependency_candidates,
        &mut reserved_semantic_bindings,
        &mut reserve,
    )? {
        return Ok(outcome);
    }
    let dependency_candidate_binding_count = reserved_semantic_bindings.len();

    let reserved_state_identities = store.reserved_state_identities()?;
    let pending_inventory = lumin_inventory::begin_scan_with_reserved_state_identities(
        root,
        inventory_request,
        &reserved_state_identities,
    )?;
    if let Some(outcome) = reserve_semantic_paths(
        root,
        pending_inventory.dependency_input_paths(),
        &mut reserved_semantic_bindings,
        &mut reserve,
    )? {
        return Ok(outcome);
    }
    let inventory = pending_inventory.finish(root)?;
    let mut session = RepositoryAnalysisSession::start_with_inventory(
        repository_root.clone(),
        inventory,
        options.jobs,
        options.scan_invocation.clone(),
        reserved_state_identities.clone(),
    )?;
    loop {
        match session.next_step(options.resolution_profile)? {
            RepositoryAnalysisStep::NeedsInputs(demands) => {
                let paths = demands
                    .iter()
                    .map(|demand| demand.path.clone())
                    .collect::<Vec<_>>();
                if let Some(outcome) = reserve_semantic_paths(
                    root,
                    &paths,
                    &mut reserved_semantic_bindings,
                    &mut reserve,
                )? {
                    return Ok(outcome);
                }
                session.capture_demands(root, demands)?;
            }
            RepositoryAnalysisStep::Finished(resolver) => {
                let mut capture = session.finish(resolver)?;
                include_dependency_candidate_topology_inputs(
                    &mut capture,
                    &reserved_semantic_bindings[..dependency_candidate_binding_count],
                );
                let stale = stale_reserved_semantic_paths(
                    root,
                    &reserved_semantic_bindings,
                    &capture.snapshot.inputs,
                    &reserved_state_identities,
                )?;
                if !stale.is_empty() {
                    return Ok(ReservedCapture::Blocked(
                        GateSignal::ProtectedInputChanged { paths: stale },
                    ));
                }
                return Ok(ReservedCapture::Finished {
                    capture: Box::new(capture),
                    reserved_semantic_bindings,
                });
            }
        }
    }
}

fn reserve_semantic_paths(
    root: &Path,
    paths: &[RepoPath],
    reserved_bindings: &mut Vec<(RepoPath, SemanticReadReservationBinding)>,
    reserve: &mut impl FnMut(
        &[SemanticReadReservationBinding],
    ) -> Result<SemanticReadReservation, EngineError>,
) -> Result<Option<ReservedCapture>, EngineError> {
    if paths.is_empty() {
        return Ok(None);
    }
    let reservations = semantic_read_reservations(root, paths)?;
    let outcome = match reserve(&reservations)? {
        SemanticReadReservation::Reserved => {
            reserved_bindings.extend(paths.iter().cloned().zip(reservations));
            return Ok(None);
        }
        SemanticReadReservation::Conflict { paths, gate_ids } => {
            ReservedCapture::Blocked(GateSignal::SemanticInputConflict { paths, gate_ids })
        }
        SemanticReadReservation::TransitionCatalogChanged => {
            ReservedCapture::Blocked(GateSignal::TransitionCatalogChanged)
        }
        SemanticReadReservation::Committed(result) => ReservedCapture::Committed(*result),
    };
    Ok(Some(outcome))
}

fn stale_reserved_semantic_paths(
    root: &Path,
    bindings: &[(RepoPath, SemanticReadReservationBinding)],
    captured_inputs: &[SemanticInputRecord],
    reserved_state_identities: &BTreeSet<PhysicalFileIdentity>,
) -> Result<Vec<RepoPathProjection>, EngineError> {
    let captured_by_path = captured_inputs
        .iter()
        .map(|input| (input.path.canonical.as_slice(), input))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut stale = Vec::new();
    for (path, reserved) in bindings {
        let current = semantic_read_reservation(root, path)?;
        if current != *reserved {
            stale.push(current.path);
            continue;
        }
        let Some(captured) = captured_by_path.get(reserved.path.canonical.as_slice()) else {
            stale.push(current.path);
            continue;
        };
        if captured.state == SemanticInputState::ConfigPresent {
            let current_payload =
                lumin_inventory::dependency_input_payload_sha256_with_reserved_state_identities(
                    root,
                    path,
                    reserved_state_identities,
                )?;
            if captured.payload_sha256.as_deref() != Some(current_payload.as_str()) {
                stale.push(current.path);
            }
        }
    }
    stale.sort();
    stale.dedup();
    Ok(stale)
}

fn final_freshness_validation_signals(
    root: &Path,
    validation: &FinalFreshnessValidation,
) -> Vec<GateSignal> {
    match stale_reserved_semantic_paths(
        root,
        &validation.bindings,
        &validation.captured_inputs,
        &validation.reserved_state_identities,
    ) {
        Ok(paths) if paths.is_empty() => Vec::new(),
        Ok(paths) => vec![GateSignal::ProtectedInputChanged { paths }],
        Err(error) => vec![GateSignal::AnalysisFailed {
            detail: error.to_string(),
        }],
    }
}

fn include_dependency_candidate_topology_inputs(
    capture: &mut RepositoryCapture,
    bindings: &[(RepoPath, SemanticReadReservationBinding)],
) {
    let mut inputs = capture.snapshot.inputs.clone();
    for (_, binding) in bindings {
        if inputs.iter().any(|input| input.path == binding.path) {
            continue;
        }
        let (state, payload_sha256) = if binding.absence_parent.is_some() {
            (SemanticInputState::Missing, None)
        } else {
            (
                SemanticInputState::Unreadable,
                Some(digest_hex(b"dependency-candidate-topology-only.v1")),
            )
        };
        inputs.push(SemanticInputRecord {
            path: binding.path.clone(),
            state,
            payload_sha256,
            physical_identity: binding.physical_identity.clone(),
            absence_parent: binding.absence_parent.clone(),
            physical_redirect_sha256: None,
        });
    }
    capture.snapshot = seal_analysis_snapshot(
        inputs,
        capture.snapshot.evidence.clone(),
        capture.snapshot.scan_invocation.clone(),
        capture.snapshot.entry_selections.clone(),
    );
}

fn semantic_read_reservations(
    root: &Path,
    paths: &[RepoPath],
) -> Result<Vec<SemanticReadReservationBinding>, EngineError> {
    paths
        .iter()
        .map(|path| semantic_read_reservation(root, path))
        .collect()
}

fn semantic_read_reservation(
    root: &Path,
    path: &RepoPath,
) -> Result<SemanticReadReservationBinding, EngineError> {
    let identity = lumin_inventory::observe_config_input_identity(root, path)?;
    Ok(SemanticReadReservationBinding {
        path: RepoPathProjection::from(path),
        physical_identity: identity.physical_identity,
        absence_parent: identity.absence_parent.map(|parent| PathPrefixIdentity {
            path: RepoPathProjection::from(&parent.path),
            physical_identity: parent.physical_identity,
        }),
    })
}

pub fn load_gate(root: &Path, gate_id: &GateId) -> Result<GateRecord, EngineError> {
    open_repository_context(root)?
        .store
        .load_gate(gate_id)
        .map_err(Into::into)
}

/// Reconstruct a full InventoryRequest from a persisted ScanInvocationTier.
/// Each RepoPathProjection is canonical-decoded and reprojected for full equality.
/// Any corruption or inconsistency returns EngineError (fail-closed, never filter_map).
fn inventory_request_from_tier(tier: &ScanInvocationTier) -> Result<InventoryRequest, EngineError> {
    let mut entries = Vec::with_capacity(tier.entries.len());
    for projection in &tier.entries {
        let path = RepoPath::from_canonical_bytes(&projection.canonical).map_err(|error| {
            EngineError::TierProjectionCorrupt(format!(
                "failed to decode entry projection {}: {error}",
                projection.display
            ))
        })?;
        // Reprojection validation
        let reprojected = RepoPathProjection::from(&path);
        if reprojected != *projection {
            return Err(EngineError::TierProjectionCorrupt(format!(
                "entry projection round-trip failed for {}",
                projection.display
            )));
        }
        entries.push(path);
    }
    let mut dependency_intents = Vec::with_capacity(tier.dependency_intents.len());
    for record in &tier.dependency_intents {
        let path = RepoPath::from_canonical_bytes(&record.path.canonical).map_err(|error| {
            EngineError::TierProjectionCorrupt(format!(
                "failed to decode dependency-intent projection {}: {error}",
                record.path.display
            ))
        })?;
        if RepoPathProjection::from(&path) != record.path {
            return Err(EngineError::TierProjectionCorrupt(format!(
                "dependency-intent projection round-trip failed for {}",
                record.path.display
            )));
        }
        dependency_intents.push(DependencyIntent {
            path,
            dependency: record.dependency.clone(),
        });
    }
    Ok(InventoryRequest {
        includes: tier.includes.clone(),
        excludes: tier.excludes.clone(),
        role_overrides: tier.role_overrides.clone(),
        entries,
        dependency_intents,
    })
}

pub fn load_operation(
    root: &Path,
    operation_id: &OperationId,
) -> Result<OperationRecord, EngineError> {
    open_repository_context(root)?
        .store
        .load_operation(operation_id)
        .map_err(Into::into)
}

fn pre_write_digest(paths: &[RepoPath], options: &GateAnalysisOptions) -> String {
    let mut bytes = Vec::new();
    append_length_prefixed(&mut bytes, b"lumin-pre-write.v4");
    // Use canonical tier framing for all invocation parameters
    options.scan_invocation.append_semantic_framing(&mut bytes);
    // Declared write paths
    bytes.extend_from_slice(&(paths.len() as u64).to_be_bytes());
    for path in paths {
        append_length_prefixed(&mut bytes, path.canonical_bytes());
    }
    // NOTE: jobs is deliberately excluded
    digest_hex(&bytes)
}

fn post_write_digest(gate_id: &GateId) -> String {
    let mut bytes = Vec::new();
    append_length_prefixed(&mut bytes, b"lumin-post-write.v2");
    append_length_prefixed(&mut bytes, gate_id.as_str().as_bytes());
    digest_hex(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_freshness_rejects_a_created_reserved_dependency_candidate()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        std::fs::create_dir_all(root.path().join("apps/project/src"))?;
        let candidate = RepoPath::from_portable("apps/project/package.json")?;
        let binding = semantic_read_reservations(root.path(), std::slice::from_ref(&candidate))?
            .pop()
            .ok_or("missing semantic-read reservation")?;
        let reserved = (candidate.clone(), binding.clone());
        let captured = SemanticInputRecord {
            path: binding.path.clone(),
            state: SemanticInputState::Missing,
            payload_sha256: None,
            physical_identity: binding.physical_identity.clone(),
            absence_parent: binding.absence_parent.clone(),
            physical_redirect_sha256: None,
        };

        assert!(
            stale_reserved_semantic_paths(
                root.path(),
                std::slice::from_ref(&reserved),
                std::slice::from_ref(&captured),
                &BTreeSet::new(),
            )?
            .is_empty()
        );

        std::fs::write(
            root.path().join("apps/project/package.json"),
            r#"{"name":"closer-owner"}"#,
        )?;
        assert_eq!(
            stale_reserved_semantic_paths(
                root.path(),
                std::slice::from_ref(&reserved),
                std::slice::from_ref(&captured),
                &BTreeSet::new(),
            )?,
            vec![binding.path]
        );
        Ok(())
    }

    #[test]
    fn final_freshness_rehashes_present_reserved_dependency_inputs()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let candidate = RepoPath::from_portable("package.json")?;
        std::fs::write(
            root.path().join("package.json"),
            r#"{"name":"root","workspaces":["packages/*"]}"#,
        )?;
        let binding = semantic_read_reservations(root.path(), std::slice::from_ref(&candidate))?
            .pop()
            .ok_or("missing semantic-read reservation")?;
        let captured = SemanticInputRecord {
            path: binding.path.clone(),
            state: SemanticInputState::ConfigPresent,
            payload_sha256: Some(lumin_inventory::dependency_input_payload_sha256(
                root.path(),
                &candidate,
            )?),
            physical_identity: binding.physical_identity.clone(),
            absence_parent: None,
            physical_redirect_sha256: None,
        };
        let reserved = (candidate.clone(), binding.clone());
        let validation = FinalFreshnessValidation {
            bindings: vec![reserved.clone()],
            captured_inputs: vec![captured.clone()],
            reserved_state_identities: BTreeSet::new(),
        };

        assert!(final_freshness_validation_signals(root.path(), &validation).is_empty());

        std::fs::write(
            root.path().join("package.json"),
            r#"{"name":"root","workspaces":["changed/*"]}"#,
        )?;
        assert_eq!(
            final_freshness_validation_signals(root.path(), &validation),
            [GateSignal::ProtectedInputChanged {
                paths: vec![binding.path]
            }]
        );
        Ok(())
    }
}
