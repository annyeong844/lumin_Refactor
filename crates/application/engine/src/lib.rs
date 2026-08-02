mod capability_query;
mod extraction;
mod gate_abandon;
mod gate_query;
mod retention;
mod write_gate;

pub use capability_query::{
    CompiledCapabilityRegistry, compiled_capability_registry, query_binary_capabilities,
    query_run_capabilities,
};
pub use gate_abandon::{AbandonGateRequest, abandon_gate};
pub use gate_query::{
    EvidenceQueryError, RunSourceEnvelope, query_gate_explain, query_gate_findings,
    query_run_explain, query_run_file_findings, query_run_findings, query_run_relations,
    query_run_source_classification, query_run_source_envelope,
};
pub use lumin_evidence::{
    GateDecision, GateOperationResult, RecordLookup, RetentionMutationResult, RetentionPlanScope,
};
pub use lumin_store::RunCatalogCursor;
pub use retention::{
    ActiveGateCatalogCursor, ActiveGateCatalogItem, ActiveGateCatalogSnapshot,
    ConfirmRetentionPlanRequest, PinRunRequest, PrepareRetentionPlanRequest, UnpinRunRequest,
    confirm_retention_plan, list_active_gates, list_runs, load_lifecycle_operation,
    load_retention_plan, lookup_gate, lookup_run, pin_run, prepare_retention_plan, unpin_run,
};
pub use write_gate::{
    PostWriteRequest, PreWriteRequest, close_write_gate, load_gate, load_operation, open_write_gate,
};

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use lumin_evidence::{
    AnalysisMetrics, AnalysisSnapshot, CapabilityRecord, DEAD_CODE_CAPABILITY_ID,
    EntrySelectionRecord, RepoPathProjection, RunEvidence, ScanInvocationTier, SemanticInputRecord,
    SemanticInputState, SourceClassificationRecord, SourceContextRecord, SourceObservationRecord,
    seal_analysis_snapshot,
};
use lumin_inventory::{
    InventoryError, InventoryRequest, InventorySnapshot, SemanticPolicyState, repository_admission,
};
use lumin_model::{
    AttemptId, AttemptStatus, CapabilityState, ConfigObservation, FileFacts, Limitation,
    ResolutionOutcome, ResolutionProfile, ResolvedSourceUse, RoleOverride, RunId, SfcDialect,
    SourceSnapshot, digest_hex,
};
use lumin_resolve::{ConfigDemand, ResolverError, ResolverOutput};
use lumin_store::{PublishedRun, RepositoryStore, RunCatalogRecord, StoreError};
use thiserror::Error;

use extraction::extract_facts;
#[cfg(test)]
use extraction::reduce_file_facts;

#[derive(Clone, Debug)]
pub struct AuditRequest {
    pub root: PathBuf,
    pub includes: Vec<String>,
    pub excludes: Vec<String>,
    pub role_overrides: Vec<RoleOverride>,
    pub entries: Vec<lumin_model::RepoPath>,
    pub jobs: usize,
    pub resolution_profile: Option<ResolutionProfile>,
}

#[derive(Clone, Debug)]
pub struct AuditResult {
    pub published: PublishedRun,
    pub evidence: RunEvidence,
}

#[derive(Clone, Debug)]
pub struct LatestAttempt {
    pub attempt_id: AttemptId,
    pub sequence: u64,
    pub status: AttemptStatus,
    pub failure: Option<String>,
}

#[derive(Debug)]
pub struct LatestOverview {
    pub latest_attempt: Option<LatestAttempt>,
    pub completed: Option<(RunCatalogRecord, RunEvidence)>,
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Inventory(#[from] InventoryError),
    #[error(transparent)]
    Resolver(#[from] ResolverError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    EvidenceQuery(#[from] EvidenceQueryError),
    #[error("invalid worker count: {0}")]
    InvalidWorkerCount(usize),
    #[error("failed to build the local worker pool: {0}")]
    Scheduler(String),
    #[error(transparent)]
    Js(#[from] lumin_js::JsExtractError),
    #[error(transparent)]
    Sfc(#[from] lumin_sfc::SfcError),
    #[error("resolver requested semantic inputs that were already captured: {0}")]
    ResolverDemandStalled(String),
    #[error("pre-write requires at least one declared path")]
    NoDeclaredPaths,
    #[error("tier projection corrupt: {0}")]
    TierProjectionCorrupt(String),
    #[error("tier resolution profile inconsistency: {0}")]
    TierProfileInconsistency(String),
    #[error("active gate omitted its sealed opening baseline: {0}")]
    GateBaselineMissing(String),
    #[error(
        "analysis failed ({analysis}) and its attempt failure could not persist ({persistence})"
    )]
    AnalysisAndPersistence {
        analysis: String,
        persistence: String,
    },
    #[error(
        "run publication failed ({publication}) and its attempt failure could not persist ({persistence})"
    )]
    PublicationAndPersistence {
        publication: String,
        persistence: String,
    },
}

