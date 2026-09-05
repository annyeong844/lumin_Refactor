use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use lumin_evidence::{
    CapabilityIntentRecord, DependencyIntentRecord, GATE_CAPABILITY_INTENT_INFERENCE_VERSION,
    GateAnalysisOptions, GateOperationResult, GateRecord, GateSignal, OperationRecord,
    PathPrefixIdentity, PostWriteFinalValidationEvidence, PreWriteFinalValidationEvidence,
    RepoPathProjection, ScanInvocationTier, SemanticInputRecord, SemanticInputState,
    SemanticReadReservationBinding, WorktreeTransition, derive_pre_write_final_validation_signals,
    gate_policy, post_write_request_digest, pre_write_request_digest, seal_analysis_snapshot,
};
use lumin_inventory::{
    InventoryError, InventoryRequest, SemanticInputExpectation, SemanticInputValidationState,
};
use lumin_model::{
    CapabilityIntent, ConfigAbsenceParent, DependencyIntent, GateDeltaRecord, GateId, OperationId,
    RepoPath, RepositoryRootIdentity, ResolutionProfile, append_length_prefixed, digest_hex,
};
use lumin_store::{
    ActiveGateLease, GateBaselineDraft, ObservationFinalization, OperationSession, PostWriteFinish,
    PostWriteStart, PreWriteFinish, PreWriteStart, SemanticReadReservation,
};

use super::capability_query::{
    active_gate_capability_intents, apply_gate_capability_availability,
    gate_capability_target_paths, normalized_gate_capability_intents,
};
use super::{
    EngineError, RepositoryAnalysisSession, RepositoryAnalysisStep, RepositoryCapture,
    RepositoryContext, analysis_cache, open_repository_context, repository_context_from_admission,
    reserved_state_identity_lookup,
};

mod domain;
mod observation;
mod transitions;

#[cfg(all(feature = "gate-test-fault", not(debug_assertions)))]
compile_error!("gate-test-fault is restricted to debug test builds");

use domain::{
    DeclaredPathInspection, close_alias_topology, expand_write_domain, inspect_declared_paths,
    lease_containment_signals, observe_write_domain_from_semantic_inputs,
    protected_semantic_inputs,
};
use observation::{
    BaselineObservationData, BaselineObservationSeed, CloseObservationSeed,
    close_observation_binding, observation_binding_matches_owner, pre_write_observation_binding,
    pre_write_observation_can_seal, unsealed_pre_write_observation_binding,
};
use transitions::{active_transition_signals, changed_paths, reconcile_transitions};

const ANALYSIS_CONTRACT_VERSION: &[u8] = b"lumin-analysis-contract.phase1-foundation.v35";
const DEPENDENCY_CANDIDATE_TOPOLOGY_ONLY: &[u8] = b"dependency-candidate-topology-only.v1";

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
    pub capability_intents: Vec<CapabilityIntent>,
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
    super::with_worker_pool(request.jobs, || open_write_gate_in_current_pool(request))?
}

fn open_write_gate_in_current_pool(
    request: &PreWriteRequest,
) -> Result<GateOperationResult, EngineError> {
    lumin_inventory::validate_caller_paths_lexically(&request.paths)?;
    validate_analysis_path_names(
        &request.entries,
        &request.dependency_intents,
        &request.capability_intents,
    )?;
    let mut paths = request.paths.clone();
    paths.sort();
    paths.dedup();
    if paths.is_empty() {
        return Err(EngineError::NoDeclaredPaths);
    }
    for intent in &request.capability_intents {
        if !paths.iter().any(|declared| intent.path.is_within(declared)) {
            return Err(EngineError::CapabilityIntentOutsideDeclaredWrite(
                intent.path.display_escaped(),
            ));
        }
    }
    let declared_write_set = paths
        .iter()
        .map(RepoPathProjection::from)
        .collect::<Vec<_>>();
    // Build the exact tier from the request
    let scan_invocation = build_gate_scan_invocation_tier(request, &[]);
    let request_digest = pre_write_request_digest(&declared_write_set, &scan_invocation);
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
                &request.capability_intents,
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
    validate_analysis_paths(
        &context.root,
        &request.entries,
        &request.dependency_intents,
        &request.capability_intents,
    )?;
    let reserved_state_lookup = reserved_state_identity_lookup(&context.store);
    lumin_inventory::validate_caller_entry_identity_lookup(
        &context.root,
        &paths,
        &reserved_state_lookup,
    )?;
    validate_analysis_path_identities(
        &context.root,
        &request.entries,
        &request.dependency_intents,
        &request.capability_intents,
        &reserved_state_lookup,
    )?;
    let unavailable_targets = gate_capability_target_paths(&scan_invocation.capability_intents)?;
    let inspection = inspect_declared_paths(&context.root, &paths, &unavailable_targets);
    let analysis_options = GateAnalysisOptions {
        jobs: request.jobs,
        resolution_profile: request.resolution_profile,
        scan_invocation: build_gate_scan_invocation_tier(request, &inspection.leases),
        capability_intent_inference: Some(GATE_CAPABILITY_INTENT_INFERENCE_VERSION.to_owned()),
    };
    let admission_observation_seed = BaselineObservationSeed {
        declared_write_set: declared_write_set.clone(),
        leased_write_set: inspection.leases.clone(),
        alias_closures: Vec::new(),
        attempted_semantic_inputs: Vec::new(),
        baseline: None,
        evidence_payload_sha256: None,
    };
    let operation = context.store.begin_operation(&request.operation_id)?;
    let (gate_id, transition_sequence, analysis_options) = match operation
        .reserve_pre_write_with_inspection(
            &request_digest,
            &declared_write_set,
            &inspection.leases,
            &inspection.evidence,
            &analysis_options,
            |signals| unsealed_pre_write_observation_binding(&admission_observation_seed, signals),
        )? {
        PreWriteStart::Committed(result) => return Ok(*result),
        PreWriteStart::Analyze {
            gate_id,
            transition_sequence,
            analysis_options,
        } => (gate_id, transition_sequence, analysis_options),
    };
    wait_at_pre_write_admission_barrier(&request.operation_id, &gate_id)?;

    let promotion = if inspection.signals.is_empty() {
        match analyze_pre_write(
            &operation,
            &context,
            &analysis_options,
            inspection,
            PreWriteAnalysisReservation {
                operation_id: &request.operation_id,
                request_digest: &request_digest,
                gate_id: &gate_id,
                transition_sequence,
            },
        )? {
            PreWriteAnalysis::Finished(promotion) => *promotion,
            PreWriteAnalysis::Committed(result) => return Ok(*result),
        }
    } else {
        PreWritePromotion::without_validation(PreWriteFinish {
            baseline: None,
            leased_write_set: inspection.leases,
            alias_closures: Vec::new(),
            attempted_semantic_inputs: Vec::new(),
            signals: inspection.signals,
        })
    };
    let PreWritePromotion {
        mut finish,
        final_validation,
        attempted_semantic_bindings,
    } = promotion;
    finish.attempted_semantic_inputs = attempted_semantic_bindings
        .iter()
        .map(|(_, binding)| binding.clone())
        .collect();
    let evidence_payload_sha256 = finish
        .baseline
        .as_ref()
        .map(|baseline| lumin_store::evidence_payload_sha256(&baseline.snapshot.evidence))
        .transpose()?;
    let observation_seed = BaselineObservationSeed {
        declared_write_set: declared_write_set.clone(),
        leased_write_set: finish.leased_write_set.clone(),
        alias_closures: finish.alias_closures.clone(),
        attempted_semantic_inputs: attempted_semantic_bindings
            .iter()
            .map(|(_, binding)| binding.clone())
            .collect(),
        baseline: finish.baseline.as_ref().map(BaselineObservationData::from),
        evidence_payload_sha256,
    };
    wait_at_pre_write_final_barrier(&request.operation_id, &gate_id)?;
    operation
        .finish_pre_write(
            &request_digest,
            &gate_id,
            finish,
            |reserved_identities, catalog_revision, store_signals| {
                let mut final_validation_evidence = None;
                let mut final_signals = if pre_write_observation_can_seal(store_signals) {
                    final_validation.as_ref().map_or_else(
                        || {
                            revalidate_attempted_semantic_inputs(
                                &context.root,
                                &attempted_semantic_bindings,
                                reserved_identities,
                            )
                        },
                        |validation| {
                            let (signals, evidence) = pre_write_final_validation(
                                &context.root,
                                validation,
                                reserved_identities,
                                &observation_seed.leased_write_set,
                                &observation_seed.alias_closures,
                            );
                            final_validation_evidence = evidence;
                            signals
                        },
                    )
                } else {
                    revalidate_attempted_semantic_inputs(
                        &context.root,
                        &attempted_semantic_bindings,
                        reserved_identities,
                    )
                };
                final_signals.retain(|signal| !store_signals.contains(signal));
                let mut all_signals = store_signals.to_vec();
                all_signals.extend(final_signals.iter().cloned());
                ObservationFinalization {
                    signals: final_signals,
                    binding: pre_write_observation_binding(
                        &observation_seed,
                        catalog_revision,
                        &all_signals,
                    ),
                    pre_write_evidence: final_validation_evidence,
                    post_write_evidence: None,
                }
            },
        )
        .map_err(Into::into)
}

