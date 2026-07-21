use std::collections::{BTreeMap, BTreeSet};

use lumin_evidence::{
    GateLifecycle, GateRecord, OperationRecord, RetentionExclusionReason, RetentionItemKind,
    RetentionPlanExclusion, RetentionPlanItem, RunEvidence, WorktreeTransition,
};
use redb::WriteTransaction;
use serde::Serialize;

use crate::StoreError;
use crate::gate::{GATES, OPERATIONS, TRANSITIONS};
use crate::namespace::NamespaceGuard;

use super::{PlanContents, read_raw_records, retention_item_from_bytes};

pub(super) fn collect(
    _guard: &NamespaceGuard,
    write: &WriteTransaction,
    terminal_before_unix_millis: u64,
) -> Result<PlanContents, StoreError> {
    let gates = read_raw_records::<GateRecord>(write, GATES, "gates")?;
    let operations = read_raw_records::<OperationRecord>(write, OPERATIONS, "operations")?;
    let transitions =
        read_raw_records::<WorktreeTransition>(write, TRANSITIONS, "worktree-transitions")?;
    let protected = protected_terminal_gates(&gates, &transitions)?;
    let mut items = Vec::new();
    let mut exclusions = Vec::new();

    for (key, (gate, gate_bytes)) in &gates {
        if key != gate.gate_id.as_str() {
            return Err(StoreError::Integrity(format!(
                "gate key {key} disagrees with its record"
            )));
        }
        if gate.lifecycle == GateLifecycle::Active {
            exclusions.push(RetentionPlanExclusion {
                kind: RetentionItemKind::Gate,
                record_id: key.clone(),
                reason: RetentionExclusionReason::ActiveGate,
            });
            continue;
        }
        let Some(committed) = gate
            .revisions
            .last()
            .and_then(|revision| revision.committed_unix_millis)
        else {
            exclusions.push(RetentionPlanExclusion {
                kind: RetentionItemKind::Gate,
                record_id: key.clone(),
                reason: RetentionExclusionReason::TerminalTimestampUnavailable,
            });
            continue;
        };
        if committed >= u128::from(terminal_before_unix_millis) {
            continue;
        }
        if let Some(gate_ids) = protected.get(key) {
            exclusions.push(RetentionPlanExclusion {
                kind: RetentionItemKind::Gate,
                record_id: key.clone(),
                reason: RetentionExclusionReason::ActiveTransitionReference {
                    gate_ids: gate_ids.clone(),
                },
            });
            continue;
        }
        collect_gate_items(gate, gate_bytes, &operations, &transitions, &mut items)?;
    }

    items.sort();
    exclusions.sort();
    Ok(PlanContents { items, exclusions })
}

fn protected_terminal_gates(
    gates: &BTreeMap<String, (GateRecord, Vec<u8>)>,
    transitions: &BTreeMap<String, (WorktreeTransition, Vec<u8>)>,
) -> Result<BTreeMap<String, Vec<lumin_model::GateId>>, StoreError> {
    let mut references = BTreeMap::<u64, Vec<lumin_model::GateId>>::new();
    for (gate, _) in gates.values() {
        if gate.lifecycle == GateLifecycle::Active {
            for sequence in &gate.transition_refs {
                references
                    .entry(*sequence)
                    .or_default()
                    .push(gate.gate_id.clone());
            }
        }
    }
    let mut protected = BTreeMap::<String, Vec<lumin_model::GateId>>::new();
    let transition_by_sequence = transitions
        .values()
        .map(|(transition, _)| (transition.sequence, transition))
        .collect::<BTreeMap<_, _>>();
    for (sequence, mut gate_ids) in references {
        let transition = transition_by_sequence.get(&sequence).ok_or_else(|| {
            StoreError::Integrity(format!(
                "active gate references missing transition {sequence}"
            ))
        })?;
        gate_ids.sort();
        gate_ids.dedup();
        protected
            .entry(transition.capsule.gate_id.as_str().to_owned())
            .or_default()
            .extend(gate_ids);
    }
    for gate_ids in protected.values_mut() {
        gate_ids.sort();
        gate_ids.dedup();
    }
    Ok(protected)
}