impl EngineError {
    pub fn lifecycle_exit_code(&self) -> i32 {
        match self {
            Self::EvidenceQuery(EvidenceQueryError::DuplicateCapabilityId(_)) => 1,
            Self::EvidenceQuery(EvidenceQueryError::DuplicateCollectionId(_)) => 1,
            Self::Inventory(
                InventoryError::ReservedEntryPath(_) | InventoryError::EntryEscapesRoot(_),
            ) => 2,
            Self::NoDeclaredPaths
            | Self::TierProjectionCorrupt(_)
            | Self::TierProfileInconsistency(_)
            | Self::EvidenceQuery(_)
            | Self::Store(
                StoreError::OperationConflict(_)
                | StoreError::OperationNotFound(_)
                | StoreError::RunNotFound(_)
                | StoreError::RunRetentionState(_)
                | StoreError::PinNotFound(_)
                | StoreError::GateNotFound(_)
                | StoreError::GateNotActive(_)
                | StoreError::RetentionPlanNotFound(_)
                | StoreError::RetentionPlanState(_)
                | StoreError::RunCatalogScopeMismatch
                | StoreError::RunCatalogAnchorMissing(_)
                | StoreError::ActiveGateCatalogScopeMismatch
                | StoreError::ActiveGateCatalogAnchorMissing(_)
                | StoreError::ActiveGateCatalogPageSize { .. },
            ) => 2,
            Self::Store(StoreError::GateRevisionBusy(_) | StoreError::OperationBusy(_)) => 4,
            Self::Store(StoreError::GateRevisionChanged(_)) => 5,
            Self::Store(
                StoreError::RunCatalogRevisionChanged { .. }
                | StoreError::ActiveGateCatalogRevisionChanged { .. },
            ) => 5,
            _ => 1,
        }
    }
}

pub fn audit(request: &AuditRequest) -> Result<AuditResult, EngineError> {
    if request.jobs == 0 {
        return Err(EngineError::InvalidWorkerCount(0));
    }
    // Fail closed: validate caller entries BEFORE audit begins an attempt
    lumin_inventory::validate_caller_entries(&request.root, &request.entries)?;
    let context = open_repository_context(&request.root)?;
    let store = &context.store;
    let mut attempt = store.begin_attempt()?;
    let inventory_request = InventoryRequest {
        includes: request.includes.clone(),
        excludes: request.excludes.clone(),
        role_overrides: request.role_overrides.clone(),
        entries: request.entries.clone(),
    };
    let evidence = match capture_repository(
        &context.root,
        &inventory_request,
        request.jobs,
        request.resolution_profile,
    )
    .map(|capture| capture.snapshot.evidence)
    {
        Ok(evidence) => evidence,
        Err(error) => {
            if let Err(persistence) = store.fail_attempt(&mut attempt, &error.to_string()) {
                return Err(EngineError::AnalysisAndPersistence {
                    analysis: error.to_string(),
                    persistence: persistence.to_string(),
                });
            }
            return Err(error);
        }
    };
    let published = match store.publish_run(&mut attempt, &evidence) {
        Ok(published) => published,
        Err(error @ StoreError::RunRetentionState(_)) => {
            return Err(EngineError::Store(error));
        }
        Err(error) => {
            if let Err(persistence) = store.fail_attempt(&mut attempt, &error.to_string()) {
                return Err(EngineError::PublicationAndPersistence {
                    publication: error.to_string(),
                    persistence: persistence.to_string(),
                });
            }
            return Err(EngineError::Store(error));
        }
    };
    Ok(AuditResult {
        published,
        evidence,
    })
}

pub fn analyze_repository(
    root: &Path,
    request: &InventoryRequest,
    jobs: usize,
    resolution_profile: Option<ResolutionProfile>,
) -> Result<RunEvidence, EngineError> {
    let admission = repository_admission(root)?;
    capture_repository(&admission.canonical_root, request, jobs, resolution_profile)
        .map(|capture| capture.snapshot.evidence)
}

struct RepositoryContext {
    root: PathBuf,
    repository_id: lumin_model::RepositoryId,
    store: RepositoryStore,
}

