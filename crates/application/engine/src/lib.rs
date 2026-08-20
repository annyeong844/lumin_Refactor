mod audit_publication;
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
    CacheCleanupDeliveryStatus, CacheCleanupResult, GateDecision, GateOperationResult,
    RecordLookup, RetentionMutationResult, RetentionPlanScope,
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
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use lumin_evidence::{
    AnalysisMetrics, AnalysisSnapshot, CapabilityRecord, DEAD_CODE_CAPABILITY_ID,
    DEPENDENCY_OWNERSHIP_CAPABILITY_ID, DependencyOwnerRecord, EntrySelectionRecord,
    PathPrefixIdentity, RepoPathProjection, RunEvidence, ScanInvocationTier, SemanticInputRecord,
    SemanticInputState, SourceClassificationRecord, SourceContextRecord, SourceObservationRecord,
    seal_analysis_snapshot,
};
use lumin_inventory::{
    InventoryError, InventoryRequest, InventorySnapshot, RepositoryAdmission, SemanticPolicyState,
    repository_admission,
};
use lumin_model::{
    AttemptId, AttemptStatus, CapabilityState, ConfigObservation, FileFacts, Limitation,
    OperationId, RepositoryRootIdentity, ResolutionOutcome, ResolutionProfile, ResolvedSourceUse,
    RoleOverride, RunId, SfcDialect, SourceSnapshot, append_length_prefixed, digest_hex,
};
use lumin_resolve::{ConfigDemand, ResolverError, ResolverOutput};
use lumin_store::{PublishedRun, RepositoryStore, RunCatalogRecord, StoreError};
use thiserror::Error;

#[cfg(test)]
use extraction::reduce_file_facts;
use extraction::{ExtractionOutput, extract_facts};

pub fn lower_native_repo_path(
    value: &OsStr,
) -> Result<lumin_model::RepoPath, lumin_model::RepoPathError> {
    lumin_inventory::lower_native_repo_path(value)
}

