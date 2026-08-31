use std::collections::{BTreeMap, BTreeSet};

use lumin_model::{
    DeltaFact, DeltaFactFamily, DeltaIdentity, DeltaIdentityKind, DeltaKey, DeltaOwnerPayloadValue,
    DeltaValue, DependencyIntentIdentity, DynamicImportTargetScope, FindingDisposition,
    ImportMetaGlobTargetScope, Limitation, LimitationGateRelevance, LimitationScopePolicy,
    LogicalSourceId, PackageScopeId, RepoPath, ResolutionOutcome, ReviewOnlyReason,
    UnresolvedTargetScope, append_length_prefixed,
};

use crate::{
    Confidence, DEPENDENCY_OWNERSHIP_CAPABILITY_ID, DependencyIntentRecord, DependencyOwnerRecord,
    FindingRecord, RepoPathProjection, RunEvidence, Severity, WriteLease, WriteLeaseKind,
};

pub(crate) struct LifecycleDeltaInput {
    pub facts: Vec<DeltaFact>,
    pub advisory_limitation_count: usize,
    pub required_evidence_gap_count: usize,
    pub required_owner_gap_count: usize,
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
    let mut required_owner_gap_count = 0;
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
                if limitation_intersects_gate_domain(
                    limitation,
                    evidence,
                    dependency_intents,
                    leased_write_set,
                ) {
                    required_evidence_gap_count += 1;
                }
            }
            LimitationDelta::RequiredOwnerGap => {
                if limitation_intersects_gate_domain(
                    limitation,
                    evidence,
                    dependency_intents,
                    leased_write_set,
                ) {
                    required_owner_gap_count += 1;
                }
            }
        }
    }
    facts.sort_by(|left, right| left.key.cmp(&right.key));
    LifecycleDeltaInput {
        facts,
        advisory_limitation_count,
        required_evidence_gap_count,
        required_owner_gap_count,
    }
}

fn limitation_intersects_gate_domain(
    limitation: &Limitation,
    evidence: &RunEvidence,
    dependency_intents: &[DependencyIntentRecord],
    leased_write_set: &[WriteLease],
) -> bool {
    match limitation.registry_entry().scope {
        LimitationScopePolicy::File => {
            let Limitation::JsRecoverableParseLocal { source_id, .. } = limitation else {
                return true;
            };
            local_definition_scope_intersects(source_id, evidence, leased_write_set)
        }
        LimitationScopePolicy::Workspace => true,
        LimitationScopePolicy::ResolvedModule => true,
        LimitationScopePolicy::ExplicitTargetsOrWorkspace
        | LimitationScopePolicy::ExplicitTargetsOrSourceInventoryOrWorkspace
        | LimitationScopePolicy::ExplicitTargetsOrKnownNoTargetOrWorkspace => true,
        LimitationScopePolicy::SourceOwnerPackageOrWorkspace => {
            let Limitation::AliasShapeUnsupported { source_id, .. } = limitation else {
                return true;
            };
            source_owner_intersects(source_id, evidence, leased_write_set)
        }
        LimitationScopePolicy::OwningPackage => limitation_path(limitation).is_none_or(|path| {
            path_owner_intersects(path, evidence, leased_write_set)
                || public_surface_consumer_intersects(limitation, evidence, leased_write_set)
        }),
        LimitationScopePolicy::OwningPackageOrWorkspace => {
            if let Limitation::DependencyOwnerAmbiguous {
                package_scope,
                required_intent,
                ..
            } = limitation
            {
                return dependency_owner_scope_intersects(
                    package_scope.as_deref(),
                    required_intent.as_deref(),
                    evidence,
                    dependency_intents,
                    leased_write_set,
                );
            }
            limitation_path(limitation)
                .is_none_or(|path| path_owner_intersects(path, evidence, leased_write_set))
        }
        LimitationScopePolicy::ConfiguredPackagesOrWorkspace => limitation_path(limitation)
            .is_none_or(|path| configured_packages_intersect(path, evidence, leased_write_set)),
        LimitationScopePolicy::ManifestOwnerPackageOrWorkspace => limitation_path(limitation)
            .is_none_or(|path| path_owner_intersects(path, evidence, leased_write_set)),
        LimitationScopePolicy::WorkspaceFromConfig => {
            limitation_path(limitation).is_none_or(|path| {
                workspace_config_intersects(path, evidence, dependency_intents, leased_write_set)
            })
        }
        LimitationScopePolicy::ParentAndTargetOwnersOrWorkspace => {
            let Limitation::VueExternalScriptModeConflict {
                source_id,
                target_source_id,
                ..
            } = limitation
            else {
                return true;
            };
            let Some(parent_scope) = source_package_scope(source_id, evidence) else {
                return true;
            };
            let Some(target_scope) = source_package_scope(target_source_id, evidence) else {
                return true;
            };
            if parent_scope.id() != target_scope.id() {
                return true;
            }
            package_scope_intersects_write_set(&parent_scope, evidence, leased_write_set)
        }
        LimitationScopePolicy::ImportedTargetsOrPackage => {
            let source_id = match limitation {
                Limitation::ImportMetaGlobUnsupported { source_id, .. }
                | Limitation::VueTemplateOpaque { source_id, .. } => source_id,
                _ => return true,
            };
            source_owner_intersects(source_id, evidence, leased_write_set)
        }
        LimitationScopePolicy::EntryOwnerPackageOrWorkspace => {
            let Limitation::ExplicitEntryUnavailable { path, .. } = limitation else {
                return true;
            };
            path_owner_intersects(path, evidence, leased_write_set)
        }
    }
}

