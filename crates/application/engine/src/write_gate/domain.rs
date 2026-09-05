use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use lumin_evidence::{
    GateRecord, GateSignal, PathPrefixIdentity, PhysicalAliasClosureRecord,
    PreWriteDeclaredPathInspection, RepoPathProjection, SemanticInputRecord, SemanticInputState,
    WriteLease, WriteLeaseKind,
};
use lumin_inventory::{InventoryError, WriteTargetError, WriteTargetKind, WriteTargetObservation};
use lumin_model::{PhysicalFileIdentity, RepoPath};

use super::RepositoryCapture;

pub(super) struct DeclaredPathInspection {
    pub(super) observations: Vec<WriteTargetObservation>,
    pub(super) leases: Vec<WriteLease>,
    pub(super) signals: Vec<GateSignal>,
    pub(super) evidence: Vec<PreWriteDeclaredPathInspection>,
}

pub(super) fn inspect_declared_paths(
    root: &Path,
    paths: &[RepoPath],
    unavailable_targets: &BTreeSet<RepoPath>,
) -> DeclaredPathInspection {
    let mut observations = Vec::new();
    let mut leases = Vec::new();
    let mut signals = Vec::new();
    let mut evidence = Vec::new();
    for path in paths {
        let projection = RepoPathProjection::from(path);
        match lumin_inventory::is_reserved_state_path(path) {
            Ok(true) => {
                let signal = unsupported_path(
                    projection,
                    lumin_evidence::DeclaredPathUnsupportedReason::ReservedState,
                );
                evidence.push(rejected_path_inspection(path, &signal));
                signals.push(signal);
                continue;
            }
            Err(_) => {
                let signal = unsupported_path(
                    projection,
                    lumin_evidence::DeclaredPathUnsupportedReason::NotAnalyzedSource,
                );
                evidence.push(rejected_path_inspection(path, &signal));
                signals.push(signal);
                continue;
            }
            Ok(false) => {}
        }
        match lumin_inventory::inspect_write_target(root, path) {
            Ok(observation) => {
                let supported_source = lumin_inventory::is_supported_source_path(path);
                let unavailable_source = unavailable_targets.contains(path);
                let unsupported_native_file = observation.kind == WriteTargetKind::ExistingFile
                    && path.file_name_portable().is_none()
                    && !supported_source
                    && !unavailable_source;
                if unsupported_native_file
                    || (observation.kind == WriteTargetKind::NewFile
                        && !supported_source
                        && !unavailable_source)
                {
                    let signal = unsupported_path(
                        projection,
                        lumin_evidence::DeclaredPathUnsupportedReason::NotAnalyzedSource,
                    );
                    evidence.push(rejected_path_inspection(path, &signal));
                    signals.push(signal);
                    continue;
                }
                let lease = write_lease(&observation);
                evidence.push(PreWriteDeclaredPathInspection {
                    path: lease.path.clone(),
                    lease: Some(lease.clone()),
                    rejection: None,
                });
                leases.push(lease);
                observations.push(observation);
            }
            Err(error) => {
                let signal = write_target_signal(projection, error);
                evidence.push(rejected_path_inspection(path, &signal));
                signals.push(signal);
            }
        }
    }
    leases.sort();
    leases.dedup();
    evidence.sort_by(|left, right| left.path.cmp(&right.path));
    DeclaredPathInspection {
        observations,
        leases,
        signals,
        evidence,
    }
}

fn rejected_path_inspection(
    path: &RepoPath,
    signal: &GateSignal,
) -> PreWriteDeclaredPathInspection {
    PreWriteDeclaredPathInspection {
        path: RepoPathProjection::from(path),
        lease: None,
        rejection: Some(signal.clone()),
    }
}

fn write_lease(observation: &WriteTargetObservation) -> WriteLease {
    let kind = match observation.kind {
        WriteTargetKind::ExistingFile => WriteLeaseKind::ExistingFile,
        WriteTargetKind::ExistingDirectory => WriteLeaseKind::Directory,
        WriteTargetKind::NewFile => WriteLeaseKind::NewFile,
    };
    WriteLease {
        path: RepoPathProjection::from(&observation.path),
        kind,
        physical_identity: observation.physical_identity.clone(),
        nearest_existing_parent: observation
            .nearest_existing_parent
            .as_ref()
            .map(RepoPathProjection::from),
        prefix_identities: observation
            .prefix_identities
            .iter()
            .map(|(path, physical_identity)| PathPrefixIdentity {
                path: RepoPathProjection::from(path),
                physical_identity: physical_identity.clone(),
            })
            .collect(),
    }
}

