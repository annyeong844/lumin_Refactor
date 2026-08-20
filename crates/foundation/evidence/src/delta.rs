use std::collections::{BTreeMap, BTreeSet};

use lumin_model::{
    DeltaFact, DeltaFactFamily, DeltaIdentity, DeltaIdentityKind, DeltaKey, DeltaOwnerPayloadValue,
    DeltaValue, DependencyIntentIdentity, DynamicImportTargetScope, FindingDisposition,
    ImportMetaGlobTargetScope, Limitation, LogicalSourceId, PackageScopeId, RepoPath,
    ReviewOnlyReason, UnresolvedTargetScope, append_length_prefixed,
};

use crate::{
    Confidence, DEPENDENCY_OWNERSHIP_CAPABILITY_ID, DependencyIntentRecord, DependencyOwnerRecord,
    FindingRecord, RepoPathProjection, RunEvidence, Severity, WriteLease, WriteLeaseKind,
};

pub(crate) struct LifecycleDeltaInput {
    pub facts: Vec<DeltaFact>,
    pub advisory_limitation_count: usize,
    pub required_evidence_gap_count: usize,
}

#[cfg(test)]
fn lifecycle_delta_input(evidence: &RunEvidence) -> LifecycleDeltaInput {
    lifecycle_delta_input_for(evidence, &[], &[])
}

pub(crate) fn lifecycle_delta_input_for(
    evidence: &RunEvidence,
    dependency_intents: &[DependencyIntentRecord],
    leased_write_set: &[WriteLease],
) -> LifecycleDeltaInput {
    let mut facts = evidence
        .findings
        .iter()
        .map(finding_delta_fact)
        .collect::<Vec<_>>();
    facts.extend(
        evidence
            .dependency_owners
            .iter()
            .map(dependency_owner_delta_fact),
    );
    let mut advisory_limitation_count = 0;
    let mut required_evidence_gap_count = 0;
    let mut dynamic_import_occurrences = BTreeMap::<(LogicalSourceId, Option<String>), u64>::new();
    let mut import_meta_glob_occurrences = BTreeMap::<(LogicalSourceId, Vec<String>), u64>::new();
    let mut commonjs_computed_occurrences = BTreeMap::<(LogicalSourceId, String), u64>::new();
    for limitation in &evidence.limitations {
        let construct_ordinal = match limitation {
            Limitation::DynamicImportNonLiteral {
                source_id,
                static_prefix,
                target_scope: DynamicImportTargetScope::ExplicitTargets,
                ..
            } => {
                let next = dynamic_import_occurrences
                    .entry((source_id.clone(), static_prefix.clone()))
                    .or_default();
                let ordinal = *next;
                *next += 1;
                ordinal
            }
            Limitation::ImportMetaGlobUnsupported {
                source_id,
                patterns,
                target_scope: ImportMetaGlobTargetScope::ExplicitTargets,
                ..
            } => {
                let next = import_meta_glob_occurrences
                    .entry((source_id.clone(), patterns.to_vec()))
                    .or_default();
                let ordinal = *next;
                *next += 1;
                ordinal
            }
            Limitation::CommonJsComputedMember {
                source_id,
                specifier,
                ..
            } => {
                let next = commonjs_computed_occurrences
                    .entry((source_id.clone(), specifier.clone()))
                    .or_default();
                let ordinal = *next;
                *next += 1;
                ordinal
            }
            _ => 0,
        };
        match limitation_delta_at(limitation, construct_ordinal) {
            LimitationDelta::Fact(fact) => {
                advisory_limitation_count += 1;
                facts.push(fact);
            }
            LimitationDelta::RequiredEvidenceGap => {
                if limitation_intersects_required_evidence(
                    limitation,
                    evidence,
                    dependency_intents,
                    leased_write_set,
                ) {
                    required_evidence_gap_count += 1;
                }
            }
        }
    }
    facts.sort_by(|left, right| left.key.cmp(&right.key));
    LifecycleDeltaInput {
        facts,
        advisory_limitation_count,
        required_evidence_gap_count,
    }
}