pub fn decode_native_repo_path_stream(
    bytes: &[u8],
) -> Result<Vec<lumin_model::RepoPath>, lumin_model::RepoPathError> {
    lumin_inventory::decode_native_repo_path_stream(bytes)
}

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
    pub repository_root: RepositoryRootIdentity,
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
    #[error("analysis extraction is unavailable after resolution completed")]
    ExtractionUnavailable,
    #[error("JS extraction omitted the requested module-format product: {0}")]
    ExtractionProductMissing(String),
    #[error("resolution discovered profile-sensitive inputs after extraction: {0}")]
    LateExtractionProfileDemand(String),
    #[error("JS parse-product count overflowed during extraction")]
    ExtractionMetricOverflow,
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
    lumin_inventory::validate_caller_paths_lexically(&request.entries)?;
    let admission = repository_admission(&request.root)?;
    lumin_inventory::validate_caller_entries(&admission.canonical_root, &request.entries)?;
    let store = RepositoryStore::open(&admission.canonical_root, &admission.binding)?;
    let context = repository_context_from_admission(admission, store);
    let store = &context.store;
    let reserved_state_lookup = reserved_state_identity_lookup(store);
    lumin_inventory::validate_caller_entry_identity_lookup(
        &context.root,
        &request.entries,
        &reserved_state_lookup,
    )?;
    let mut attempt = store.begin_attempt()?;
    let inventory_request = InventoryRequest {
        includes: request.includes.clone(),
        excludes: request.excludes.clone(),
        role_overrides: request.role_overrides.clone(),
        entries: request.entries.clone(),
        dependency_intents: Vec::new(),
    };
    let capture = match capture_admitted_repository(
        &context.root,
        context.repository_root.clone(),
        &inventory_request,
        request.jobs,
        request.resolution_profile,
        &reserved_state_lookup,
    ) {
        Ok(capture) => capture,
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
    let published = match audit_publication::publish(
        store,
        &mut attempt,
        &context.root,
        &reserved_state_lookup,
        &capture.snapshot,
    ) {
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
    let evidence = capture.snapshot.evidence;
    Ok(AuditResult {
        published,
        repository_root: context.repository_root.clone(),
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
    let reserved_state_lookup = lumin_inventory::ReservedStateIdentityLookup::empty();
    capture_admitted_repository(
        &admission.canonical_root,
        admission.binding.root().clone(),
        request,
        jobs,
        resolution_profile,
        &reserved_state_lookup,
    )
    .map(|capture| capture.snapshot.evidence)
}

struct RepositoryContext {
    root: PathBuf,
    repository_id: lumin_model::RepositoryId,
    repository_root: RepositoryRootIdentity,
    store: RepositoryStore,
}

fn open_repository_context(root: &Path) -> Result<RepositoryContext, EngineError> {
    let admission = repository_admission(root)?;
    let store = RepositoryStore::open(&admission.canonical_root, &admission.binding)?;
    Ok(repository_context_from_admission(admission, store))
}

fn repository_context_from_admission(
    admission: RepositoryAdmission,
    store: RepositoryStore,
) -> RepositoryContext {
    RepositoryContext {
        root: admission.canonical_root,
        repository_id: admission.binding.repository_id().clone(),
        repository_root: admission.binding.root().clone(),
        store,
    }
}

fn reserved_state_identity_lookup(
    store: &RepositoryStore,
) -> lumin_inventory::ReservedStateIdentityLookup {
    let store = store.clone();
    lumin_inventory::ReservedStateIdentityLookup::new(move |identity| {
        store
            .owns_reserved_state_identity(identity)
            .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))
    })
}

struct RepositoryCapture {
    snapshot: AnalysisSnapshot,
    source_paths: Vec<lumin_model::RepoPath>,
    source_adjacency: BTreeMap<lumin_model::RepoPath, BTreeSet<lumin_model::RepoPath>>,
    inferred_write_paths: Vec<lumin_model::RepoPath>,
}

struct RepositoryAnalysisSession {
    repository_root: RepositoryRootIdentity,
    inventory: InventorySnapshot,
    reserved_state_lookup: lumin_inventory::ReservedStateIdentityLookup,
    extraction: Option<ExtractionOutput>,
    js_parse_product_count: usize,
    jobs: usize,
    scan_invocation: ScanInvocationTier,
}

enum RepositoryAnalysisStep {
    NeedsInputs(Vec<ConfigDemand>),
    Finished(ResolverOutput),
}

#[cfg(test)]
fn capture_repository(
    root: &Path,
    request: &InventoryRequest,
    jobs: usize,
    resolution_profile: Option<ResolutionProfile>,
) -> Result<RepositoryCapture, EngineError> {
    let admission = repository_admission(root)?;
    let reserved_state_lookup = lumin_inventory::ReservedStateIdentityLookup::empty();
    capture_admitted_repository(
        &admission.canonical_root,
        admission.binding.root().clone(),
        request,
        jobs,
        resolution_profile,
        &reserved_state_lookup,
    )
}

fn capture_admitted_repository(
    root: &Path,
    repository_root: RepositoryRootIdentity,
    request: &InventoryRequest,
    jobs: usize,
    resolution_profile: Option<ResolutionProfile>,
    reserved_state_lookup: &lumin_inventory::ReservedStateIdentityLookup,
) -> Result<RepositoryCapture, EngineError> {
    let tier = build_scan_invocation_tier(request, resolution_profile);
    let mut session = RepositoryAnalysisSession::start(
        root,
        repository_root,
        request,
        jobs,
        tier,
        reserved_state_lookup,
    )?;
    loop {
        match session.next_step(resolution_profile)? {
            RepositoryAnalysisStep::NeedsInputs(demands) => {
                session.capture_demands(root, demands)?;
            }
            RepositoryAnalysisStep::Finished(resolver) => {
                return session.finish(resolver);
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
        dependency_intents: Vec::new(),
        resolution_profile,
    }
}

impl RepositoryAnalysisSession {
    fn start(
        root: &Path,
        repository_root: RepositoryRootIdentity,
        request: &InventoryRequest,
        jobs: usize,
        scan_invocation: ScanInvocationTier,
        reserved_state_lookup: &lumin_inventory::ReservedStateIdentityLookup,
    ) -> Result<Self, EngineError> {
        let inventory = lumin_inventory::begin_scan_with_reserved_state_lookup(
            root,
            request,
            reserved_state_lookup,
        )?
        .finish(root)?;
        Self::start_with_inventory(
            repository_root,
            inventory,
            jobs,
            scan_invocation,
            reserved_state_lookup.clone(),
        )
    }

    fn start_with_inventory(
        repository_root: RepositoryRootIdentity,
        inventory: InventorySnapshot,
        jobs: usize,
        scan_invocation: ScanInvocationTier,
        reserved_state_lookup: lumin_inventory::ReservedStateIdentityLookup,
    ) -> Result<Self, EngineError> {
        if jobs == 0 {
            return Err(EngineError::InvalidWorkerCount(0));
        }
        Ok(Self {
            repository_root,
            inventory,
            reserved_state_lookup,
            extraction: None,
            js_parse_product_count: 0,
            jobs,
            scan_invocation,
        })
    }

    fn next_step(
        &mut self,
        resolution_profile: Option<ResolutionProfile>,
    ) -> Result<RepositoryAnalysisStep, EngineError> {
        let profile_selection = lumin_resolve::select_resolution_profiles(
            &self.inventory.sources,
            &self.inventory.config,
            &self.repository_root,
            resolution_profile,
        )?;
        if !profile_selection.demands.is_empty() {
            if self.extraction.is_some() {
                let paths = profile_selection
                    .demands
                    .iter()
                    .map(|demand| demand.path.display_escaped())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(EngineError::LateExtractionProfileDemand(paths));
            }
            return self
                .pending_demands(profile_selection.demands)
                .map(RepositoryAnalysisStep::NeedsInputs);
        }
        if self.extraction.is_none() {
            let extraction = extract_facts(
                &self.inventory.sources,
                &self.inventory.config,
                &profile_selection.profiles,
                self.jobs,
            )?;
            self.js_parse_product_count = self
                .js_parse_product_count
                .checked_add(extraction.js_parse_product_count)
                .ok_or(EngineError::ExtractionMetricOverflow)?;
            self.extraction = Some(extraction);
        }
        let extraction = self
            .extraction
            .as_ref()
            .ok_or(EngineError::ExtractionUnavailable)?;
        let output = lumin_resolve::resolve_all(
            &self.inventory.sources,
            &self.inventory.physical_path_redirects,
            &extraction.facts,
            &extraction.inventory_bound_uses,
            &self.inventory.config,
            &self.repository_root,
            resolution_profile,
        )?;
        if output.demands.is_empty() {
            Ok(RepositoryAnalysisStep::Finished(output))
        } else {
            self.pending_demands(output.demands)
                .map(RepositoryAnalysisStep::NeedsInputs)
        }
    }

    fn pending_demands(
        &self,
        demands: Vec<ConfigDemand>,
    ) -> Result<Vec<ConfigDemand>, EngineError> {
        let requested = demands
            .iter()
            .map(|demand| demand.path.display_escaped())
            .collect::<Vec<_>>();
        let mut pending = demands
            .into_iter()
            .filter(|demand| {
                !self
                    .inventory
                    .config
                    .observations
                    .contains_key(&demand.path)
            })
            .collect::<Vec<_>>();
        pending.sort();
        pending.dedup();
        if pending.is_empty() {
            return Err(EngineError::ResolverDemandStalled(requested.join(", ")));
        }
        Ok(pending)
    }

    fn capture_demands(
        &mut self,
        root: &Path,
        demands: Vec<ConfigDemand>,
    ) -> Result<(), EngineError> {
        capture_config_demands(
            root,
            &mut self.inventory,
            demands,
            &self.reserved_state_lookup,
        )?;
        Ok(())
    }

    fn finish(mut self, resolver: ResolverOutput) -> Result<RepositoryCapture, EngineError> {
        let extraction = self
            .extraction
            .take()
            .ok_or(EngineError::ExtractionUnavailable)?;
        let ResolverOutput {
            resolved,
            package_surfaces,
            profiles,
            limitations: mut resolver_limitations,
            demands: _,
        } = resolver;
        resolver_limitations.extend(lumin_js::scope_commonjs_computed_limitations(&resolved));
        let limitations = collect_limitations(
            &mut self.inventory.limitations,
            &extraction.facts,
            resolver_limitations,
        );

        let source_adjacency = source_adjacency(&self.inventory.sources, &resolved, &limitations);
        let graph = lumin_graph::build(
            &self.inventory.sources,
            &extraction.facts,
            &resolved,
            &package_surfaces,
            &limitations,
        );
        let findings = lumin_dead::analyze(
            &self.inventory.sources,
            &graph,
            &self.inventory.config,
            &limitations,
        );
        let state = dead_code_state(&limitations);
        let mut capabilities = vec![
            CapabilityRecord {
                capability_id: DEAD_CODE_CAPABILITY_ID.to_owned(),
                state,
            },
            CapabilityRecord {
                capability_id: DEPENDENCY_OWNERSHIP_CAPABILITY_ID.to_owned(),
                state: dependency_ownership_state(&limitations),
            },
        ];
        capabilities.extend(sfc_capability_records(&extraction.sfc_states));
        let source_classifications = self
            .inventory
            .sources
            .iter()
            .map(|source| SourceClassificationRecord {
                source_id: source.id.clone(),
                path: RepoPathProjection::from(&source.path),
                classifications: source.roles.classifications().to_vec(),
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
        let dependency_owners = self
            .inventory
            .config
            .dependency_owners
            .iter()
            .map(|owner| DependencyOwnerRecord {
                consumer: lumin_model::LogicalSourceId::from_path(&owner.intent.path),
                consumer_path: RepoPathProjection::from(&owner.intent.path),
                dependency: owner.intent.dependency.clone(),
                package_root: RepoPathProjection::from(&owner.package_root),
                manifest_path: RepoPathProjection::from(&owner.manifest_path),
                manifest_payload_sha256: owner.manifest_payload_sha256.clone(),
                lockfile_path: owner.lockfile_path.as_ref().map(RepoPathProjection::from),
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
            dependency_owners,
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
        let mut inferred_write_paths = self
            .inventory
            .config
            .dependency_owners
            .iter()
            .flat_map(|owner| {
                std::iter::once(owner.manifest_path.clone()).chain(owner.lockfile_path.clone())
            })
            .collect::<Vec<_>>();
        inferred_write_paths.sort();
        inferred_write_paths.dedup();

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
                semantic_input_records(&self.inventory),
                evidence,
                self.scan_invocation,
                entry_selections,
            ),
            source_paths,
            source_adjacency,
            inferred_write_paths,
        })
    }
}

fn capture_config_demands(
    root: &Path,
    inventory: &mut InventorySnapshot,
    demands: Vec<ConfigDemand>,
    reserved_state_lookup: &lumin_inventory::ReservedStateIdentityLookup,
) -> Result<(), EngineError> {
    let requested = demands
        .iter()
        .map(|demand| demand.path.display_escaped())
        .collect::<Vec<_>>();
    let mut uncaptured = demands
        .into_iter()
        .filter(|demand| !inventory.config.observations.contains_key(&demand.path))
        .collect::<Vec<_>>();
    uncaptured.sort();
    uncaptured.dedup();
    if uncaptured.is_empty() {
        return Err(EngineError::ResolverDemandStalled(requested.join(", ")));
    }
    for demand in uncaptured {
        let capture = lumin_inventory::capture_config_with_reserved_state_lookup(
            root,
            &demand.path,
            demand.syntax,
            reserved_state_lookup,
        )?;
        if let Some(limitation) = capture.limitation {
            inventory.limitations.push(limitation);
        }
        inventory
            .config
            .observations
            .insert(demand.path, capture.observation);
    }
    Ok(())
}

fn semantic_input_records(inventory: &InventorySnapshot) -> Vec<SemanticInputRecord> {
    let mut inputs = Vec::new();
    for source in &inventory.sources {
        inputs.push(SemanticInputRecord {
            path: RepoPathProjection::from(&source.path),
            state: SemanticInputState::Source,
            payload_sha256: Some(source.payload_sha256.clone()),
            physical_identity: Some(source.physical_identity.clone()),
            absence_parent: None,
            physical_redirect_sha256: None,
        });
    }
    for observation in inventory.config.observations.values() {
        let (state, payload_sha256) = match observation {
            ConfigObservation::Present { document, .. } => (
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
        inputs.push(SemanticInputRecord {
            path: RepoPathProjection::from(observation.path()),
            state,
            payload_sha256,
            physical_identity: observation.physical_identity().cloned(),
            absence_parent: observation
                .absence_parent()
                .map(|parent| PathPrefixIdentity {
                    path: RepoPathProjection::from(&parent.path),
                    physical_identity: parent.physical_identity.clone(),
                }),
            physical_redirect_sha256: None,
        });
    }
    // Convert policy inputs (lumin.json, .gitignore files) to semantic input records
    for policy_input in &inventory.policy_inputs {
        let (state, payload_sha256) = match policy_input.state {
            SemanticPolicyState::Present => (
                SemanticInputState::ConfigPresent,
                policy_input.payload_sha256.clone(),
            ),
            SemanticPolicyState::Missing => (SemanticInputState::Missing, None),
            SemanticPolicyState::NonRegular => (SemanticInputState::NonRegular, None),
            SemanticPolicyState::Unreadable => (
                SemanticInputState::Unreadable,
                policy_input
                    .detail
                    .as_ref()
                    .map(|detail| digest_hex(detail.as_bytes())),
            ),
        };
        inputs.push(SemanticInputRecord {
            path: RepoPathProjection::from(&policy_input.path),
            state,
            payload_sha256,
            physical_identity: policy_input.physical_identity.clone(),
            absence_parent: policy_input
                .absence_parent
                .as_ref()
                .map(|parent| PathPrefixIdentity {
                    path: RepoPathProjection::from(&parent.path),
                    physical_identity: parent.physical_identity.clone(),
                }),
            physical_redirect_sha256: None,
        });
    }
    for redirect in &inventory.physical_path_redirects {
        let path = RepoPathProjection::from(&redirect.path);
        let sha256 = redirect.semantic_sha256();
        if let Some(input) = inputs.iter_mut().find(|input| input.path == path) {
            input.physical_redirect_sha256 = Some(sha256);
            if input.physical_identity.is_none() {
                input.physical_identity = redirect.target_physical_identity.clone();
            }
        } else {
            inputs.push(SemanticInputRecord {
                path,
                state: SemanticInputState::PathRedirect,
                payload_sha256: None,
                physical_identity: redirect.target_physical_identity.clone(),
                absence_parent: None,
                physical_redirect_sha256: Some(sha256),
            });
        }
    }
    inputs
}

fn source_adjacency(
    sources: &[SourceSnapshot],
    resolved: &[ResolvedSourceUse],
    limitations: &[Limitation],
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
    for limitation in limitations {
        let (source_id, candidates) = match limitation {
            Limitation::DynamicImportNonLiteral {
                source_id,
                candidates,
                target_scope: lumin_model::DynamicImportTargetScope::ExplicitTargets,
                ..
            }
            | Limitation::ImportMetaGlobUnsupported {
                source_id,
                candidates,
                target_scope: lumin_model::ImportMetaGlobTargetScope::ExplicitTargets,
                ..
            } => (source_id, candidates),
            _ => continue,
        };
        let Some(importer) = paths_by_id.get(source_id) else {
            continue;
        };
        for candidate in candidates {
            let Some(target) = paths_by_id.get(candidate) else {
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

// The architecture check must inspect Limitation variants outside macro token streams.
#[allow(clippy::match_like_matches_macro)]
fn dependency_ownership_state(limitations: &[Limitation]) -> CapabilityState {
    if limitations.iter().any(|limitation| match limitation {
        Limitation::PackageMetadataUnobservable { .. }
        | Limitation::PackageIdentityUnsupported { .. }
        | Limitation::DependencyOwnerAmbiguous { .. }
        | Limitation::WorkspaceOwnershipUnsupported { .. }
        | Limitation::PnpmDependencySemanticsUnsupported { .. } => true,
        _ => false,
    }) {
        CapabilityState::Incomplete
    } else {
        CapabilityState::Complete
    }
}

// The architecture check must inspect Limitation variants outside macro token streams.
#[allow(clippy::match_like_matches_macro)]
fn dead_code_state(limitations: &[Limitation]) -> CapabilityState {
    if limitations.iter().any(|limitation| match limitation {
        Limitation::DependencyOwnerAmbiguous { .. }
        | Limitation::PnpmDependencySemanticsUnsupported { .. } => false,
        _ => true,
    }) {
        CapabilityState::Incomplete
    } else {
        CapabilityState::Complete
    }
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
    limitations.sort_by(Limitation::canonical_cmp);
    limitations.dedup();
    limitations
}

#[derive(Clone, Debug)]
pub struct CleanCacheRequest {
    pub root: PathBuf,
    pub operation_id: OperationId,
}

pub fn clean_cache(request: &CleanCacheRequest) -> Result<CacheCleanupResult, EngineError> {
    let context = open_repository_context(&request.root)?;
    let request_digest = cache_cleanup_request_digest(&context.repository_id);
    context
        .store
        .clean_cache_payloads(&request.operation_id, &request_digest)
        .map_err(Into::into)
}

pub fn record_cache_cleanup_delivery(
    root: &Path,
    operation_id: &OperationId,
    request_digest: &str,
    delivery: CacheCleanupDeliveryStatus,
) -> Result<(), EngineError> {
    open_repository_context(root)?
        .store
        .record_cache_cleanup_delivery(operation_id, request_digest, delivery)
        .map_err(Into::into)
}

fn cache_cleanup_request_digest(repository_id: &lumin_model::RepositoryId) -> String {
    let mut framed = Vec::new();
    append_length_prefixed(&mut framed, b"lumin-cache-clean-request.v2");
    append_length_prefixed(&mut framed, repository_id.as_str().as_bytes());
    append_length_prefixed(&mut framed, b"cache-clean");
    append_length_prefixed(&mut framed, b"lumin.cache-cleanup.v2");
    digest_hex(&framed)
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
