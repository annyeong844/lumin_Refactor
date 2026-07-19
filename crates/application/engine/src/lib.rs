use std::path::{Path, PathBuf};

use lumin_evidence::{CapabilityRecord, DEAD_CODE_CAPABILITY_ID, RunEvidence};
use lumin_inventory::{InventoryError, InventoryRequest, InventorySnapshot};
use lumin_model::{
    CapabilityState, FileFacts, Limitation, ResolutionOutcome, ResolutionProfile,
    ResolvedSourceUse, RoleOverride, RunId, SourceSnapshot,
};
use lumin_resolve::{ResolverError, ResolverOutput};
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
    #[error("resolver requested semantic inputs that were already captured: {0}")]
    ResolverDemandStalled(String),
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

pub fn audit(request: &AuditRequest) -> Result<AuditResult, EngineError> {
    if request.jobs == 0 {
        return Err(EngineError::InvalidWorkerCount(0));
    }
    let store = RepositoryStore::open(&request.root)?;
    let attempt = store.begin_attempt()?;
    let inventory_request = InventoryRequest {
        includes: request.includes.clone(),
        excludes: request.excludes.clone(),
        role_overrides: request.role_overrides.clone(),
    };
    let evidence = match analyze_repository(
        &request.root,
        &inventory_request,
        request.jobs,
        request.resolution_profile,
    ) {
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
    if jobs == 0 {
        return Err(EngineError::InvalidWorkerCount(0));
    }
    let mut inventory = lumin_inventory::scan(root, request)?;
    let facts = extract_facts(&inventory.sources, jobs)?;
    let resolver = resolve_config_fixed_point(root, &mut inventory, &facts, resolution_profile)?;
    let ResolverOutput {
        resolved,
        package_surfaces,
        profiles,
        limitations: resolver_limitations,
        demands: _,
    } = resolver;
    let limitations = collect_limitations(
        &mut inventory.limitations,
        &facts,
        &resolved,
        resolver_limitations,
    );

    let graph = lumin_graph::build(&inventory.sources, &facts, &resolved, &package_surfaces);
    let findings = lumin_dead::analyze(&inventory.sources, &graph, &inventory.config, &limitations);
    let state = if limitations.is_empty() {
        CapabilityState::Complete
    } else {
        CapabilityState::Incomplete
    };
    Ok(RunEvidence {
        schema_version: "lumin-evidence.v1".to_owned(),
        capabilities: vec![CapabilityRecord {
            capability_id: DEAD_CODE_CAPABILITY_ID.to_owned(),
            state,
        }],
        resolution_profiles: profiles,
        findings,
        limitations,
    })
}

fn extract_facts(sources: &[SourceSnapshot], jobs: usize) -> Result<Vec<FileFacts>, EngineError> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .thread_name(|index| format!("lumin-worker-{index}"))
        .build()
        .map_err(|error| EngineError::Scheduler(error.to_string()))?;
    let mut facts = pool.install(|| {
        sources
            .par_iter()
            .map(lumin_js::extract)
            .collect::<Vec<_>>()
    });
    facts.sort_by(|left, right| left.source_id.cmp(&right.source_id));
    Ok(facts)
}

fn resolve_config_fixed_point(
    root: &Path,
    inventory: &mut InventorySnapshot,
    facts: &[FileFacts],
    resolution_profile: Option<ResolutionProfile>,
) -> Result<ResolverOutput, EngineError> {
    loop {
        let output = lumin_resolve::resolve_all(
            &inventory.sources,
            facts,
            &inventory.config,
            resolution_profile,
        )?;
        if output.demands.is_empty() {
            return Ok(output);
        }
        let requested = output
            .demands
            .iter()
            .map(|demand| demand.path.display_escaped())
            .collect::<Vec<_>>();
        let mut captured = Vec::new();
        for demand in output.demands {
            if inventory.config.observations.contains_key(&demand.path) {
                continue;
            }
            let observation = lumin_inventory::observe_config(root, &demand.path, demand.syntax)?;
            captured.push(demand.path.display_escaped());
            inventory
                .config
                .observations
                .insert(demand.path, observation);
        }
        if captured.is_empty() {
            return Err(EngineError::ResolverDemandStalled(requested.join(", ")));
        }
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
    RepositoryStore::open(root)?
        .load_run(run_id)
        .map_err(Into::into)
}

pub fn load_latest_run(
    root: &Path,
) -> Result<Option<(RunCatalogRecord, RunEvidence)>, EngineError> {
    let store = RepositoryStore::open(root)?;
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
