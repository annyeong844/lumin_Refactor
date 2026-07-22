mod files;
mod latest;
mod liveness;
mod run;

#[cfg(feature = "publication-test-crash")]
mod barrier;
#[cfg(feature = "publication-test-crash")]
mod crash;

#[cfg(all(feature = "publication-test-crash", not(debug_assertions)))]
compile_error!("publication-test-crash is restricted to debug test builds");

use lumin_evidence::RunEvidence;
use lumin_model::{AttemptId, AttemptStatus, RunId};
use serde::{Deserialize, Serialize};

use crate::{RepositoryStore, RunCatalogRecord, StoreError};

pub use liveness::AttemptSession;

pub type AttemptState = AttemptStatus;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttemptEnvelope {
    pub schema_version: String,
    pub attempt_id: AttemptId,
    pub sequence: u64,
    pub state: AttemptStatus,
    pub started_unix_millis: u128,
    pub finished_unix_millis: Option<u128>,
    pub run_id: Option<RunId>,
    pub failure: Option<String>,
}

#[derive(Debug)]
pub struct LatestRunSnapshot {
    pub latest_attempt: Option<AttemptEnvelope>,
    pub completed: Option<(RunCatalogRecord, RunEvidence)>,
}

impl RepositoryStore {
    pub fn begin_attempt(&self) -> Result<AttemptSession<'_>, StoreError> {
        liveness::begin(self)
    }

    pub fn fail_attempt(
        &self,
        attempt: &mut AttemptSession<'_>,
        failure: &str,
    ) -> Result<(), StoreError> {
        liveness::finish_failed(self, attempt, failure)
    }

    pub fn publish_run(
        &self,
        attempt: &mut AttemptSession<'_>,
        evidence: &RunEvidence,
    ) -> Result<crate::PublishedRun, StoreError> {
        run::publish(self, attempt, evidence)
    }

    pub fn latest_snapshot(&self) -> Result<LatestRunSnapshot, StoreError> {
        latest::snapshot(self)
    }

    pub(super) fn recover_publication(&self) -> Result<(), StoreError> {
        liveness::recover(self)
    }
}

pub(super) fn latest_run_id(store: &RepositoryStore) -> Result<Option<RunId>, StoreError> {
    latest::completed_run_id(store)
}

pub(crate) fn validate_attempt_leases(
    rows: &std::collections::BTreeMap<String, Vec<u8>>,
) -> Result<(), StoreError> {
    liveness::validate_snapshot(rows)
}

pub(crate) fn validate_attempt_lease_locks(
    rows: &std::collections::BTreeMap<String, Vec<u8>>,
    guard: &crate::namespace::NamespaceGuard,
) -> Result<(), StoreError> {
    liveness::validate_snapshot_locks(rows, guard)
}

pub(super) fn run_id(sequence: u64) -> RunId {
    RunId::from_string(format!("run_{sequence:016x}"))
}

pub(super) fn attempt_directory(
    store: &RepositoryStore,
    attempt_id: &AttemptId,
) -> std::path::PathBuf {
    store.state_dir.join("attempts").join(attempt_id.as_str())
}

pub(super) fn attempt_path(store: &RepositoryStore, attempt_id: &AttemptId) -> std::path::PathBuf {
    attempt_directory(store, attempt_id).join("attempt.json")
}
