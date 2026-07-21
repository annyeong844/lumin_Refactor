use lumin_evidence::{
    GateAnalysisOptions, GateDecision, GateLifecycle, GateRecord, GateRevision, RecordLookup,
    RetentionItemKind, RetentionMutationResult, RetentionPlanScope,
};
use lumin_model::GateId;
use tempfile::TempDir;

use super::*;

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
    store.with_exclusive_lock(|guard| {
        let database = guard.open_database()?;
        let write = database.begin_write()?;
        let gate_id = GateId::from_string("gate_0000000000000001".to_owned());
        let gate = GateRecord {
            schema_version: "lumin-gate.v1".to_owned(),
            gate_id: gate_id.clone(),
            lifecycle: GateLifecycle::Abandoned,
            current_revision: 0,
            declared_write_set: Vec::new(),
            leased_write_set: Vec::new(),
            alias_closures: Vec::new(),
            transition_refs: Vec::new(),
            analysis_options: GateAnalysisOptions {
                jobs: 1,
                resolution_profile: None,
            },
            baseline: None,
            protected_semantic_inputs: Vec::new(),
            revisions: vec![GateRevision {
                revision: 0,
                operation_id: operation("terminal-gate-owner"),
                committed_unix_millis: Some(1),
                decision: GateDecision::Deny,
                reason: Some("test terminal gate".to_owned()),
                signals: Vec::new(),
                changed_paths: Vec::new(),
                snapshot: None,
                protected_semantic_inputs: Vec::new(),
                alias_closures: Vec::new(),
                reconciled_transition_sequences: Vec::new(),
                deltas: Vec::new(),
            }],
        };
        crate::gate::records::write_record(&write, crate::gate::GATES, gate_id.as_str(), &gate)?;
        guard.commit(write)?;
        Ok(gate_id)
    })
}
