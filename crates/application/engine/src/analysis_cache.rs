use lumin_evidence::{
    AnalysisSnapshot, EntrySelectionRecord, RUN_EVIDENCE_SCHEMA_VERSION, RepoPathProjection,
    ScanInvocationTier, SemanticInputRecord, seal_analysis_snapshot,
    validate_run_evidence_identities, validate_run_evidence_inputs,
};
use lumin_model::{
    AnalysisInputId, ConfigSyntax, RepoPath, RepositoryRootIdentity, append_length_prefixed,
    digest_hex,
};
use lumin_resolve::ConfigDemand;
use lumin_store::RepositoryStore;
use serde::{Deserialize, Serialize};

use super::{EngineError, RepositoryCapture};

const CACHE_ENVELOPE_SCHEMA: &str = "lumin-repository-analysis-cache.v1";
const SUPPLIED_INPUT_KEY_SCHEMA: &[u8] = b"lumin-analysis-supplied-input-key.v1";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum CachedRepositoryAnalysisStep {
    NeedsInputs {
        schema_version: String,
        owner_contract_version: String,
        supplied_input_key: String,
        demands: Vec<CachedConfigDemand>,
    },
    Finished {
        schema_version: String,
        owner_contract_version: String,
        supplied_input_key: String,
        semantic_input_key: AnalysisInputId,
        snapshot: Box<AnalysisSnapshot>,
    },
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CachedConfigDemand {
    path: RepoPathProjection,
    syntax: ConfigSyntax,
}

pub(crate) enum ReplayedAnalysisStep {
    NeedsInputs(Vec<ConfigDemand>),
    Finished(Box<RepositoryCapture>),
}

pub(crate) struct AnalysisCacheContext {
    pub(crate) supplied_input_key: String,
    inputs: Vec<SemanticInputRecord>,
    entry_selections: Vec<EntrySelectionRecord>,
}

pub(crate) fn context(
    repository_root: &RepositoryRootIdentity,
    owner_contract_version: &str,
    scan_invocation: &ScanInvocationTier,
    mut inputs: Vec<SemanticInputRecord>,
    mut entry_selections: Vec<EntrySelectionRecord>,
) -> Result<AnalysisCacheContext, EngineError> {
    inputs.sort();
    inputs.dedup();
    entry_selections.sort();
    entry_selections.dedup();

    let encoded_inputs = serde_json::to_vec(&inputs)
        .map_err(|error| EngineError::CacheEncoding(error.to_string()))?;
    let encoded_entries = serde_json::to_vec(&entry_selections)
        .map_err(|error| EngineError::CacheEncoding(error.to_string()))?;
    let mut framed = Vec::new();
    append_length_prefixed(&mut framed, SUPPLIED_INPUT_KEY_SCHEMA);
    append_length_prefixed(&mut framed, owner_contract_version.as_bytes());
    append_length_prefixed(&mut framed, repository_root.canonical_bytes());
    scan_invocation.append_semantic_framing(&mut framed);
    append_length_prefixed(&mut framed, &encoded_entries);
    append_length_prefixed(&mut framed, &encoded_inputs);

    Ok(AnalysisCacheContext {
        supplied_input_key: digest_hex(&framed),
        inputs,
        entry_selections,
    })
}

pub(crate) fn load(
    store: &RepositoryStore,
    context: &AnalysisCacheContext,
    owner_contract_version: &str,
    scan_invocation: &ScanInvocationTier,
) -> Result<Option<ReplayedAnalysisStep>, EngineError> {
    let mut replay = None;
    for bytes in store.read_analysis_cache_candidates(&context.supplied_input_key)? {
        let Ok(candidate) = serde_json::from_slice::<CachedRepositoryAnalysisStep>(&bytes) else {
            continue;
        };
        if serde_json::to_vec(&candidate).ok().as_deref() != Some(bytes.as_slice()) {
            continue;
        }
        let Some(candidate) =
            validate_candidate(candidate, context, owner_contract_version, scan_invocation)
        else {
            continue;
        };
        if replay.is_some() {
            return Ok(None);
        }
        replay = Some(candidate);
    }
    Ok(replay)
}

pub(crate) fn store_demands(
    store: &RepositoryStore,
    context: &AnalysisCacheContext,
    owner_contract_version: &str,
    demands: &[ConfigDemand],
) -> Result<(), EngineError> {
    let demands = demands
        .iter()
        .map(|demand| CachedConfigDemand {
            path: RepoPathProjection::from(&demand.path),
            syntax: demand.syntax,
        })
        .collect();
    let candidate = CachedRepositoryAnalysisStep::NeedsInputs {
        schema_version: CACHE_ENVELOPE_SCHEMA.to_owned(),
        owner_contract_version: owner_contract_version.to_owned(),
        supplied_input_key: context.supplied_input_key.clone(),
        demands,
    };
    store_candidate(store, context, &candidate)
}

pub(crate) fn store_finished(
    store: &RepositoryStore,
    context: &AnalysisCacheContext,
    owner_contract_version: &str,
    capture: &RepositoryCapture,
) -> Result<(), EngineError> {
    let candidate = CachedRepositoryAnalysisStep::Finished {
        schema_version: CACHE_ENVELOPE_SCHEMA.to_owned(),
        owner_contract_version: owner_contract_version.to_owned(),
        supplied_input_key: context.supplied_input_key.clone(),
        semantic_input_key: capture.snapshot.analysis_input_id.clone(),
        snapshot: Box::new(capture.snapshot.clone()),
    };
    store_candidate(store, context, &candidate)
}

fn store_candidate(
    store: &RepositoryStore,
    context: &AnalysisCacheContext,
    candidate: &CachedRepositoryAnalysisStep,
) -> Result<(), EngineError> {
    let bytes = serde_json::to_vec(candidate)
        .map_err(|error| EngineError::CacheEncoding(error.to_string()))?;
    store.write_analysis_cache_candidate(&context.supplied_input_key, &bytes)?;
    Ok(())
}

fn validate_candidate(
    candidate: CachedRepositoryAnalysisStep,
    context: &AnalysisCacheContext,
    owner_contract_version: &str,
    scan_invocation: &ScanInvocationTier,
) -> Option<ReplayedAnalysisStep> {
    match candidate {
        CachedRepositoryAnalysisStep::NeedsInputs {
            schema_version,
            owner_contract_version: observed_contract,
            supplied_input_key,
            demands,
        } => {
            if schema_version != CACHE_ENVELOPE_SCHEMA
                || observed_contract != owner_contract_version
                || supplied_input_key != context.supplied_input_key
                || demands.is_empty()
            {
                return None;
            }
            let mut canonical = demands.clone();
            canonical.sort();
            canonical.dedup();
            if canonical != demands {
                return None;
            }
            let supplied_paths = context
                .inputs
                .iter()
                .map(|input| input.path.canonical.as_slice())
                .collect::<std::collections::BTreeSet<_>>();
            let mut decoded = Vec::with_capacity(demands.len());
            for demand in demands {
                if supplied_paths.contains(demand.path.canonical.as_slice()) {
                    return None;
                }
                let path = RepoPath::from_canonical_bytes(&demand.path.canonical).ok()?;
                if RepoPathProjection::from(&path) != demand.path {
                    return None;
                }
                decoded.push(ConfigDemand {
                    path,
                    syntax: demand.syntax,
                });
            }
            Some(ReplayedAnalysisStep::NeedsInputs(decoded))
        }
        CachedRepositoryAnalysisStep::Finished {
            schema_version,
            owner_contract_version: observed_contract,
            supplied_input_key,
            semantic_input_key,
            snapshot,
        } => {
            if schema_version != CACHE_ENVELOPE_SCHEMA
                || observed_contract != owner_contract_version
                || supplied_input_key != context.supplied_input_key
                || snapshot.evidence.schema_version != RUN_EVIDENCE_SCHEMA_VERSION
                || snapshot.analysis_input_id != semantic_input_key
                || snapshot.inputs != context.inputs
                || snapshot.scan_invocation != *scan_invocation
                || snapshot.entry_selections != context.entry_selections
            {
                return None;
            }
            let resealed = seal_analysis_snapshot(
                snapshot.inputs.clone(),
                snapshot.evidence.clone(),
                snapshot.scan_invocation.clone(),
                snapshot.entry_selections.clone(),
            );
            if resealed != *snapshot
                || validate_run_evidence_identities(&snapshot.evidence).is_err()
                || validate_run_evidence_inputs(&snapshot.evidence, &snapshot.inputs).is_err()
            {
                return None;
            }
            let mut inferred_write_paths = Vec::new();
            for owner in &snapshot.evidence.dependency_owners {
                inferred_write_paths
                    .push(RepoPath::from_canonical_bytes(&owner.manifest_path.canonical).ok()?);
                if let Some(lockfile) = &owner.lockfile_path {
                    inferred_write_paths
                        .push(RepoPath::from_canonical_bytes(&lockfile.canonical).ok()?);
                }
            }
            inferred_write_paths.sort();
            inferred_write_paths.dedup();
            Some(ReplayedAnalysisStep::Finished(Box::new(
                RepositoryCapture {
                    snapshot: *snapshot,
                    inferred_write_paths,
                },
            )))
        }
    }
}
