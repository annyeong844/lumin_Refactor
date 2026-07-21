mod gate_abandon;
mod retention;
mod write_gate;

pub use gate_abandon::{AbandonGateRequest, abandon_gate};
pub use lumin_evidence::{
    GateDecision, GateOperationResult, RecordLookup, RetentionMutationResult, RetentionPlanScope,
};
pub use lumin_store::RunCatalogCursor;
pub use retention::{
    ConfirmRetentionPlanRequest, PinRunRequest, PrepareRetentionPlanRequest, UnpinRunRequest,
    confirm_retention_plan, list_runs, load_lifecycle_operation, load_retention_plan, lookup_gate,
    lookup_run, pin_run, prepare_retention_plan, unpin_run,
};
pub use write_gate::{
    PostWriteRequest, PreWriteRequest, close_write_gate, load_gate, load_operation, open_write_gate,
};

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use lumin_evidence::{
    AnalysisSnapshot, CapabilityRecord, DEAD_CODE_CAPABILITY_ID, RepoPathProjection, RunEvidence,
    SemanticInputRecord, SemanticInputState, seal_analysis_snapshot,
};
use lumin_inventory::{InventoryError, InventoryRequest, InventorySnapshot, repository_admission};
use lumin_model::{
    CapabilityState, ConfigObservation, FileFacts, Limitation, ResolutionOutcome,
    ResolutionProfile, ResolvedSourceUse, RoleOverride, RunId, SfcDialect, SourceSnapshot,
    digest_hex,
};
use lumin_resolve::{ConfigDemand, ResolverError, ResolverOutput};
use lumin_store::{PublishedRun, RepositoryStore, RunCatalogRecord, StoreError};
use rayon::prelude::*;
use thiserror::Error;

#[derive(Clone, Debug)]
pub struct AuditRequest {
    pub root: PathBuf,
    pub includes: Vec<String>,
    pub excludes: Vec<String>,
    pub role_overrides: Vec<RoleOverride>,
    pub jobs: usize,
    pub resolution_profile: Option<ResolutionProfile>,
}

#[derive(Clone, Debug)]
pub struct AuditResult {
    pub published: PublishedRun,
    pub evidence: RunEvidence,
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Inventory(#[from] InventoryError),
    #[error(transparent)]
    Resolver(#[from] ResolverError),
    #[error(transparent)]
    Store(#[from] StoreError),
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
            Self::NoDeclaredPaths
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
                | StoreError::RunCatalogAnchorMissing(_),
            ) => 2,
            Self::Store(StoreError::GateRevisionBusy(_) | StoreError::OperationBusy(_)) => 4,
            Self::Store(StoreError::GateRevisionChanged(_)) => 5,
            Self::Store(StoreError::RunCatalogRevisionChanged { .. }) => 5,
            _ => 1,
        }
    }
}