fn open_repository_context(root: &Path) -> Result<RepositoryContext, EngineError> {
    let admission = repository_admission(root)?;
    let repository_id = admission.binding.repository_id().clone();
    let store = RepositoryStore::open(&admission.canonical_root, &admission.binding)?;
    Ok(RepositoryContext {
        root: admission.canonical_root,
        repository_id,
        store,
    })
}

struct RepositoryCapture {
    snapshot: AnalysisSnapshot,
    source_paths: Vec<lumin_model::RepoPath>,
    source_adjacency: BTreeMap<lumin_model::RepoPath, BTreeSet<lumin_model::RepoPath>>,
}

struct RepositoryAnalysisSession {
    inventory: InventorySnapshot,
    facts: Vec<FileFacts>,
    sfc_states: BTreeMap<SfcDialect, CapabilityState>,
    js_parse_product_count: usize,
    scan_invocation: ScanInvocationTier,
}

enum RepositoryAnalysisStep {
    NeedsInputs(Vec<ConfigDemand>),
    Finished(ResolverOutput),
}

fn capture_repository(
    root: &Path,
    request: &InventoryRequest,
    jobs: usize,
    resolution_profile: Option<ResolutionProfile>,
) -> Result<RepositoryCapture, EngineError> {
    let tier = build_scan_invocation_tier(request, resolution_profile);
    let mut session = RepositoryAnalysisSession::start(root, request, jobs, tier)?;
    loop {
        match session.next_step(resolution_profile)? {
            RepositoryAnalysisStep::NeedsInputs(demands) => {
                session.capture_demands(root, demands)?;
            }
            RepositoryAnalysisStep::Finished(resolver) => {
                return session.finish(root, resolver);
            }
        }
    }
}

/// Build a ScanInvocationTier from the request parameters (exact semantic order).
fn build_scan_invocation_tier(
    request: &InventoryRequest,
    resolution_profile: Option<ResolutionProfile>,
) -> ScanInvocationTier {
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
        resolution_profile,
    }
}

impl RepositoryAnalysisSession {
    fn start(
        root: &Path,
        request: &InventoryRequest,
        jobs: usize,
        scan_invocation: ScanInvocationTier,
    ) -> Result<Self, EngineError> {
        if jobs == 0 {
            return Err(EngineError::InvalidWorkerCount(0));
        }
        let inventory = lumin_inventory::scan(root, request)?;
        let extraction = extract_facts(&inventory.sources, jobs)?;
        Ok(Self {
            inventory,
            facts: extraction.facts,
            sfc_states: extraction.sfc_states,
            js_parse_product_count: extraction.js_parse_product_count,
            scan_invocation,
        })
    }

    fn next_step(
        &self,
        resolution_profile: Option<ResolutionProfile>,
    ) -> Result<RepositoryAnalysisStep, EngineError> {
        let output = lumin_resolve::resolve_all(
            &self.inventory.sources,
            &self.facts,
            &self.inventory.config,
            resolution_profile,
        )?;
        if output.demands.is_empty() {
            Ok(RepositoryAnalysisStep::Finished(output))
        } else {
            let requested = output
                .demands
                .iter()
                .map(|demand| demand.path.display_escaped())
                .collect::<Vec<_>>();
            let mut demands = output
                .demands
                .into_iter()
                .filter(|demand| {
                    !self
                        .inventory
                        .config
                        .observations
                        .contains_key(&demand.path)
                })
                .collect::<Vec<_>>();
            demands.sort();
            demands.dedup();
            if demands.is_empty() {
                return Err(EngineError::ResolverDemandStalled(requested.join(", ")));
            }
            Ok(RepositoryAnalysisStep::NeedsInputs(demands))
        }
    }

    fn capture_demands(
        &mut self,
        root: &Path,
        demands: Vec<ConfigDemand>,
    ) -> Result<(), EngineError> {
        for demand in demands {
            let capture = lumin_inventory::capture_config(root, &demand.path, demand.syntax)?;
            if let Some(limitation) = capture.limitation {
                self.inventory.limitations.push(limitation);
            }
            self.inventory
                .config
                .observations
                .insert(demand.path, capture.observation);
        }
        Ok(())
    }

