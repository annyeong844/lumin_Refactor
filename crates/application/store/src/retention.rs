mod confirmation;
#[cfg(feature = "retention-test-crash")]
mod crash;
mod pins;
mod planning;
pub(crate) mod records;

#[cfg(all(feature = "retention-test-crash", not(debug_assertions)))]
compile_error!("retention-test-crash is restricted to debug test builds");

#[cfg(test)]
mod tests;

use lumin_evidence::{
    LifecycleOperationRecord, RetentionOperationRecord, RetentionPlanRecord, RetentionPlanScope,
};
use lumin_model::{OperationId, PinId, RetentionPlanId, RunId};
use redb::TableDefinition;

use crate::StoreError;
use crate::namespace::NamespaceGuard;

pub(crate) const RETENTION_PLANS: TableDefinition<&str, &[u8]> =
    TableDefinition::new("retention-plans");
pub(crate) const RETENTION_OPERATIONS: TableDefinition<&str, &[u8]> =
    TableDefinition::new("retention-operations");
pub(crate) const RETENTION_TOMBSTONES: TableDefinition<&str, &[u8]> =
    TableDefinition::new("retention-tombstones");
pub(crate) const RUN_PINS: TableDefinition<&str, &[u8]> = TableDefinition::new("run-pins");

pub const RETENTION_PLAN_ITEMS_ORDERING: &str = "retention-plan-items.v1";

pub(crate) fn validate_migration_payloads(
    guard: &NamespaceGuard,
    plan: &records::StoredRetentionPlan,
) -> Result<std::collections::BTreeMap<String, std::path::PathBuf>, StoreError> {
    confirmation::payload::validate_migration_state(guard, plan)
}

#[derive(Clone, Debug)]
pub struct RetentionPlanRequest {
    pub scope: RetentionPlanScope,
    pub operation_id: OperationId,
}

impl crate::RepositoryStore {
    pub fn prepare_retention_plan(
        &self,
        request: &RetentionPlanRequest,
    ) -> Result<lumin_evidence::RetentionMutationResult, StoreError> {
        planning::prepare(self, request)
    }

    pub fn load_retention_plan(
        &self,
        plan_id: &RetentionPlanId,
    ) -> Result<RetentionPlanRecord, StoreError> {
        records::load_public_plan(self, plan_id)
    }

    pub fn confirm_retention_plan(
        &self,
        plan_id: &RetentionPlanId,
        operation_id: &OperationId,
    ) -> Result<lumin_evidence::RetentionMutationResult, StoreError> {
        confirmation::confirm(self, plan_id, operation_id)
    }

    pub fn pin_run(
        &self,
        run_id: &RunId,
        operation_id: &OperationId,
        reason: &str,
    ) -> Result<lumin_evidence::RunPinRecord, StoreError> {
        pins::create(self, run_id, operation_id, reason)
    }

    pub fn unpin_run(
        &self,
        pin_id: &PinId,
        operation_id: &OperationId,
    ) -> Result<lumin_evidence::RunPinRecord, StoreError> {
        pins::remove(self, pin_id, operation_id)
    }

    pub fn lookup_run_pin(
        &self,
        pin_id: &PinId,
    ) -> Result<lumin_evidence::RecordLookup<lumin_evidence::RunPinRecord>, StoreError> {
        pins::lookup(self, pin_id)
    }

    pub fn load_retention_operation(
        &self,
        operation_id: &OperationId,
    ) -> Result<RetentionOperationRecord, StoreError> {
        self.with_shared_lock(|guard| {
            let database = guard.open_database()?;
            let operation = crate::gate::records::load_record(
                &database,
                RETENTION_OPERATIONS,
                operation_id.as_str(),
            )?
            .ok_or_else(|| StoreError::OperationNotFound(operation_id.as_str().to_owned()))?;
            records::project_retention_operation(&database, operation)
        })
    }

    pub fn load_lifecycle_operation(
        &self,
        operation_id: &OperationId,
    ) -> Result<LifecycleOperationRecord, StoreError> {
        match self.load_operation(operation_id) {
            Ok(operation) => Ok(LifecycleOperationRecord::Gate(Box::new(operation))),
            Err(StoreError::OperationNotFound(_)) => self
                .load_retention_operation(operation_id)
                .map(Box::new)
                .map(LifecycleOperationRecord::Retention),
            Err(error) => Err(error),
        }
    }
}