fn local_definition_scope_intersects(
    source_id: &LogicalSourceId,
    evidence: &RunEvidence,
    leased_write_set: &[WriteLease],
) -> bool {
    let mut affected_sources = BTreeSet::from([source_id.clone()]);
    affected_sources.extend(
        evidence
            .resolutions
            .iter()
            .filter(|resolution| {
                matches!(
                    &resolution.outcome,
                    ResolutionOutcome::Internal { target } if target == source_id
                )
            })
            .map(|resolution| resolution.source_use.importer.clone()),
    );
    affected_sources.iter().any(|affected_source| {
        let Some(path) = evidence
            .source_contexts
            .iter()
            .find(|context| &context.source_id == affected_source)
            .map(|context| &context.path)
        else {
            return true;
        };
        leased_write_set.iter().any(|lease| lease.covers(path))
    })
}

fn source_owner_intersects(
    source_id: &LogicalSourceId,
    evidence: &RunEvidence,
    leased_write_set: &[WriteLease],
) -> bool {
    source_context_inputs_intersect(source_id, evidence, leased_write_set)
        || source_package_scope(source_id, evidence).is_none_or(|scope| {
            package_scope_intersects_write_set(&scope, evidence, leased_write_set)
        })
}

fn source_context_inputs_intersect(
    source_id: &LogicalSourceId,
    evidence: &RunEvidence,
    leased_write_set: &[WriteLease],
) -> bool {
    let Some(context) = evidence
        .source_contexts
        .iter()
        .find(|context| &context.source_id == source_id)
    else {
        return true;
    };
    if std::iter::once(&context.path)
        .chain(&context.configuration_paths)
        .any(|path| leased_write_set.iter().any(|lease| lease.covers(path)))
    {
        return true;
    }
    let Some(package_root) = &context.package_root else {
        return false;
    };
    let Some(manifest_path) = RepoPath::from_canonical_bytes(&package_root.canonical)
        .ok()
        .and_then(|root| root.join_portable("package.json").ok())
        .map(|path| RepoPathProjection::from(&path))
    else {
        return true;
    };
    leased_write_set
        .iter()
        .any(|lease| lease.covers(&manifest_path))
}

fn source_package_scope(
    source_id: &LogicalSourceId,
    evidence: &RunEvidence,
) -> Option<lumin_model::PackageScope> {
    evidence
        .source_contexts
        .iter()
        .find(|context| &context.source_id == source_id)?
        .package_root
        .as_ref()
        .and_then(|root| RepoPath::from_canonical_bytes(&root.canonical).ok())
        .map(|root| lumin_model::PackageScope::from_root(&root))
}

