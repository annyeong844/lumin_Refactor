use std::path::{Path, PathBuf};

use lumin_evidence::{
    LifecycleOperationRecord, RecordLookup, RetentionMutationResult, RetentionPlanRecord,
    RetentionPlanScope, RunPinRecord,
};
use lumin_model::{GateId, OperationId, PinId, RetentionPlanId, RunId};
use lumin_store::RetentionPlanRequest as StoreRetentionPlanRequest;

use crate::{EngineError, open_repository_context};

#[derive(Clone, Debug)]
pub struct PrepareRetentionPlanRequest {
    pub root: PathBuf,
    pub scope: RetentionPlanScope,
    pub operation_id: OperationId,
}

#[derive(Clone, Debug)]
pub struct ConfirmRetentionPlanRequest {
    pub root: PathBuf,
    pub plan_id: RetentionPlanId,
    pub operation_id: OperationId,
}

#[derive(Clone, Debug)]
pub struct PinRunRequest {
    pub root: PathBuf,
    pub run_id: RunId,
    pub operation_id: OperationId,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct UnpinRunRequest {
    pub root: PathBuf,
    pub pin_id: PinId,
    pub operation_id: OperationId,
}

pub fn prepare_retention_plan(
    request: &PrepareRetentionPlanRequest,
) -> Result<RetentionMutationResult, EngineError> {
    open_repository_context(&request.root)?
        .store
        .prepare_retention_plan(&StoreRetentionPlanRequest {
            scope: request.scope.clone(),
            operation_id: request.operation_id.clone(),
        })
        .map_err(Into::into)
}

pub fn confirm_retention_plan(
    request: &ConfirmRetentionPlanRequest,
) -> Result<RetentionMutationResult, EngineError> {
    open_repository_context(&request.root)?
        .store
        .confirm_retention_plan(&request.plan_id, &request.operation_id)
        .map_err(Into::into)
}

pub fn load_retention_plan(
    root: &Path,
    plan_id: &RetentionPlanId,
) -> Result<RetentionPlanRecord, EngineError> {
    open_repository_context(root)?
        .store
        .load_retention_plan(plan_id)
        .map_err(Into::into)
}

pub fn pin_run(request: &PinRunRequest) -> Result<RunPinRecord, EngineError> {
    open_repository_context(&request.root)?
        .store
        .pin_run(&request.run_id, &request.operation_id, &request.reason)
        .map_err(Into::into)
}

pub fn unpin_run(request: &UnpinRunRequest) -> Result<RunPinRecord, EngineError> {
    open_repository_context(&request.root)?
        .store
        .unpin_run(&request.pin_id, &request.operation_id)
        .map_err(Into::into)
}

pub fn lookup_run(
    root: &Path,
    run_id: &RunId,
) -> Result<RecordLookup<(lumin_store::RunCatalogRecord, lumin_evidence::RunEvidence)>, EngineError>
{
    open_repository_context(root)?
        .store
        .lookup_run(run_id)
        .map_err(Into::into)
}

pub fn lookup_gate(
    root: &Path,
    gate_id: &GateId,
) -> Result<RecordLookup<lumin_evidence::GateRecord>, EngineError> {
    open_repository_context(root)?
        .store
        .lookup_gate(gate_id)
        .map_err(Into::into)
}

pub fn load_lifecycle_operation(
    root: &Path,
    operation_id: &OperationId,
) -> Result<LifecycleOperationRecord, EngineError> {
    open_repository_context(root)?
        .store
        .load_lifecycle_operation(operation_id)
        .map_err(Into::into)
}

pub fn list_runs(
    root: &Path,
    cursor: Option<&lumin_store::RunCatalogCursor>,
    limit: usize,
) -> Result<lumin_store::RunCatalogSnapshot, EngineError> {
    open_repository_context(root)?
        .store
        .list_runs(cursor, limit)
        .map_err(Into::into)
}