pub fn audit(request: &AuditRequest) -> Result<AuditResult, EngineError> {
    if request.jobs == 0 {
        return Err(EngineError::InvalidWorkerCount(0));
    }
    let context = open_repository_context(&request.root)?;
    let store = &context.store;
    let attempt = store.begin_attempt()?;
    let inventory_request = InventoryRequest {
        includes: request.includes.clone(),
        excludes: request.excludes.clone(),
        role_overrides: request.role_overrides.clone(),
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
            if let Err(persistence) = store.fail_attempt(&attempt, &error.to_string()) {
                return Err(EngineError::AnalysisAndPersistence {
                    analysis: error.to_string(),
                    persistence: persistence.to_string(),
                });
            }
            return Err(error);
        }
    };
    let published = match store.publish_run(&attempt, &evidence) {
        Ok(published) => published,
        Err(error) => {
            if let Err(persistence) = store.fail_attempt(&attempt, &error.to_string()) {
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
    store: RepositoryStore,
}

fn open_repository_context(root: &Path) -> Result<RepositoryContext, EngineError> {
    let admission = repository_admission(root)?;
    let store = RepositoryStore::open(&admission.canonical_root, &admission.binding)?;
    Ok(RepositoryContext {
        root: admission.canonical_root,
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
    let mut session = RepositoryAnalysisSession::start(root, request, jobs)?;
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

impl RepositoryAnalysisSession {
    fn start(root: &Path, request: &InventoryRequest, jobs: usize) -> Result<Self, EngineError> {
        if jobs == 0 {
            return Err(EngineError::InvalidWorkerCount(0));
        }
        let inventory = lumin_inventory::scan(root, request)?;
        let extraction = extract_facts(&inventory.sources, jobs)?;
        Ok(Self {
            inventory,
            facts: extraction.facts,
            sfc_states: extraction.sfc_states,
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
            let observation = lumin_inventory::observe_config(root, &demand.path, demand.syntax)?;
            self.inventory
                .config
                .observations
                .insert(demand.path, observation);
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
            &resolved,
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
        let evidence = RunEvidence {
            schema_version: "lumin-evidence.v1".to_owned(),
            capabilities,
            resolution_profiles: profiles,
            findings,
            limitations,
        };
        let source_paths = self
            .inventory
            .sources
            .iter()
            .map(|source| source.path.clone())
            .collect();
        Ok(RepositoryCapture {
            snapshot: seal_analysis_snapshot(
                semantic_input_records(root, &self.inventory)?,
                evidence,
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
            physical_identity: Some(lumin_inventory::physical_file_identity(
                &root.join(source.path.to_native_relative()),
            )?),
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
            Some(lumin_inventory::physical_file_identity(
                &root.join(observation.path().to_native_relative()),
            )?)
        };
        inputs.push(SemanticInputRecord {
            path: RepoPathProjection::from(observation.path()),
            state,
            payload_sha256,
            physical_identity,
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

struct ExtractionOutput {
    facts: Vec<FileFacts>,
    sfc_states: BTreeMap<SfcDialect, CapabilityState>,
}

fn extract_facts(sources: &[SourceSnapshot], jobs: usize) -> Result<ExtractionOutput, EngineError> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .thread_name(|index| format!("lumin-worker-{index}"))
        .build()
        .map_err(|error| EngineError::Scheduler(error.to_string()))?;
    let source_index = lumin_sfc::source_index(sources);
    pool.install(|| {
        let mut physical_facts = sources
            .par_iter()
            .filter(|source| source.kind.is_js_family())
            .map(lumin_js::extract)
            .collect::<Result<Vec<_>, _>>()?;
        physical_facts.sort_by(|left, right| left.source_id.cmp(&right.source_id));

        let mut decompositions = sources
            .par_iter()
            .filter(|source| !source.kind.is_js_family())
            .map(|source| lumin_sfc::decompose(source, &source_index))
            .collect::<Result<Vec<_>, _>>()?;
        decompositions.sort_by(|left, right| left.source_id.cmp(&right.source_id));

        let mut embedded_by_parent = BTreeMap::<_, Vec<FileFacts>>::new();
        let mut embedded = decompositions
            .par_iter()
            .flat_map_iter(|decomposition| {
                decomposition
                    .inline_scripts
                    .iter()
                    .map(move |unit| (&decomposition.source_id, unit))
            })
            .map(|(parent, unit)| {
                lumin_js::extract_embedded(unit).map(|facts| (parent.clone(), facts))
            })
            .collect::<Result<Vec<_>, _>>()?;
        embedded.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.source_unit.cmp(&right.1.source_unit))
        });
        for (parent, facts) in embedded {
            embedded_by_parent.entry(parent).or_default().push(facts);
        }

        let mut sfc_states = BTreeMap::from([
            (SfcDialect::Vue, CapabilityState::Complete),
            (SfcDialect::Svelte, CapabilityState::Unavailable),
            (SfcDialect::Astro, CapabilityState::Unavailable),
        ]);
        let mut sfc_facts = Vec::new();
        for decomposition in decompositions {
            let parent = decomposition.source_id.clone();
            let analysis = lumin_sfc::finalize(
                decomposition,
                embedded_by_parent.remove(&parent).unwrap_or_default(),
                &physical_facts,
            )?;
            sfc_states
                .entry(analysis.dialect)
                .and_modify(|state| *state = less_complete(*state, analysis.state))
                .or_insert(analysis.state);
            sfc_facts.extend(analysis.file_facts);
        }

        let mut facts = physical_facts;
        facts.extend(sfc_facts);
        facts.sort_by(|left, right| {
            left.source_id
                .cmp(&right.source_id)
                .then_with(|| left.source_unit.cmp(&right.source_unit))
        });
        Ok(ExtractionOutput { facts, sfc_states })
    })
}

fn sfc_capability_records(states: &BTreeMap<SfcDialect, CapabilityState>) -> Vec<CapabilityRecord> {
    [SfcDialect::Vue, SfcDialect::Svelte, SfcDialect::Astro]
        .into_iter()
        .map(|dialect| CapabilityRecord {
            capability_id: lumin_sfc::capability_id(dialect).to_owned(),
            state: states
                .get(&dialect)
                .copied()
                .unwrap_or(CapabilityState::Unavailable),
        })
        .collect()
}

fn less_complete(left: CapabilityState, right: CapabilityState) -> CapabilityState {
    fn rank(state: CapabilityState) -> u8 {
        match state {
            CapabilityState::Complete => 0,
            CapabilityState::Incomplete => 1,
            CapabilityState::Unavailable => 2,
            CapabilityState::Failed => 3,
        }
    }
    if rank(left) >= rank(right) {
        left
    } else {
        right
    }
}

fn collect_limitations(
    inventory_limitations: &mut Vec<Limitation>,
    facts: &[FileFacts],
    resolved: &[ResolvedSourceUse],
    resolver_limitations: Vec<Limitation>,
) -> Vec<Limitation> {
    let mut limitations = std::mem::take(inventory_limitations);
    limitations.extend(resolver_limitations);
    for file in facts {
        limitations.extend(file.limitations.iter().cloned());
    }
    for resolution in resolved {
        match &resolution.outcome {
            ResolutionOutcome::Unresolved {
                specifier,
                candidates,
            } => limitations.push(Limitation::InternalSpecifierUnresolved {
                importer: resolution.source_use.importer.clone(),
                specifier: specifier.clone(),
                candidates: candidates.clone(),
            }),
            ResolutionOutcome::Unsupported { .. } => {}
            ResolutionOutcome::Internal { .. }
            | ResolutionOutcome::External { .. }
            | ResolutionOutcome::NonSourceAsset { .. } => {}
        }
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
    let store = open_repository_context(root)?.store;
    let Some(run_id) = store.latest_run_id()? else {
        return Ok(None);
    };
    store.load_run(&run_id).map(Some).map_err(Into::into)
}

fn limitation_sort_key(limitation: &Limitation) -> String {
    format!("{limitation:?}")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use lumin_model::{FindingDisposition, ResolutionProfileSource};

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
            Limitation::JsModuleUseUnknown { detail, .. }
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
}