fn wait_at_pre_write_admission_barrier(
    operation_id: &OperationId,
    gate_id: &GateId,
) -> Result<(), EngineError> {
    wait_at_gate_test_barrier(
        "LUMIN_TEST_GATE_ADMISSION_BARRIER",
        "reserved",
        operation_id,
        gate_id,
    )
}

fn wait_at_pre_write_final_barrier(
    operation_id: &OperationId,
    gate_id: &GateId,
) -> Result<(), EngineError> {
    wait_at_gate_test_barrier(
        "LUMIN_TEST_GATE_PREWRITE_FINAL_BARRIER",
        "finalizing",
        operation_id,
        gate_id,
    )
}

fn wait_at_capture_freshness_barrier(
    operation_id: &OperationId,
    gate_id: &GateId,
) -> Result<(), EngineError> {
    wait_at_gate_test_barrier(
        "LUMIN_TEST_GATE_CAPTURE_FRESHNESS_BARRIER",
        "capture-freshness",
        operation_id,
        gate_id,
    )
}

fn wait_at_analysis_cache_replay_barrier(
    stage: &str,
    operation_id: &OperationId,
    gate_id: &GateId,
) -> Result<(), EngineError> {
    wait_at_gate_test_barrier(
        "LUMIN_TEST_GATE_CACHE_REPLAY_BARRIER",
        stage,
        operation_id,
        gate_id,
    )
}

fn wait_at_post_write_capture_barrier(
    operation_id: &OperationId,
    gate_id: &GateId,
) -> Result<(), EngineError> {
    wait_at_gate_test_barrier(
        "LUMIN_TEST_GATE_POSTWRITE_CAPTURE_BARRIER",
        "close-capturing",
        operation_id,
        gate_id,
    )
}

fn wait_at_post_write_final_barrier(
    operation_id: &OperationId,
    gate_id: &GateId,
) -> Result<(), EngineError> {
    wait_at_gate_test_barrier(
        "LUMIN_TEST_GATE_POSTWRITE_FINAL_BARRIER",
        "close-finalizing",
        operation_id,
        gate_id,
    )
}

#[cfg(feature = "gate-test-fault")]
fn wait_at_gate_test_barrier(
    environment: &str,
    stage: &str,
    operation_id: &OperationId,
    gate_id: &GateId,
) -> Result<(), EngineError> {
    use std::io::{BufRead, BufReader, Write};
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;

    let Some(address) = std::env::var_os(environment) else {
        return Ok(());
    };
    let address = address.to_str().ok_or_else(|| {
        lumin_store::StoreError::Integrity(format!(
            "gate {stage} test barrier address is not UTF-8"
        ))
    })?;
    let address = address.parse::<SocketAddr>().map_err(|error| {
        lumin_store::StoreError::Integrity(format!(
            "gate {stage} test barrier address is malformed: {error}"
        ))
    })?;
    if !address.ip().is_loopback() {
        return Err(lumin_store::StoreError::Integrity(format!(
            "gate {stage} test barrier must use a loopback address"
        ))
        .into());
    }
    let mut stream = TcpStream::connect(address).map_err(|error| {
        lumin_store::StoreError::Io(format!("gate {stage} test barrier failed: {error}"))
    })?;
    let timeout = Some(Duration::from_secs(30));
    stream.set_read_timeout(timeout).map_err(|error| {
        lumin_store::StoreError::Io(format!("gate {stage} test barrier failed: {error}"))
    })?;
    stream.set_write_timeout(timeout).map_err(|error| {
        lumin_store::StoreError::Io(format!("gate {stage} test barrier failed: {error}"))
    })?;
    writeln!(
        stream,
        "{stage} {} {}",
        operation_id.as_str(),
        gate_id.as_str()
    )
    .map_err(|error| {
        lumin_store::StoreError::Io(format!("gate {stage} test barrier failed: {error}"))
    })?;
    let mut release = String::new();
    BufReader::new(stream)
        .read_line(&mut release)
        .map_err(|error| {
            lumin_store::StoreError::Io(format!("gate {stage} test barrier failed: {error}"))
        })?;
    let release = release.trim_end();
    if release == "fail-analysis" {
        return Err(EngineError::ExtractionUnavailable);
    }
    if release != "release" {
        return Err(lumin_store::StoreError::Integrity(format!(
            "gate {stage} test barrier returned an invalid release frame"
        ))
        .into());
    }
    Ok(())
}

#[cfg(not(feature = "gate-test-fault"))]
fn wait_at_gate_test_barrier(
    _environment: &str,
    _stage: &str,
    _operation_id: &OperationId,
    _gate_id: &GateId,
) -> Result<(), EngineError> {
    Ok(())
}

/// Build the exact ScanInvocationTier from a PreWriteRequest with normalized entries.
fn build_gate_scan_invocation_tier(
    request: &PreWriteRequest,
    declared_leases: &[lumin_evidence::WriteLease],
) -> ScanInvocationTier {
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
    let capability_intents = normalized_gate_capability_intents(
        &request.paths,
        &request.capability_intents,
        declared_leases,
    );
    ScanInvocationTier {
        includes: request.includes.clone(),
        excludes: request.excludes.clone(),
        role_overrides: request.role_overrides.clone(),
        entries,
        dependency_intents,
        capability_intents,
        resolution_profile: request.resolution_profile,
    }
}

enum PreWriteAnalysis {
    Finished(Box<PreWritePromotion>),
    Committed(Box<GateOperationResult>),
}

struct PreWritePromotion {
    finish: PreWriteFinish,
    final_validation: Option<FinalFreshnessValidation>,
    attempted_semantic_bindings: Vec<(RepoPath, SemanticReadReservationBinding)>,
}

impl PreWritePromotion {
    fn without_validation(finish: PreWriteFinish) -> Self {
        Self {
            finish,
            final_validation: None,
            attempted_semantic_bindings: Vec::new(),
        }
    }
}

struct FinalFreshnessValidation {
    bindings: Vec<(RepoPath, SemanticReadReservationBinding)>,
    captured_inputs: Vec<SemanticInputRecord>,
    inventory_request: InventoryRequest,
    reserved_state_lookup: lumin_inventory::ReservedStateIdentityLookup,
    unavailable_capability_targets: BTreeSet<RepoPath>,
    capability_intents: Vec<CapabilityIntentRecord>,
    active_capability_intents: BTreeSet<CapabilityIntentRecord>,
}

struct PreWriteAnalysisReservation<'a> {
    operation_id: &'a OperationId,
    request_digest: &'a str,
    gate_id: &'a GateId,
    transition_sequence: u64,
}

