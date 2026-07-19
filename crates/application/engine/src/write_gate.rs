use std::fs;
use std::path::{Path, PathBuf};

use lumin_evidence::{
    GateAnalysisOptions, GateBaseline, GateOperationResult, GateRecord, GateSignal,
    OperationRecord, RepoPathProjection, SemanticInputState, gate_policy,
};
use lumin_inventory::InventoryRequest;
use lumin_model::{
    GateId, OperationId, RepoPath, ResolutionProfile, append_length_prefixed, digest_hex,
};
use lumin_store::{PostWriteStart, PreWriteStart, RepositoryStore};

use super::{EngineError, capture_repository};

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
    let store = RepositoryStore::open(&request.root)?;
    let gate_id = match store.reserve_pre_write(
        &request.operation_id,
        &request_digest,
        &declared_write_set,
        &analysis_options,
    )? {
        PreWriteStart::Committed(result) => return Ok(result),
        PreWriteStart::Analyze { gate_id } => gate_id,
    };

    let (baseline, signals) = analyze_pre_write(request, &paths);
    store
        .finish_pre_write(
            &request.operation_id,
            &request_digest,
            &gate_id,
            baseline,
            signals,
        )
        .map_err(Into::into)
}

fn analyze_pre_write(
    request: &PreWriteRequest,
    paths: &[RepoPath],
) -> (Option<GateBaseline>, Vec<GateSignal>) {
    let mut signals = paths
        .iter()
        .filter_map(|path| inspect_declared_path(&request.root, path))
        .collect::<Vec<_>>();
    if !signals.is_empty() {
        return (None, signals);
    }

    let capture = match capture_repository(
        &request.root,
        &InventoryRequest::default(),
        request.jobs,
        request.resolution_profile,
    ) {
        Ok(capture) => capture,
        Err(error) => {
            signals.push(GateSignal::AnalysisFailed {
                detail: error.to_string(),
            });
            return (None, signals);
        }
    };

    for path in paths {
        let projection = RepoPathProjection::from(path);
        let is_source = capture
            .snapshot
            .inputs
            .iter()
            .any(|input| input.path == projection && input.state == SemanticInputState::Source);
        if !is_source {
            signals.push(GateSignal::DeclaredPathUnsupported {
                path: projection,
                reason: lumin_evidence::DeclaredPathUnsupportedReason::NotAnalyzedSource,
            });
            continue;
        }
        match lumin_inventory::admitted_physical_aliases(&request.root, path, &capture.source_paths)
        {
            Ok(aliases) if aliases.len() == 1 && aliases[0] == *path => {}
            Ok(_) => signals.push(GateSignal::DeclaredPathUnsupported {
                path: projection,
                reason: lumin_evidence::DeclaredPathUnsupportedReason::MultiplyLinked,
            }),
            Err(error) => signals.push(GateSignal::AnalysisFailed {
                detail: error.to_string(),
            }),
        }
    }
    signals.extend(gate_policy::opening_signals(&capture.snapshot.evidence));
    let baseline = GateBaseline {
        analysis_contract: ANALYSIS_CONTRACT.to_owned(),
        snapshot: capture.snapshot,
    };
    (Some(baseline), signals)
}