fn limitation_path(limitation: &Limitation) -> Option<&str> {
    match limitation {
        Limitation::SourcePayloadUnavailable { path, .. }
        | Limitation::PackageImportsUnsupported { path, .. }
        | Limitation::ImporterFormatUnsupported { path, .. }
        | Limitation::PublicSurfaceUnsupported { path, .. }
        | Limitation::TsconfigSemanticsUnsupported { path, .. }
        | Limitation::PackageIdentityUnsupported { path, .. }
        | Limitation::PackageMetadataUnobservable { path, .. }
        | Limitation::PackagePrivacyUnsupported { path, .. }
        | Limitation::DependencyOwnerAmbiguous { path, .. }
        | Limitation::WorkspaceOwnershipUnsupported { path, .. }
        | Limitation::PnpmDependencySemanticsUnsupported { path, .. }
        | Limitation::TsconfigPayloadUnavailable { path, .. }
        | Limitation::ExplicitEntryUnavailable { path, .. } => Some(path),
        Limitation::JsRecoverableParseLocal { .. }
        | Limitation::JsModuleUseUnknown { .. }
        | Limitation::DynamicImportNonLiteral { .. }
        | Limitation::ImportMetaGlobUnsupported { .. }
        | Limitation::CommonJsComputedMember { .. }
        | Limitation::InternalSpecifierUnresolved { .. }
        | Limitation::AliasShapeUnsupported { .. }
        | Limitation::AbsoluteInternalSpecifierUnsupported { .. }
        | Limitation::SfcDialectUnavailable { .. }
        | Limitation::SfcDecompositionUnknown { .. }
        | Limitation::VueExternalScriptModeConflict { .. }
        | Limitation::VueTemplateOpaque { .. }
        | Limitation::CapabilityUnavailable { .. } => None,
    }
}

fn path_owner_intersects(
    path: &str,
    evidence: &RunEvidence,
    leased_write_set: &[WriteLease],
) -> bool {
    let Some(projection) = known_limitation_path(path, evidence, leased_write_set) else {
        return true;
    };
    let Some(root) = evidence
        .source_contexts
        .iter()
        .filter_map(|context| context.package_root.as_ref())
        .filter(|root| projection.components.starts_with(&root.components))
        .max_by_key(|root| root.components.len())
        .and_then(|root| RepoPath::from_canonical_bytes(&root.canonical).ok())
    else {
        return true;
    };
    package_scope_intersects_write_set(
        &lumin_model::PackageScope::from_root(&root),
        evidence,
        leased_write_set,
    )
}

fn configured_packages_intersect(
    path: &str,
    evidence: &RunEvidence,
    leased_write_set: &[WriteLease],
) -> bool {
    let configured_paths = evidence
        .source_contexts
        .iter()
        .flat_map(|context| &context.configuration_paths)
        .filter(|configured_path| configured_path.display == path)
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut configured_paths = configured_paths.into_iter();
    let Some(config_path) = configured_paths.next() else {
        return true;
    };
    if configured_paths.next().is_some() {
        return true;
    }
    let affected_contexts = evidence
        .source_contexts
        .iter()
        .filter(|context| {
            context
                .configuration_paths
                .iter()
                .any(|configured_path| configured_path.canonical == config_path.canonical)
        })
        .collect::<Vec<_>>();
    if affected_contexts.is_empty() {
        return true;
    }
    if affected_contexts.iter().any(|context| {
        context.configuration_paths.iter().any(|configured_path| {
            leased_write_set
                .iter()
                .any(|lease| lease.covers(configured_path))
        })
    }) {
        return true;
    }
    let mut configured_scope = None::<lumin_model::PackageScope>;
    for context in affected_contexts {
        let Some(root) = context
            .package_root
            .as_ref()
            .and_then(|root| RepoPath::from_canonical_bytes(&root.canonical).ok())
        else {
            return true;
        };
        let scope = lumin_model::PackageScope::from_root(&root);
        match &configured_scope {
            Some(existing) if existing.id() != scope.id() => return true,
            Some(_) => {}
            None => configured_scope = Some(scope),
        }
    }
    configured_scope
        .is_none_or(|scope| package_scope_intersects_write_set(&scope, evidence, leased_write_set))
}

fn public_surface_consumer_intersects(
    limitation: &Limitation,
    evidence: &RunEvidence,
    leased_write_set: &[WriteLease],
) -> bool {
    let Limitation::PublicSurfaceUnsupported {
        importer: Some(importer),
        ..
    } = limitation
    else {
        return false;
    };
    source_context_inputs_intersect(importer, evidence, leased_write_set)
}

fn workspace_config_intersects(
    path: &str,
    evidence: &RunEvidence,
    dependency_intents: &[DependencyIntentRecord],
    leased_write_set: &[WriteLease],
) -> bool {
    let Some(root) = known_limitation_path(path, evidence, leased_write_set)
        .and_then(|path| RepoPath::from_canonical_bytes(&path.canonical).ok())
        .and_then(|path| path.parent())
        .map(|root| RepoPathProjection::from(&root))
    else {
        return true;
    };
    leased_write_set.iter().any(|lease| {
        lease.path.components.starts_with(&root.components)
            || (lease.kind == WriteLeaseKind::Directory
                && root.components.starts_with(&lease.path.components))
    }) || dependency_intents
        .iter()
        .any(|intent| intent.path.components.starts_with(&root.components))
}

