use std::collections::{BTreeMap, BTreeSet};

use lumin_model::{
    DeltaFact, DeltaFactFamily, DeltaIdentity, DeltaIdentityKind, DeltaKey, DeltaOwnerPayloadValue,
    DeltaValue, DynamicImportTargetScope, FindingDisposition, Limitation, LogicalSourceId,
    ReviewOnlyReason, SourceUnitId, UnresolvedTargetScope, append_length_prefixed,
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
    let evidence_identity = if !finding.nested_collections_available {
        DeltaValue::bytes(frame([
            b"lumin-finding-nested-evidence-unavailable.v1".as_slice(),
            finding.path.canonical.as_slice(),
            finding.span.start.to_be_bytes().as_slice(),
            finding.span.end.to_be_bytes().as_slice(),
        ]))
    } else if finding.evidence.is_empty() && finding.relations.is_empty() {
        DeltaValue::bytes(frame([
            finding.path.canonical.as_slice(),
            finding.span.start.to_be_bytes().as_slice(),
            finding.span.end.to_be_bytes().as_slice(),
        ]))
    } else {
        let mut identity = Vec::new();
        append_length_prefixed(&mut identity, b"lumin-finding-evidence-set.v1");
        for evidence in &finding.evidence {
            append_length_prefixed(&mut identity, evidence.evidence_id.as_str().as_bytes());
        }
        for relation in &finding.relations {
            append_length_prefixed(&mut identity, relation.relation_id.as_str().as_bytes());
        }
        DeltaValue::bytes(identity)
    };
    let owner_payload = BTreeMap::from([
        (
            "claim".to_owned(),
            DeltaOwnerPayloadValue::unordered(DeltaValue::text(&finding.claim)),
        ),
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
            target_scope,
        } => {
            let owner_payload = match target_scope {
                Some(UnresolvedTargetScope::KnownNoTarget { package }) if candidates.is_empty() => {
                    BTreeMap::from([
                        (
                            "targetScope".to_owned(),
                            DeltaOwnerPayloadValue::unordered(DeltaValue::text("known-no-target")),
                        ),
                        (
                            "package".to_owned(),
                            DeltaOwnerPayloadValue::unordered(DeltaValue::text(package)),
                        ),
                    ])
                }
                Some(UnresolvedTargetScope::ExplicitTargets) | None if !candidates.is_empty() => {
                    BTreeMap::new()
                }
                Some(UnresolvedTargetScope::OpaqueWorkspace)
                | Some(UnresolvedTargetScope::KnownNoTarget { .. })
                | Some(UnresolvedTargetScope::ExplicitTargets)
                | None => return LimitationDelta::RequiredEvidenceGap,
            };
            let normalized_specifier = normalized_unresolved_specifier(specifier);
            let semantic_identity = frame([
                importer.as_str().as_bytes(),
                b"module-request",
                normalized_specifier.as_bytes(),
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
                owner_payload,
            })
        }
        Limitation::DynamicImportNonLiteral {
            source_id,
            source_unit,
            span,
            static_prefix,
            candidates,
            target_scope,
        } => {
            if *target_scope == DynamicImportTargetScope::Workspace {
                return LimitationDelta::RequiredEvidenceGap;
            }
            let semantic_identity = frame([
                source_id.as_str().as_bytes(),
                source_unit_identity(source_unit).as_slice(),
                b"dynamic-import",
                span.start.to_be_bytes().as_slice(),
                span.end.to_be_bytes().as_slice(),
            ]);
            LimitationDelta::Fact(DeltaFact {
                key: DeltaKey {
                    owner_capability: "js/module-use.v1".to_owned(),
                    family: DeltaFactFamily::Opacity,
                    semantic_identity: semantic_identity.clone(),
                },
                targets: candidates
                    .iter()
                    .map(|candidate| target(candidate.as_str().as_bytes()))
                    .collect(),
                affected_identities: BTreeSet::from([logical_source(source_id)]),
                confidence: lumin_model::ConfidenceRank::High,
                grounding: lumin_model::GroundingRank::Opaque,
                evidence_identity: DeltaValue::bytes(semantic_identity),
                owner_payload: BTreeMap::from([
                    (
                        "staticPrefix".to_owned(),
                        DeltaOwnerPayloadValue::unordered(match static_prefix {
                            Some(prefix) => DeltaValue::text(prefix),
                            None => DeltaValue::Absent,
                        }),
                    ),
                    (
                        "targetScope".to_owned(),
                        DeltaOwnerPayloadValue::unordered(DeltaValue::text("explicit-targets")),
                    ),
                ]),
            })
        }
        Limitation::JsModuleUseUnknown { .. }
        | Limitation::SourcePayloadUnavailable { .. }
        | Limitation::PackageImportsUnsupported { .. }
        | Limitation::AliasShapeUnsupported { .. }
        | Limitation::AbsoluteInternalSpecifierUnsupported { .. }
        | Limitation::ImporterFormatUnsupported { .. }
        | Limitation::PublicSurfaceUnsupported { .. }
        | Limitation::TsconfigSemanticsUnsupported { .. }
        | Limitation::PackageIdentityUnsupported { .. }
        | Limitation::PackageMetadataUnobservable { .. }
        | Limitation::PackagePrivacyUnsupported { .. }
        | Limitation::DependencyOwnerAmbiguous { .. }
        | Limitation::WorkspaceOwnershipUnsupported { .. }
        | Limitation::PnpmDependencySemanticsUnsupported { .. }
        | Limitation::TsconfigPayloadUnavailable { .. }
        | Limitation::SfcDialectUnavailable { .. }
        | Limitation::SfcDecompositionUnknown { .. }
        | Limitation::VueExternalScriptModeConflict { .. }
        | Limitation::VueTemplateOpaque { .. }
        | Limitation::ExplicitEntryUnavailable { .. } => LimitationDelta::RequiredEvidenceGap,
    }
}