fn limitation_intersects_required_evidence(
    limitation: &Limitation,
    evidence: &RunEvidence,
    dependency_intents: &[DependencyIntentRecord],
    leased_write_set: &[WriteLease],
) -> bool {
    if let Limitation::JsRecoverableParseLocal { source_id, .. } = limitation {
        let Some(path) = evidence
            .source_contexts
            .iter()
            .find(|context| &context.source_id == source_id)
            .map(|context| &context.path)
        else {
            return true;
        };
        return leased_write_set.iter().any(|lease| lease.covers(path));
    }
    if let Limitation::ImportMetaGlobUnsupported {
        source_id,
        target_scope: ImportMetaGlobTargetScope::Package,
        ..
    } = limitation
    {
        let Some(root) = evidence
            .source_contexts
            .iter()
            .find(|context| &context.source_id == source_id)
            .and_then(|context| context.package_root.as_ref())
            .and_then(|root| RepoPath::from_canonical_bytes(&root.canonical).ok())
        else {
            return true;
        };
        return package_scope_intersects_write_set(
            &lumin_model::PackageScope::from_root(&root),
            evidence,
            leased_write_set,
        );
    }
    let Limitation::DependencyOwnerAmbiguous {
        package_scope,
        required_intent,
        ..
    } = limitation
    else {
        return true;
    };
    if let Some(required_intent) = required_intent {
        return dependency_intents
            .iter()
            .any(|intent| dependency_intent_matches(required_intent, intent));
    }
    let Some(package_scope) = package_scope else {
        return true;
    };
    dependency_intents.iter().any(|intent| {
        let Some(owner) = evidence.dependency_owners.iter().find(|owner| {
            owner.consumer_path == intent.path && owner.dependency == intent.dependency
        }) else {
            return false;
        };
        projected_package_scope(&owner.package_root)
            .is_none_or(|scope| &scope == package_scope.id())
    }) || package_scope_intersects_write_set(package_scope, evidence, leased_write_set)
}

fn package_scope_intersects_write_set(
    package_scope: &lumin_model::PackageScope,
    evidence: &RunEvidence,
    leased_write_set: &[WriteLease],
) -> bool {
    let Some(scope_root) = RepoPath::from_canonical_bytes(package_scope.canonical_root())
        .ok()
        .map(|root| RepoPathProjection::from(&root))
    else {
        return true;
    };
    leased_write_set.iter().any(|lease| {
        if lease.kind == WriteLeaseKind::Directory
            && scope_root.components.starts_with(&lease.path.components)
        {
            return true;
        }
        if !lease.path.components.starts_with(&scope_root.components) {
            return false;
        }
        match nearest_known_package_root(evidence, &lease.path) {
            Some(root)
                if projected_package_scope(root)
                    .is_some_and(|scope| &scope == package_scope.id()) =>
            {
                true
            }
            Some(root)
                if root.components.len() > scope_root.components.len()
                    && root.components.starts_with(&scope_root.components) =>
            {
                false
            }
            _ => true,
        }
    })
}

fn nearest_known_package_root<'a>(
    evidence: &'a RunEvidence,
    path: &RepoPathProjection,
) -> Option<&'a RepoPathProjection> {
    evidence
        .source_contexts
        .iter()
        .filter_map(|context| context.package_root.as_ref())
        .filter(|root| path.components.starts_with(&root.components))
        .max_by_key(|root| root.components.len())
}

fn dependency_intent_matches(
    required: &DependencyIntentIdentity,
    actual: &DependencyIntentRecord,
) -> bool {
    actual.dependency == required.dependency
        && projected_source_id(&actual.path).is_none_or(|consumer| consumer == required.consumer)
}

fn projected_source_id(path: &RepoPathProjection) -> Option<LogicalSourceId> {
    RepoPath::from_canonical_bytes(&path.canonical)
        .ok()
        .map(|path| LogicalSourceId::from_path(&path))
}

fn projected_package_scope(path: &RepoPathProjection) -> Option<PackageScopeId> {
    RepoPath::from_canonical_bytes(&path.canonical)
        .ok()
        .map(|path| PackageScopeId::from_root(&path))
}