fn known_limitation_path(
    display: &str,
    evidence: &RunEvidence,
    leased_write_set: &[WriteLease],
) -> Option<RepoPathProjection> {
    let mut matches = BTreeSet::new();
    {
        let mut retain = |candidate: &RepoPathProjection| {
            if candidate.display == display {
                matches.insert(candidate.clone());
            }
        };
        for context in &evidence.source_contexts {
            retain(&context.path);
            if let Some(root) = &context.package_root {
                retain(root);
                if let Ok(mut ancestor) = RepoPath::from_canonical_bytes(&root.canonical) {
                    loop {
                        for name in ["package.json", "pnpm-workspace.yaml"] {
                            if let Ok(path) = ancestor.join_portable(name) {
                                retain(&RepoPathProjection::from(&path));
                            }
                        }
                        let Some(parent) = ancestor.parent() else {
                            break;
                        };
                        ancestor = parent;
                    }
                }
            }
            for path in &context.configuration_paths {
                retain(path);
            }
        }
        for owner in &evidence.dependency_owners {
            retain(&owner.consumer_path);
            retain(&owner.package_root);
            retain(&owner.manifest_path);
            if let Some(path) = &owner.lockfile_path {
                retain(path);
            }
        }
        for lease in leased_write_set {
            retain(&lease.path);
        }
    }

    match matches.len() {
        0 => RepoPath::from_portable(display)
            .ok()
            .map(|path| RepoPathProjection::from(&path)),
        1 => matches.into_iter().next(),
        _ => None,
    }
}