fn collect_gate_items(
    gate: &GateRecord,
    gate_bytes: &[u8],
    operations: &BTreeMap<String, (OperationRecord, Vec<u8>)>,
    transitions: &BTreeMap<String, (WorktreeTransition, Vec<u8>)>,
    items: &mut Vec<RetentionPlanItem>,
) -> Result<(), StoreError> {
    let sequence = sequence_from_id(gate.gate_id.as_str(), "gate_")?;
    items.push(retention_item_from_bytes(
        RetentionItemKind::Gate,
        sequence,
        gate.gate_id.as_str().to_owned(),
        gate_bytes,
    ));
    if let Some(baseline) = &gate.baseline {
        collect_evidence(
            sequence,
            format!("gate:{}/baseline", gate.gate_id.as_str()),
            &baseline.snapshot.evidence,
            items,
        )?;
    }
    for revision in &gate.revisions {
        let revision_id = format!(
            "gate:{}/revision:{}",
            gate.gate_id.as_str(),
            revision.revision
        );
        items.push(item_from_value(
            RetentionItemKind::GateRevision,
            sequence,
            revision_id.clone(),
            revision,
        )?);
        if let Some(snapshot) = &revision.snapshot {
            collect_evidence(sequence, revision_id, &snapshot.evidence, items)?;
        }
    }
    for (key, (operation, bytes)) in operations {
        if operation.gate_id == gate.gate_id {
            items.push(retention_item_from_bytes(
                RetentionItemKind::Operation,
                sequence,
                key.clone(),
                bytes,
            ));
        }
    }
    for (key, (transition, bytes)) in transitions {
        if transition.capsule.gate_id == gate.gate_id {
            items.push(retention_item_from_bytes(
                RetentionItemKind::Transition,
                transition.sequence,
                key.clone(),
                bytes,
            ));
        }
    }
    Ok(())
}

fn collect_evidence(
    sequence: u64,
    owner: String,
    evidence: &RunEvidence,
    items: &mut Vec<RetentionPlanItem>,
) -> Result<(), StoreError> {
    items.push(item_from_value(
        RetentionItemKind::Evidence,
        sequence,
        format!("{owner}/evidence"),
        evidence,
    )?);
    let mut seen = BTreeSet::new();
    for finding in &evidence.findings {
        let record_id = format!("{owner}/finding:{}", finding.finding_id.as_str());
        if !seen.insert(record_id.clone()) {
            return Err(StoreError::Integrity(format!(
                "evidence owner {owner} contains duplicate finding identities"
            )));
        }
        items.push(item_from_value(
            RetentionItemKind::Finding,
            sequence,
            record_id,
            finding,
        )?);
    }
    Ok(())
}

fn item_from_value(
    kind: RetentionItemKind,
    owning_sequence: u64,
    record_id: String,
    value: &impl Serialize,
) -> Result<RetentionPlanItem, StoreError> {
    let bytes = serde_json::to_vec(value).map_err(crate::serialization_error)?;
    Ok(retention_item_from_bytes(
        kind,
        owning_sequence,
        record_id,
        &bytes,
    ))
}

fn sequence_from_id(value: &str, prefix: &str) -> Result<u64, StoreError> {
    let suffix = value.strip_prefix(prefix).ok_or_else(|| {
        StoreError::Integrity(format!("{value} is outside the {prefix} identity grammar"))
    })?;
    if suffix.len() != 16 {
        return Err(StoreError::Integrity(format!(
            "{value} is outside the {prefix} identity grammar"
        )));
    }
    u64::from_str_radix(suffix, 16)
        .map_err(|error| StoreError::Integrity(format!("{value} has invalid sequence: {error}")))
}
