use std::path::{Path, PathBuf};

use lumin_evidence::{CapabilityRecord, DEAD_CODE_CAPABILITY_ID, RunEvidence};
use lumin_inventory::{InventoryError, InventoryRequest};
use lumin_model::{CapabilityState, Limitation, ResolutionOutcome, RoleOverride, RunId};
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
    Store(#[from] StoreError),
    #[error("invalid worker count: {0}")]
    InvalidWorkerCount(usize),
    #[error("failed to build the local worker pool: {0}")]
    Scheduler(String),
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
    let evidence = match analyze_repository(&request.root, &inventory_request, request.jobs) {
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
) -> Result<RunEvidence, EngineError> {
    if jobs == 0 {
        return Err(EngineError::InvalidWorkerCount(0));
    }
    let inventory = lumin_inventory::scan(root, request)?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .thread_name(|index| format!("lumin-worker-{index}"))
        .build()
        .map_err(|error| EngineError::Scheduler(error.to_string()))?;
    let mut facts = pool.install(|| {
        inventory
            .sources
            .par_iter()
            .map(lumin_js::extract)
            .collect::<Vec<_>>()
    });
    facts.sort_by(|left, right| left.source_id.cmp(&right.source_id));

    let resolved = lumin_resolve::resolve_all(&inventory.sources, &facts);
    let mut limitations = inventory.limitations;
    for file in &facts {
        limitations.extend(file.limitations.iter().cloned());
    }
    for resolution in &resolved {
        match &resolution.outcome {
            ResolutionOutcome::Unresolved {
                specifier,
                candidates,
            } => limitations.push(Limitation::InternalSpecifierUnresolved {
                importer: resolution.source_use.importer.clone(),
                specifier: specifier.clone(),
                candidates: candidates.clone(),
            }),
            ResolutionOutcome::Unsupported { specifier, reason } => {
                limitations.push(Limitation::JsModuleUseUnknown {
                    source_id: resolution.source_use.importer.clone(),
                    detail: format!("unsupported specifier {specifier}: {reason}"),
                });
            }
            ResolutionOutcome::Internal { .. }
            | ResolutionOutcome::External { .. }
            | ResolutionOutcome::NonSourceAsset { .. } => {}
        }
        if matches!(
            (&resolution.outcome, resolution.source_use.kind),
            (
                ResolutionOutcome::Internal { .. },
                lumin_model::ImportKind::Namespace
            )
        ) {
            limitations.push(Limitation::JsModuleUseUnknown {
                source_id: resolution.source_use.importer.clone(),
                detail: "internal namespace member precision is not implemented in this increment"
                    .to_owned(),
            });
        }
    }
    limitations.sort_by_key(limitation_sort_key);
    limitations.dedup();

    let graph = lumin_graph::build(&inventory.sources, &facts, &resolved);
    let findings = lumin_dead::analyze(&inventory.sources, &graph, &limitations);
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
        findings,
        limitations,
    })
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

    use lumin_model::FindingDisposition;

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
        let one = analyze_repository(root.path(), &request, 1)?;
        let many = analyze_repository(root.path(), &request, 4)?;
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
        let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 2)?;
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
        let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 1)?;
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
        let evidence = analyze_repository(root.path(), &InventoryRequest::default(), 1)?;
        assert_eq!(evidence.dead_code_state(), CapabilityState::Incomplete);
        assert_eq!(evidence.findings.len(), 1);
        assert_eq!(evidence.findings[0].exported_name, "dead");
        assert!(evidence.limitations.iter().any(|limitation| matches!(
            limitation,
            Limitation::InternalSpecifierUnresolved { .. }
        )));
        Ok(())
    }
}