fn dependency_owner_scope_intersects(
    package_scope: Option<&lumin_model::PackageScope>,
    required_intent: Option<&DependencyIntentIdentity>,
    evidence: &RunEvidence,
    dependency_intents: &[DependencyIntentRecord],
    leased_write_set: &[WriteLease],
) -> bool {
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
    RequiredOwnerGap,
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
        | Limitation::ExplicitEntryUnavailable { .. }
        | Limitation::CapabilityUnavailable { .. } => {
            match limitation.registry_entry().gate_relevance {
                LimitationGateRelevance::RequiredOwner => LimitationDelta::RequiredOwnerGap,
                LimitationGateRelevance::RequiredEvidence
                | LimitationGateRelevance::NormalizedOpacity
                | LimitationGateRelevance::NormalizedUnresolvedOrRequiredEvidence
                | LimitationGateRelevance::NormalizedOpacityOrRequiredEvidence => {
                    LimitationDelta::RequiredEvidenceGap
                }
            }
        }
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
        DeltaDimensionChange, FindingId, GateDeltaClassification, ImportKind, ModuleRequestKind,
        ResolvedSourceUse, SourceKind, SourceSpan, SourceUnitId, SourceUseFact, SymbolNamespace,
        classify_lifecycle_deltas,
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
    fn pnpm_workspace_gap_intersects_dependency_intent_context()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace_root = RepoPath::from_portable("workspaces/affected")?;
        let workspace_config = workspace_root.join_portable("pnpm-workspace.yaml")?;
        let affected_package = RepoPath::from_portable("workspaces/affected/packages/app")?;
        let affected_source =
            RepoPath::from_portable("workspaces/affected/packages/app/src/main.ts")?;
        let clear_package = RepoPath::from_portable("packages/clear")?;
        let clear_source = RepoPath::from_portable("packages/clear/src/main.ts")?;
        let mut evidence =
            evidence_with_limitations(vec![Limitation::PnpmDependencySemanticsUnsupported {
                path: workspace_config.display_escaped(),
                detail: "pnpm packageConfigs semantics are unsupported".to_owned(),
            }]);
        evidence.source_contexts = vec![
            source_context(&affected_source, &affected_package),
            source_context(&clear_source, &clear_package),
        ];
        let intent = dependency_intent("workspaces/affected/packages/app/src/main.ts", "zod")?;

        assert_eq!(
            lifecycle_delta_input_for(
                &evidence,
                std::slice::from_ref(&intent),
                &[existing_file_lease(&clear_source)],
            )
            .required_evidence_gap_count,
            1,
            "a dependency intent inside the affected pnpm workspace lost its required gap",
        );
        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[existing_file_lease(&clear_source)])
                .required_evidence_gap_count,
            0,
            "pnpm dependency uncertainty escaped into a disjoint ordinary source write",
        );
        Ok(())
    }

    #[test]
    fn configured_scope_uses_affected_importer_owners() -> Result<(), Box<dyn std::error::Error>> {
        let config = RepoPath::from_portable("tsconfig.json")?;
        let root_source = RepoPath::from_portable("src/root.ts")?;
        let nested_package = RepoPath::from_portable("packages/nested")?;
        let nested_source = RepoPath::from_portable("packages/nested/src/main.ts")?;
        let mut evidence =
            evidence_with_limitations(vec![Limitation::TsconfigSemanticsUnsupported {
                path: config.display_escaped(),
                detail: "unknown compiler option madeUpFlag".to_owned(),
            }]);
        let controlling_config = RepoPathProjection::from(&config);
        let mut root_context = source_context(&root_source, &RepoPath::empty());
        root_context.configuration_paths = vec![controlling_config.clone()];
        let mut nested_context = source_context(&nested_source, &nested_package);
        nested_context.configuration_paths = vec![controlling_config];
        evidence.source_contexts = vec![root_context, nested_context];

        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[existing_file_lease(&nested_source)],)
                .required_evidence_gap_count,
            1,
            "a root config controlling multiple package owners must remain workspace-scoped",
        );
        Ok(())
    }

    #[test]
    fn configured_scope_survives_failed_or_overridden_profile_selection()
    -> Result<(), Box<dyn std::error::Error>> {
        let package_a = RepoPath::from_portable("packages/a")?;
        let package_b = RepoPath::from_portable("packages/b")?;
        let config = RepoPath::from_portable("packages/a/tsconfig.json")?;
        let source_a = RepoPath::from_portable("packages/a/src/main.ts")?;
        let source_b = RepoPath::from_portable("packages/b/src/main.ts")?;
        let mut context_a = source_context(&source_a, &package_a);
        context_a.configuration_paths = vec![RepoPathProjection::from(&config)];
        let mut evidence =
            evidence_with_limitations(vec![Limitation::TsconfigSemanticsUnsupported {
                path: config.display_escaped(),
                detail: "unsupported moduleResolution value classic".to_owned(),
            }]);
        evidence.source_contexts = vec![context_a, source_context(&source_b, &package_b)];

        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[existing_file_lease(&source_b)])
                .required_evidence_gap_count,
            0,
            "a failed package-local profile selection escaped to workspace scope",
        );
        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[existing_file_lease(&source_a)])
                .required_evidence_gap_count,
            1,
            "the affected package lost its failed profile-selection gap",
        );

        evidence.resolution_profiles = vec![lumin_model::SelectedResolutionProfile {
            source_id: LogicalSourceId::from_path(&source_a),
            profile: lumin_model::ResolutionProfile::Bundler,
            source: lumin_model::ResolutionProfileSource::Invocation,
        }];
        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[existing_file_lease(&source_b)])
                .required_evidence_gap_count,
            0,
            "an invocation profile override erased package-local config ownership",
        );
        Ok(())
    }

    #[test]
    fn configured_scope_uses_canonical_paths_and_includes_the_limiting_config()
    -> Result<(), Box<dyn std::error::Error>> {
        let non_scalar_root = RepoPath::from_canonical_bytes(
            b"LUMRPATH\x00\x01\x00\x00\x00\x01\x03\x00\x00\x00\x02\xd8\x00",
        )?;
        let config = non_scalar_root.join_portable("tsconfig.json")?;
        let package_a = RepoPath::from_portable("packages/a")?;
        let package_b = RepoPath::from_portable("packages/b")?;
        let source_a = RepoPath::from_portable("packages/a/src/main.ts")?;
        let source_b = RepoPath::from_portable("packages/b/src/main.ts")?;
        let mut context_a = source_context(&source_a, &package_a);
        context_a.configuration_paths = vec![RepoPathProjection::from(&config)];
        let mut evidence =
            evidence_with_limitations(vec![Limitation::TsconfigSemanticsUnsupported {
                path: config.display_escaped(),
                detail: "unsupported config under a non-scalar component".to_owned(),
            }]);
        evidence.source_contexts = vec![context_a, source_context(&source_b, &package_b)];

        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[existing_file_lease(&source_b)])
                .required_evidence_gap_count,
            0,
            "a canonical package-local config path escaped to workspace scope",
        );
        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[existing_file_lease(&source_a)])
                .required_evidence_gap_count,
            1,
            "the affected package lost its canonical config gap",
        );
        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[existing_file_lease(&config)])
                .required_evidence_gap_count,
            1,
            "a write to the limiting config itself escaped the required gap",
        );
        Ok(())
    }

    #[test]
    fn configured_scope_includes_every_consulted_config() -> Result<(), Box<dyn std::error::Error>>
    {
        let inherited_config = RepoPath::from_portable("tsconfig.json")?;
        let package_a = RepoPath::from_portable("packages/a")?;
        let package_b = RepoPath::from_portable("packages/b")?;
        let controlling_config = RepoPath::from_portable("packages/a/tsconfig.json")?;
        let source_a = RepoPath::from_portable("packages/a/src/main.ts")?;
        let source_b = RepoPath::from_portable("packages/b/src/main.ts")?;
        let mut context_a = source_context(&source_a, &package_a);
        context_a.configuration_paths = vec![
            RepoPathProjection::from(&inherited_config),
            RepoPathProjection::from(&controlling_config),
        ];
        let mut evidence =
            evidence_with_limitations(vec![Limitation::TsconfigSemanticsUnsupported {
                path: controlling_config.display_escaped(),
                detail: "inherited profile and child module are incompatible".to_owned(),
            }]);
        evidence.source_contexts = vec![context_a, source_context(&source_b, &package_b)];

        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[existing_file_lease(&inherited_config)],)
                .required_evidence_gap_count,
            1,
            "a consulted parent config escaped the affected importer's required gap",
        );
        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[existing_file_lease(&source_b)])
                .required_evidence_gap_count,
            0,
            "consulted parent config ownership escaped into a disjoint package",
        );
        Ok(())
    }

    #[test]
    fn alias_gap_intersects_its_importer_configuration() -> Result<(), Box<dyn std::error::Error>> {
        let package_a = RepoPath::from_portable("packages/a")?;
        let package_b = RepoPath::from_portable("packages/b")?;
        let source_a = RepoPath::from_portable("packages/a/src/main.ts")?;
        let source_b = RepoPath::from_portable("packages/b/src/main.ts")?;
        let inherited_config = RepoPath::from_portable("tsconfig.json")?;
        let source_a_id = LogicalSourceId::from_path(&source_a);
        let mut context_a = source_context(&source_a, &package_a);
        context_a.configuration_paths = vec![RepoPathProjection::from(&inherited_config)];
        let mut evidence = evidence_with_limitations(vec![Limitation::AliasShapeUnsupported {
            source_id: source_a_id,
            detail: "node16 import-mode resolution requires an explicit extension".to_owned(),
        }]);
        evidence.source_contexts = vec![context_a, source_context(&source_b, &package_b)];

        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[existing_file_lease(&inherited_config)],)
                .required_evidence_gap_count,
            1,
            "an importer alias gap lost its consulted ancestor config",
        );
        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[existing_file_lease(&source_b)])
                .required_evidence_gap_count,
            0,
            "an importer alias gap escaped into a disjoint package",
        );
        Ok(())
    }

    #[test]
    fn package_scope_uses_the_canonical_limitation_path() -> Result<(), Box<dyn std::error::Error>>
    {
        let package_a = RepoPath::from_canonical_bytes(
            b"LUMRPATH\x00\x01\x00\x00\x00\x01\x03\x00\x00\x00\x02\xd8\x00",
        )?;
        let manifest_a = package_a.join_portable("package.json")?;
        let source_a = package_a.join_portable("src")?.join_portable("main.ts")?;
        let package_b = RepoPath::from_portable("packages/b")?;
        let source_b = RepoPath::from_portable("packages/b/src/main.ts")?;
        let mut evidence = evidence_with_limitations(vec![Limitation::PackageImportsUnsupported {
            path: manifest_a.display_escaped(),
            detail: "unsupported imports under a non-scalar component".to_owned(),
        }]);
        evidence.source_contexts = vec![
            source_context(&source_a, &package_a),
            source_context(&source_b, &package_b),
        ];

        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[existing_file_lease(&source_b)])
                .required_evidence_gap_count,
            0,
            "a canonical package limitation escaped into a disjoint package",
        );
        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[existing_file_lease(&source_a)])
                .required_evidence_gap_count,
            1,
            "the owning package lost its canonical limitation",
        );
        Ok(())
    }

    #[test]
    fn cross_package_sfc_mode_conflict_remains_workspace_scoped()
    -> Result<(), Box<dyn std::error::Error>> {
        let package_a = RepoPath::from_portable("packages/a")?;
        let package_b = RepoPath::from_portable("packages/b")?;
        let package_c = RepoPath::from_portable("packages/c")?;
        let parent = RepoPath::from_portable("packages/a/src/App.vue")?;
        let target = RepoPath::from_portable("packages/b/src/external.ts")?;
        let unrelated = RepoPath::from_portable("packages/c/src/candidate.ts")?;
        let mut evidence =
            evidence_with_limitations(vec![Limitation::VueExternalScriptModeConflict {
                source_id: LogicalSourceId::from_path(&parent),
                target_source_id: LogicalSourceId::from_path(&target),
                declared: "tsx".to_owned(),
                actual: "typescript".to_owned(),
            }]);
        evidence.source_contexts = vec![
            source_context(&parent, &package_a),
            source_context(&target, &package_b),
            source_context(&unrelated, &package_c),
        ];

        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[existing_file_lease(&unrelated)])
                .required_evidence_gap_count,
            1,
            "different known parent/target owners must retain the contract's workspace fallback",
        );
        Ok(())
    }

    #[test]
    fn public_surface_scope_uses_exact_originating_importer()
    -> Result<(), Box<dyn std::error::Error>> {
        let consumer_a_package = RepoPath::from_portable("packages/app-a")?;
        let consumer_b_package = RepoPath::from_portable("packages/app-b")?;
        let target_a_package = RepoPath::from_portable("packages/lib-a")?;
        let target_b_package = RepoPath::from_portable("packages/lib-b")?;
        let consumer_a = RepoPath::from_portable("packages/app-a/main.ts")?;
        let consumer_b = RepoPath::from_portable("packages/app-b/main.ts")?;
        let consumer_a_config = RepoPath::from_portable("packages/app-a/tsconfig.json")?;
        let consumer_a_manifest = RepoPath::from_portable("packages/app-a/package.json")?;
        let target_a = RepoPath::from_portable("packages/lib-a/index.ts")?;
        let target_b = RepoPath::from_portable("packages/lib-b/index.ts")?;
        let importer_a = LogicalSourceId::from_path(&consumer_a);
        let importer_b = LogicalSourceId::from_path(&consumer_b);
        let detail = "conditional exports mix custom and default branches";
        let mut evidence = evidence_with_limitations(vec![
            Limitation::PublicSurfaceUnsupported {
                path: "packages/lib-a/package.json".to_owned(),
                detail: detail.to_owned(),
                importer: Some(importer_a.clone()),
            },
            Limitation::PublicSurfaceUnsupported {
                path: "packages/lib-b/package.json".to_owned(),
                detail: detail.to_owned(),
                importer: Some(importer_b.clone()),
            },
        ]);
        let mut consumer_a_context = source_context(&consumer_a, &consumer_a_package);
        consumer_a_context.configuration_paths = vec![RepoPathProjection::from(&consumer_a_config)];
        evidence.source_contexts = vec![
            consumer_a_context,
            source_context(&consumer_b, &consumer_b_package),
            source_context(&target_a, &target_a_package),
            source_context(&target_b, &target_b_package),
        ];
        let unsupported = |importer: LogicalSourceId, specifier: &str| ResolvedSourceUse {
            source_use: SourceUseFact {
                importer,
                specifier: specifier.to_owned(),
                imported_name: Some("selected".to_owned()),
                local_name: Some("selected".to_owned()),
                namespace: SymbolNamespace::Value,
                kind: ImportKind::Named,
                request_kind: ModuleRequestKind::StaticImport,
                span: SourceSpan { start: 0, end: 40 },
            },
            outcome: ResolutionOutcome::Unsupported {
                specifier: specifier.to_owned(),
                reason: detail.to_owned(),
            },
        };
        evidence.resolutions = vec![
            unsupported(importer_a, "@scope/lib-a"),
            unsupported(importer_b, "@scope/lib-b"),
        ];

        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[existing_file_lease(&consumer_a)])
                .required_evidence_gap_count,
            1,
            "an equal diagnostic from another package was attributed to this consumer",
        );
        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[existing_file_lease(&consumer_a_config)])
                .required_evidence_gap_count,
            1,
            "the originating consumer's consulted config lost its public-surface gap",
        );
        assert_eq!(
            lifecycle_delta_input_for(&evidence, &[], &[existing_file_lease(&consumer_a_manifest)])
                .required_evidence_gap_count,
            1,
            "the originating consumer's manifest lost its public-surface gap",
        );
        Ok(())
    }

    #[test]
    fn recoverable_parse_gap_intersects_its_source_and_direct_consumers()
    -> Result<(), Box<dyn std::error::Error>> {
        let broken = RepoPath::from_portable("src/broken.ts")?;
        let consumer = RepoPath::from_portable("src/consumer.ts")?;
        let unrelated = RepoPath::from_portable("src/unrelated.ts")?;
        let source_root = RepoPath::from_portable("src")?;
        let source_id = LogicalSourceId::from_path(&broken);
        let mut evidence = evidence_with_limitations(vec![Limitation::JsRecoverableParseLocal {
            source_id: source_id.clone(),
            detail: "local definitions are incomplete".to_owned(),
        }]);
        evidence.source_contexts = vec![
            source_context(&broken, &RepoPath::empty()),
            source_context(&consumer, &RepoPath::empty()),
            source_context(&unrelated, &RepoPath::empty()),
        ];
        evidence.resolutions = vec![ResolvedSourceUse {
            source_use: SourceUseFact {
                importer: LogicalSourceId::from_path(&consumer),
                specifier: "./broken.js".to_owned(),
                imported_name: Some("visible".to_owned()),
                local_name: Some("visible".to_owned()),
                namespace: SymbolNamespace::Value,
                kind: ImportKind::Named,
                request_kind: ModuleRequestKind::StaticImport,
                span: SourceSpan { start: 0, end: 43 },
            },
            outcome: ResolutionOutcome::Internal {
                target: source_id.clone(),
            },
        }];

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
            lifecycle_delta_input_for(&evidence, &[], &[existing_file_lease(&consumer)])
                .required_evidence_gap_count,
            1,
            "a direct consumer write lost the source's missing local-definition gap",
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
            LimitationDelta::RequiredEvidenceGap | LimitationDelta::RequiredOwnerGap => {
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
            LimitationDelta::RequiredEvidenceGap | LimitationDelta::RequiredOwnerGap => {
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
            LimitationDelta::RequiredEvidenceGap | LimitationDelta::RequiredOwnerGap => {
                return Err("resolved computed require should produce an opacity fact");
            }
        };
        let shifted = match limitation_delta(&limitation(107)) {
            LimitationDelta::Fact(fact) => fact,
            LimitationDelta::RequiredEvidenceGap | LimitationDelta::RequiredOwnerGap => {
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
            LimitationDelta::RequiredEvidenceGap | LimitationDelta::RequiredOwnerGap => {
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
    fn unavailable_sfc_dialect_is_a_required_owner_gap() {
        let input = lifecycle_delta_input(&evidence_with_limitations(vec![
            Limitation::SfcDialectUnavailable {
                source_id: LogicalSourceId::from_string("source-svelte".to_owned()),
                dialect: "svelte".to_owned(),
            },
        ]));
        assert_eq!(input.required_owner_gap_count, 1);
        assert_eq!(input.required_evidence_gap_count, 0);
        assert_eq!(input.advisory_limitation_count, 0);
    }

    #[test]
    fn required_owner_gaps_are_aggregated_across_owners() {
        let input = lifecycle_delta_input(&evidence_with_limitations(vec![
            Limitation::SfcDialectUnavailable {
                source_id: LogicalSourceId::from_string("source-svelte".to_owned()),
                dialect: "svelte".to_owned(),
            },
            Limitation::CapabilityUnavailable {
                capability: lumin_model::CapabilityIntentKind::Rust,
                targets: vec![LogicalSourceId::from_string("source-rust".to_owned())],
            },
        ]));
        assert_eq!(input.required_owner_gap_count, 2);
        assert_eq!(input.required_evidence_gap_count, 0);
        assert_eq!(input.advisory_limitation_count, 0);
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
            LimitationDelta::RequiredEvidenceGap | LimitationDelta::RequiredOwnerGap => {
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
            configuration_paths: Vec::new(),
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
            LimitationDelta::RequiredEvidenceGap | LimitationDelta::RequiredOwnerGap => {
                Err("bounded unresolved edge was not a fact")
            }
        }
    }
}