fn write_target_signal(path: RepoPathProjection, error: WriteTargetError) -> GateSignal {
    use lumin_evidence::DeclaredPathUnsupportedReason as Reason;
    let reason = match error {
        WriteTargetError::UnboundedDirectory => Reason::UnboundedDirectory,
        WriteTargetError::MissingParent(_) => Reason::MissingParent,
        WriteTargetError::OutsideRoot(_) => Reason::OutsideRoot,
        WriteTargetError::LinkedDirectory(_) => Reason::SymlinkOrAliasedPrefix,
        WriteTargetError::NonRegular(_) | WriteTargetError::Io { .. } => Reason::NonRegular,
        WriteTargetError::PhysicalIdentity(_) => {
            return GateSignal::AnalysisFailed {
                detail: error.to_string(),
            };
        }
    };
    unsupported_path(path, reason)
}

fn unsupported_path(
    path: RepoPathProjection,
    reason: lumin_evidence::DeclaredPathUnsupportedReason,
) -> GateSignal {
    GateSignal::DeclaredPathUnsupported { path, reason }
}

pub(super) fn expand_write_domain(
    root: &Path,
    observations: &[WriteTargetObservation],
    mut leases: Vec<WriteLease>,
    capture: &RepositoryCapture,
    unavailable_targets: &BTreeSet<RepoPath>,
) -> (
    Vec<WriteLease>,
    Vec<PhysicalAliasClosureRecord>,
    Vec<GateSignal>,
) {
    let semantic_paths = match captured_input_physical_paths(&capture.snapshot.inputs) {
        Ok(paths) => paths,
        Err(signal) => return (leases, Vec::new(), vec![signal]),
    };
    let mut seeds = BTreeSet::new();
    let mut signals = Vec::new();
    let mut inferred_observations = Vec::new();
    for path in &capture.inferred_write_paths {
        match lumin_inventory::inspect_write_target(root, path) {
            Ok(observation) if observation.kind == WriteTargetKind::ExistingFile => {
                match lumin_inventory::rehash_existing_write_target(root, &observation) {
                    Ok(payload_sha256)
                        if inferred_observation_matches_capture(
                            &capture.snapshot.inputs,
                            &observation,
                            &payload_sha256,
                        ) =>
                    {
                        leases.push(write_lease(&observation));
                        inferred_observations.push(observation);
                    }
                    Ok(_) => signals.push(GateSignal::ProtectedInputChanged {
                        paths: vec![RepoPathProjection::from(path)],
                    }),
                    Err(error) => signals.push(GateSignal::AnalysisFailed {
                        detail: error.to_string(),
                    }),
                }
            }
            Ok(_) => signals.push(GateSignal::ProtectedInputChanged {
                paths: vec![RepoPathProjection::from(path)],
            }),
            Err(error) => signals.push(write_target_signal(RepoPathProjection::from(path), error)),
        }
    }
    for observation in observations.iter().chain(&inferred_observations) {
        match observation.kind {
            WriteTargetKind::ExistingFile => {
                if semantic_paths.contains(&observation.path) {
                    seeds.insert(observation.path.clone());
                } else if unavailable_targets.contains(&observation.path) {
                    // The engine registry owns the visible unavailable-capability fact.
                    // No compiled language owner may route this path into semantic analysis.
                } else {
                    signals.push(unsupported_path(
                        RepoPathProjection::from(&observation.path),
                        lumin_evidence::DeclaredPathUnsupportedReason::NotAnalyzedSource,
                    ));
                }
            }
            WriteTargetKind::ExistingDirectory => {
                seeds.extend(
                    semantic_paths
                        .iter()
                        .filter(|path| path.is_within(&observation.path))
                        .cloned(),
                );
            }
            WriteTargetKind::NewFile => {}
        }
    }

    let mut groups = BTreeMap::<PhysicalFileIdentity, BTreeSet<RepoPath>>::new();
    match lumin_inventory::physical_alias_write_closures(
        root,
        &seeds.iter().cloned().collect::<Vec<_>>(),
        &semantic_paths,
    ) {
        Ok(closures) => {
            for (seed, closure) in closures {
                if closure.members.is_empty() {
                    signals.push(unsupported_path(
                        RepoPathProjection::from(&seed),
                        lumin_evidence::DeclaredPathUnsupportedReason::NotAnalyzedSource,
                    ));
                    continue;
                }
                groups
                    .entry(closure.physical_identity)
                    .or_default()
                    .extend(closure.members);
            }
        }
        Err(error) => signals.push(GateSignal::AnalysisFailed {
            detail: error.to_string(),
        }),
    }
    for member in groups.values().flatten() {
        match lumin_inventory::inspect_write_target(root, member) {
            Ok(observation) => leases.push(write_lease(&observation)),
            Err(error) => signals.push(GateSignal::AnalysisFailed {
                detail: error.to_string(),
            }),
        }
    }
    let alias_closures = alias_closure_records(groups);
    leases.sort();
    leases.dedup();
    (leases, alias_closures, signals)
}