    fn finish(
        mut self,
        root: &Path,
        resolver: ResolverOutput,
    ) -> Result<RepositoryCapture, EngineError> {
        let ResolverOutput {
            resolved,
            package_surfaces,
            profiles,
            limitations: resolver_limitations,
            demands: _,
        } = resolver;
        let limitations = collect_limitations(
            &mut self.inventory.limitations,
            &self.facts,
            resolver_limitations,
        );

        let source_adjacency = source_adjacency(&self.inventory.sources, &resolved);
        let graph = lumin_graph::build(
            &self.inventory.sources,
            &self.facts,
            &resolved,
            &package_surfaces,
        );
        let findings = lumin_dead::analyze(
            &self.inventory.sources,
            &graph,
            &self.inventory.config,
            &limitations,
        );
        let state = if limitations.is_empty() {
            CapabilityState::Complete
        } else {
            CapabilityState::Incomplete
        };
        let mut capabilities = vec![CapabilityRecord {
            capability_id: DEAD_CODE_CAPABILITY_ID.to_owned(),
            state,
        }];
        capabilities.extend(sfc_capability_records(&self.sfc_states));
        let source_classifications = self
            .inventory
            .sources
            .iter()
            .map(|source| SourceClassificationRecord {
                source_id: source.id.clone(),
                path: RepoPathProjection::from(&source.path),
                classifications: source.roles.classifications.clone(),
            })
            .collect();
        let source_contexts = self
            .inventory
            .sources
            .iter()
            .map(|source| SourceContextRecord {
                source_id: source.id.clone(),
                path: RepoPathProjection::from(&source.path),
                kind: source.kind,
                package_root: self
                    .inventory
                    .config
                    .source_packages
                    .get(&source.id)
                    .map(RepoPathProjection::from),
            })
            .collect();
        let source_observations = self
            .inventory
            .sources
            .iter()
            .map(|source| SourceObservationRecord {
                source_id: source.id.clone(),
                physical_identity: source.physical_identity.clone(),
                payload_snapshot_id: source.payload_snapshot_id.clone(),
            })
            .collect::<Vec<_>>();
        let physical_source_count = source_observations
            .iter()
            .map(|observation| observation.physical_identity.clone())
            .collect::<BTreeSet<_>>()
            .len();
        let payload_snapshot_count = source_observations
            .iter()
            .map(|observation| observation.payload_snapshot_id.clone())
            .collect::<BTreeSet<_>>()
            .len();
        let metrics = AnalysisMetrics {
            logical_source_count: self.inventory.sources.len(),
            physical_source_count,
            payload_snapshot_count,
            js_parse_product_count: self.js_parse_product_count,
        };
        let evidence = RunEvidence {
            schema_version: "lumin-evidence.v1".to_owned(),
            capabilities,
            resolution_profiles: profiles,
            source_classifications,
            source_contexts,
            source_observations,
            resolutions: resolved,
            metrics,
            findings,
            limitations,
        };
        let source_paths = self
            .inventory
            .sources
            .iter()
            .map(|source| source.path.clone())
            .collect();

        // Build entry selection records from ALL inventory entries (available + unavailable)
        let entry_selections: Vec<EntrySelectionRecord> = self
            .inventory
            .entry_selections
            .iter()
            .map(|entry| EntrySelectionRecord {
                path: RepoPathProjection::from(&entry.path),
                source: entry.source,
                unavailable_reason: entry.unavailable_reason,
            })
            .collect();

        Ok(RepositoryCapture {
            snapshot: seal_analysis_snapshot(
                semantic_input_records(root, &self.inventory)?,
                evidence,
                self.scan_invocation,
                entry_selections,
            ),
            source_paths,
            source_adjacency,
        })
    }
}

fn semantic_input_records(
    root: &Path,
    inventory: &InventorySnapshot,
) -> Result<Vec<SemanticInputRecord>, EngineError> {
    let mut inputs = Vec::new();
    for source in &inventory.sources {
        inputs.push(SemanticInputRecord {
            path: RepoPathProjection::from(&source.path),
            state: SemanticInputState::Source,
            payload_sha256: Some(source.payload_sha256.clone()),
            physical_identity: Some(source.physical_identity.clone()),
        });
    }
    for observation in inventory.config.observations.values() {
        let (state, payload_sha256) = match observation {
            ConfigObservation::Present(document) => (
                SemanticInputState::ConfigPresent,
                Some(document.payload_sha256.clone()),
            ),
            ConfigObservation::Missing { .. } => (SemanticInputState::Missing, None),
            ConfigObservation::NonRegular { .. } => (SemanticInputState::NonRegular, None),
            ConfigObservation::Unreadable { detail, .. } => (
                SemanticInputState::Unreadable,
                Some(digest_hex(detail.as_bytes())),
            ),
        };
        let physical_identity = if state == SemanticInputState::Missing {
            None
        } else {
            Some(lumin_inventory::observe_physical_file_identity(
                root,
                observation.path(),
            )?)
        };
        inputs.push(SemanticInputRecord {
            path: RepoPathProjection::from(observation.path()),
            state,
            payload_sha256,
            physical_identity,
        });
    }
    // Convert policy inputs (lumin.json, .gitignore files) to semantic input records
    for policy_input in &inventory.policy_inputs {
        let state = match policy_input.state {
            SemanticPolicyState::Present => SemanticInputState::ConfigPresent,
            SemanticPolicyState::Missing => SemanticInputState::Missing,
        };
        inputs.push(SemanticInputRecord {
            path: RepoPathProjection::from(&policy_input.path),
            state,
            payload_sha256: policy_input.payload_sha256.clone(),
            physical_identity: policy_input.physical_identity.clone(),
        });
    }
    Ok(inputs)
}

