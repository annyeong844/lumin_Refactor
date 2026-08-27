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

pub(crate) use latest::{
    migration_pointer_ids, reconcile_migration_pointer_index, validate_attempt_envelope,
};
pub use liveness::AttemptSession;
pub(crate) use run::{
    read_validated_directory as read_validated_run_directory,
    validate_directory_for_migration as validate_run_directory_for_migration,
};
#[cfg(test)]
pub(crate) use run::{
    validate_directory_with_evidence_read_hook, validate_directory_with_inventory_hooks,
};

pub type AttemptState = AttemptStatus;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttemptEnvelope {
    pub schema_version: String,
    pub attempt_id: AttemptId,
    pub sequence: u64,
    pub state: AttemptStatus,
    pub started_unix_millis: u64,
    pub finished_unix_millis: Option<u64>,
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
        final_validation: impl FnOnce(
            &std::collections::BTreeSet<lumin_model::PhysicalFileIdentity>,
        ) -> Result<(), StoreError>,
    ) -> Result<crate::PublishedRun, StoreError> {
        run::publish(self, attempt, evidence, final_validation)
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

pub(crate) fn migration_has_active_lease(
    rows: &std::collections::BTreeMap<String, Vec<u8>>,
    attempt_id: &AttemptId,
) -> Result<bool, StoreError> {
    liveness::migration_has_active_lease(rows, attempt_id)
}

pub(crate) fn validate_attempt_leases(
    rows: &std::collections::BTreeMap<String, Vec<u8>>,
) -> Result<(), StoreError> {
    liveness::validate_snapshot(rows)
}

pub(crate) fn validate_migration_attempt_leases(
    rows: &std::collections::BTreeMap<String, Vec<u8>>,
) -> Result<(), StoreError> {
    liveness::validate_migration_snapshot(rows)
}

pub(crate) fn migration_attempt_lock_names(
    rows: &std::collections::BTreeMap<String, Vec<u8>>,
) -> Result<std::collections::BTreeSet<String>, StoreError> {
    liveness::migration_lock_names(rows)
}

pub(crate) fn reconcile_migration_attempt_allocations(
    rows: &mut std::collections::BTreeMap<String, Vec<u8>>,
    guard: &crate::namespace::NamespaceGuard,
) -> Result<(), StoreError> {
    liveness::reconcile_migration_allocations(rows, guard)
}

#[cfg(test)]
pub(crate) fn reserve_migration_attempt_allocation_for_test(
    store: &RepositoryStore,
    lock_binding: Option<bool>,
) -> Result<(AttemptId, String), StoreError> {
    liveness::reserve_migration_allocation_for_test(store, lock_binding)
}

pub(crate) fn validate_migration_attempt_links(
    rows: &std::collections::BTreeMap<String, Vec<u8>>,
    attempts: &std::collections::BTreeMap<String, Option<AttemptEnvelope>>,
    pending_attempts: &std::collections::BTreeSet<String>,
) -> Result<(), StoreError> {
    liveness::validate_migration_attempt_links(rows, attempts, pending_attempts)
}

pub(crate) fn validate_attempt_lease_locks(
    rows: &std::collections::BTreeMap<String, Vec<u8>>,
    guard: &crate::namespace::NamespaceGuard,
) -> Result<(), StoreError> {
    liveness::validate_snapshot_locks(rows, guard)
}

pub(crate) fn validate_completed_run_payload(
    guard: &crate::namespace::NamespaceGuard,
    envelope: &AttemptEnvelope,
) -> Result<RunCatalogRecord, StoreError> {
    run::validate_published(guard, envelope)
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