#[cfg(test)]
pub(super) fn revalidate_write_domain(
    root: &Path,
    expected_leases: &[WriteLease],
    expected_alias_closures: &[PhysicalAliasClosureRecord],
    current_source_paths: &[RepoPath],
) -> Vec<GateSignal> {
    let observation = observe_write_domain(
        root,
        expected_leases,
        current_source_paths,
        &BTreeSet::new(),
    );
    let mut signals = observation
        .failures
        .iter()
        .cloned()
        .map(|detail| GateSignal::AnalysisFailed { detail })
        .collect::<Vec<_>>();
    if !observation.drift_paths.is_empty() {
        signals.push(GateSignal::ProtectedInputChanged {
            paths: observation.drift_paths.clone(),
        });
        return signals;
    }

    let mut expected_leases = expected_leases.to_vec();
    expected_leases.sort();
    expected_leases.dedup();
    let expected_alias_closures = normalized_alias_closures(expected_alias_closures.to_vec());
    if observation.leases == expected_leases
        && observation.alias_closures == expected_alias_closures
    {
        return signals;
    }

    let mut changed = expected_leases
        .iter()
        .chain(&observation.leases)
        .map(|lease| lease.path.clone())
        .chain(
            expected_alias_closures
                .iter()
                .chain(&observation.alias_closures)
                .flat_map(|closure| closure.members.iter().cloned()),
        )
        .collect::<Vec<_>>();
    changed.sort();
    changed.dedup();
    signals.push(GateSignal::ProtectedInputChanged { paths: changed });
    signals
}

pub(super) struct WriteDomainObservation {
    pub(super) leases: Vec<WriteLease>,
    pub(super) alias_closures: Vec<PhysicalAliasClosureRecord>,
    pub(super) drift_paths: Vec<RepoPathProjection>,
    pub(super) failures: Vec<String>,
}

#[cfg(test)]
pub(super) fn observe_write_domain(
    root: &Path,
    lease_paths: &[WriteLease],
    current_source_paths: &[RepoPath],
    unavailable_targets: &BTreeSet<RepoPath>,
) -> WriteDomainObservation {
    let mut semantic_paths = current_source_paths.to_vec();
    semantic_paths.sort();
    semantic_paths.dedup();
    let mut leases = Vec::new();
    let mut seeds = BTreeSet::new();
    let mut failures = Vec::new();
    let mut drift_paths = Vec::new();
    semantic_paths.retain(
        |path| match lumin_inventory::inspect_write_target(root, path) {
            Ok(observation) if observation.kind == WriteTargetKind::NewFile => {
                drift_paths.push(RepoPathProjection::from(path));
                false
            }
            Ok(_) | Err(WriteTargetError::LinkedDirectory(_)) => true,
            Err(error) => {
                failures.push(error.to_string());
                false
            }
        },
    );

    for expected in lease_paths {
        let path = match RepoPath::from_canonical_bytes(&expected.path.canonical) {
            Ok(path) if RepoPathProjection::from(&path) == expected.path => path,
            Ok(_) => {
                failures.push(format!(
                    "stored write lease projection round-trip failed for {}",
                    expected.path.display
                ));
                continue;
            }
            Err(error) => {
                failures.push(format!(
                    "stored write lease path is not canonical: {} ({error})",
                    expected.path.display
                ));
                continue;
            }
        };
        match lumin_inventory::inspect_write_target(root, &path) {
            Ok(observation) => {
                match observation.kind {
                    WriteTargetKind::ExistingFile => {
                        if semantic_paths.binary_search(&observation.path).is_ok()
                            || !unavailable_targets.contains(&observation.path)
                        {
                            seeds.insert(observation.path.clone());
                        }
                    }
                    WriteTargetKind::ExistingDirectory => {
                        seeds.extend(
                            semantic_paths
                                .iter()
                                .filter(|candidate| candidate.is_within(&observation.path))
                                .cloned(),
                        );
                    }
                    WriteTargetKind::NewFile => {}
                }
                leases.push(write_lease(&observation));
            }
            Err(error) => failures.push(error.to_string()),
        }
    }

    let mut groups = BTreeMap::<PhysicalFileIdentity, BTreeSet<RepoPath>>::new();
    match lumin_inventory::physical_alias_write_closures(
        root,
        &seeds.iter().cloned().collect::<Vec<_>>(),
        &semantic_paths,
    ) {
        Ok(closures) => {
            for closure in closures.into_values() {
                groups
                    .entry(closure.physical_identity)
                    .or_default()
                    .extend(closure.members);
            }
        }
        Err(error) => failures.push(error.to_string()),
    }
    for member in groups.values().flatten() {
        match lumin_inventory::inspect_write_target(root, member) {
            Ok(observation) => leases.push(write_lease(&observation)),
            Err(error) => classify_disappeared_domain_path(
                root,
                member,
                error.to_string(),
                &mut drift_paths,
                &mut failures,
            ),
        }
    }

    failures.sort();
    failures.dedup();
    drift_paths.sort();
    drift_paths.dedup();
    leases.sort();
    leases.dedup();
    let alias_closures = normalized_alias_closures(alias_closure_records(groups));
    WriteDomainObservation {
        leases,
        alias_closures,
        drift_paths,
        failures,
    }
}