fn source_adjacency(
    sources: &[SourceSnapshot],
    resolved: &[ResolvedSourceUse],
) -> BTreeMap<lumin_model::RepoPath, BTreeSet<lumin_model::RepoPath>> {
    let paths_by_id = sources
        .iter()
        .map(|source| (source.id.clone(), source.path.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut adjacency = sources
        .iter()
        .map(|source| (source.path.clone(), BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    for resolution in resolved {
        let ResolutionOutcome::Internal { target } = &resolution.outcome else {
            continue;
        };
        let Some(importer) = paths_by_id.get(&resolution.source_use.importer) else {
            continue;
        };
        let Some(target) = paths_by_id.get(target) else {
            continue;
        };
        adjacency
            .entry(importer.clone())
            .or_default()
            .insert(target.clone());
        adjacency
            .entry(target.clone())
            .or_default()
            .insert(importer.clone());
    }
    adjacency
}

fn sfc_capability_records(states: &BTreeMap<SfcDialect, CapabilityState>) -> Vec<CapabilityRecord> {
    lumin_sfc::compiled_dialect_states()
        .into_iter()
        .map(|(dialect, capability_id, initial_state)| CapabilityRecord {
            capability_id: capability_id.to_owned(),
            state: states.get(&dialect).copied().unwrap_or(initial_state),
        })
        .collect()
}

fn collect_limitations(
    inventory_limitations: &mut Vec<Limitation>,
    facts: &[FileFacts],
    resolver_limitations: Vec<Limitation>,
) -> Vec<Limitation> {
    let mut limitations = std::mem::take(inventory_limitations);
    limitations.extend(resolver_limitations);
    for file in facts {
        limitations.extend(file.limitations.iter().cloned());
    }
    limitations.sort_by_key(limitation_sort_key);
    limitations.dedup();
    limitations
}

pub fn load_run(
    root: &Path,
    run_id: &RunId,
) -> Result<(RunCatalogRecord, RunEvidence), EngineError> {
    open_repository_context(root)?
        .store
        .load_run(run_id)
        .map_err(Into::into)
}

pub fn load_latest_run(
    root: &Path,
) -> Result<Option<(RunCatalogRecord, RunEvidence)>, EngineError> {
    load_latest_overview(root).map(|overview| overview.completed)
}

pub fn load_latest_overview(root: &Path) -> Result<LatestOverview, EngineError> {
    let snapshot = open_repository_context(root)?.store.latest_snapshot()?;
    Ok(LatestOverview {
        latest_attempt: snapshot.latest_attempt.map(|attempt| LatestAttempt {
            attempt_id: attempt.attempt_id,
            sequence: attempt.sequence,
            status: attempt.state,
            failure: attempt.failure,
        }),
        completed: snapshot.completed,
    })
}

fn limitation_sort_key(limitation: &Limitation) -> String {
    format!("{limitation:?}")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use lumin_model::{FindingDisposition, LogicalSourceId, RepoPath, ResolutionProfileSource};

    use super::*;

    #[test]
    fn jobs_do_not_change_semantic_evidence() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("src"))?;
        fs::write(
            root.path().join("src/lib.ts"),
            "export const used = 1; export const dead = 2;",
        )?;
        fs::write(
            root.path().join("src/main.ts"),
            "import { used } from './lib.js'; console.log(used);",
        )?;
        let request = InventoryRequest::default();
        let one = analyze_repository(root.path(), &request, 1, None)?;
        let many = analyze_repository(root.path(), &request, 4, None)?;
        assert_eq!(one, many);
        assert_eq!(one.findings.len(), 1);
        Ok(())
    }

    #[test]
    fn randomized_worker_completion_order_preserves_reduced_facts()
    -> Result<(), Box<dyn std::error::Error>> {
        let facts = ["src/z.ts", "src/a.ts", "src/m.ts", "src/n.ts"]
            .into_iter()
            .map(|path| {
                let path = RepoPath::from_portable(path)?;
                Ok(FileFacts::physical(LogicalSourceId::from_path(&path)))
            })
            .collect::<Result<Vec<_>, lumin_model::RepoPathError>>()?;
        let expected = reduce_file_facts(facts.clone());

        for seed in 1_u64..=128 {
            let mut state = seed;
            let mut completion_order = facts.clone();
            for index in (1..completion_order.len()).rev() {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let width = u64::try_from(index + 1)?;
                let swap_index = usize::try_from(state % width)?;
                completion_order.swap(index, swap_index);
            }
            assert_eq!(reduce_file_facts(completion_order), expected, "seed={seed}");
        }
        Ok(())
    }

    #[test]
    fn analysis_only_does_not_initialize_lifecycle_state() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("src"))?;
        fs::write(root.path().join("src/main.ts"), "export const value = 1;")?;

        let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 1, None)?;

        assert_eq!(evidence.schema_version, "lumin-evidence.v1");
        assert!(!root.path().join(".lumin").exists());
        Ok(())
    }

    #[test]
    fn generated_and_vendor_findings_remain_canonical() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("src"))?;
        fs::write(
            root.path().join("lumin.json"),
            r#"{"schemaVersion":"lumin-config.v1","scan":{"roles":[{"pattern":"src/vendor.ts","role":"vendor"}]}}"#,
        )?;
        fs::write(
            root.path().join("src/authored.ts"),
            "export const authored = 1;",
        )?;
        fs::write(
            root.path().join("src/generated.ts"),
            "// @generated\nexport const generated = 1;",
        )?;
        fs::write(
            root.path().join("src/vendor.ts"),
            "export const vendor = 1;",
        )?;
        let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 2, None)?;
        assert_eq!(evidence.findings.len(), 3);
        assert_eq!(
            evidence
                .findings
                .iter()
                .filter(|finding| matches!(
                    finding.disposition,
                    FindingDisposition::ReviewOnly { .. }
                ))
                .count(),
            2
        );
        Ok(())
    }

    #[test]
    fn parse_failure_produces_incomplete_not_zero_complete()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::write(root.path().join("broken.ts"), "export const = ;")?;
        let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 1, None)?;
        assert_eq!(evidence.dead_code_state(), CapabilityState::Incomplete);
        assert!(!evidence.limitations.is_empty());
        assert!(evidence.findings.is_empty());
        Ok(())
    }

    #[test]
    fn unresolved_internal_use_blocks_only_its_candidate_scope()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::write(
            root.path().join("main.ts"),
            "import { missing } from './missing.js'; console.log(missing);",
        )?;
        fs::write(root.path().join("candidate.ts"), "export const dead = 1;")?;
        let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 1, None)?;
        assert_eq!(evidence.dead_code_state(), CapabilityState::Incomplete);
        assert_eq!(evidence.findings.len(), 1);
        assert_eq!(evidence.findings[0].exported_name, "dead");
        assert!(evidence.limitations.iter().any(|limitation| matches!(
            limitation,
            Limitation::InternalSpecifierUnresolved { .. }
        )));
        Ok(())
    }

    #[test]
    fn node16_esm_rejects_extensionless_relative_imports_without_fallback()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("src"))?;
        fs::write(
            root.path().join("package.json"),
            r#"{"name":"app","type":"module"}"#,
        )?;
        fs::write(
            root.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"moduleResolution":"node16","module":"node16"}}"#,
        )?;
        fs::write(
            root.path().join("src/lib.ts"),
            "export const used = 1; export const dead = 2;",
        )?;
        fs::write(
            root.path().join("src/main.ts"),
            "import { used } from './lib'; console.log(used);",
        )?;

        let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 1, None)?;

        assert_eq!(evidence.dead_code_state(), CapabilityState::Incomplete);
        assert!(evidence.findings.is_empty());
        assert!(
            evidence
                .resolution_profiles
                .iter()
                .all(|selected| selected.profile == ResolutionProfile::Node16)
        );
        assert!(evidence.limitations.iter().any(|limitation| matches!(
            limitation,
            Limitation::AliasShapeUnsupported { detail, .. }
                if detail.contains("requires an explicit relative extension")
        )));
        Ok(())
    }

    #[test]
    fn invocation_profile_replaces_only_the_configured_profile()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("src"))?;
        fs::write(
            root.path().join("package.json"),
            r#"{"name":"app","type":"module"}"#,
        )?;
        fs::write(
            root.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"moduleResolution":"node16"}}"#,
        )?;
        fs::write(
            root.path().join("src/lib.ts"),
            "export const used = 1; export const dead = 2;",
        )?;
        fs::write(
            root.path().join("src/main.ts"),
            "import { used } from './lib'; console.log(used);",
        )?;

        let evidence = analyze_repository(
            root.path(),
            &InventoryRequest::default(),
            1,
            Some(ResolutionProfile::Bundler),
        )?;

        assert_eq!(evidence.dead_code_state(), CapabilityState::Complete);
        assert_eq!(evidence.findings.len(), 1);
        assert_eq!(evidence.findings[0].exported_name, "dead");
        assert!(evidence.resolution_profiles.iter().all(|selected| {
            selected.profile == ResolutionProfile::Bundler
                && selected.source == ResolutionProfileSource::Invocation
        }));
        Ok(())
    }

    #[test]
    fn unknown_compiler_option_is_incomplete_instead_of_falling_back()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::write(root.path().join("package.json"), r#"{"name":"app"}"#)?;
        fs::write(
            root.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"madeUpFlag":true}}"#,
        )?;
        fs::write(root.path().join("lib.ts"), "export const dead = 1;")?;

        let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 1, None)?;

        assert_eq!(evidence.dead_code_state(), CapabilityState::Incomplete);
        assert!(evidence.findings.is_empty());
        assert!(evidence.limitations.iter().any(|limitation| matches!(
            limitation,
            Limitation::TsconfigSemanticsUnsupported { detail, .. }
                if detail.contains("unknown compiler option madeUpFlag")
        )));
        Ok(())
    }

    #[test]
    fn relative_extends_demands_exact_then_json_and_child_overrides_parent()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("src"))?;
        fs::write(
            root.path().join("package.json"),
            r#"{"name":"app","type":"module"}"#,
        )?;
        fs::write(
            root.path().join("base.json"),
            r#"{"compilerOptions":{"moduleResolution":"node16"}}"#,
        )?;
        fs::write(
            root.path().join("tsconfig.json"),
            r#"{"extends":"./base","compilerOptions":{"moduleResolution":"bundler"}}"#,
        )?;
        fs::write(
            root.path().join("src/lib.ts"),
            "export const used = 1; export const dead = 2;",
        )?;
        fs::write(
            root.path().join("src/main.ts"),
            "import { used } from './lib'; console.log(used);",
        )?;

        let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 1, None)?;

        assert_eq!(evidence.dead_code_state(), CapabilityState::Complete);
        assert_eq!(evidence.findings.len(), 1);
        assert_eq!(evidence.findings[0].exported_name, "dead");
        assert!(
            evidence
                .resolution_profiles
                .iter()
                .all(|selected| selected.profile == ResolutionProfile::Bundler)
        );
        assert!(evidence.resolution_profiles.iter().all(|selected| matches!(
            &selected.source,
            ResolutionProfileSource::Config { path_display, .. }
                if path_display == "tsconfig.json"
        )));
        Ok(())
    }

    #[test]
    fn paths_uses_base_url_regardless_of_json_field_order() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("src"))?;
        fs::write(
            root.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"paths":{"@lib":["src/lib"]},"baseUrl":"."}}"#,
        )?;
        fs::write(
            root.path().join("src/lib.ts"),
            "export const used = 1; export const dead = 2;",
        )?;
        fs::write(
            root.path().join("src/main.ts"),
            "import { used } from '@lib'; console.log(used);",
        )?;

        let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 1, None)?;

        assert_eq!(evidence.dead_code_state(), CapabilityState::Complete);
        assert_eq!(evidence.findings.len(), 1);
        assert_eq!(evidence.findings[0].exported_name, "dead");
        Ok(())
    }

    #[test]
    fn snapshot_id_varies_with_includes_excludes_and_profile_not_jobs()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("src"))?;
        fs::write(
            root.path().join("src/lib.ts"),
            "export const used = 1; export const dead = 2;",
        )?;
        fs::write(
            root.path().join("src/main.ts"),
            "import { used } from './lib.js'; console.log(used);",
        )?;

        let default_request = InventoryRequest::default();
        let include_request = InventoryRequest {
            includes: vec!["src/**".to_owned()],
            ..Default::default()
        };
        let exclude_request = InventoryRequest {
            excludes: vec!["dist/**".to_owned()],
            ..Default::default()
        };

        let snap_default = capture_repository(root.path(), &default_request, 1, None)?;
        let snap_default_4jobs = capture_repository(root.path(), &default_request, 4, None)?;
        let snap_include = capture_repository(root.path(), &include_request, 1, None)?;
        let snap_exclude = capture_repository(root.path(), &exclude_request, 1, None)?;
        let snap_profile = capture_repository(
            root.path(),
            &default_request,
            1,
            Some(ResolutionProfile::Bundler),
        )?;

        // Same request with different jobs must produce same snapshot ID
        assert_eq!(
            snap_default.snapshot.analysis_input_id, snap_default_4jobs.snapshot.analysis_input_id,
            "jobs must not affect the analysis_input_id"
        );
        // Different includes/excludes/profile must produce different snapshot IDs
        assert_ne!(
            snap_default.snapshot.analysis_input_id, snap_include.snapshot.analysis_input_id,
            "includes must vary the analysis_input_id"
        );
        assert_ne!(
            snap_default.snapshot.analysis_input_id, snap_exclude.snapshot.analysis_input_id,
            "excludes must vary the analysis_input_id"
        );
        assert_ne!(
            snap_default.snapshot.analysis_input_id, snap_profile.snapshot.analysis_input_id,
            "resolution profile must vary the analysis_input_id"
        );
        Ok(())
    }

    #[test]
    fn lumin_json_missing_appears_in_semantic_inputs() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::write(root.path().join("lib.ts"), "export const a = 1;")?;

        let capture = capture_repository(root.path(), &InventoryRequest::default(), 1, None)?;

        let lumin_input = capture
            .snapshot
            .inputs
            .iter()
            .find(|input| input.path.display == "lumin.json")
            .ok_or_else(|| std::io::Error::other("missing lumin.json semantic input"))?;
        assert_eq!(lumin_input.state, SemanticInputState::Missing);
        assert!(lumin_input.payload_sha256.is_none());
        Ok(())
    }

    #[test]
    fn lumin_json_present_appears_in_semantic_inputs() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::write(
            root.path().join("lumin.json"),
            r#"{"schemaVersion":"lumin-config.v1"}"#,
        )?;
        fs::write(root.path().join("lib.ts"), "export const a = 1;")?;

        let capture = capture_repository(root.path(), &InventoryRequest::default(), 1, None)?;

        let lumin_input = capture
            .snapshot
            .inputs
            .iter()
            .find(|input| input.path.display == "lumin.json")
            .ok_or_else(|| std::io::Error::other("missing lumin.json semantic input"))?;
        assert_eq!(lumin_input.state, SemanticInputState::ConfigPresent);
        assert!(lumin_input.payload_sha256.is_some());
        Ok(())
    }

    #[test]
    fn gitignore_appears_in_semantic_inputs() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::write(root.path().join(".gitignore"), "dist/\n")?;
        fs::write(root.path().join("lib.ts"), "export const a = 1;")?;

        let capture = capture_repository(root.path(), &InventoryRequest::default(), 1, None)?;

        let gitignore_input = capture
            .snapshot
            .inputs
            .iter()
            .find(|input| input.path.display == ".gitignore")
            .ok_or_else(|| std::io::Error::other("missing .gitignore semantic input"))?;
        assert_eq!(gitignore_input.state, SemanticInputState::ConfigPresent);
        assert!(gitignore_input.payload_sha256.is_some());
        assert!(gitignore_input.physical_identity.is_some());
        Ok(())
    }

    #[test]
    fn configured_entry_availability_appears_in_snapshot() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("src"))?;
        fs::write(
            root.path().join("lumin.json"),
            r#"{"schemaVersion":"lumin-config.v1","entries":["src/present.ts","src/missing.ts"]}"#,
        )?;
        fs::write(root.path().join("src/present.ts"), "export const a = 1;")?;
        // src/missing.ts intentionally not created

        let capture = capture_repository(root.path(), &InventoryRequest::default(), 1, None)?;

        // Both entries appear in entry_selections
        assert_eq!(capture.snapshot.entry_selections.len(), 2);

        let present = capture
            .snapshot
            .entry_selections
            .iter()
            .find(|entry| entry.path.display == "src/present.ts")
            .ok_or_else(|| std::io::Error::other("missing present entry selection"))?;
        assert_eq!(present.source, lumin_model::EntrySource::Configuration);
        assert!(present.unavailable_reason.is_none());

        let missing = capture
            .snapshot
            .entry_selections
            .iter()
            .find(|entry| entry.path.display == "src/missing.ts")
            .ok_or_else(|| std::io::Error::other("missing unavailable entry selection"))?;
        assert_eq!(missing.source, lumin_model::EntrySource::Configuration);
        assert_eq!(
            missing.unavailable_reason,
            Some(lumin_model::EntryUnavailableReason::Missing)
        );
        Ok(())
    }
}
