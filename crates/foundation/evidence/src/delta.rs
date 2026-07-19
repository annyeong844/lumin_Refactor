use std::collections::{BTreeMap, BTreeSet};

use lumin_model::{
    DeltaFact, DeltaFactFamily, DeltaIdentity, DeltaIdentityKind, DeltaKey, DeltaOwnerPayloadValue,
    DeltaValue, FindingDisposition, Limitation, LogicalSourceId, ReviewOnlyReason,
    append_length_prefixed,
};

use crate::{Confidence, FindingRecord, RunEvidence, Severity};

pub(crate) struct LifecycleDeltaInput {
    pub facts: Vec<DeltaFact>,
    pub advisory_limitation_count: usize,
    pub required_evidence_gap_count: usize,
}

pub(crate) fn lifecycle_delta_input(evidence: &RunEvidence) -> LifecycleDeltaInput {
    let mut facts = evidence
        .findings
        .iter()
        .map(finding_delta_fact)
        .collect::<Vec<_>>();
    let mut advisory_limitation_count = 0;
    let mut required_evidence_gap_count = 0;
    for limitation in &evidence.limitations {
        match limitation_delta(limitation) {
            LimitationDelta::Fact(fact) => {
                advisory_limitation_count += 1;
                facts.push(fact);
            }
            LimitationDelta::RequiredEvidenceGap => required_evidence_gap_count += 1,
        }
    }
    facts.sort_by(|left, right| left.key.cmp(&right.key));
    LifecycleDeltaInput {
        facts,
        advisory_limitation_count,
        required_evidence_gap_count,
    }
}

fn finding_delta_fact(finding: &FindingRecord) -> DeltaFact {
    let key = DeltaKey {
        owner_capability: finding.owner_capability.clone(),
        family: DeltaFactFamily::DeadExport,
        semantic_identity: frame([
            finding.finding_id.as_str().as_bytes(),
            finding.rule_id.as_bytes(),
        ]),
    };
    let targets = BTreeSet::from([DeltaIdentity {
        kind: DeltaIdentityKind::Target,
        canonical: finding.finding_id.as_str().as_bytes().to_vec(),
    }]);
    let affected_identities = BTreeSet::from([logical_source(&finding.source_id)]);
    let evidence_identity = DeltaValue::bytes(frame([
        finding.path.canonical.as_slice(),
        finding.span.start.to_be_bytes().as_slice(),
        finding.span.end.to_be_bytes().as_slice(),
    ]));
    let owner_payload = BTreeMap::from([
        (
            "disposition".to_owned(),
            DeltaOwnerPayloadValue::unordered(DeltaValue::bytes(disposition_bytes(
                &finding.disposition,
            ))),
        ),
        (
            "severity".to_owned(),
            DeltaOwnerPayloadValue::ranked(
                DeltaValue::text(severity_name(finding.severity)),
                i64::from(finding.severity.rank()),
            ),
        ),
    ]);

    DeltaFact {
        key,
        targets,
        affected_identities,
        confidence: match finding.confidence {
            Confidence::Grounded => lumin_model::ConfidenceRank::High,
        },
        grounding: lumin_model::GroundingRank::Grounded,
        evidence_identity,
        owner_payload,
    }
}

enum LimitationDelta {
    Fact(DeltaFact),
    RequiredEvidenceGap,
}

fn limitation_delta(limitation: &Limitation) -> LimitationDelta {
    match limitation {
        Limitation::InternalSpecifierUnresolved {
            importer,
            specifier,
            candidates,
        } if !candidates.is_empty() => {
            let semantic_identity = frame([
                importer.as_str().as_bytes(),
                b"module-request",
                specifier.as_bytes(),
            ]);
            LimitationDelta::Fact(DeltaFact {
                key: DeltaKey {
                    owner_capability: "resolve/module.v1".to_owned(),
                    family: DeltaFactFamily::UnresolvedInternalEdge,
                    semantic_identity: semantic_identity.clone(),
                },
                targets: candidates
                    .iter()
                    .map(|candidate| target(candidate.as_bytes()))
                    .collect(),
                affected_identities: BTreeSet::from([logical_source(importer)]),
                confidence: lumin_model::ConfidenceRank::High,
                grounding: lumin_model::GroundingRank::Grounded,
                evidence_identity: DeltaValue::bytes(semantic_identity),
                owner_payload: BTreeMap::new(),
            })
        }
        Limitation::InternalSpecifierUnresolved { .. }
        | Limitation::JsModuleUseUnknown { .. }
        | Limitation::SourcePayloadUnavailable { .. }
        | Limitation::PackageImportsUnsupported { .. }
        | Limitation::ImporterFormatUnsupported { .. }
        | Limitation::PublicSurfaceUnsupported { .. }
        | Limitation::TsconfigSemanticsUnsupported { .. }
        | Limitation::PackageDependencySemanticsUnsupported { .. }
        | Limitation::PackageIdentityUnsupported { .. }
        | Limitation::PackageMetadataUnobservable { .. }
        | Limitation::PackagePrivacyUnsupported { .. }
        | Limitation::DependencyOwnerAmbiguous { .. }
        | Limitation::WorkspaceOwnershipUnsupported { .. }
        | Limitation::PnpmDependencySemanticsUnsupported { .. }
        | Limitation::TsconfigPayloadUnavailable { .. }
        | Limitation::SfcDialectUnavailable { .. }
        | Limitation::SfcDecompositionUnknown { .. }
        | Limitation::SfcExternalScriptUnresolved { .. }
        | Limitation::VueExternalScriptModeConflict { .. }
        | Limitation::VueTemplateOpaque { .. } => LimitationDelta::RequiredEvidenceGap,
    }
}