fn analyze_pre_write(
    operation: &OperationSession<'_>,
    context: &RepositoryContext,
    options: &GateAnalysisOptions,
    inspection: DeclaredPathInspection,
    reservation: PreWriteAnalysisReservation<'_>,
) -> Result<PreWriteAnalysis, EngineError> {
    let PreWriteAnalysisReservation {
        operation_id,
        request_digest,
        gate_id,
        transition_sequence,
    } = reservation;
    let analysis_contract = analysis_contract_id();
    let inventory_request = inventory_request_from_tier(&options.scan_invocation)?;
    let reserved_state_lookup = reserved_state_identity_lookup(&context.store);
    let capture = match capture_reserved_repository(
        ReservedCaptureContext {
            store: &context.store,
            operation_id,
            gate_id,
            root: &context.root,
            repository_root: &context.repository_root,
            options,
            owner_contract_version: &analysis_contract,
            inventory_request: &inventory_request,
            reserved_state_lookup: &reserved_state_lookup,
        },
        |paths| {
            operation
                .reserve_pre_write_semantic_inputs(request_digest, gate_id, paths)
                .map_err(Into::into)
        },
        || wait_at_capture_freshness_barrier(operation_id, gate_id),
    ) {
        Ok(ReservedCapture::Finished {
            capture,
            reserved_semantic_bindings,
        }) => (capture, reserved_semantic_bindings),
        Ok(ReservedCapture::Blocked {
            signal,
            attempted_semantic_bindings,
        }) => {
            return Ok(PreWriteAnalysis::Finished(Box::new(PreWritePromotion {
                finish: PreWriteFinish {
                    baseline: None,
                    leased_write_set: inspection.leases,
                    alias_closures: Vec::new(),
                    attempted_semantic_inputs: Vec::new(),
                    signals: vec![signal],
                },
                final_validation: None,
                attempted_semantic_bindings,
            })));
        }
        Ok(ReservedCapture::Committed(result)) => {
            return Ok(PreWriteAnalysis::Committed(result));
        }
        Err(ReservedCaptureFailure {
            error: EngineError::Store(error),
            ..
        }) => return Err(EngineError::Store(error)),
        Err(ReservedCaptureFailure {
            error,
            attempted_semantic_bindings,
        }) => {
            return Ok(PreWriteAnalysis::Finished(Box::new(PreWritePromotion {
                finish: PreWriteFinish {
                    baseline: None,
                    leased_write_set: inspection.leases,
                    alias_closures: Vec::new(),
                    attempted_semantic_inputs: Vec::new(),
                    signals: vec![GateSignal::AnalysisFailed {
                        detail: error.to_string(),
                    }],
                },
                final_validation: None,
                attempted_semantic_bindings,
            })));
        }
    };
    let (mut capture, reserved_semantic_bindings) = capture;
    let (snapshot, active_capability_intents) = match apply_gate_capability_availability(
        &context.root,
        capture.snapshot,
        &reserved_state_lookup,
    ) {
        Ok(projected) => projected,
        Err(error) => {
            return Ok(PreWriteAnalysis::Finished(Box::new(PreWritePromotion {
                finish: PreWriteFinish {
                    baseline: None,
                    leased_write_set: inspection.leases,
                    alias_closures: Vec::new(),
                    attempted_semantic_inputs: Vec::new(),
                    signals: vec![GateSignal::AnalysisFailed {
                        detail: error.to_string(),
                    }],
                },
                final_validation: None,
                attempted_semantic_bindings: reserved_semantic_bindings,
            })));
        }
    };
    capture.snapshot = snapshot;
    let mut signals = Vec::new();
    let unavailable_targets =
        gate_capability_target_paths(&options.scan_invocation.capability_intents)?;
    let (leased_write_set, alias_closures, domain_signals) = expand_write_domain(
        &context.root,
        &inspection.observations,
        inspection.leases,
        &capture,
        &unavailable_targets,
    );
    signals.extend(domain_signals);
    let protected_semantic_inputs = protected_semantic_inputs(&capture, &leased_write_set);
    signals.extend(gate_policy::opening_signals(
        &capture.snapshot,
        &leased_write_set,
    ));
    let final_validation = FinalFreshnessValidation {
        bindings: reserved_semantic_bindings.clone(),
        captured_inputs: capture.snapshot.inputs.clone(),
        inventory_request,
        reserved_state_lookup,
        unavailable_capability_targets: unavailable_targets,
        capability_intents: options.scan_invocation.capability_intents.clone(),
        active_capability_intents,
    };
    let baseline = GateBaselineDraft {
        analysis_contract,
        snapshot: capture.snapshot,
        protected_semantic_inputs,
        transition_sequence,
    };
    Ok(PreWriteAnalysis::Finished(Box::new(PreWritePromotion {
        finish: PreWriteFinish {
            baseline: Some(baseline),
            leased_write_set,
            alias_closures,
            attempted_semantic_inputs: Vec::new(),
            signals,
        },
        final_validation: Some(final_validation),
        attempted_semantic_bindings: reserved_semantic_bindings,
    })))
}

pub fn close_write_gate(request: &PostWriteRequest) -> Result<GateOperationResult, EngineError> {
    let request_digest = post_write_request_digest(&request.gate_id);
    let context = open_repository_context(&request.root)?;
    let operation = context.store.begin_operation(&request.operation_id)?;
    let (gate, transitions, active_gates) =
        match operation.begin_post_write(&request_digest, &request.gate_id)? {
            PostWriteStart::Committed(result) => return Ok(*result),
            PostWriteStart::Analyze {
                gate,
                transitions,
                active_gates,
            } => (*gate, transitions, active_gates),
        };
    let jobs = gate.analysis_options.jobs;
    super::with_worker_pool(jobs, || {
        close_write_gate_in_current_pool(
            request,
            &request_digest,
            &context,
            &operation,
            gate,
            transitions,
            active_gates,
        )
    })?
}