fn normalized_unresolved_specifier(specifier: &str) -> &str {
    if (specifier.starts_with("./") || specifier.starts_with("../"))
        && let Some(stem) = specifier.strip_suffix(".js")
    {
        stem
    } else {
        specifier
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

fn source_unit_identity(source_unit: &SourceUnitId) -> Vec<u8> {
    match source_unit {
        SourceUnitId::Logical(source_id) => {
            frame([b"logical".as_slice(), source_id.as_str().as_bytes()])
        }
        SourceUnitId::Embedded(unit_id) => {
            frame([b"embedded".as_slice(), unit_id.as_str().as_bytes()])
        }
    }
}

#[cfg(test)]
mod tests {
    use lumin_model::{
        DeltaDimensionChange, FindingId, GateDeltaClassification, SourceSpan, SymbolNamespace,
        classify_lifecycle_deltas,
    };

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
            nested_collections_available: true,
            evidence: Vec::new(),
            relations: Vec::new(),
        };
        let fact = finding_delta_fact(&finding);
        assert_eq!(fact.key.family, DeltaFactFamily::DeadExport);
        assert_eq!(
            fact.owner_payload["disposition"],
            DeltaOwnerPayloadValue::unordered(DeltaValue::bytes(vec![2, 1]))
        );
    }

    #[test]
    fn finding_claim_change_is_an_incomparable_payload_delta() {
        let baseline = finding("no exact production fan-in");
        let current = finding("consumed only from test-like sources");
        let baseline_fact = finding_delta_fact(&baseline);
        let current_fact = finding_delta_fact(&current);

        assert_eq!(baseline_fact.key, current_fact.key);
        assert!(matches!(
            &classify_lifecycle_deltas(
                Some(std::slice::from_ref(&baseline_fact)),
                std::slice::from_ref(&current_fact),
            )[0]
                .classification,
            GateDeltaClassification::ChangedIncomparable {
                incomparable_changes,
                ..
            } if matches!(
                incomparable_changes.as_slice(),
                [DeltaDimensionChange::OwnerPayloadChanged { field_id, .. }]
                    if field_id == "claim"
            )
        ));
    }

    #[test]
    fn bounded_unresolved_targets_are_comparable_adverse_facts() -> Result<(), &'static str> {
        let delta = limitation_delta(&Limitation::InternalSpecifierUnresolved {
            importer: LogicalSourceId::from_string("source-1".to_owned()),
            specifier: "./missing".to_owned(),
            candidates: vec!["src/missing.ts".to_owned()],
            target_scope: Some(UnresolvedTargetScope::ExplicitTargets),
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
    fn bounded_dynamic_import_is_a_comparable_opacity_fact() -> Result<(), &'static str> {
        let candidate = LogicalSourceId::from_string("source-candidate".to_owned());
        let delta = limitation_delta(&Limitation::DynamicImportNonLiteral {
            source_id: LogicalSourceId::from_string("source-importer".to_owned()),
            source_unit: SourceUnitId::Logical(LogicalSourceId::from_string(
                "source-importer".to_owned(),
            )),
            span: SourceSpan { start: 7, end: 30 },
            static_prefix: Some("./features/".to_owned()),
            candidates: vec![candidate.clone()],
            target_scope: DynamicImportTargetScope::ExplicitTargets,
        });
        let fact = match delta {
            LimitationDelta::Fact(fact) => fact,
            LimitationDelta::RequiredEvidenceGap => {
                return Err("bounded dynamic import should produce an opacity fact");
            }
        };
        assert_eq!(fact.key.family, DeltaFactFamily::Opacity);
        assert!(!fact.key.family.blocks_when_adverse());
        assert_eq!(fact.grounding, lumin_model::GroundingRank::Opaque);
        assert_eq!(fact.targets.len(), 1);
        assert!(
            fact.targets
                .iter()
                .any(|target| { target.canonical == candidate.as_str().as_bytes() })
        );
        Ok(())
    }

    #[test]
    fn embedded_dynamic_imports_with_equal_spans_have_distinct_keys() -> Result<(), &'static str> {
        let fact = |unit: &str| {
            limitation_delta(&Limitation::DynamicImportNonLiteral {
                source_id: LogicalSourceId::from_string("source-parent".to_owned()),
                source_unit: SourceUnitId::Embedded(
                    lumin_model::EmbeddedSourceUnitId::from_string(unit.to_owned()),
                ),
                span: SourceSpan { start: 3, end: 20 },
                static_prefix: Some("./features/".to_owned()),
                candidates: Vec::new(),
                target_scope: DynamicImportTargetScope::ExplicitTargets,
            })
        };
        let first = match fact("embedded-first") {
            LimitationDelta::Fact(fact) => fact,
            LimitationDelta::RequiredEvidenceGap => return Err("first opacity was not a fact"),
        };
        let second = match fact("embedded-second") {
            LimitationDelta::Fact(fact) => fact,
            LimitationDelta::RequiredEvidenceGap => return Err("second opacity was not a fact"),
        };
        assert_ne!(first.key, second.key);
        Ok(())
    }

    #[test]
    fn unbounded_dynamic_import_remains_a_required_evidence_gap() {
        assert!(matches!(
            limitation_delta(&Limitation::DynamicImportNonLiteral {
                source_id: LogicalSourceId::from_string("source-importer".to_owned()),
                source_unit: SourceUnitId::Logical(LogicalSourceId::from_string(
                    "source-importer".to_owned(),
                )),
                span: SourceSpan { start: 7, end: 20 },
                static_prefix: None,
                candidates: Vec::new(),
                target_scope: DynamicImportTargetScope::Workspace,
            }),
            LimitationDelta::RequiredEvidenceGap
        ));
    }

    #[test]
    fn equivalent_extensionless_and_js_requests_share_one_delta_key() -> Result<(), &'static str> {
        let extensionless = bounded_unresolved_fact("./missing")?;
        let explicit_js = bounded_unresolved_fact("./missing.js")?;
        let explicit_mjs = bounded_unresolved_fact("./missing.mjs")?;

        assert_eq!(extensionless.key, explicit_js.key);
        assert_ne!(extensionless.key, explicit_mjs.key);
        assert_eq!(
            classify_lifecycle_deltas(
                Some(std::slice::from_ref(&extensionless)),
                std::slice::from_ref(&explicit_js),
            )[0]
            .classification,
            GateDeltaClassification::Unchanged
        );
        Ok(())
    }

    #[test]
    fn unbounded_or_unsupported_semantics_remain_required_evidence_gaps() {
        assert!(matches!(
            limitation_delta(&Limitation::InternalSpecifierUnresolved {
                importer: LogicalSourceId::from_string("source-1".to_owned()),
                specifier: "./missing".to_owned(),
                candidates: Vec::new(),
                target_scope: Some(UnresolvedTargetScope::OpaqueWorkspace),
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

    #[test]
    fn known_no_target_is_a_complete_unresolved_fact() -> Result<(), &'static str> {
        let delta = limitation_delta(&Limitation::InternalSpecifierUnresolved {
            importer: LogicalSourceId::from_string("source-1".to_owned()),
            specifier: "@app/ui/blocked".to_owned(),
            candidates: Vec::new(),
            target_scope: Some(UnresolvedTargetScope::KnownNoTarget {
                package: "@app/ui".to_owned(),
            }),
        });
        let fact = match delta {
            LimitationDelta::Fact(fact) => fact,
            LimitationDelta::RequiredEvidenceGap => {
                return Err("known closed export should produce a delta fact");
            }
        };
        assert!(fact.targets.is_empty());
        assert_eq!(
            fact.owner_payload.get("package").map(|value| &value.value),
            Some(&DeltaValue::text("@app/ui"))
        );
        Ok(())
    }

    fn finding(claim: &str) -> FindingRecord {
        FindingRecord {
            finding_id: FindingId::from_string("finding-1".to_owned()),
            rule_id: DEAD_EXPORT_RULE_ID.to_owned(),
            owner_capability: DEAD_CODE_CAPABILITY_ID.to_owned(),
            severity: Severity::Warning,
            confidence: Confidence::Grounded,
            disposition: FindingDisposition::ReviewCandidate,
            claim: claim.to_owned(),
            source_id: LogicalSourceId::from_string("source-1".to_owned()),
            path: RepoPathProjection {
                canonical: b"path".to_vec(),
                components: vec![b"path".to_vec()],
                display: "path".to_owned(),
            },
            span: SourceSpan { start: 1, end: 2 },
            exported_name: "dead".to_owned(),
            namespace: SymbolNamespace::Value,
            nested_collections_available: true,
            evidence: Vec::new(),
            relations: Vec::new(),
        }
    }

    fn bounded_unresolved_fact(specifier: &str) -> Result<DeltaFact, &'static str> {
        match limitation_delta(&Limitation::InternalSpecifierUnresolved {
            importer: LogicalSourceId::from_string("source-1".to_owned()),
            specifier: specifier.to_owned(),
            candidates: vec![
                "src/missing.ts".to_owned(),
                "src/missing.tsx".to_owned(),
                "src/missing.js".to_owned(),
                "src/missing.jsx".to_owned(),
            ],
            target_scope: Some(UnresolvedTargetScope::ExplicitTargets),
        }) {
            LimitationDelta::Fact(fact) => Ok(fact),
            LimitationDelta::RequiredEvidenceGap => Err("bounded unresolved edge was not a fact"),
        }
    }
}
