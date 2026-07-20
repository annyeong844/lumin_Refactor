use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::append_length_prefixed;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfidenceRank {
    Low,
    Medium,
    High,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GroundingRank {
    Opaque,
    Partial,
    Grounded,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeltaFactFamily {
    UnresolvedInternalEdge,
    DependencyOwnership,
    DeadExport,
    Opacity,
}

impl DeltaFactFamily {
    pub fn blocks_when_adverse(self) -> bool {
        !matches!(self, Self::Opacity)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeltaIdentityKind {
    LogicalSource,
    Package,
    Target,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeltaIdentity {
    pub kind: DeltaIdentityKind,
    pub canonical: Vec<u8>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeltaKey {
    pub owner_capability: String,
    pub family: DeltaFactFamily,
    pub semantic_identity: Vec<u8>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum DeltaValue {
    Absent,
    Bytes(Vec<u8>),
}

impl DeltaValue {
    pub fn bytes(value: impl Into<Vec<u8>>) -> Self {
        Self::Bytes(value.into())
    }

    pub fn text(value: &str) -> Self {
        Self::Bytes(value.as_bytes().to_vec())
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeltaOwnerPayloadValue {
    pub value: DeltaValue,
    pub rank: Option<i64>,
}

impl DeltaOwnerPayloadValue {
    pub fn unordered(value: DeltaValue) -> Self {
        Self { value, rank: None }
    }

    pub fn ranked(value: DeltaValue, rank: i64) -> Self {
        Self {
            value,
            rank: Some(rank),
        }
    }

    fn absent() -> Self {
        Self::unordered(DeltaValue::Absent)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeltaFact {
    pub key: DeltaKey,
    pub targets: BTreeSet<DeltaIdentity>,
    pub affected_identities: BTreeSet<DeltaIdentity>,
    pub confidence: ConfidenceRank,
    pub grounding: GroundingRank,
    pub evidence_identity: DeltaValue,
    pub owner_payload: BTreeMap<String, DeltaOwnerPayloadValue>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DeltaDimensionChange {
    TargetAdded {
        identity: DeltaIdentity,
    },
    TargetRemoved {
        identity: DeltaIdentity,
    },
    AffectedIdentityAdded {
        identity: DeltaIdentity,
    },
    AffectedIdentityRemoved {
        identity: DeltaIdentity,
    },
    ConfidenceRaised {
        from: ConfidenceRank,
        to: ConfidenceRank,
    },
    ConfidenceLowered {
        from: ConfidenceRank,
        to: ConfidenceRank,
    },
    GroundingRaised {
        from: GroundingRank,
        to: GroundingRank,
    },
    GroundingLowered {
        from: GroundingRank,
        to: GroundingRank,
    },
    EvidenceIdentityChanged {
        from: DeltaValue,
        to: DeltaValue,
    },
    OwnerPayloadRegressed {
        field_id: String,
        from: DeltaOwnerPayloadValue,
        to: DeltaOwnerPayloadValue,
    },
    OwnerPayloadImproved {
        field_id: String,
        from: DeltaOwnerPayloadValue,
        to: DeltaOwnerPayloadValue,
    },
    OwnerPayloadChanged {
        field_id: String,
        from: DeltaOwnerPayloadValue,
        to: DeltaOwnerPayloadValue,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum GateDeltaClassification {
    Introduced,
    Unchanged,
    Regressed {
        changes: Vec<DeltaDimensionChange>,
    },
    Improved {
        changes: Vec<DeltaDimensionChange>,
    },
    ChangedIncomparable {
        regressions: Vec<DeltaDimensionChange>,
        improvements: Vec<DeltaDimensionChange>,
        incomparable_changes: Vec<DeltaDimensionChange>,
    },
    Resolved,
    BaselineUnavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateDeltaRecord {
    pub key: DeltaKey,
    pub classification: GateDeltaClassification,
}

pub fn classify_lifecycle_deltas(
    baseline: Option<&[DeltaFact]>,
    current: &[DeltaFact],
) -> Vec<GateDeltaRecord> {
    let current = canonicalize(current);
    let Some(baseline) = baseline else {
        return current
            .into_keys()
            .map(|key| GateDeltaRecord {
                key,
                classification: GateDeltaClassification::BaselineUnavailable,
            })
            .collect();
    };
    let baseline = canonicalize(baseline);
    let keys = baseline
        .keys()
        .chain(current.keys())
        .cloned()
        .collect::<BTreeSet<_>>();

    keys.into_iter()
        .map(|key| GateDeltaRecord {
            classification: classify_pair(baseline.get(&key), current.get(&key)),
            key,
        })
        .collect()
}

fn classify_pair(
    baseline: Option<&DeltaFact>,
    current: Option<&DeltaFact>,
) -> GateDeltaClassification {
    let (baseline, current) = match (baseline, current) {
        (Some(baseline), Some(current)) => (baseline, current),
        (None, Some(_)) => return GateDeltaClassification::Introduced,
        (Some(_), None) => return GateDeltaClassification::Resolved,
        (None, None) => return GateDeltaClassification::BaselineUnavailable,
    };
    if baseline == current {
        return GateDeltaClassification::Unchanged;
    }

    classify_changed_pair(baseline, current)
}

#[derive(Default)]
struct ClassifiedChanges {
    regressions: Vec<DeltaDimensionChange>,
    improvements: Vec<DeltaDimensionChange>,
    incomparable: Vec<DeltaDimensionChange>,
}

impl ClassifiedChanges {
    fn finish(self) -> GateDeltaClassification {
        match (
            self.regressions.is_empty(),
            self.improvements.is_empty(),
            self.incomparable.is_empty(),
        ) {
            (true, true, true) => GateDeltaClassification::Unchanged,
            (false, true, true) => GateDeltaClassification::Regressed {
                changes: self.regressions,
            },
            (true, false, true) => GateDeltaClassification::Improved {
                changes: self.improvements,
            },
            _ => GateDeltaClassification::ChangedIncomparable {
                regressions: self.regressions,
                improvements: self.improvements,
                incomparable_changes: self.incomparable,
            },
        }
    }
}

fn classify_changed_pair(baseline: &DeltaFact, current: &DeltaFact) -> GateDeltaClassification {
    let mut changes = ClassifiedChanges::default();
    collect_target_changes(baseline, current, &mut changes);
    collect_affected_identity_changes(baseline, current, &mut changes);
    collect_rank_changes(baseline, current, &mut changes);
    collect_evidence_change(baseline, current, &mut changes);
    collect_owner_payload_changes(baseline, current, &mut changes);
    changes.finish()
}

fn collect_target_changes(
    baseline: &DeltaFact,
    current: &DeltaFact,
    changes: &mut ClassifiedChanges,
) {
    for identity in current.targets.difference(&baseline.targets) {
        changes.regressions.push(DeltaDimensionChange::TargetAdded {
            identity: identity.clone(),
        });
    }
    for identity in baseline.targets.difference(&current.targets) {
        changes
            .improvements
            .push(DeltaDimensionChange::TargetRemoved {
                identity: identity.clone(),
            });
    }
}

fn collect_affected_identity_changes(
    baseline: &DeltaFact,
    current: &DeltaFact,
    changes: &mut ClassifiedChanges,
) {
    for identity in current
        .affected_identities
        .difference(&baseline.affected_identities)
    {
        changes
            .regressions
            .push(DeltaDimensionChange::AffectedIdentityAdded {
                identity: identity.clone(),
            });
    }
    for identity in baseline
        .affected_identities
        .difference(&current.affected_identities)
    {
        changes
            .improvements
            .push(DeltaDimensionChange::AffectedIdentityRemoved {
                identity: identity.clone(),
            });
    }
}

fn collect_rank_changes(
    baseline: &DeltaFact,
    current: &DeltaFact,
    changes: &mut ClassifiedChanges,
) {
    match current.confidence.cmp(&baseline.confidence) {
        std::cmp::Ordering::Less => {
            changes
                .regressions
                .push(DeltaDimensionChange::ConfidenceLowered {
                    from: baseline.confidence,
                    to: current.confidence,
                });
        }
        std::cmp::Ordering::Greater => {
            changes
                .improvements
                .push(DeltaDimensionChange::ConfidenceRaised {
                    from: baseline.confidence,
                    to: current.confidence,
                });
        }
        std::cmp::Ordering::Equal => {}
    }
    match current.grounding.cmp(&baseline.grounding) {
        std::cmp::Ordering::Less => {
            changes
                .regressions
                .push(DeltaDimensionChange::GroundingLowered {
                    from: baseline.grounding,
                    to: current.grounding,
                });
        }
        std::cmp::Ordering::Greater => {
            changes
                .improvements
                .push(DeltaDimensionChange::GroundingRaised {
                    from: baseline.grounding,
                    to: current.grounding,
                });
        }
        std::cmp::Ordering::Equal => {}
    }
}

fn collect_evidence_change(
    baseline: &DeltaFact,
    current: &DeltaFact,
    changes: &mut ClassifiedChanges,
) {
    if baseline.evidence_identity != current.evidence_identity {
        changes
            .incomparable
            .push(DeltaDimensionChange::EvidenceIdentityChanged {
                from: baseline.evidence_identity.clone(),
                to: current.evidence_identity.clone(),
            });
    }
}

fn collect_owner_payload_changes(
    baseline: &DeltaFact,
    current: &DeltaFact,
    changes: &mut ClassifiedChanges,
) {
    let payload_fields = baseline
        .owner_payload
        .keys()
        .chain(current.owner_payload.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for field_id in payload_fields {
        let from = baseline
            .owner_payload
            .get(&field_id)
            .cloned()
            .unwrap_or_else(DeltaOwnerPayloadValue::absent);
        let to = current
            .owner_payload
            .get(&field_id)
            .cloned()
            .unwrap_or_else(DeltaOwnerPayloadValue::absent);
        if from == to {
            continue;
        }
        match (from.rank, to.rank) {
            (Some(from_rank), Some(to_rank)) if to_rank < from_rank => {
                changes
                    .regressions
                    .push(DeltaDimensionChange::OwnerPayloadRegressed { field_id, from, to });
            }
            (Some(from_rank), Some(to_rank)) if to_rank > from_rank => {
                changes
                    .improvements
                    .push(DeltaDimensionChange::OwnerPayloadImproved { field_id, from, to });
            }
            _ => changes
                .incomparable
                .push(DeltaDimensionChange::OwnerPayloadChanged { field_id, from, to }),
        }
    }
}

fn canonicalize(facts: &[DeltaFact]) -> BTreeMap<DeltaKey, DeltaFact> {
    let mut groups = BTreeMap::<DeltaKey, Vec<&DeltaFact>>::new();
    for fact in facts {
        groups.entry(fact.key.clone()).or_default().push(fact);
    }
    groups
        .into_iter()
        .map(|(key, group)| {
            let fact = merge_fact_group(key.clone(), &group);
            (key, fact)
        })
        .collect()
}

fn merge_fact_group(key: DeltaKey, facts: &[&DeltaFact]) -> DeltaFact {
    let mut targets = BTreeSet::new();
    let mut affected_identities = BTreeSet::new();
    let mut confidence = ConfidenceRank::High;
    let mut grounding = GroundingRank::Grounded;
    let mut evidence_values = BTreeSet::new();
    let mut owner_fields = BTreeSet::new();

    for fact in facts {
        targets.extend(fact.targets.iter().cloned());
        affected_identities.extend(fact.affected_identities.iter().cloned());
        confidence = confidence.min(fact.confidence);
        grounding = grounding.min(fact.grounding);
        evidence_values.insert(fact.evidence_identity.clone());
        owner_fields.extend(fact.owner_payload.keys().cloned());
    }

    let owner_payload = owner_fields
        .into_iter()
        .map(|field_id| {
            let values = facts
                .iter()
                .map(|fact| {
                    fact.owner_payload
                        .get(&field_id)
                        .cloned()
                        .unwrap_or_else(DeltaOwnerPayloadValue::absent)
                })
                .collect::<BTreeSet<_>>();
            (field_id, merge_owner_values(values))
        })
        .collect();

    DeltaFact {
        key,
        targets,
        affected_identities,
        confidence,
        grounding,
        evidence_identity: merge_values(b"evidence-identity", evidence_values),
        owner_payload,
    }
}

fn merge_owner_values(values: BTreeSet<DeltaOwnerPayloadValue>) -> DeltaOwnerPayloadValue {
    if values.len() == 1 {
        return values
            .into_iter()
            .next()
            .unwrap_or_else(DeltaOwnerPayloadValue::absent);
    }
    let encoded = values
        .iter()
        .map(encode_owner_value)
        .collect::<BTreeSet<_>>();
    DeltaOwnerPayloadValue::unordered(frame_value_set(b"owner-payload", encoded))
}

fn merge_values(domain: &[u8], values: BTreeSet<DeltaValue>) -> DeltaValue {
    if values.len() == 1 {
        return values.into_iter().next().unwrap_or(DeltaValue::Absent);
    }
    frame_value_set(domain, values.iter().map(encode_value).collect())
}

fn frame_value_set(domain: &[u8], values: BTreeSet<Vec<u8>>) -> DeltaValue {
    let mut merged = Vec::new();
    append_length_prefixed(&mut merged, b"lumin.delta.value-set.v1");
    append_length_prefixed(&mut merged, domain);
    for value in values {
        append_length_prefixed(&mut merged, &value);
    }
    DeltaValue::Bytes(merged)
}

fn encode_owner_value(value: &DeltaOwnerPayloadValue) -> Vec<u8> {
    let mut encoded = Vec::new();
    match value.rank {
        Some(rank) => {
            encoded.push(1);
            encoded.extend_from_slice(&rank.to_be_bytes());
        }
        None => encoded.push(0),
    }
    append_length_prefixed(&mut encoded, &encode_value(&value.value));
    encoded
}

fn encode_value(value: &DeltaValue) -> Vec<u8> {
    match value {
        DeltaValue::Absent => vec![0],
        DeltaValue::Bytes(bytes) => {
            let mut encoded = Vec::with_capacity(bytes.len() + 9);
            encoded.push(1);
            encoded.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
            encoded.extend_from_slice(bytes);
            encoded
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(value: &str) -> DeltaIdentity {
        DeltaIdentity {
            kind: DeltaIdentityKind::Target,
            canonical: value.as_bytes().to_vec(),
        }
    }

    fn fact() -> DeltaFact {
        DeltaFact {
            key: DeltaKey {
                owner_capability: "owner.v1".to_owned(),
                family: DeltaFactFamily::DeadExport,
                semantic_identity: b"fact".to_vec(),
            },
            targets: BTreeSet::from([identity("a"), identity("b")]),
            affected_identities: BTreeSet::new(),
            confidence: ConfidenceRank::High,
            grounding: GroundingRank::Grounded,
            evidence_identity: DeltaValue::text("evidence-a"),
            owner_payload: BTreeMap::new(),
        }
    }

    #[test]
    fn absent_and_equal_pairs_have_total_classifications() {
        let current = fact();
        assert_eq!(
            classify_lifecycle_deltas(Some(&[]), std::slice::from_ref(&current))[0].classification,
            GateDeltaClassification::Introduced
        );
        assert_eq!(
            classify_lifecycle_deltas(Some(std::slice::from_ref(&current)), &[])[0].classification,
            GateDeltaClassification::Resolved
        );
        assert_eq!(
            classify_lifecycle_deltas(
                Some(std::slice::from_ref(&current)),
                std::slice::from_ref(&current)
            )[0]
            .classification,
            GateDeltaClassification::Unchanged
        );
    }

    #[test]
    fn overlapping_target_sets_are_changed_incomparable() {
        let baseline = fact();
        let mut current = baseline.clone();
        current.targets = BTreeSet::from([identity("b"), identity("c")]);
        let classification = &classify_lifecycle_deltas(
            Some(std::slice::from_ref(&baseline)),
            std::slice::from_ref(&current),
        )[0]
        .classification;
        assert!(matches!(
            classification,
            GateDeltaClassification::ChangedIncomparable {
                regressions,
                improvements,
                incomparable_changes,
            } if matches!(regressions.as_slice(), [DeltaDimensionChange::TargetAdded { identity: added }] if added == &identity("c"))
                && matches!(improvements.as_slice(), [DeltaDimensionChange::TargetRemoved { identity: removed }] if removed == &identity("a"))
                && incomparable_changes.is_empty()
        ));
    }

    #[test]
    fn rank_loss_and_gain_cannot_cancel_each_other() {
        let mut baseline = fact();
        baseline.grounding = GroundingRank::Partial;
        let mut current = baseline.clone();
        current.confidence = ConfidenceRank::Medium;
        current.grounding = GroundingRank::Grounded;
        assert!(matches!(
            &classify_lifecycle_deltas(
                Some(std::slice::from_ref(&baseline)),
                std::slice::from_ref(&current)
            )[0]
                .classification,
            GateDeltaClassification::ChangedIncomparable {
                regressions,
                improvements,
                incomparable_changes,
            } if matches!(regressions.as_slice(), [DeltaDimensionChange::ConfidenceLowered { .. }])
                && matches!(improvements.as_slice(), [DeltaDimensionChange::GroundingRaised { .. }])
                && incomparable_changes.is_empty()
        ));
    }

    #[test]
    fn evidence_and_unordered_owner_payload_changes_are_incomparable() {
        let mut baseline = fact();
        baseline.owner_payload.insert(
            "disposition".to_owned(),
            DeltaOwnerPayloadValue::unordered(DeltaValue::text("review-candidate")),
        );
        let mut current = baseline.clone();
        current.evidence_identity = DeltaValue::text("evidence-b");
        current.owner_payload.insert(
            "disposition".to_owned(),
            DeltaOwnerPayloadValue::unordered(DeltaValue::text("review-only")),
        );
        assert!(matches!(
            &classify_lifecycle_deltas(
                Some(std::slice::from_ref(&baseline)),
                std::slice::from_ref(&current)
            )[0]
                .classification,
            GateDeltaClassification::ChangedIncomparable {
                regressions,
                improvements,
                incomparable_changes,
            } if regressions.is_empty()
                && improvements.is_empty()
                && incomparable_changes.len() == 2
        ));
    }

    #[test]
    fn ranked_owner_payload_has_direction() {
        let mut baseline = fact();
        baseline.owner_payload.insert(
            "owner-rank".to_owned(),
            DeltaOwnerPayloadValue::ranked(DeltaValue::text("high"), 3),
        );
        let mut current = baseline.clone();
        current.owner_payload.insert(
            "owner-rank".to_owned(),
            DeltaOwnerPayloadValue::ranked(DeltaValue::text("low"), 1),
        );
        assert!(matches!(
            &classify_lifecycle_deltas(
                Some(std::slice::from_ref(&baseline)),
                std::slice::from_ref(&current)
            )[0]
                .classification,
            GateDeltaClassification::Regressed { changes }
                if matches!(changes.as_slice(), [DeltaDimensionChange::OwnerPayloadRegressed { .. }])
        ));
    }

    #[test]
    fn unavailable_baseline_is_not_an_introduction_guess() {
        assert_eq!(
            classify_lifecycle_deltas(None, &[fact()])[0].classification,
            GateDeltaClassification::BaselineUnavailable
        );
    }

    #[test]
    fn exact_duplicate_rows_do_not_change_semantics() {
        let one = fact();
        let duplicate = vec![one.clone(), one.clone()];
        assert_eq!(
            classify_lifecycle_deltas(Some(&duplicate), std::slice::from_ref(&one))[0]
                .classification,
            GateDeltaClassification::Unchanged
        );
    }

    #[test]
    fn nonidentical_duplicate_rows_canonicalize_independent_of_order() {
        let mut first = fact();
        first.evidence_identity = DeltaValue::text("evidence-a");
        first.owner_payload.insert(
            "disposition".to_owned(),
            DeltaOwnerPayloadValue::unordered(DeltaValue::text("review-candidate")),
        );
        let mut second = fact();
        second.evidence_identity = DeltaValue::text("evidence-b");
        second.owner_payload.insert(
            "disposition".to_owned(),
            DeltaOwnerPayloadValue::unordered(DeltaValue::text("review-only")),
        );
        let mut third = fact();
        third.evidence_identity = DeltaValue::text("evidence-c");

        let baseline = vec![first.clone(), second.clone(), third.clone()];
        let current = vec![third, first, second];
        assert_eq!(
            classify_lifecycle_deltas(Some(&baseline), &current)[0].classification,
            GateDeltaClassification::Unchanged
        );
    }

    #[test]
    fn missing_duplicate_payload_is_preserved_as_a_semantic_value() {
        let missing = fact();
        let mut present = fact();
        present.owner_payload.insert(
            "disposition".to_owned(),
            DeltaOwnerPayloadValue::unordered(DeltaValue::text("review-candidate")),
        );

        assert!(matches!(
            &classify_lifecycle_deltas(
                Some(&[missing, present.clone()]),
                std::slice::from_ref(&present),
            )[0]
                .classification,
            GateDeltaClassification::ChangedIncomparable {
                regressions,
                improvements,
                incomparable_changes,
            } if regressions.is_empty()
                && improvements.is_empty()
                && matches!(
                    incomparable_changes.as_slice(),
                    [DeltaDimensionChange::OwnerPayloadChanged { field_id, .. }]
                        if field_id == "disposition"
                )
        ));
    }
}
