use std::path::PathBuf;

use lumin_evidence::GateOperationResult;
use lumin_model::{GateId, OperationId, append_length_prefixed, digest_hex};
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
    let request_digest = abandon_digest(&request.gate_id, target_revision, &request.reason);
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

fn abandon_digest(gate_id: &GateId, target_revision: u64, reason: &str) -> String {
    let mut bytes = Vec::new();
    append_length_prefixed(&mut bytes, b"lumin-gate-abandon.v1");
    append_length_prefixed(&mut bytes, gate_id.as_str().as_bytes());
    bytes.extend_from_slice(&target_revision.to_be_bytes());
    append_length_prefixed(&mut bytes, reason.as_bytes());
    digest_hex(&bytes)
}