fn dependency_owner_delta_fact(owner: &DependencyOwnerRecord) -> DeltaFact {
    let owner_payload = BTreeMap::from([
        (
            "packageRoot".to_owned(),
            DeltaOwnerPayloadValue::unordered(DeltaValue::bytes(
                owner.package_root.canonical.clone(),
            )),
        ),
        (
            "manifestPath".to_owned(),
            DeltaOwnerPayloadValue::unordered(DeltaValue::bytes(
                owner.manifest_path.canonical.clone(),
            )),
        ),
        (
            "manifestPayloadSha256".to_owned(),
            DeltaOwnerPayloadValue::unordered(DeltaValue::text(&owner.manifest_payload_sha256)),
        ),
        (
            "lockfilePath".to_owned(),
            DeltaOwnerPayloadValue::unordered(
                owner
                    .lockfile_path
                    .as_ref()
                    .map_or(DeltaValue::Absent, |path| {
                        DeltaValue::bytes(path.canonical.clone())
                    }),
            ),
        ),
    ]);
    DeltaFact {
        key: DeltaKey {
            owner_capability: DEPENDENCY_OWNERSHIP_CAPABILITY_ID.to_owned(),
            family: DeltaFactFamily::DependencyOwnership,
            semantic_identity: frame([
                owner.consumer.as_str().as_bytes(),
                owner.dependency.as_bytes(),
            ]),
        },
        targets: BTreeSet::new(),
        affected_identities: BTreeSet::from([logical_source(&owner.consumer)]),
        confidence: lumin_model::ConfidenceRank::High,
        grounding: lumin_model::GroundingRank::Grounded,
        evidence_identity: DeltaValue::bytes(frame([
            owner.consumer_path.canonical.as_slice(),
            owner.dependency.as_bytes(),
        ])),
        owner_payload,
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

#[cfg(test)]
fn limitation_delta(limitation: &Limitation) -> LimitationDelta {
    limitation_delta_at(limitation, 0)
}

fn limitation_delta_at(limitation: &Limitation, construct_ordinal: u64) -> LimitationDelta {
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
            static_prefix,
            candidates,
            target_scope,
            ..
        } => {
            if matches!(
                target_scope,
                DynamicImportTargetScope::SourceInventory | DynamicImportTargetScope::Workspace
            ) {
                return LimitationDelta::RequiredEvidenceGap;
            }
            let normalized_prefix = static_prefix.as_deref().unwrap_or_default();
            let ordinal = construct_ordinal.to_be_bytes();
            let semantic_identity = frame([
                source_id.as_str().as_bytes(),
                b"dynamic-import-opacity.v2",
                normalized_prefix.as_bytes(),
                ordinal.as_slice(),
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
        Limitation::ImportMetaGlobUnsupported {
            source_id,
            patterns,
            candidates,
            target_scope,
            detail,
            ..
        } => {
            if *target_scope == ImportMetaGlobTargetScope::Package {
                return LimitationDelta::RequiredEvidenceGap;
            }
            let normalized_patterns = frame(patterns.iter().map(|pattern| pattern.as_bytes()));
            let ordinal = construct_ordinal.to_be_bytes();
            let semantic_identity = frame([
                source_id.as_str().as_bytes(),
                b"import-meta-glob-opacity.v1",
                normalized_patterns.as_slice(),
                ordinal.as_slice(),
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
                        "patterns".to_owned(),
                        DeltaOwnerPayloadValue::unordered(DeltaValue::bytes(normalized_patterns)),
                    ),
                    (
                        "detail".to_owned(),
                        DeltaOwnerPayloadValue::unordered(DeltaValue::text(detail)),
                    ),
                    (
                        "targetScope".to_owned(),
                        DeltaOwnerPayloadValue::unordered(DeltaValue::text("explicit-targets")),
                    ),
                ]),
            })
        }
        Limitation::CommonJsComputedMember {
            source_id,
            specifier,
            target: resolved_target,
            ..
        } => {
            let ordinal = construct_ordinal.to_be_bytes();
            let semantic_identity = frame([
                source_id.as_str().as_bytes(),
                b"commonjs-computed-opacity.v1",
                specifier.as_bytes(),
                ordinal.as_slice(),
            ]);
            LimitationDelta::Fact(DeltaFact {
                key: DeltaKey {
                    owner_capability: "js/module-use.v1".to_owned(),
                    family: DeltaFactFamily::Opacity,
                    semantic_identity: semantic_identity.clone(),
                },
                targets: BTreeSet::from([target(resolved_target.as_str().as_bytes())]),
                affected_identities: BTreeSet::from([logical_source(source_id)]),
                confidence: lumin_model::ConfidenceRank::High,
                grounding: lumin_model::GroundingRank::Opaque,
                evidence_identity: DeltaValue::bytes(semantic_identity),
                owner_payload: BTreeMap::from([
                    (
                        "specifier".to_owned(),
                        DeltaOwnerPayloadValue::unordered(DeltaValue::text(specifier)),
                    ),
                    (
                        "targetScope".to_owned(),
                        DeltaOwnerPayloadValue::unordered(DeltaValue::text("resolved-module")),
                    ),
                ]),
            })
        }
        Limitation::JsRecoverableParseLocal { .. }
        | Limitation::JsModuleUseUnknown { .. }
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

#[cfg(test)]
mod tests {
    use lumin_model::{
        DeltaDimensionChange, FindingId, GateDeltaClassification, SourceKind, SourceSpan,
        SourceUnitId, SymbolNamespace, classify_lifecycle_deltas,
    };

    use super::*;
    use crate::{
        DEAD_CODE_CAPABILITY_ID, DEAD_EXPORT_RULE_ID, RepoPathProjection, SourceContextRecord,
        WriteLease, WriteLeaseKind,
    };

    fn evidence_with_limitations(limitations: Vec<Limitation>) -> RunEvidence {
        RunEvidence {
            schema_version: "lumin-evidence.v1".to_owned(),
            capabilities: Vec::new(),
            resolution_profiles: Vec::new(),
            source_classifications: Vec::new(),
            source_contexts: Vec::new(),
            source_observations: Vec::new(),
            dependency_owners: Vec::new(),
            resolutions: Vec::new(),
            metrics: Default::default(),
            findings: Vec::new(),
            limitations,
        }
    }

    fn bounded_dynamic_limitation(unit: &str, start: u32) -> Limitation {
        Limitation::DynamicImportNonLiteral {
            source_id: LogicalSourceId::from_string("source-parent".to_owned()),
            source_unit: SourceUnitId::Embedded(lumin_model::EmbeddedSourceUnitId::from_string(
                unit.to_owned(),
            )),
            span: SourceSpan {
                start,
                end: start + 17,
            },
            static_prefix: Some("./features/".to_owned()),
            candidates: Vec::new(),
            target_scope: DynamicImportTargetScope::ExplicitTargets,
        }
    }

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
    fn dependency_owner_manifest_change_is_an_incomparable_payload_delta()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut baseline = evidence_with_limitations(Vec::new());
        baseline.dependency_owners = vec![dependency_owner("package.json", "baseline-hash")?];
        let mut current = evidence_with_limitations(Vec::new());
        current.dependency_owners = vec![dependency_owner(
            "packages/app/package.json",
            "current-hash",
        )?];

        let baseline = lifecycle_delta_input(&baseline);
        let current = lifecycle_delta_input(&current);
        assert_eq!(baseline.facts.len(), 1);
        assert_eq!(current.facts.len(), 1);
        assert_eq!(
            baseline.facts[0].key.family,
            DeltaFactFamily::DependencyOwnership
        );
        assert_eq!(baseline.facts[0].key, current.facts[0].key);
        assert!(matches!(
            &classify_lifecycle_deltas(Some(&baseline.facts), &current.facts)[0].classification,
            GateDeltaClassification::ChangedIncomparable {
                incomparable_changes,
                ..
            } if incomparable_changes.iter().any(|change| matches!(
                change,
                DeltaDimensionChange::OwnerPayloadChanged { field_id, .. }
                    if field_id == "manifestPath"
            ))
        ));
        Ok(())
    }

    #[test]
    fn dependency_owner_manifest_payload_change_is_incomparable_at_the_same_path()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut baseline = evidence_with_limitations(Vec::new());
        baseline.dependency_owners = vec![dependency_owner(
            "packages/app/package.json",
            "baseline-hash",
        )?];
        let mut current = evidence_with_limitations(Vec::new());
        current.dependency_owners = vec![dependency_owner(
            "packages/app/package.json",
            "current-hash",
        )?];

        let baseline = lifecycle_delta_input(&baseline);
        let current = lifecycle_delta_input(&current);
        assert!(matches!(
            &classify_lifecycle_deltas(Some(&baseline.facts), &current.facts)[0].classification,
            GateDeltaClassification::ChangedIncomparable {
                incomparable_changes,
                ..
            } if incomparable_changes.iter().any(|change| matches!(
                change,
                DeltaDimensionChange::OwnerPayloadChanged { field_id, .. }
                    if field_id == "manifestPayloadSha256"
            ))
        ));
        Ok(())
    }

    #[test]
    fn dependency_owner_gaps_count_only_for_the_requested_owner_scope()
    -> Result<(), Box<dyn std::error::Error>> {
        let package_b = lumin_model::RepoPath::from_portable("packages/b")?;
        let mut evidence = evidence_with_limitations(vec![Limitation::DependencyOwnerAmbiguous {
            path: "packages/b/package.json".to_owned(),
            package_scope: Some(Box::new(lumin_model::PackageScope::from_root(&package_b))),
            required_intent: None,
            detail: "malformed dependencies".to_owned(),
        }]);
        evidence.dependency_owners = vec![dependency_owner(
            "packages/app/package.json",
            "manifest-hash",
        )?];
        let app_intent = dependency_intent("packages/app/src/main.ts", "zod")?;

        assert_eq!(
            lifecycle_delta_input_for(&evidence, std::slice::from_ref(&app_intent), &[])
                .required_evidence_gap_count,
            0,
            "an unrelated package gap blocked a resolved dependency owner",
        );

        let package_b_intent = dependency_intent("packages/b/src/main.ts", "zod")?;
        evidence
            .limitations
            .push(Limitation::DependencyOwnerAmbiguous {
                path: "packages/b/src/main.ts".to_owned(),
                package_scope: Some(Box::new(lumin_model::PackageScope::from_root(&package_b))),
                required_intent: Some(Box::new(DependencyIntentIdentity {
                    consumer: projected_source_id(&package_b_intent.path)
                        .ok_or("invalid package B intent path")?,
                    dependency: package_b_intent.dependency.clone(),
                })),
                detail: "selected package dependency ownership is unsupported".to_owned(),
            });
        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[package_b_intent], &[])
                .required_evidence_gap_count,
            1,
            "the requested package's owner gap was ignored",
        );

        evidence.limitations = vec![Limitation::DependencyOwnerAmbiguous {
            path: "package.json".to_owned(),
            package_scope: Some(Box::new(lumin_model::PackageScope::from_root(
                &lumin_model::RepoPath::empty(),
            ))),
            required_intent: None,
            detail: "malformed root dependencies".to_owned(),
        }];
        assert_eq!(
            lifecycle_delta_input_for(&evidence, std::slice::from_ref(&app_intent), &[])
                .required_evidence_gap_count,
            0,
            "a resolved nested owner inherited its ancestor package's gap",
        );

        evidence.limitations = vec![Limitation::DependencyOwnerAmbiguous {
            path: "packages/unknown".to_owned(),
            package_scope: None,
            required_intent: None,
            detail: "package ownership is ambiguous".to_owned(),
        }];
        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[app_intent], &[]).required_evidence_gap_count,
            1,
            "a workspace-scoped owner gap must remain fail-closed",
        );
        Ok(())
    }

    #[test]
    fn dependency_owner_gap_intersects_declared_writes_in_its_package()
    -> Result<(), Box<dyn std::error::Error>> {
        let package_a = RepoPath::from_portable("packages/a")?;
        let package_b = RepoPath::from_portable("packages/b")?;
        let source_a = RepoPath::from_portable("packages/a/src/main.ts")?;
        let source_b = RepoPath::from_portable("packages/b/src/main.ts")?;
        let mut evidence = evidence_with_limitations(vec![Limitation::DependencyOwnerAmbiguous {
            path: "packages/b/package.json".to_owned(),
            package_scope: Some(Box::new(lumin_model::PackageScope::from_root(&package_b))),
            required_intent: None,
            detail: "malformed dependencies".to_owned(),
        }]);
        evidence.source_contexts = vec![
            source_context(&source_a, &package_a),
            source_context(&source_b, &package_b),
        ];
        let lease_a = existing_file_lease(&source_a);
        let lease_b = existing_file_lease(&source_b);

        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[lease_a]).required_evidence_gap_count,
            0,
            "an unrelated package gap blocked an ordinary source write",
        );
        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[lease_b]).required_evidence_gap_count,
            1,
            "the written package's owner gap was discarded without a dependency intent",
        );

        evidence.limitations = vec![Limitation::DependencyOwnerAmbiguous {
            path: "package.json".to_owned(),
            package_scope: Some(Box::new(lumin_model::PackageScope::from_root(
                &RepoPath::empty(),
            ))),
            required_intent: None,
            detail: "malformed root dependencies".to_owned(),
        }];
        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[directory_lease(&package_a)])
                .required_evidence_gap_count,
            0,
            "a nested package directory inherited its ancestor package's gap",
        );

        let parent_package = RepoPath::from_portable("packages/app")?;
        let nested_package = RepoPath::from_portable("packages/app/nested")?;
        let nested_source = RepoPath::from_portable("packages/app/nested/src/main.ts")?;
        let root_source = RepoPath::from_portable("src/root.ts")?;
        evidence.source_contexts = vec![
            source_context(&root_source, &RepoPath::empty()),
            source_context(&nested_source, &nested_package),
        ];
        evidence.limitations = vec![Limitation::DependencyOwnerAmbiguous {
            path: "packages/app/package.json".to_owned(),
            package_scope: Some(Box::new(lumin_model::PackageScope::from_root(
                &parent_package,
            ))),
            required_intent: None,
            detail: "malformed parent dependencies".to_owned(),
        }];
        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[directory_lease(&parent_package)])
                .required_evidence_gap_count,
            1,
            "a broad directory lease discarded its own package-scoped gap",
        );
        Ok(())
    }

    #[test]
    fn recoverable_parse_gap_intersects_only_its_source_file()
    -> Result<(), Box<dyn std::error::Error>> {
        let broken = RepoPath::from_portable("src/broken.ts")?;
        let unrelated = RepoPath::from_portable("src/unrelated.ts")?;
        let source_root = RepoPath::from_portable("src")?;
        let source_id = LogicalSourceId::from_path(&broken);
        let mut evidence = evidence_with_limitations(vec![Limitation::JsRecoverableParseLocal {
            source_id,
            detail: "local definitions are incomplete".to_owned(),
        }]);
        evidence.source_contexts = vec![source_context(&broken, &RepoPath::empty())];

        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[existing_file_lease(&unrelated)])
                .required_evidence_gap_count,
            0,
            "a file-local parse gap blocked an unrelated write",
        );
        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[existing_file_lease(&broken)])
                .required_evidence_gap_count,
            1,
            "the broken source write lost its required local-definition gap",
        );
        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[directory_lease(&source_root)])
                .required_evidence_gap_count,
            1,
            "a directory write covering the broken source lost its parse gap",
        );
        Ok(())
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
    fn computed_commonjs_member_is_stable_module_opacity() -> Result<(), &'static str> {
        let importer = LogicalSourceId::from_string("source-importer".to_owned());
        let candidate = LogicalSourceId::from_string("source-candidate".to_owned());
        let limitation = |start| Limitation::CommonJsComputedMember {
            source_id: importer.clone(),
            specifier: "./candidate.js".to_owned(),
            span: SourceSpan {
                start,
                end: start + 25,
            },
            target: candidate.clone(),
        };

        let first = match limitation_delta(&limitation(7)) {
            LimitationDelta::Fact(fact) => fact,
            LimitationDelta::RequiredEvidenceGap => {
                return Err("resolved computed require should produce an opacity fact");
            }
        };
        let shifted = match limitation_delta(&limitation(107)) {
            LimitationDelta::Fact(fact) => fact,
            LimitationDelta::RequiredEvidenceGap => {
                return Err("shifted computed require should remain comparable");
            }
        };
        assert_eq!(first.key, shifted.key);
        assert_eq!(first.evidence_identity, shifted.evidence_identity);
        assert_eq!(first.key.family, DeltaFactFamily::Opacity);
        assert_eq!(first.grounding, lumin_model::GroundingRank::Opaque);
        assert_eq!(
            first.targets,
            BTreeSet::from([target(candidate.as_str().as_bytes())])
        );
        let input = lifecycle_delta_input(&evidence_with_limitations(vec![limitation(7)]));
        assert_eq!(input.advisory_limitation_count, 1);
        assert_eq!(input.required_evidence_gap_count, 0);
        Ok(())
    }

    #[test]
    fn import_meta_glob_scope_is_comparable_only_for_explicit_targets()
    -> Result<(), Box<dyn std::error::Error>> {
        let importer = LogicalSourceId::from_string("source-importer".to_owned());
        let candidate = LogicalSourceId::from_string("source-candidate".to_owned());
        let limitation = |target_scope| Limitation::ImportMetaGlobUnsupported {
            source_id: importer.clone(),
            source_unit: Box::new(SourceUnitId::Logical(importer.clone())),
            span: SourceSpan { start: 7, end: 40 },
            patterns: vec!["./features/*.ts".to_owned()].into_boxed_slice(),
            candidates: vec![candidate.clone()],
            target_scope,
            detail: "glob options are unsupported".to_owned(),
        };

        let fact = match limitation_delta(&limitation(ImportMetaGlobTargetScope::ExplicitTargets)) {
            LimitationDelta::Fact(fact) => fact,
            LimitationDelta::RequiredEvidenceGap => {
                return Err("explicit glob candidates were not comparable".into());
            }
        };
        assert_eq!(fact.key.family, DeltaFactFamily::Opacity);
        assert_eq!(fact.targets.len(), 1);
        assert!(matches!(
            limitation_delta(&limitation(ImportMetaGlobTargetScope::Package)),
            LimitationDelta::RequiredEvidenceGap
        ));
        Ok(())
    }

    #[test]
    fn package_glob_gap_intersects_only_its_owner_package() -> Result<(), Box<dyn std::error::Error>>
    {
        let package_a = RepoPath::from_portable("packages/a")?;
        let package_b = RepoPath::from_portable("packages/b")?;
        let source_a = RepoPath::from_portable("packages/a/src/main.ts")?;
        let source_b = RepoPath::from_portable("packages/b/src/main.ts")?;
        let source_a_id = LogicalSourceId::from_path(&source_a);
        let limitation = Limitation::ImportMetaGlobUnsupported {
            source_id: source_a_id.clone(),
            source_unit: Box::new(SourceUnitId::Logical(source_a_id)),
            span: SourceSpan { start: 0, end: 32 },
            patterns: vec!["@alias/*.ts".to_owned()].into_boxed_slice(),
            candidates: Vec::new(),
            target_scope: ImportMetaGlobTargetScope::Package,
            detail: "aliases are unsupported".to_owned(),
        };
        let mut evidence = evidence_with_limitations(vec![limitation]);
        evidence.source_contexts = vec![
            source_context(&source_a, &package_a),
            source_context(&source_b, &package_b),
        ];

        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[existing_file_lease(&source_b)])
                .required_evidence_gap_count,
            0,
        );
        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[existing_file_lease(&source_a)])
                .required_evidence_gap_count,
            1,
        );
        Ok(())
    }

    #[test]
    fn bounded_dynamic_import_key_survives_position_and_embedded_unit_changes()
    -> Result<(), &'static str> {
        let mut first = lifecycle_delta_input(&evidence_with_limitations(vec![
            bounded_dynamic_limitation("embedded-first", 3),
        ]))
        .facts;
        let mut second = lifecycle_delta_input(&evidence_with_limitations(vec![
            bounded_dynamic_limitation("embedded-second", 30),
        ]))
        .facts;
        let first = first.pop().ok_or("first opacity was not a fact")?;
        let second = second.pop().ok_or("second opacity was not a fact")?;
        assert_eq!(first.key, second.key);
        assert!(matches!(
            classify_lifecycle_deltas(Some(&[first]), &[second])[0].classification,
            GateDeltaClassification::Unchanged
        ));
        Ok(())
    }

    #[test]
    fn same_prefix_dynamic_imports_keep_distinct_stable_occurrence_keys() {
        let baseline = lifecycle_delta_input(&evidence_with_limitations(vec![
            bounded_dynamic_limitation("embedded-baseline", 3),
        ]));
        let current = lifecycle_delta_input(&evidence_with_limitations(vec![
            bounded_dynamic_limitation("embedded-moved", 30),
            bounded_dynamic_limitation("embedded-added", 60),
        ]));

        assert_eq!(baseline.facts.len(), 1);
        assert_eq!(current.facts.len(), 2);
        assert_ne!(current.facts[0].key, current.facts[1].key);
        let deltas = classify_lifecycle_deltas(Some(&baseline.facts), &current.facts);
        assert_eq!(
            deltas
                .iter()
                .filter(|delta| matches!(delta.classification, GateDeltaClassification::Unchanged))
                .count(),
            1
        );
        assert_eq!(
            deltas
                .iter()
                .filter(|delta| matches!(delta.classification, GateDeltaClassification::Introduced))
                .count(),
            1
        );
    }

    #[test]
    fn growing_and_unbounded_dynamic_imports_remain_required_evidence_gaps() {
        for target_scope in [
            DynamicImportTargetScope::SourceInventory,
            DynamicImportTargetScope::Workspace,
        ] {
            assert!(matches!(
                limitation_delta(&Limitation::DynamicImportNonLiteral {
                    source_id: LogicalSourceId::from_string("source-importer".to_owned()),
                    source_unit: SourceUnitId::Logical(LogicalSourceId::from_string(
                        "source-importer".to_owned(),
                    )),
                    span: SourceSpan { start: 7, end: 20 },
                    static_prefix: (target_scope == DynamicImportTargetScope::SourceInventory)
                        .then(|| "./features/".to_owned()),
                    candidates: Vec::new(),
                    target_scope,
                }),
                LimitationDelta::RequiredEvidenceGap
            ));
        }
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

    fn dependency_owner(
        manifest_path: &str,
        manifest_payload_sha256: &str,
    ) -> Result<DependencyOwnerRecord, Box<dyn std::error::Error>> {
        let consumer_path = lumin_model::RepoPath::from_portable("packages/app/src/main.ts")?;
        let package_root = lumin_model::RepoPath::from_portable("packages/app")?;
        let manifest_path = lumin_model::RepoPath::from_portable(manifest_path)?;
        Ok(DependencyOwnerRecord {
            consumer: LogicalSourceId::from_path(&consumer_path),
            consumer_path: RepoPathProjection::from(&consumer_path),
            dependency: "zod".to_owned(),
            package_root: RepoPathProjection::from(&package_root),
            manifest_path: RepoPathProjection::from(&manifest_path),
            manifest_payload_sha256: manifest_payload_sha256.to_owned(),
            lockfile_path: None,
        })
    }

    fn source_context(path: &RepoPath, package_root: &RepoPath) -> SourceContextRecord {
        SourceContextRecord {
            source_id: LogicalSourceId::from_path(path),
            path: RepoPathProjection::from(path),
            kind: SourceKind::TypeScript,
            package_root: Some(RepoPathProjection::from(package_root)),
        }
    }

    fn existing_file_lease(path: &RepoPath) -> WriteLease {
        WriteLease {
            path: RepoPathProjection::from(path),
            kind: WriteLeaseKind::ExistingFile,
            physical_identity: None,
            nearest_existing_parent: None,
            prefix_identities: Vec::new(),
        }
    }

    fn directory_lease(path: &RepoPath) -> WriteLease {
        WriteLease {
            path: RepoPathProjection::from(path),
            kind: WriteLeaseKind::Directory,
            physical_identity: None,
            nearest_existing_parent: None,
            prefix_identities: Vec::new(),
        }
    }

    fn dependency_intent(
        path: &str,
        dependency: &str,
    ) -> Result<DependencyIntentRecord, Box<dyn std::error::Error>> {
        let path = lumin_model::RepoPath::from_portable(path)?;
        Ok(DependencyIntentRecord {
            path: RepoPathProjection::from(&path),
            dependency: dependency.to_owned(),
        })
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
