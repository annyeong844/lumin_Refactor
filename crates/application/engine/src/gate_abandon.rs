use std::path::PathBuf;

use lumin_evidence::{GateOperationResult, gate_abandon_request_digest};
use lumin_model::{GateId, OperationId};
use lumin_store::StoreError;

use super::{EngineError, open_repository_context};

#[derive(Clone, Debug)]
pub struct AbandonGateRequest {
    pub root: PathBuf,
    pub gate_id: GateId,
    pub operation_id: OperationId,
    pub reason: String,
}

pub fn abandon_gate(request: &AbandonGateRequest) -> Result<GateOperationResult, EngineError> {
    let store = open_repository_context(&request.root)?.store;
    let target_revision = match store.load_operation(&request.operation_id) {
        Ok(operation) => operation.target_revision,
        Err(StoreError::OperationNotFound(_)) => {
            store.load_gate(&request.gate_id)?.current_revision
        }
        Err(error) => return Err(error.into()),
    };
    let request_digest =
        gate_abandon_request_digest(&request.gate_id, target_revision, &request.reason);
    store
        .begin_operation(&request.operation_id)?
        .abandon_gate(
            &request_digest,
            &request.gate_id,
            target_revision,
            &request.reason,
        )
        .map_err(Into::into)
}