/// Revalidates a write domain from a complete, freshly authenticated semantic-input snapshot.
///
/// Capture has already opened every source and retained its physical identity. Building the
/// candidate alias groups from that snapshot lets final validation reopen only the leased paths
/// and their actual aliases. A path changed after capture is still observed directly and becomes
/// fail-closed drift.
pub(super) fn observe_write_domain_from_semantic_inputs(
    root: &Path,
    lease_paths: &[WriteLease],
    current_inputs: &[SemanticInputRecord],
    unavailable_targets: &BTreeSet<RepoPath>,
) -> WriteDomainObservation {
    let mut semantic_identities = BTreeMap::<RepoPath, PhysicalFileIdentity>::new();
    let mut leases = Vec::new();
    let mut seed_identities = BTreeSet::new();
    let mut failures = Vec::new();
    let mut drift_paths = Vec::new();

    for input in current_inputs {
        let Some(identity) = &input.physical_identity else {
            continue;
        };
        let path = match RepoPath::from_canonical_bytes(&input.path.canonical) {
            Ok(path) if RepoPathProjection::from(&path) == input.path => path,
            Ok(_) => {
                failures.push(format!(
                    "captured semantic path projection round-trip failed for {}",
                    input.path.display
                ));
                continue;
            }
            Err(error) => {
                failures.push(format!(
                    "captured semantic path is not canonical: {} ({error})",
                    input.path.display
                ));
                continue;
            }
        };
        if semantic_identities
            .insert(path.clone(), identity.clone())
            .is_some_and(|prior| prior != *identity)
        {
            failures.push(format!(
                "captured semantic path has conflicting physical identities: {}",
                path.display_escaped()
            ));
        }
    }

    for expected in lease_paths {
        let path = match RepoPath::from_canonical_bytes(&expected.path.canonical) {
            Ok(path) if RepoPathProjection::from(&path) == expected.path => path,
            Ok(_) => {
                failures.push(format!(
                    "stored write lease projection round-trip failed for {}",
                    expected.path.display
                ));
                continue;
            }
            Err(error) => {
                failures.push(format!(
                    "stored write lease path is not canonical: {} ({error})",
                    expected.path.display
                ));
                continue;
            }
        };
        match lumin_inventory::inspect_write_target(root, &path) {
            Ok(observation) => {
                match observation.kind {
                    WriteTargetKind::ExistingFile => {
                        if (semantic_identities.contains_key(&observation.path)
                            || !unavailable_targets.contains(&observation.path))
                            && let Some(identity) = &observation.physical_identity
                        {
                            seed_identities.insert(identity.clone());
                            if semantic_identities
                                .get(&observation.path)
                                .is_some_and(|captured| captured != identity)
                            {
                                drift_paths.push(RepoPathProjection::from(&observation.path));
                            }
                        }
                    }
                    WriteTargetKind::ExistingDirectory => {
                        seed_identities.extend(
                            semantic_identities
                                .iter()
                                .filter(|(candidate, _)| candidate.is_within(&observation.path))
                                .map(|(_, identity)| identity.clone()),
                        );
                    }
                    WriteTargetKind::NewFile => {}
                }
                leases.push(write_lease(&observation));
            }
            Err(error) => failures.push(error.to_string()),
        }
    }

    let mut groups = seed_identities
        .into_iter()
        .map(|identity| (identity, BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    for (path, identity) in &semantic_identities {
        if let Some(members) = groups.get_mut(identity) {
            members.insert(path.clone());
        }
    }
    for (identity, members) in &groups {
        for member in members {
            match lumin_inventory::inspect_write_target(root, member) {
                Ok(observation)
                    if observation.kind == WriteTargetKind::ExistingFile
                        && observation.physical_identity.as_ref() == Some(identity) =>
                {
                    leases.push(write_lease(&observation));
                }
                Ok(observation) => {
                    drift_paths.push(RepoPathProjection::from(&observation.path));
                }
                Err(error) => classify_disappeared_domain_path(
                    root,
                    member,
                    error.to_string(),
                    &mut drift_paths,
                    &mut failures,
                ),
            }
        }
    }

    failures.sort();
    failures.dedup();
    drift_paths.sort();
    drift_paths.dedup();
    leases.sort();
    leases.dedup();
    WriteDomainObservation {
        leases,
        alias_closures: normalized_alias_closures(alias_closure_records(groups)),
        drift_paths,
        failures,
    }
}

fn classify_disappeared_domain_path(
    root: &Path,
    path: &RepoPath,
    failure: String,
    drift_paths: &mut Vec<RepoPathProjection>,
    failures: &mut Vec<String>,
) {
    match lumin_inventory::inspect_write_target(root, path) {
        Ok(observation) if observation.kind == WriteTargetKind::NewFile => {
            drift_paths.push(RepoPathProjection::from(path));
        }
        _ => failures.push(failure),
    }
}

pub(super) fn captured_input_physical_paths(
    inputs: &[SemanticInputRecord],
) -> Result<Vec<RepoPath>, GateSignal> {
    let mut paths = inputs
        .iter()
        .filter(|input| input.physical_identity.is_some())
        .map(|input| {
            RepoPath::from_canonical_bytes(&input.path.canonical).map_err(|error| {
                GateSignal::AnalysisFailed {
                    detail: format!(
                        "captured semantic path is not canonical: {} ({error})",
                        input.path.display
                    ),
                }
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn normalized_alias_closures(
    mut closures: Vec<PhysicalAliasClosureRecord>,
) -> Vec<PhysicalAliasClosureRecord> {
    for closure in &mut closures {
        closure.members.sort();
        closure.members.dedup();
    }
    closures.sort();
    closures.dedup();
    closures
}

fn inferred_observation_matches_capture(
    inputs: &[SemanticInputRecord],
    observation: &WriteTargetObservation,
    payload_sha256: &str,
) -> bool {
    let path = RepoPathProjection::from(&observation.path);
    let mut matching = inputs.iter().filter(|input| input.path == path);
    let Some(captured) = matching.next() else {
        return false;
    };
    matching.next().is_none()
        && captured.state == SemanticInputState::ConfigPresent
        && captured.payload_sha256.as_deref() == Some(payload_sha256)
        && captured.physical_identity == observation.physical_identity
        && captured.absence_parent.is_none()
}

fn alias_closure_records(
    groups: BTreeMap<PhysicalFileIdentity, BTreeSet<RepoPath>>,
) -> Vec<PhysicalAliasClosureRecord> {
    groups
        .into_iter()
        .map(|(physical_identity, members)| PhysicalAliasClosureRecord {
            physical_identity,
            members: members.iter().map(RepoPathProjection::from).collect(),
        })
        .collect()
}

pub(super) fn protected_semantic_inputs(
    capture: &RepositoryCapture,
    leases: &[WriteLease],
) -> Vec<SemanticInputRecord> {
    lumin_evidence::derive_protected_semantic_inputs(&capture.snapshot, leases)
}

pub(super) fn close_alias_topology(
    root: &Path,
    gate: &GateRecord,
    capture: &RepositoryCapture,
) -> (
    Vec<WriteLease>,
    Vec<PhysicalAliasClosureRecord>,
    Vec<GateSignal>,
) {
    let mut signals = validate_stable_lease_parents(root, &gate.leased_write_set);
    let observation = observe_write_domain_from_semantic_inputs(
        root,
        &gate.leased_write_set,
        &capture.snapshot.inputs,
        &BTreeSet::new(),
    );
    if !observation.failures.is_empty() || !observation.drift_paths.is_empty() {
        signals.extend(
            observation
                .failures
                .into_iter()
                .map(|detail| GateSignal::AnalysisFailed { detail }),
        );
        if !observation.drift_paths.is_empty() {
            signals.push(GateSignal::ProtectedInputChanged {
                paths: observation.drift_paths,
            });
        }
        return (Vec::new(), Vec::new(), signals);
    }
    let leases = observation.leases;
    let alias_closures = observation.alias_closures;
    for member in alias_closures.iter().flat_map(|closure| &closure.members) {
        if !gate
            .leased_write_set
            .iter()
            .any(|lease| lease.covers(member))
        {
            signals.push(GateSignal::UnplannedWrite {
                paths: vec![member.clone()],
            });
        }
    }
    (leases, alias_closures, signals)
}

pub(super) fn lease_containment_signals(root: &Path, leases: &[WriteLease]) -> Vec<GateSignal> {
    let mut stale = Vec::new();
    let mut blocked = Vec::new();
    let mut failures = Vec::new();
    for lease in leases {
        let path = match RepoPath::from_canonical_bytes(&lease.path.canonical) {
            Ok(path) if RepoPathProjection::from(&path) == lease.path => path,
            Ok(_) => {
                failures.push(format!(
                    "stored write lease projection round-trip failed for {}",
                    lease.path.display
                ));
                continue;
            }
            Err(error) => {
                failures.push(format!(
                    "stored write lease path is not canonical: {} ({error})",
                    lease.path.display
                ));
                continue;
            }
        };
        match lumin_inventory::validate_caller_entries(root, std::slice::from_ref(&path)) {
            Ok(()) => {}
            Err(InventoryError::EntryEscapesRoot(_)) => match lease.kind {
                WriteLeaseKind::NewFile => blocked.push(lease.path.clone()),
                WriteLeaseKind::ExistingFile | WriteLeaseKind::Directory => {
                    stale.push(lease.path.clone());
                }
            },
            Err(error) => failures.push(error.to_string()),
        }
    }

    stale.sort();
    stale.dedup();
    blocked.sort();
    blocked.dedup();
    failures.sort();
    failures.dedup();
    let mut signals = Vec::new();
    if !stale.is_empty() {
        signals.push(GateSignal::ProtectedInputChanged { paths: stale });
    }
    if !blocked.is_empty() {
        signals.push(GateSignal::PlannedPathContainmentViolation { paths: blocked });
    }
    signals.extend(
        failures
            .into_iter()
            .map(|detail| GateSignal::AnalysisFailed { detail }),
    );
    signals
}

fn validate_stable_lease_parents(root: &Path, leases: &[WriteLease]) -> Vec<GateSignal> {
    let mut signals = lease_containment_signals(root, leases);
    let escaped = signals
        .iter()
        .flat_map(|signal| match signal {
            GateSignal::ProtectedInputChanged { paths }
            | GateSignal::PlannedPathContainmentViolation { paths } => paths.as_slice(),
            _ => &[],
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut stale = Vec::new();
    let mut incomplete = Vec::new();
    for lease in leases {
        if escaped.contains(&lease.path) {
            continue;
        }
        for prefix in &lease.prefix_identities {
            let prefix_path = match RepoPath::from_canonical_bytes(&prefix.path.canonical) {
                Ok(path) => path,
                Err(error) => {
                    incomplete.push(format!(
                        "stored write lease prefix is not canonical: {} ({error})",
                        prefix.path.display
                    ));
                    continue;
                }
            };
            match lumin_inventory::directory_physical_identity(root, &prefix_path) {
                Ok(identity) if identity == prefix.physical_identity => {}
                Ok(_) | Err(WriteTargetError::OutsideRoot(_)) => {
                    stale.push(prefix.path.clone());
                }
                Err(error) => incomplete.push(error.to_string()),
            }
        }
        let path = match RepoPath::from_canonical_bytes(&lease.path.canonical) {
            Ok(path) => path,
            Err(error) => {
                incomplete.push(format!(
                    "stored write lease path is not canonical: {} ({error})",
                    lease.path.display
                ));
                continue;
            }
        };
        match lease.kind {
            WriteLeaseKind::Directory => match lumin_inventory::inspect_write_target(root, &path) {
                Ok(observation)
                    if observation.kind == WriteTargetKind::ExistingDirectory
                        && observation.physical_identity == lease.physical_identity => {}
                Ok(_) | Err(WriteTargetError::OutsideRoot(_)) => stale.push(lease.path.clone()),
                Err(error) => incomplete.push(error.to_string()),
            },
            WriteLeaseKind::NewFile => {
                let nearest_matches =
                    lease
                        .nearest_existing_parent
                        .as_ref()
                        .is_some_and(|nearest| {
                            lease
                                .prefix_identities
                                .last()
                                .is_some_and(|prefix| prefix.path.canonical == nearest.canonical)
                        });
                if !nearest_matches {
                    incomplete.push(format!(
                        "new path omitted its nearest existing parent binding: {}",
                        lease.path.display
                    ));
                    continue;
                }
                match lumin_inventory::inspect_write_target(root, &path) {
                    Ok(observation)
                        if matches!(
                            observation.kind,
                            WriteTargetKind::ExistingFile | WriteTargetKind::NewFile
                        ) => {}
                    Ok(_) | Err(WriteTargetError::OutsideRoot(_)) => {
                        stale.push(lease.path.clone());
                    }
                    Err(error) => incomplete.push(error.to_string()),
                }
            }
            WriteLeaseKind::ExistingFile => {}
        }
    }
    if !stale.is_empty() {
        stale.sort();
        stale.dedup();
        signals.push(GateSignal::ProtectedInputChanged { paths: stale });
    }
    signals.extend(
        incomplete
            .into_iter()
            .map(|detail| GateSignal::AnalysisFailed { detail }),
    );
    signals
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonportable_descendant_of_reserved_state_is_rejected_before_inspection()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let mut native = std::path::PathBuf::from(".lumin");
        native.push(nonportable_component());
        native.push("mod.ts");
        let path = RepoPath::from_native_relative(&native)?;

        let inspection =
            inspect_declared_paths(root.path(), std::slice::from_ref(&path), &BTreeSet::new());

        assert!(inspection.observations.is_empty());
        assert!(inspection.leases.is_empty());
        assert_eq!(
            inspection.signals,
            [GateSignal::DeclaredPathUnsupported {
                path: RepoPathProjection::from(&path),
                reason: lumin_evidence::DeclaredPathUnsupportedReason::ReservedState,
            }]
        );
        Ok(())
    }

    #[cfg(unix)]
    fn nonportable_component() -> std::ffi::OsString {
        use std::os::unix::ffi::OsStringExt;

        std::ffi::OsString::from_vec(vec![b'n', 0x80])
    }

    #[cfg(windows)]
    fn nonportable_component() -> std::ffi::OsString {
        use std::os::windows::ffi::OsStringExt;

        std::ffi::OsString::from_wide(&[b'n' as u16, 0xd800])
    }

    #[test]
    fn atomic_replacement_of_inferred_write_is_rejected_before_lease()
    -> Result<(), Box<dyn std::error::Error>> {
        let (root, capture, path) = inferred_manifest_fixture()?;
        let captured_identity = capture
            .snapshot
            .inputs
            .iter()
            .find(|input| input.path == RepoPathProjection::from(&path))
            .and_then(|input| input.physical_identity.clone())
            .ok_or("captured manifest identity is missing")?;

        let manifest = root.path().join("package.json");
        let replacement = root.path().join("package.replacement.json");
        std::fs::write(&replacement, r#"{"name":"fixture","private":true}"#)?;
        // Allocate the replacement while the captured file still exists so
        // Unix filesystems cannot recycle its inode for the fixture.
        #[cfg(windows)]
        std::fs::remove_file(&manifest)?;
        std::fs::rename(replacement, &manifest)?;
        let replacement = lumin_inventory::inspect_write_target(root.path(), &path)?;
        assert_ne!(
            replacement.physical_identity,
            Some(captured_identity),
            "the fixture did not replace the manifest identity",
        );

        let (leases, alias_closures, signals) =
            expand_write_domain(root.path(), &[], Vec::new(), &capture, &BTreeSet::new());
        assert!(leases.is_empty());
        assert!(alias_closures.is_empty());
        assert_eq!(
            signals,
            [GateSignal::ProtectedInputChanged {
                paths: vec![RepoPathProjection::from(&path)],
            }]
        );
        Ok(())
    }

    #[test]
    fn in_place_rewrite_of_inferred_write_is_rejected_before_lease()
    -> Result<(), Box<dyn std::error::Error>> {
        let (root, capture, path) = inferred_manifest_fixture()?;
        let captured_identity = capture
            .snapshot
            .inputs
            .iter()
            .find(|input| input.path == RepoPathProjection::from(&path))
            .and_then(|input| input.physical_identity.clone())
            .ok_or("captured manifest identity is missing")?;

        std::fs::write(
            root.path().join("package.json"),
            r#"{"name":"fixture","private":true,"description":"external rewrite"}"#,
        )?;
        let rewritten = lumin_inventory::inspect_write_target(root.path(), &path)?;
        assert_eq!(
            rewritten.physical_identity,
            Some(captured_identity),
            "the in-place fixture unexpectedly replaced the manifest identity",
        );

        let (leases, alias_closures, signals) =
            expand_write_domain(root.path(), &[], Vec::new(), &capture, &BTreeSet::new());
        assert!(leases.is_empty());
        assert!(alias_closures.is_empty());
        assert_eq!(
            signals,
            [GateSignal::ProtectedInputChanged {
                paths: vec![RepoPathProjection::from(&path)],
            }]
        );
        Ok(())
    }

    fn inferred_manifest_fixture()
    -> Result<(tempfile::TempDir, RepositoryCapture, RepoPath), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        std::fs::create_dir(root.path().join("src"))?;
        std::fs::write(
            root.path().join("package.json"),
            r#"{"name":"fixture","private":true}"#,
        )?;
        std::fs::write(root.path().join("src/main.ts"), "console.log('fixture');\n")?;
        let capture = crate::capture_repository(
            root.path(),
            &lumin_inventory::InventoryRequest {
                dependency_intents: vec![lumin_model::DependencyIntent {
                    path: RepoPath::from_portable("src/main.ts")?,
                    dependency: "zod".to_owned(),
                }],
                ..Default::default()
            },
            1,
            None,
        )?;
        Ok((root, capture, RepoPath::from_portable("package.json")?))
    }

    #[test]
    fn final_write_domain_revalidation_detects_a_new_physical_alias()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        std::fs::create_dir(root.path().join("src"))?;
        let source = RepoPath::from_portable("src/main.ts")?;
        let planned = RepoPath::from_portable("src/new.ts")?;
        std::fs::write(root.path().join("src/main.ts"), "export const main = 1;\n")?;
        let inspection = inspect_declared_paths(
            root.path(),
            std::slice::from_ref(&planned),
            &BTreeSet::new(),
        );
        assert!(inspection.signals.is_empty());
        assert_eq!(inspection.leases.len(), 1);
        assert_eq!(inspection.leases[0].kind, WriteLeaseKind::NewFile);

        std::fs::hard_link(
            root.path().join("src/main.ts"),
            root.path().join("src/new.ts"),
        )?;
        let current_source_paths = vec![source.clone(), planned.clone()];
        let signals =
            revalidate_write_domain(root.path(), &inspection.leases, &[], &current_source_paths);

        assert_eq!(signals.len(), 1);
        let GateSignal::ProtectedInputChanged { paths } = &signals[0] else {
            return Err(format!("unexpected final validation signals: {signals:?}").into());
        };
        assert!(paths.contains(&RepoPathProjection::from(&source)));
        assert!(paths.contains(&RepoPathProjection::from(&planned)));
        Ok(())
    }

    #[test]
    fn semantic_input_alias_index_matches_full_observation_and_detects_late_drift()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        std::fs::create_dir(root.path().join("src"))?;
        let source = RepoPath::from_portable("src/main.ts")?;
        let alias = RepoPath::from_portable("src/alias.ts")?;
        std::fs::write(root.path().join("src/main.ts"), "export const main = 1;\n")?;
        std::fs::hard_link(
            root.path().join("src/main.ts"),
            root.path().join("src/alias.ts"),
        )?;
        let expected_lease = write_lease(&lumin_inventory::inspect_write_target(
            root.path(),
            &source,
        )?);
        let inputs = [&source, &alias]
            .into_iter()
            .map(|path| {
                let observation = lumin_inventory::inspect_write_target(root.path(), path)?;
                Ok(SemanticInputRecord {
                    path: RepoPathProjection::from(path),
                    state: SemanticInputState::Source,
                    payload_sha256: Some("0".repeat(64)),
                    physical_identity: observation.physical_identity,
                    absence_parent: None,
                    physical_redirect_sha256: None,
                })
            })
            .collect::<Result<Vec<_>, WriteTargetError>>()?;

        let full = observe_write_domain(
            root.path(),
            std::slice::from_ref(&expected_lease),
            &[source.clone(), alias.clone()],
            &BTreeSet::new(),
        );
        let indexed = observe_write_domain_from_semantic_inputs(
            root.path(),
            std::slice::from_ref(&expected_lease),
            &inputs,
            &BTreeSet::new(),
        );
        assert_eq!(indexed.leases, full.leases);
        assert_eq!(indexed.alias_closures, full.alias_closures);
        assert_eq!(indexed.drift_paths, full.drift_paths);
        assert_eq!(indexed.failures, full.failures);

        std::fs::remove_file(root.path().join("src/alias.ts"))?;
        std::fs::write(
            root.path().join("src/alias.ts"),
            "export const other = 2;\n",
        )?;
        let drifted = observe_write_domain_from_semantic_inputs(
            root.path(),
            &[expected_lease],
            &inputs,
            &BTreeSet::new(),
        );
        assert_eq!(drifted.drift_paths, [RepoPathProjection::from(&alias)]);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn unchanged_directory_and_redirect_semantic_paths_are_not_drift()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        std::fs::create_dir(root.path().join("config"))?;
        std::fs::create_dir(root.path().join("src"))?;
        std::fs::write(root.path().join("src/main.ts"), "export const main = 1;\n")?;
        symlink(outside.path(), root.path().join("redirect"))?;

        let source = RepoPath::from_portable("src/main.ts")?;
        let config = RepoPath::from_portable("config")?;
        let redirect = RepoPath::from_portable("redirect")?;
        let source_observation = lumin_inventory::inspect_write_target(root.path(), &source)?;
        let expected_lease = write_lease(&source_observation);

        let observed = observe_write_domain(
            root.path(),
            std::slice::from_ref(&expected_lease),
            &[source.clone(), config, redirect],
            &BTreeSet::new(),
        );

        assert!(observed.failures.is_empty());
        assert!(observed.drift_paths.is_empty());
        assert_eq!(observed.leases, [expected_lease]);
        assert_eq!(observed.alias_closures.len(), 1);
        assert_eq!(
            observed.alias_closures[0].members,
            [RepoPathProjection::from(&source)]
        );
        Ok(())
    }
}
