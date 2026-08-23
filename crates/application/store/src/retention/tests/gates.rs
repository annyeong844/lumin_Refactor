use lumin_evidence::{
    GateAnalysisOptions, RecordLookup, RepoPathProjection, RetentionItemKind,
    RetentionMutationResult, RetentionPlanScope, UnsealedGateObservationInputs, WriteLease,
    derive_unsealed_gate_observation_binding,
};
use lumin_model::{GateId, RepoPath};
use tempfile::TempDir;

use super::*;
use crate::gate::PreWriteStart;

#[test]
fn terminal_gate_plan_removes_gate_but_keeps_tombstone() -> Result<(), Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    let store = open_store(root.path())?;
    let gate_id = insert_terminal_gate(&store)?;
    let result = store.prepare_retention_plan(&RetentionPlanRequest {
        scope: RetentionPlanScope::Gates {
            terminal_before_unix_millis: 9_000_000_000_000,
        },
        operation_id: operation("plan-gates"),
    })?;
    let plan_id = prepared_plan_id(&result)?;
    let plan = store.load_retention_plan(&plan_id)?;
    assert!(plan.items.iter().any(|item| {
        item.kind == RetentionItemKind::Gate && item.record_id == gate_id.as_str()
    }));

    let result = store.confirm_retention_plan(&plan_id, &operation("confirm-gates"))?;
    assert!(matches!(result, RetentionMutationResult::Pruned { .. }));
    assert!(matches!(
        store.lookup_gate(&gate_id)?,
        RecordLookup::Pruned(_)
    ));
    Ok(())
}

fn insert_terminal_gate(store: &crate::RepositoryStore) -> Result<GateId, crate::StoreError> {
    let path = RepoPathProjection::from(
        &RepoPath::from_portable("src/retention-terminal.ts")
            .map_err(|error| crate::StoreError::Integrity(error.to_string()))?,
    );
    let lease = WriteLease {
        path: path.clone(),
        kind: lumin_evidence::WriteLeaseKind::ExistingFile,
        physical_identity: None,
        nearest_existing_parent: None,
        prefix_identities: Vec::new(),
    };
    let options = GateAnalysisOptions {
        jobs: 1,
        resolution_profile: None,
        scan_invocation: Default::default(),
    };
    let blocker = store.begin_operation(&operation("terminal-gate-blocker"))?;
    if !matches!(
        blocker.reserve_pre_write(
            "terminal-gate-blocker",
            std::slice::from_ref(&path),
            std::slice::from_ref(&lease),
            &options,
            |signals| {
                derive_unsealed_gate_observation_binding(
                    &[],
                    &UnsealedGateObservationInputs::new(
                        vec![lease.clone()],
                        Vec::new(),
                        Vec::new(),
                    ),
                    signals,
                )
            },
        )?,
        PreWriteStart::Analyze { .. }
    ) {
        return Err(crate::StoreError::Integrity(
            "terminal gate blocker did not retain its reservation".to_owned(),
        ));
    }
    let rejected = store.begin_operation(&operation("terminal-gate-owner"))?;
    match rejected.reserve_pre_write(
        "terminal-gate-owner",
        std::slice::from_ref(&path),
        std::slice::from_ref(&lease),
        &options,
        |signals| {
            derive_unsealed_gate_observation_binding(
                &[],
                &UnsealedGateObservationInputs::new(vec![lease.clone()], Vec::new(), Vec::new()),
                signals,
            )
        },
    )? {
        PreWriteStart::Committed(result) => Ok(result.gate_id),
        PreWriteStart::Analyze { .. } => Err(crate::StoreError::Integrity(
            "terminal gate fixture was unexpectedly authorized".to_owned(),
        )),
    }
}