#[allow(clippy::too_many_arguments)]
fn close_write_gate_in_current_pool(
    request: &PostWriteRequest,
    request_digest: &str,
    context: &RepositoryContext,
    operation: &OperationSession<'_>,
    gate: GateRecord,
    transitions: Vec<WorktreeTransition>,
    active_gates: Vec<ActiveGateLease>,
) -> Result<GateOperationResult, EngineError> {
    let baseline = gate
        .baseline
        .as_ref()
        .ok_or_else(|| EngineError::GateBaselineMissing(request.gate_id.as_str().to_owned()))?;
    let analysis_contract = analysis_contract_id();
    if baseline.analysis_contract != analysis_contract {
        return finish_failed_close(
            operation,
            request,
            request_digest,
            &gate,
            vec![GateSignal::AnalysisContractChanged],
            Vec::new(),
            None,
        );
    }

    // Reconstruct InventoryRequest from persisted tier (not default)
    let inventory_request =
        match inventory_request_from_tier(&gate.analysis_options.scan_invocation) {
            Ok(request) => request,
            Err(error) => {
                return finish_failed_close(
                    operation,
                    request,
                    request_digest,
                    &gate,
                    vec![GateSignal::AnalysisFailed {
                        detail: error.to_string(),
                    }],
                    Vec::new(),
                    None,
                );
            }
        };
    let opening_entry_paths = match opening_entry_paths(&baseline.snapshot.entry_selections) {
        Ok(paths) => paths,
        Err(error) => {
            return finish_failed_close(
                operation,
                request,
                request_digest,
                &gate,
                vec![GateSignal::AnalysisFailed {
                    detail: error.to_string(),
                }],
                Vec::new(),
                None,
            );
        }
    };
    let containment_context = CloseContainmentContext {
        root: &context.root,
        entries: &opening_entry_paths,
        dependency_intents: &inventory_request.dependency_intents,
    };
    let containment_signals = close_containment_signals(
        &context.root,
        &gate.leased_write_set,
        &opening_entry_paths,
        &inventory_request.dependency_intents,
    );
    if !containment_signals.is_empty() {
        return finish_failed_close(
            operation,
            request,
            request_digest,
            &gate,
            containment_signals,
            Vec::new(),
            Some(containment_context),
        );
    }

    // Validate tier resolution_profile agrees with legacy options.resolution_profile
    if gate.analysis_options.scan_invocation.resolution_profile
        != gate.analysis_options.resolution_profile
    {
        return finish_failed_close(
            operation,
            request,
            request_digest,
            &gate,
            vec![GateSignal::AnalysisFailed {
                detail: EngineError::TierProfileInconsistency(format!(
                    "tier profile {:?} != options profile {:?}",
                    gate.analysis_options.scan_invocation.resolution_profile,
                    gate.analysis_options.resolution_profile
                ))
                .to_string(),
            }],
            Vec::new(),
            None,
        );
    }

    wait_at_post_write_capture_barrier(&request.operation_id, &request.gate_id)?;
    let reserved_state_lookup = reserved_state_identity_lookup(&context.store);
    let (mut capture, reserved_semantic_bindings) = match capture_reserved_repository(
        ReservedCaptureContext {
            store: &context.store,
            operation_id: &request.operation_id,
            gate_id: &request.gate_id,
            root: &context.root,
            repository_root: &context.repository_root,
            options: &gate.analysis_options,
            owner_contract_version: &analysis_contract,
            inventory_request: &inventory_request,
            reserved_state_lookup: &reserved_state_lookup,
        },
        |paths| {
            operation
                .reserve_post_write_semantic_inputs(request_digest, &request.gate_id, paths)
                .map_err(Into::into)
        },
        || wait_at_capture_freshness_barrier(&request.operation_id, &request.gate_id),
    ) {
        Ok(ReservedCapture::Finished {
            capture,
            reserved_semantic_bindings,
        }) => (capture, reserved_semantic_bindings),
        Ok(ReservedCapture::Blocked {
            signal,
            attempted_semantic_bindings,
        }) => {
            return finish_failed_close(
                operation,
                request,
                request_digest,
                &gate,
                vec![signal],
                attempted_semantic_bindings,
                Some(containment_context),
            );
        }
        Ok(ReservedCapture::Committed(result)) => return Ok(*result),
        Err(ReservedCaptureFailure {
            error: EngineError::Store(error),
            ..
        }) => return Err(EngineError::Store(error)),
        Err(ReservedCaptureFailure {
            error,
            attempted_semantic_bindings,
        }) => {
            let signals = post_write_capture_failure_signals(
                &context.root,
                &gate.leased_write_set,
                &opening_entry_paths,
                &inventory_request.dependency_intents,
                &error,
            );
            return finish_failed_close(
                operation,
                request,
                request_digest,
                &gate,
                signals,
                attempted_semantic_bindings,
                Some(containment_context),
            );
        }
    };

    let (snapshot, active_capability_intents) = match apply_gate_capability_availability(
        &context.root,
        capture.snapshot,
        &reserved_state_lookup,
    ) {
        Ok(projected) => projected,
        Err(error) => {
            let signals = post_write_capture_failure_signals(
                &context.root,
                &gate.leased_write_set,
                &opening_entry_paths,
                &inventory_request.dependency_intents,
                &error,
            );
            return finish_failed_close(
                operation,
                request,
                request_digest,
                &gate,
                signals,
                reserved_semantic_bindings,
                Some(containment_context),
            );
        }
    };
    capture.snapshot = snapshot;
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
    let (close_write_leases, alias_closures, topology_signals) =
        close_alias_topology(&context.root, &gate, &capture);
    let actual_write_set = if gate_policy::actual_write_attribution_is_complete(&signals)
        && gate_policy::actual_write_attribution_is_complete(&topology_signals)
    {
        Some(gate_policy::closure_expanded_actual_write_set(
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
        bindings: reserved_semantic_bindings.clone(),
        captured_inputs: capture.snapshot.inputs.clone(),
        inventory_request,
        reserved_state_lookup,
        unavailable_capability_targets: gate_capability_target_paths(
            &gate.analysis_options.scan_invocation.capability_intents,
        )?,
        capability_intents: gate
            .analysis_options
            .scan_invocation
            .capability_intents
            .clone(),
        active_capability_intents,
    };
    let evidence_payload_sha256 = lumin_store::evidence_payload_sha256(&capture.snapshot.evidence)?;
    let observation_seed = CloseObservationSeed {
        gate_id: request.gate_id.clone(),
        opening_observation_id: Some(baseline.observation_id.clone()),
        opening_analysis_contract: Some(baseline.analysis_contract.clone()),
        prior_revision: gate.current_revision,
        leased_write_set: gate.leased_write_set.clone(),
        analysis_input_id: Some(capture.snapshot.analysis_input_id.clone()),
        evidence_payload_sha256: Some(evidence_payload_sha256),
        prior_protected_semantic_inputs: gate.protected_semantic_inputs.clone(),
        protected_semantic_inputs: protected_semantic_inputs.clone(),
        changed_paths: changed_paths.clone(),
        actual_write_set: actual_write_set.clone(),
        alias_closures: alias_closures.clone(),
        reconciled_transition_sequences: reconciled_sequences.clone(),
        attempted_semantic_inputs: reserved_semantic_bindings
            .iter()
            .map(|(_, binding)| binding.clone())
            .collect(),
    };
    wait_at_post_write_final_barrier(&request.operation_id, &request.gate_id)?;
    operation
        .finish_post_write(
            request_digest,
            &request.gate_id,
            PostWriteFinish {
                snapshot: Some(capture.snapshot),
                protected_semantic_inputs,
                reconciled_baseline: Some(reconciled_baseline),
                changed_paths,
                actual_write_set,
                alias_closures,
                reconciled_transition_sequences: reconciled_sequences,
                attempted_semantic_inputs: reserved_semantic_bindings
                    .iter()
                    .map(|(_, binding)| binding.clone())
                    .collect(),
                signals,
                deltas,
            },
            |reserved_identities, catalog_revision, store_signals| {
                let (final_signals, final_validation_evidence) = post_write_final_validation(
                    &context.root,
                    &final_validation,
                    reserved_identities,
                    &observation_seed.leased_write_set,
                    &opening_entry_paths,
                    &close_write_leases,
                    &observation_seed.alias_closures,
                );
                let mut all_signals = store_signals.to_vec();
                all_signals.extend(final_signals.iter().cloned());
                ObservationFinalization {
                    signals: final_signals,
                    binding: close_observation_binding(
                        &observation_seed,
                        catalog_revision,
                        &all_signals,
                    ),
                    pre_write_evidence: None,
                    post_write_evidence: final_validation_evidence.map(|observation| {
                        PostWriteFinalValidationEvidence {
                            expected_leased_write_set: close_write_leases.clone(),
                            expected_alias_closures: observation_seed.alias_closures.clone(),
                            observation,
                        }
                    }),
                }
            },
        )
        .map_err(Into::into)
}

fn validate_analysis_paths(
    root: &Path,
    entries: &[RepoPath],
    dependency_intents: &[DependencyIntent],
    capability_intents: &[CapabilityIntent],
) -> Result<(), EngineError> {
    lumin_inventory::validate_caller_entries(root, entries)?;
    let intent_paths = dependency_intents
        .iter()
        .map(|intent| intent.path.clone())
        .chain(capability_intents.iter().map(|intent| intent.path.clone()))
        .collect::<Vec<_>>();
    lumin_inventory::validate_caller_entries(root, &intent_paths)?;
    Ok(())
}

fn opening_entry_paths(
    selections: &[lumin_evidence::EntrySelectionRecord],
) -> Result<Vec<RepoPath>, EngineError> {
    let mut paths = Vec::with_capacity(selections.len());
    for selection in selections {
        let path = RepoPath::from_canonical_bytes(&selection.path.canonical).map_err(|error| {
            EngineError::TierProjectionCorrupt(format!(
                "failed to decode opening entry selection {}: {error}",
                selection.path.display
            ))
        })?;
        if RepoPathProjection::from(&path) != selection.path {
            return Err(EngineError::TierProjectionCorrupt(format!(
                "opening entry selection projection round-trip failed for {}",
                selection.path.display
            )));
        }
        paths.push(path);
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn close_containment_signals(
    root: &Path,
    leases: &[lumin_evidence::WriteLease],
    entries: &[RepoPath],
    dependency_intents: &[DependencyIntent],
) -> Vec<GateSignal> {
    let mut signals = lease_containment_signals(root, leases);
    let mut stale = Vec::new();
    for path in entries {
        match lumin_inventory::validate_caller_entries(root, std::slice::from_ref(path)) {
            Ok(()) => {}
            Err(InventoryError::EntryEscapesRoot(_)) => {
                stale.push(RepoPathProjection::from(path));
            }
            Err(error) => signals.push(GateSignal::AnalysisFailed {
                detail: error.to_string(),
            }),
        }
    }
    for intent in dependency_intents {
        if let Err(error) =
            lumin_inventory::validate_caller_entries(root, std::slice::from_ref(&intent.path))
        {
            signals.push(GateSignal::AnalysisFailed {
                detail: error.to_string(),
            });
        }
    }
    stale.sort();
    stale.dedup();
    if !stale.is_empty() {
        signals.push(GateSignal::ProtectedInputChanged { paths: stale });
    }
    signals
}

fn post_write_capture_failure_signals(
    root: &Path,
    opening_leases: &[lumin_evidence::WriteLease],
    opening_entries: &[RepoPath],
    dependency_intents: &[DependencyIntent],
    error: &EngineError,
) -> Vec<GateSignal> {
    if let EngineError::Inventory(InventoryError::EntryEscapesRoot(escaped)) = error {
        let escaped_projection = RepoPathProjection::from(escaped);
        let lease = opening_leases
            .iter()
            .find(|lease| lease.path.canonical == escaped_projection.canonical)
            .or_else(|| {
                opening_leases
                    .iter()
                    .find(|lease| lease.covers(&escaped_projection))
            });
        if let Some(lease) = lease {
            return vec![match lease.kind {
                lumin_evidence::WriteLeaseKind::NewFile => {
                    GateSignal::PlannedPathContainmentViolation {
                        paths: vec![lease.path.clone()],
                    }
                }
                lumin_evidence::WriteLeaseKind::ExistingFile
                | lumin_evidence::WriteLeaseKind::Directory => GateSignal::ProtectedInputChanged {
                    paths: vec![lease.path.clone()],
                },
            }];
        }
        if let Some(path) = opening_entries.iter().find(|path| *path == escaped) {
            return vec![GateSignal::ProtectedInputChanged {
                paths: vec![RepoPathProjection::from(path)],
            }];
        }
        let signals =
            close_containment_signals(root, opening_leases, opening_entries, dependency_intents);
        if !signals.is_empty() {
            return signals;
        }
    }
    vec![GateSignal::AnalysisFailed {
        detail: error.to_string(),
    }]
}

#[derive(Clone, Copy)]
struct CloseContainmentContext<'a> {
    root: &'a Path,
    entries: &'a [RepoPath],
    dependency_intents: &'a [DependencyIntent],
}

fn validate_analysis_path_names(
    entries: &[RepoPath],
    dependency_intents: &[DependencyIntent],
    capability_intents: &[CapabilityIntent],
) -> Result<(), EngineError> {
    lumin_inventory::validate_caller_paths_lexically(entries)?;
    let intent_paths = dependency_intents
        .iter()
        .map(|intent| intent.path.clone())
        .chain(capability_intents.iter().map(|intent| intent.path.clone()))
        .collect::<Vec<_>>();
    lumin_inventory::validate_caller_paths_lexically(&intent_paths)?;
    Ok(())
}

fn validate_analysis_path_identities(
    root: &Path,
    entries: &[RepoPath],
    dependency_intents: &[DependencyIntent],
    capability_intents: &[CapabilityIntent],
    reserved_state_lookup: &lumin_inventory::ReservedStateIdentityLookup,
) -> Result<(), EngineError> {
    lumin_inventory::validate_caller_entry_identity_lookup(root, entries, reserved_state_lookup)?;
    let intent_paths = dependency_intents
        .iter()
        .map(|intent| intent.path.clone())
        .chain(capability_intents.iter().map(|intent| intent.path.clone()))
        .collect::<Vec<_>>();
    lumin_inventory::validate_caller_entry_identity_lookup(
        root,
        &intent_paths,
        reserved_state_lookup,
    )?;
    Ok(())
}

fn finish_failed_close(
    operation: &OperationSession<'_>,
    request: &PostWriteRequest,
    request_digest: &str,
    gate: &GateRecord,
    signals: Vec<GateSignal>,
    attempted_semantic_bindings: Vec<(RepoPath, SemanticReadReservationBinding)>,
    final_containment: Option<CloseContainmentContext<'_>>,
) -> Result<GateOperationResult, EngineError> {
    let baseline = gate.baseline.as_ref();
    let observation_seed = CloseObservationSeed {
        gate_id: request.gate_id.clone(),
        opening_observation_id: baseline.map(|baseline| baseline.observation_id.clone()),
        opening_analysis_contract: baseline.map(|baseline| baseline.analysis_contract.clone()),
        prior_revision: gate.current_revision,
        leased_write_set: gate.leased_write_set.clone(),
        analysis_input_id: None,
        evidence_payload_sha256: None,
        prior_protected_semantic_inputs: gate.protected_semantic_inputs.clone(),
        protected_semantic_inputs: gate.protected_semantic_inputs.clone(),
        changed_paths: Vec::new(),
        actual_write_set: None,
        alias_closures: gate.alias_closures.clone(),
        reconciled_transition_sequences: Vec::new(),
        attempted_semantic_inputs: attempted_semantic_bindings
            .iter()
            .map(|(_, binding)| binding.clone())
            .collect(),
    };
    let attempted_inputs = attempted_semantic_bindings
        .iter()
        .map(|(_, binding)| binding.clone())
        .collect();
    wait_at_post_write_final_barrier(&request.operation_id, &request.gate_id)?;
    operation
        .finish_post_write(
            request_digest,
            &request.gate_id,
            PostWriteFinish {
                snapshot: None,
                protected_semantic_inputs: gate.protected_semantic_inputs.clone(),
                reconciled_baseline: None,
                changed_paths: Vec::new(),
                actual_write_set: None,
                alias_closures: gate.alias_closures.clone(),
                reconciled_transition_sequences: Vec::new(),
                attempted_semantic_inputs: attempted_inputs,
                signals,
                deltas: Vec::new(),
            },
            |reserved_identities, catalog_revision, store_signals| {
                let mut observed_signals = final_containment.map_or_else(Vec::new, |context| {
                    close_containment_signals(
                        context.root,
                        &gate.leased_write_set,
                        context.entries,
                        context.dependency_intents,
                    )
                });
                observed_signals.extend(revalidate_attempted_semantic_inputs(
                    &request.root,
                    &attempted_semantic_bindings,
                    reserved_identities,
                ));
                let mut final_signals = Vec::new();
                for signal in observed_signals {
                    if !store_signals.contains(&signal) && !final_signals.contains(&signal) {
                        final_signals.push(signal);
                    }
                }
                let mut all_signals = store_signals.to_vec();
                all_signals.extend(final_signals.iter().cloned());
                ObservationFinalization {
                    signals: final_signals,
                    binding: close_observation_binding(
                        &observation_seed,
                        catalog_revision,
                        &all_signals,
                    ),
                    pre_write_evidence: None,
                    post_write_evidence: None,
                }
            },
        )
        .map_err(Into::into)
}

enum ReservedCapture {
    Finished {
        capture: Box<RepositoryCapture>,
        reserved_semantic_bindings: Vec<(RepoPath, SemanticReadReservationBinding)>,
    },
    Blocked {
        signal: GateSignal,
        attempted_semantic_bindings: Vec<(RepoPath, SemanticReadReservationBinding)>,
    },
    Committed(Box<GateOperationResult>),
}

struct ReservedCaptureFailure {
    error: EngineError,
    attempted_semantic_bindings: Vec<(RepoPath, SemanticReadReservationBinding)>,
}

struct ReservedCaptureContext<'a> {
    store: &'a lumin_store::RepositoryStore,
    operation_id: &'a OperationId,
    gate_id: &'a GateId,
    root: &'a Path,
    repository_root: &'a RepositoryRootIdentity,
    options: &'a GateAnalysisOptions,
    owner_contract_version: &'a str,
    inventory_request: &'a InventoryRequest,
    reserved_state_lookup: &'a lumin_inventory::ReservedStateIdentityLookup,
}

fn capture_reserved_repository(
    context: ReservedCaptureContext<'_>,
    mut reserve: impl FnMut(
        &[SemanticReadReservationBinding],
    ) -> Result<SemanticReadReservation, EngineError>,
    mut before_freshness: impl FnMut() -> Result<(), EngineError>,
) -> Result<ReservedCapture, ReservedCaptureFailure> {
    let ReservedCaptureContext {
        store,
        operation_id,
        gate_id,
        root,
        repository_root,
        options,
        owner_contract_version,
        inventory_request,
        reserved_state_lookup,
    } = context;
    let mut reserved_semantic_bindings = Vec::new();
    let result = (|| -> Result<ReservedCapture, EngineError> {
        let dependency_candidates = lumin_inventory::dependency_owner_candidate_paths(
            &inventory_request.dependency_intents,
        )?;
        if let Some(outcome) = reserve_semantic_paths(
            root,
            &dependency_candidates,
            &mut reserved_semantic_bindings,
            &mut reserve,
        )? {
            return Ok(outcome);
        }
        let dependency_candidate_binding_count = reserved_semantic_bindings.len();

        let pending_inventory =
            lumin_inventory::begin_scan_in_current_pool_with_reserved_state_lookup(
                root,
                inventory_request,
                reserved_state_lookup,
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
            options.scan_invocation.clone(),
            reserved_state_lookup.clone(),
        )?;
        let mut capture = loop {
            let cache_context = session.analysis_cache_context(owner_contract_version)?;
            if let Some(replayed) = analysis_cache::load(
                store,
                &cache_context,
                owner_contract_version,
                &session.scan_invocation,
            )? {
                match replayed {
                    analysis_cache::ReplayedAnalysisStep::NeedsInputs(demands) => {
                        wait_at_analysis_cache_replay_barrier(
                            "cache-demand-hit",
                            operation_id,
                            gate_id,
                        )?;
                        let demands = session.pending_demands(demands)?;
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
                        continue;
                    }
                    analysis_cache::ReplayedAnalysisStep::Finished(capture) => {
                        wait_at_analysis_cache_replay_barrier(
                            "cache-finished-hit",
                            operation_id,
                            gate_id,
                        )?;
                        break *capture;
                    }
                }
            }
            match session.next_step(options.resolution_profile)? {
                RepositoryAnalysisStep::NeedsInputs(demands) => {
                    analysis_cache::store_demands(
                        store,
                        &cache_context,
                        owner_contract_version,
                        &demands,
                    )?;
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
                    let capture = session.finish(resolver)?;
                    analysis_cache::store_finished(
                        store,
                        &cache_context,
                        owner_contract_version,
                        &capture,
                    )?;
                    break capture;
                }
            }
        };
        include_dependency_candidate_topology_inputs(
            &mut capture,
            &reserved_semantic_bindings[..dependency_candidate_binding_count],
        );
        before_freshness()?;
        let stale = stale_reserved_semantic_paths(
            root,
            &reserved_semantic_bindings,
            &capture.snapshot.inputs,
            reserved_state_lookup,
        )?;
        if !stale.is_empty() {
            return Ok(ReservedCapture::Blocked {
                signal: GateSignal::ProtectedInputChanged { paths: stale },
                attempted_semantic_bindings: std::mem::take(&mut reserved_semantic_bindings),
            });
        }
        Ok(ReservedCapture::Finished {
            capture: Box::new(capture),
            reserved_semantic_bindings: std::mem::take(&mut reserved_semantic_bindings),
        })
    })();
    result.map_err(|error| ReservedCaptureFailure {
        error,
        attempted_semantic_bindings: reserved_semantic_bindings,
    })
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
    let current_bindings = paths
        .iter()
        .cloned()
        .zip(reservations.iter().cloned())
        .collect::<Vec<_>>();
    let outcome = match reserve(&reservations)? {
        SemanticReadReservation::Reserved => {
            reserved_bindings.extend(current_bindings);
            return Ok(None);
        }
        SemanticReadReservation::Conflict {
            paths: conflict_paths,
            gate_ids,
        } => {
            let mut attempted_semantic_bindings = reserved_bindings.clone();
            attempted_semantic_bindings.extend(current_bindings);
            attempted_semantic_bindings.sort();
            attempted_semantic_bindings.dedup();
            ReservedCapture::Blocked {
                signal: GateSignal::SemanticInputConflict {
                    paths: conflict_paths,
                    gate_ids,
                },
                attempted_semantic_bindings,
            }
        }
        SemanticReadReservation::TransitionCatalogChanged => ReservedCapture::Blocked {
            signal: GateSignal::TransitionCatalogChanged,
            attempted_semantic_bindings: {
                let mut attempted_semantic_bindings = reserved_bindings.clone();
                attempted_semantic_bindings.extend(current_bindings);
                attempted_semantic_bindings.sort();
                attempted_semantic_bindings.dedup();
                attempted_semantic_bindings
            },
        },
        SemanticReadReservation::Committed(result) => ReservedCapture::Committed(result),
    };
    Ok(Some(outcome))
}

fn stale_reserved_semantic_paths(
    root: &Path,
    bindings: &[(RepoPath, SemanticReadReservationBinding)],
    captured_inputs: &[SemanticInputRecord],
    reserved_state_lookup: &lumin_inventory::ReservedStateIdentityLookup,
) -> Result<Vec<RepoPathProjection>, EngineError> {
    let captured_by_path = captured_inputs
        .iter()
        .map(|input| (input.path.canonical.as_slice(), input))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut stale =
        stale_captured_input_topology_paths(root, captured_inputs, reserved_state_lookup)?;
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
                lumin_inventory::dependency_input_payload_sha256_with_reserved_state_lookup(
                    root,
                    path,
                    reserved_state_lookup,
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

fn stale_captured_input_topology_paths(
    root: &Path,
    captured_inputs: &[SemanticInputRecord],
    reserved_state_lookup: &lumin_inventory::ReservedStateIdentityLookup,
) -> Result<Vec<RepoPathProjection>, EngineError> {
    let mut stale = Vec::new();
    for input in captured_inputs {
        if !matches!(
            input.state,
            SemanticInputState::Source
                | SemanticInputState::ConfigPresent
                | SemanticInputState::CapabilityTarget
        ) {
            continue;
        }
        let Some(expected_identity) = &input.physical_identity else {
            stale.push(input.path.clone());
            continue;
        };
        let path = decode_semantic_input_path(&input.path)?;
        if lumin_inventory::validate_captured_semantic_input_topology(
            root,
            &path,
            expected_identity,
            reserved_state_lookup,
        )
        .is_err()
        {
            stale.push(input.path.clone());
        }
    }
    Ok(stale)
}

#[cfg(test)]
fn final_freshness_validation_signals(
    root: &Path,
    validation: &FinalFreshnessValidation,
    reserved_identities: &std::collections::BTreeSet<lumin_model::PhysicalFileIdentity>,
) -> Vec<GateSignal> {
    let final_lookup = validation
        .reserved_state_lookup
        .for_final_validation(reserved_identities);
    let stale = stale_reserved_semantic_paths(
        root,
        &validation.bindings,
        &validation.captured_inputs,
        &final_lookup,
    )
    .and_then(|mut paths| {
        paths.extend(stale_complete_semantic_input_paths(
            root,
            &validation.bindings,
            &validation.captured_inputs,
            &final_lookup,
            reserved_identities,
        )?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    });
    match stale {
        Ok(paths) if paths.is_empty() => Vec::new(),
        Ok(paths) => vec![GateSignal::ProtectedInputChanged { paths }],
        Err(error) => vec![GateSignal::AnalysisFailed {
            detail: error.to_string(),
        }],
    }
}

fn capability_availability_drift_paths(
    root: &Path,
    validation: &FinalFreshnessValidation,
    reserved_state_lookup: &lumin_inventory::ReservedStateIdentityLookup,
) -> Result<Vec<RepoPathProjection>, EngineError> {
    let observed = active_gate_capability_intents(
        root,
        &validation.capability_intents,
        reserved_state_lookup,
    )?;
    let mut paths = validation
        .active_capability_intents
        .symmetric_difference(&observed)
        .map(|intent| intent.path.clone())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn pre_write_final_validation(
    root: &Path,
    validation: &FinalFreshnessValidation,
    reserved_identities: &std::collections::BTreeSet<lumin_model::PhysicalFileIdentity>,
    expected_leases: &[lumin_evidence::WriteLease],
    expected_alias_closures: &[lumin_evidence::PhysicalAliasClosureRecord],
) -> (Vec<GateSignal>, Option<PreWriteFinalValidationEvidence>) {
    let final_lookup = validation
        .reserved_state_lookup
        .for_final_validation(reserved_identities);
    // A fresh inventory captures sources, but it does not replay resolver demands. Validate those
    // retained inputs independently, including absent and nonregular configuration candidates.
    // Keep the old topology check separate from the new observation so real drift can be sealed.
    let demanded_inputs = validation
        .captured_inputs
        .iter()
        .filter(|input| input.state != SemanticInputState::Source)
        .cloned()
        .collect::<Vec<_>>();
    let semantic_validation_drift =
        stale_captured_input_topology_paths(root, &validation.captured_inputs, &final_lookup)
            .and_then(|mut paths| {
                paths.extend(stale_complete_semantic_input_paths(
                    root,
                    &validation.bindings,
                    &demanded_inputs,
                    &final_lookup,
                    reserved_identities,
                )?);
                paths.sort();
                paths.dedup();
                Ok(paths)
            });
    let mut semantic_validation_drift = match semantic_validation_drift {
        Ok(paths) => paths,
        Err(error) => {
            return (
                vec![GateSignal::AnalysisFailed {
                    detail: error.to_string(),
                }],
                None,
            );
        }
    };

    let current_lookup =
        lumin_inventory::ReservedStateIdentityLookup::from_identities(reserved_identities.clone());
    let current_inventory = lumin_inventory::begin_scan_in_current_pool_with_reserved_state_lookup(
        root,
        &validation.inventory_request,
        &current_lookup,
    )
    .and_then(|pending| pending.finish(root));
    match current_inventory {
        Ok(inventory) => {
            let mut observed_semantic_inputs = crate::semantic_input_records(&inventory);
            let mut expected_semantic_read_bindings = validation
                .bindings
                .iter()
                .map(|(_, binding)| binding.clone())
                .collect::<Vec<_>>();
            expected_semantic_read_bindings.sort();
            expected_semantic_read_bindings.dedup();
            let mut observed_semantic_read_bindings = Vec::new();
            for (path, _) in &validation.bindings {
                match semantic_read_reservation(root, path) {
                    Ok(binding) => observed_semantic_read_bindings.push(binding),
                    Err(error) => {
                        return (
                            vec![GateSignal::AnalysisFailed {
                                detail: error.to_string(),
                            }],
                            None,
                        );
                    }
                }
            }
            observed_semantic_read_bindings.sort();
            observed_semantic_read_bindings.dedup();

            let topology_only_sha256 = digest_hex(DEPENDENCY_CANDIDATE_TOPOLOGY_ONLY);
            for input in &validation.captured_inputs {
                let topology_only = matches!(
                    input.state,
                    SemanticInputState::Missing | SemanticInputState::Unreadable
                ) && input.payload_sha256.as_deref()
                    == Some(topology_only_sha256.as_str());
                if topology_only
                    && !observed_semantic_inputs
                        .iter()
                        .any(|observed| observed.path == input.path)
                    && expected_semantic_read_bindings
                        .iter()
                        .find(|binding| binding.path == input.path)
                        == observed_semantic_read_bindings
                            .iter()
                            .find(|binding| binding.path == input.path)
                {
                    observed_semantic_inputs.push(input.clone());
                }
            }
            observed_semantic_inputs.sort();
            observed_semantic_inputs.dedup();

            for input in &validation.captured_inputs {
                if input.state != SemanticInputState::Source
                    && !semantic_validation_drift.contains(&input.path)
                    && !observed_semantic_inputs
                        .iter()
                        .any(|observed| observed.path == input.path)
                {
                    observed_semantic_inputs.push(input.clone());
                }
            }
            observed_semantic_inputs.sort();
            observed_semantic_inputs.dedup();

            // `derive_pre_write_final_validation_signals` already detects every newly observed or
            // changed record. Record the other half here so a source/config input that disappeared
            // completely from the new inventory is also a fail-closed freshness drift.
            semantic_validation_drift.extend(
                validation
                    .captured_inputs
                    .iter()
                    .filter(|expected| !observed_semantic_inputs.contains(expected))
                    .map(|expected| expected.path.clone()),
            );
            semantic_validation_drift.sort();
            semantic_validation_drift.dedup();

            let write_domain = observe_write_domain_from_semantic_inputs(
                root,
                expected_leases,
                &observed_semantic_inputs,
                &validation.unavailable_capability_targets,
            );
            if !write_domain.failures.is_empty() {
                let signals = write_domain
                    .failures
                    .into_iter()
                    .map(|detail| GateSignal::AnalysisFailed { detail })
                    .collect();
                return (signals, None);
            }

            match capability_availability_drift_paths(root, validation, &final_lookup) {
                Ok(paths) => {
                    semantic_validation_drift.extend(paths);
                    semantic_validation_drift.sort();
                    semantic_validation_drift.dedup();
                }
                Err(error) => {
                    return (
                        vec![GateSignal::AnalysisFailed {
                            detail: error.to_string(),
                        }],
                        None,
                    );
                }
            }

            let evidence = PreWriteFinalValidationEvidence {
                expected_semantic_read_bindings,
                observed_semantic_read_bindings,
                observed_semantic_inputs,
                observed_leased_write_set: write_domain.leases,
                observed_alias_closures: write_domain.alias_closures,
                write_domain_drift_paths: write_domain.drift_paths,
                semantic_input_validation_drift_paths: semantic_validation_drift,
            };
            let signals = derive_pre_write_final_validation_signals(
                &validation.captured_inputs,
                expected_leases,
                expected_alias_closures,
                &evidence,
            );
            (signals, Some(evidence))
        }
        Err(error) => (
            vec![GateSignal::AnalysisFailed {
                detail: error.to_string(),
            }],
            None,
        ),
    }
}

fn post_write_final_validation(
    root: &Path,
    validation: &FinalFreshnessValidation,
    reserved_identities: &std::collections::BTreeSet<lumin_model::PhysicalFileIdentity>,
    opening_leases: &[lumin_evidence::WriteLease],
    opening_entries: &[RepoPath],
    expected_leases: &[lumin_evidence::WriteLease],
    expected_alias_closures: &[lumin_evidence::PhysicalAliasClosureRecord],
) -> (Vec<GateSignal>, Option<PreWriteFinalValidationEvidence>) {
    let mut containment_signals = close_containment_signals(
        root,
        opening_leases,
        opening_entries,
        &validation.inventory_request.dependency_intents,
    );
    if !containment_signals.is_empty() {
        let needs_unsealed_marker = containment_signals
            .iter()
            .any(|signal| matches!(signal, GateSignal::ProtectedInputChanged { .. }))
            && !containment_signals.iter().any(|signal| {
                matches!(
                    signal,
                    GateSignal::AnalysisFailed { .. }
                        | GateSignal::PlannedPathContainmentViolation { .. }
                )
            });
        if needs_unsealed_marker {
            containment_signals.push(GateSignal::AnalysisFailed {
                detail: "post-write containment drift prevented a complete final observation"
                    .to_owned(),
            });
        }
        return (containment_signals, None);
    }
    pre_write_final_validation(
        root,
        validation,
        reserved_identities,
        expected_leases,
        expected_alias_closures,
    )
}

fn revalidate_attempted_semantic_inputs(
    root: &Path,
    bindings: &[(RepoPath, SemanticReadReservationBinding)],
    reserved_identities: &std::collections::BTreeSet<lumin_model::PhysicalFileIdentity>,
) -> Vec<GateSignal> {
    let mut stale = Vec::new();
    for (path, expected) in bindings {
        match semantic_read_reservation(root, path) {
            Ok(current)
                if current == *expected
                    && !current
                        .physical_identity
                        .as_ref()
                        .is_some_and(|identity| reserved_identities.contains(identity))
                    && !current.absence_parent.as_ref().is_some_and(|parent| {
                        reserved_identities.contains(&parent.physical_identity)
                    }) => {}
            Ok(current) => stale.push(current.path),
            Err(error) => {
                return vec![GateSignal::AnalysisFailed {
                    detail: error.to_string(),
                }];
            }
        }
    }
    stale.sort();
    stale.dedup();
    if stale.is_empty() {
        Vec::new()
    } else {
        vec![GateSignal::ProtectedInputChanged { paths: stale }]
    }
}

fn stale_complete_semantic_input_paths(
    root: &Path,
    bindings: &[(RepoPath, SemanticReadReservationBinding)],
    inputs: &[SemanticInputRecord],
    reserved_state_lookup: &lumin_inventory::ReservedStateIdentityLookup,
    reserved_identities: &std::collections::BTreeSet<lumin_model::PhysicalFileIdentity>,
) -> Result<Vec<RepoPathProjection>, EngineError> {
    let topology_only_sha256 = digest_hex(DEPENDENCY_CANDIDATE_TOPOLOGY_ONLY);
    let mut stale = Vec::new();
    for input in inputs {
        let path = decode_semantic_input_path(&input.path)?;
        let topology_only_dependency_candidate = matches!(
            input.state,
            SemanticInputState::Missing | SemanticInputState::Unreadable
        ) && input.payload_sha256.as_deref()
            == Some(topology_only_sha256.as_str())
            && bindings
                .iter()
                .any(|(_, binding)| binding.path == input.path);
        if topology_only_dependency_candidate {
            continue;
        }

        let redirect_is_current = input
            .physical_redirect_sha256
            .as_ref()
            .is_none_or(|sha256| {
                lumin_inventory::validate_captured_physical_path_redirect(
                    root,
                    &path,
                    sha256,
                    reserved_identities,
                )
                .is_ok()
            });
        let input_is_current = if input.state == SemanticInputState::PathRedirect {
            input.physical_redirect_sha256.is_some()
        } else {
            let expectation = semantic_input_expectation(input, path)?;
            lumin_inventory::validate_captured_semantic_input(
                root,
                &expectation,
                reserved_state_lookup,
            )
            .is_ok()
        };
        if !redirect_is_current || !input_is_current {
            stale.push(input.path.clone());
        }
    }
    Ok(stale)
}

fn semantic_input_expectation(
    input: &SemanticInputRecord,
    path: RepoPath,
) -> Result<SemanticInputExpectation, EngineError> {
    let state = match input.state {
        SemanticInputState::Source
        | SemanticInputState::ConfigPresent
        | SemanticInputState::CapabilityTarget => SemanticInputValidationState::Regular,
        SemanticInputState::Missing => SemanticInputValidationState::Missing,
        SemanticInputState::NonRegular => SemanticInputValidationState::NonRegular,
        SemanticInputState::Unreadable => SemanticInputValidationState::Unreadable,
        SemanticInputState::PathRedirect => {
            return Err(EngineError::TierProjectionCorrupt(format!(
                "standalone redirect entered ordinary gate-input validation: {}",
                input.path.display
            )));
        }
    };
    Ok(SemanticInputExpectation {
        path,
        state,
        payload_sha256: input.payload_sha256.clone(),
        physical_identity: input.physical_identity.clone(),
        absence_parent: input
            .absence_parent
            .as_ref()
            .map(|parent| -> Result<ConfigAbsenceParent, EngineError> {
                Ok(ConfigAbsenceParent {
                    path: decode_semantic_input_path(&parent.path)?,
                    physical_identity: parent.physical_identity.clone(),
                })
            })
            .transpose()?,
    })
}

fn decode_semantic_input_path(path: &RepoPathProjection) -> Result<RepoPath, EngineError> {
    let decoded = RepoPath::from_canonical_bytes(&path.canonical).map_err(|error| {
        EngineError::TierProjectionCorrupt(format!(
            "failed to decode captured input {}: {error}",
            path.display
        ))
    })?;
    if RepoPathProjection::from(&decoded) != *path {
        return Err(EngineError::TierProjectionCorrupt(format!(
            "captured input projection round-trip failed for {}",
            path.display
        )));
    }
    Ok(decoded)
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
        let topology_only_sha256 = digest_hex(DEPENDENCY_CANDIDATE_TOPOLOGY_ONLY);
        let (state, payload_sha256) = if binding.absence_parent.is_some() {
            (SemanticInputState::Missing, Some(topology_only_sha256))
        } else {
            (SemanticInputState::Unreadable, Some(topology_only_sha256))
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

pub fn gate_observation_binding_matches_owner(
    gate: &GateRecord,
    revision: &lumin_evidence::GateRevision,
) -> Result<bool, EngineError> {
    observation_binding_matches_owner(gate, revision).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_escape_classifies_by_canonical_path_when_displays_collide()
    -> Result<(), Box<dyn std::error::Error>> {
        let escaped = RepoPath::from_canonical_bytes(
            b"LUMRPATH\x00\x01\x00\x00\x00\x01\x03\x00\x00\x00\x02\xd8\x00",
        )?;
        let decoy = RepoPath::from_portable("wtf16[d800]")?;
        assert_ne!(escaped, decoy);
        assert_eq!(escaped.display_escaped(), decoy.display_escaped());

        let lease = |path: &RepoPath, kind| lumin_evidence::WriteLease {
            path: RepoPathProjection::from(path),
            kind,
            physical_identity: None,
            nearest_existing_parent: None,
            prefix_identities: Vec::new(),
        };
        let leases = [
            lease(&decoy, lumin_evidence::WriteLeaseKind::NewFile),
            lease(&escaped, lumin_evidence::WriteLeaseKind::ExistingFile),
        ];
        let error = EngineError::Inventory(InventoryError::EntryEscapesRoot(escaped.clone()));

        assert_eq!(
            post_write_capture_failure_signals(Path::new("."), &leases, &[], &[], &error),
            [GateSignal::ProtectedInputChanged {
                paths: vec![RepoPathProjection::from(&escaped)]
            }]
        );
        Ok(())
    }

    #[test]
    fn capture_dependency_context_escape_remains_an_analysis_failure()
    -> Result<(), Box<dyn std::error::Error>> {
        let context = RepoPath::from_portable("context")?;
        let error = EngineError::Inventory(InventoryError::EntryEscapesRoot(context.clone()));
        let signals = post_write_capture_failure_signals(
            Path::new("."),
            &[],
            &[],
            &[DependencyIntent {
                path: context,
                dependency: "zod".to_owned(),
            }],
            &error,
        );

        assert_eq!(signals.len(), 1);
        assert!(matches!(signals[0], GateSignal::AnalysisFailed { .. }));
        Ok(())
    }

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
                &lumin_inventory::ReservedStateIdentityLookup::empty(),
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
                &lumin_inventory::ReservedStateIdentityLookup::empty(),
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
        let reserved_state_lookup = lumin_inventory::ReservedStateIdentityLookup::empty();
        lumin_inventory::validate_captured_semantic_input_topology(
            root.path(),
            &candidate,
            binding
                .physical_identity
                .as_ref()
                .ok_or("present dependency reservation omitted its physical identity")?,
            &reserved_state_lookup,
        )?;
        let validation = FinalFreshnessValidation {
            bindings: vec![reserved.clone()],
            captured_inputs: vec![captured.clone()],
            inventory_request: InventoryRequest::default(),
            reserved_state_lookup,
            unavailable_capability_targets: BTreeSet::new(),
            capability_intents: Vec::new(),
            active_capability_intents: BTreeSet::new(),
        };

        assert!(
            final_freshness_validation_signals(
                root.path(),
                &validation,
                &std::collections::BTreeSet::new(),
            )
            .is_empty()
        );

        std::fs::write(
            root.path().join("package.json"),
            r#"{"name":"root","workspaces":["changed/*"]}"#,
        )?;
        assert_eq!(
            final_freshness_validation_signals(
                root.path(),
                &validation,
                &std::collections::BTreeSet::new(),
            ),
            [GateSignal::ProtectedInputChanged {
                paths: vec![binding.path]
            }]
        );
        Ok(())
    }

    #[test]
    fn final_freshness_rehashes_captured_source_payloads() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = tempfile::tempdir()?;
        std::fs::create_dir_all(root.path().join("src"))?;
        let path = RepoPath::from_portable("src/lib.ts")?;
        let native = root.path().join("src/lib.ts");
        let original = b"export const value = 1;\n";
        std::fs::write(&native, original)?;
        let identity = lumin_inventory::observe_physical_file_identity(root.path(), &path)?;
        let reserved_state_lookup = lumin_inventory::ReservedStateIdentityLookup::empty();
        lumin_inventory::validate_captured_semantic_input_topology(
            root.path(),
            &path,
            &identity,
            &reserved_state_lookup,
        )?;
        let validation = FinalFreshnessValidation {
            bindings: Vec::new(),
            captured_inputs: vec![SemanticInputRecord {
                path: RepoPathProjection::from(&path),
                state: SemanticInputState::Source,
                payload_sha256: Some(digest_hex(original)),
                physical_identity: Some(identity),
                absence_parent: None,
                physical_redirect_sha256: None,
            }],
            inventory_request: InventoryRequest::default(),
            reserved_state_lookup,
            unavailable_capability_targets: BTreeSet::new(),
            capability_intents: Vec::new(),
            active_capability_intents: BTreeSet::new(),
        };

        assert!(
            final_freshness_validation_signals(
                root.path(),
                &validation,
                &std::collections::BTreeSet::new(),
            )
            .is_empty()
        );

        std::fs::write(&native, "export const value = 2;\n")?;
        assert_eq!(
            final_freshness_validation_signals(
                root.path(),
                &validation,
                &std::collections::BTreeSet::new(),
            ),
            [GateSignal::ProtectedInputChanged {
                paths: vec![RepoPathProjection::from(&path)]
            }]
        );
        Ok(())
    }

    #[test]
    fn final_freshness_rejects_a_new_reserved_link_to_a_captured_source()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        std::fs::create_dir_all(root.path().join("src"))?;
        std::fs::create_dir_all(root.path().join(".lumin/cache"))?;
        let path = RepoPath::from_portable("src/lib.ts")?;
        let native = root.path().join("src/lib.ts");
        std::fs::write(&native, "export const value = 1;\n")?;
        let identity = lumin_inventory::observe_physical_file_identity(root.path(), &path)?;
        let lookup = lumin_inventory::ReservedStateIdentityLookup::empty();
        lumin_inventory::validate_captured_semantic_input_topology(
            root.path(),
            &path,
            &identity,
            &lookup,
        )?;
        let validation = FinalFreshnessValidation {
            bindings: Vec::new(),
            captured_inputs: vec![SemanticInputRecord {
                path: RepoPathProjection::from(&path),
                state: SemanticInputState::Source,
                payload_sha256: Some(digest_hex(b"export const value = 1;\n")),
                physical_identity: Some(identity),
                absence_parent: None,
                physical_redirect_sha256: None,
            }],
            inventory_request: InventoryRequest::default(),
            reserved_state_lookup: lookup,
            unavailable_capability_targets: BTreeSet::new(),
            capability_intents: Vec::new(),
            active_capability_intents: BTreeSet::new(),
        };

        std::fs::hard_link(&native, root.path().join(".lumin/cache/alias.ts"))?;
        assert_eq!(
            final_freshness_validation_signals(
                root.path(),
                &validation,
                &std::collections::BTreeSet::new(),
            ),
            [GateSignal::ProtectedInputChanged {
                paths: vec![RepoPathProjection::from(&path)]
            }]
        );
        Ok(())
    }

    #[test]
    fn final_freshness_rechecks_reserved_membership_without_candidate_topology_change()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        std::fs::create_dir_all(root.path().join("src"))?;
        let path = RepoPath::from_portable("src/lib.ts")?;
        std::fs::write(root.path().join("src/lib.ts"), "export const value = 1;\n")?;
        let identity = lumin_inventory::observe_physical_file_identity(root.path(), &path)?;
        let lookup = lumin_inventory::ReservedStateIdentityLookup::empty();
        lumin_inventory::validate_captured_semantic_input_topology(
            root.path(),
            &path,
            &identity,
            &lookup,
        )?;
        let validation = FinalFreshnessValidation {
            bindings: Vec::new(),
            captured_inputs: vec![SemanticInputRecord {
                path: RepoPathProjection::from(&path),
                state: SemanticInputState::Source,
                payload_sha256: Some(digest_hex(b"export const value = 1;\n")),
                physical_identity: Some(identity.clone()),
                absence_parent: None,
                physical_redirect_sha256: None,
            }],
            inventory_request: InventoryRequest::default(),
            reserved_state_lookup: lookup,
            unavailable_capability_targets: BTreeSet::new(),
            capability_intents: Vec::new(),
            active_capability_intents: BTreeSet::new(),
        };
        let reserved_identities = std::collections::BTreeSet::from([identity]);

        assert_eq!(
            final_freshness_validation_signals(root.path(), &validation, &reserved_identities,),
            [GateSignal::ProtectedInputChanged {
                paths: vec![RepoPathProjection::from(&path)]
            }]
        );
        Ok(())
    }
}