pub fn close_write_gate(request: &PostWriteRequest) -> Result<GateOperationResult, EngineError> {
    let request_digest = post_write_digest(&request.gate_id);
    let store = RepositoryStore::open(&request.root)?;
    let gate =
        match store.begin_post_write(&request.operation_id, &request_digest, &request.gate_id)? {
            PostWriteStart::Committed(result) => return Ok(result),
            PostWriteStart::Analyze { gate } => gate,
        };
    let baseline = gate
        .baseline
        .as_ref()
        .ok_or_else(|| EngineError::GateBaselineMissing(request.gate_id.as_str().to_owned()))?;
    if baseline.analysis_contract != ANALYSIS_CONTRACT {
        return store
            .finish_post_write(
                &request.operation_id,
                &request_digest,
                &request.gate_id,
                None,
                Vec::new(),
                vec![GateSignal::AnalysisContractChanged],
            )
            .map_err(Into::into);
    }

    match capture_repository(
        &request.root,
        &InventoryRequest::default(),
        gate.analysis_options.jobs,
        gate.analysis_options.resolution_profile,
    ) {
        Ok(capture) => {
            let (signals, changed_paths) = gate_policy::closing_signals(
                &baseline.snapshot,
                &capture.snapshot,
                &gate.declared_write_set,
            );
            store
                .finish_post_write(
                    &request.operation_id,
                    &request_digest,
                    &request.gate_id,
                    Some(capture.snapshot),
                    changed_paths,
                    signals,
                )
                .map_err(Into::into)
        }
        Err(error) => store
            .finish_post_write(
                &request.operation_id,
                &request_digest,
                &request.gate_id,
                None,
                Vec::new(),
                vec![GateSignal::AnalysisFailed {
                    detail: error.to_string(),
                }],
            )
            .map_err(Into::into),
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

fn inspect_declared_path(root: &Path, path: &RepoPath) -> Option<GateSignal> {
    let projection = RepoPathProjection::from(path);
    let Some(portable) = path.portable() else {
        return Some(GateSignal::DeclaredPathUnsupported {
            path: projection,
            reason: lumin_evidence::DeclaredPathUnsupportedReason::NotAnalyzedSource,
        });
    };
    if portable == ".lumin" || portable.starts_with(".lumin/") {
        return Some(GateSignal::DeclaredPathUnsupported {
            path: projection,
            reason: lumin_evidence::DeclaredPathUnsupportedReason::ReservedState,
        });
    }
    let full_path = root.join(path.to_native_relative());
    let metadata = match fs::symlink_metadata(&full_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Some(GateSignal::DeclaredPathUnsupported {
                path: projection,
                reason: lumin_evidence::DeclaredPathUnsupportedReason::Missing,
            });
        }
        Err(_) => {
            return Some(GateSignal::DeclaredPathUnsupported {
                path: projection,
                reason: lumin_evidence::DeclaredPathUnsupportedReason::NonRegular,
            });
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Some(GateSignal::DeclaredPathUnsupported {
            path: projection,
            reason: if metadata.file_type().is_symlink() {
                lumin_evidence::DeclaredPathUnsupportedReason::SymlinkOrAliasedPrefix
            } else {
                lumin_evidence::DeclaredPathUnsupportedReason::NonRegular
            },
        });
    }
    let canonical_root = match fs::canonicalize(root) {
        Ok(path) => path,
        Err(_) => {
            return Some(GateSignal::DeclaredPathUnsupported {
                path: projection,
                reason: lumin_evidence::DeclaredPathUnsupportedReason::NonRegular,
            });
        }
    };
    let canonical_path = match fs::canonicalize(&full_path) {
        Ok(path) => path,
        Err(_) => {
            return Some(GateSignal::DeclaredPathUnsupported {
                path: projection,
                reason: lumin_evidence::DeclaredPathUnsupportedReason::NonRegular,
            });
        }
    };
    if canonical_path != canonical_root.join(path.to_native_relative()) {
        return Some(GateSignal::DeclaredPathUnsupported {
            path: projection,
            reason: lumin_evidence::DeclaredPathUnsupportedReason::SymlinkOrAliasedPrefix,
        });
    }
    None
}

fn pre_write_digest(paths: &[RepoPath], options: &GateAnalysisOptions) -> String {
    let mut bytes = Vec::new();
    append_length_prefixed(&mut bytes, b"lumin-pre-write.v1");
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
    append_length_prefixed(&mut bytes, b"lumin-post-write.v1");
    append_length_prefixed(&mut bytes, gate_id.as_str().as_bytes());
    digest_hex(&bytes)
}