fn logical_source(source_id: &LogicalSourceId) -> DeltaIdentity {
    DeltaIdentity {
        kind: DeltaIdentityKind::LogicalSource,
        canonical: source_id.as_str().as_bytes().to_vec(),
    }
}

fn target(canonical: &[u8]) -> DeltaIdentity {
    DeltaIdentity {
        kind: DeltaIdentityKind::Target,
        canonical: canonical.to_vec(),
    }
}

fn frame<'a>(parts: impl IntoIterator<Item = &'a [u8]>) -> Vec<u8> {
    let mut framed = Vec::new();
    for part in parts {
        append_length_prefixed(&mut framed, part);
    }
    framed
}

fn disposition_bytes(disposition: &FindingDisposition) -> Vec<u8> {
    match disposition {
        FindingDisposition::ReviewCandidate => vec![1],
        FindingDisposition::ReviewOnly { reason } => vec![
            2,
            match reason {
                ReviewOnlyReason::GeneratedSource => 1,
                ReviewOnlyReason::VendoredSource => 2,
                ReviewOnlyReason::GeneratedAndVendoredSource => 3,
            },
        ],
    }
}

fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Warning => "warning",
    }
}

#[cfg(test)]
mod tests {
    use lumin_model::{FindingId, SourceSpan, SymbolNamespace};

    use super::*;
    use crate::{DEAD_CODE_CAPABILITY_ID, DEAD_EXPORT_RULE_ID, RepoPathProjection};

    #[test]
    fn finding_disposition_is_payload_not_delta_key() {
        let finding = FindingRecord {
            finding_id: FindingId::from_string("finding-1".to_owned()),
            rule_id: DEAD_EXPORT_RULE_ID.to_owned(),
            owner_capability: DEAD_CODE_CAPABILITY_ID.to_owned(),
            severity: Severity::Warning,
            confidence: Confidence::Grounded,
            disposition: FindingDisposition::ReviewOnly {
                reason: ReviewOnlyReason::GeneratedSource,
            },
            claim: "claim".to_owned(),
            source_id: LogicalSourceId::from_string("source-1".to_owned()),
            path: RepoPathProjection {
                canonical: b"path".to_vec(),
                components: vec![b"path".to_vec()],
                display: "path".to_owned(),
            },
            span: SourceSpan { start: 1, end: 2 },
            exported_name: "dead".to_owned(),
            namespace: SymbolNamespace::Value,
        };
        let fact = finding_delta_fact(&finding);
        assert_eq!(fact.key.family, DeltaFactFamily::DeadExport);
        assert_eq!(
            fact.owner_payload["disposition"],
            DeltaOwnerPayloadValue::unordered(DeltaValue::bytes(vec![2, 1]))
        );
    }

    #[test]
    fn bounded_unresolved_targets_are_comparable_adverse_facts() -> Result<(), &'static str> {
        let delta = limitation_delta(&Limitation::InternalSpecifierUnresolved {
            importer: LogicalSourceId::from_string("source-1".to_owned()),
            specifier: "./missing".to_owned(),
            candidates: vec!["src/missing.ts".to_owned()],
        });
        let fact = match delta {
            LimitationDelta::Fact(fact) => fact,
            LimitationDelta::RequiredEvidenceGap => {
                return Err("bounded unresolved edge should produce a delta fact");
            }
        };
        assert_eq!(fact.key.family, DeltaFactFamily::UnresolvedInternalEdge);
        assert!(fact.key.family.blocks_when_adverse());
        assert_eq!(fact.targets.len(), 1);
        Ok(())
    }

    #[test]
    fn unbounded_or_unsupported_semantics_remain_required_evidence_gaps() {
        assert!(matches!(
            limitation_delta(&Limitation::InternalSpecifierUnresolved {
                importer: LogicalSourceId::from_string("source-1".to_owned()),
                specifier: "./missing".to_owned(),
                candidates: Vec::new(),
            }),
            LimitationDelta::RequiredEvidenceGap
        ));
        assert!(matches!(
            limitation_delta(&Limitation::TsconfigSemanticsUnsupported {
                path: "tsconfig.json".to_owned(),
                detail: "unsupported key".to_owned(),
            }),
            LimitationDelta::RequiredEvidenceGap
        ));
    }
}
