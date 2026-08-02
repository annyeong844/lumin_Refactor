use std::path::{Path, PathBuf};

use lumin_evidence::{
    GateAnalysisOptions, GateBaseline, GateOperationResult, GateRecord, GateSignal,
    OperationRecord, RepoPathProjection, ScanInvocationTier, SemanticReadReservationBinding,
    gate_policy,
};
use lumin_inventory::InventoryRequest;
use lumin_model::{
    GateDeltaRecord, GateId, OperationId, RepoPath, RepositoryRootIdentity, ResolutionProfile,
    append_length_prefixed, digest_hex,
};
use lumin_store::{
    OperationSession, PostWriteFinish, PostWriteStart, PreWriteFinish, PreWriteStart,
    SemanticReadReservation,
};

use super::{
    EngineError, RepositoryAnalysisSession, RepositoryAnalysisStep, RepositoryCapture,
    RepositoryContext, open_repository_context,
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

const ANALYSIS_CONTRACT_VERSION: &[u8] = b"lumin-analysis-contract.phase1-foundation.v4";

fn analysis_contract_id() -> String {
    let inputs = [
        ANALYSIS_CONTRACT_VERSION,
        lumin_model::PATH_CODEC_ARTIFACT_SHA256.as_bytes(),
        lumin_model::PATH_CODEC_TABLE_SHA256.as_bytes(),
        lumin_model::SOURCE_CLASSIFICATION_RULE_VERSION.as_bytes(),
        lumin_inventory::INVENTORY_CONFIG_ARTIFACT_SHA256.as_bytes(),
        lumin_inventory::INVENTORY_CONFIG_TABLE_SHA256.as_bytes(),
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
    // Fail closed: validate caller entries BEFORE opening/reserving an operation/gate
    lumin_inventory::validate_caller_entries(&request.root, &request.entries)?;
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
    let context = open_repository_context(&request.root)?;
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

    let finish = if inspection.signals.is_empty() {
        match analyze_pre_write(
            &operation,
            &context,
            request,
            inspection,
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
            leased_write_set: inspection.leases,
            alias_closures: Vec::new(),
            signals: inspection.signals,
        }
    };
    operation
        .finish_pre_write(&request_digest, &gate_id, finish)
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
    ScanInvocationTier {
        includes: request.includes.clone(),
        excludes: request.excludes.clone(),
        role_overrides: request.role_overrides.clone(),
        entries,
        resolution_profile: request.resolution_profile,
    }
}

#[allow(clippy::large_enum_variant)]
enum PreWriteAnalysis {
    Finished(PreWriteFinish),
    Committed(GateOperationResult),
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
        Ok(ReservedCapture::Finished { capture, .. }) => capture,
        Ok(ReservedCapture::Blocked(signal)) => {
            return Ok(PreWriteAnalysis::Finished(PreWriteFinish {
                baseline: None,
                leased_write_set: inspection.leases,
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
                leased_write_set: inspection.leases,
                alias_closures: Vec::new(),
                signals: vec![GateSignal::AnalysisFailed {
                    detail: error.to_string(),
                }],
            }));
        }
    };
    let (leased_write_set, alias_closures, mut signals) = expand_write_domain(
        &context.root,
        &inspection.observations,
        inspection.leases,
        &capture,
    );
    let protected_semantic_inputs = protected_semantic_inputs(&capture, &leased_write_set);
    signals.extend(gate_policy::opening_signals(&capture.snapshot.evidence));
    let baseline = GateBaseline {
        analysis_contract: analysis_contract_id(),
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
    if let Err(error) =
        lumin_inventory::validate_caller_entries(&context.root, &inventory_request.entries)
    {
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

    let capture = match capture_reserved_repository(
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
        Ok(ReservedCapture::Finished { capture }) => capture,
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
        )
        .map_err(Into::into)
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
        )
        .map_err(Into::into)
}

enum ReservedCapture {
    Finished { capture: Box<RepositoryCapture> },
    Blocked(GateSignal),
    Committed(GateOperationResult),
}

fn capture_reserved_repository(
    root: &Path,
    repository_root: &RepositoryRootIdentity,
    options: &GateAnalysisOptions,
    inventory_request: &InventoryRequest,
    mut reserve: impl FnMut(
        &[SemanticReadReservationBinding],
    ) -> Result<SemanticReadReservation, EngineError>,
) -> Result<ReservedCapture, EngineError> {
    let mut session = RepositoryAnalysisSession::start(
        root,
        repository_root.clone(),
        inventory_request,
        options.jobs,
        options.scan_invocation.clone(),
    )?;
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
                        return Ok(ReservedCapture::Committed(*result));
                    }
                }
            }
            RepositoryAnalysisStep::Finished(resolver) => {
                return session
                    .finish(root, resolver)
                    .map(|capture| ReservedCapture::Finished {
                        capture: Box::new(capture),
                    });
            }
        }
    }
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
    Ok(InventoryRequest {
        includes: tier.includes.clone(),
        excludes: tier.excludes.clone(),
        role_overrides: tier.role_overrides.clone(),
        entries,
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
    append_length_prefixed(&mut bytes, b"lumin-pre-write.v3");
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
