use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use lumin_evidence::{
    GateRecord, GateSignal, PathPrefixIdentity, PhysicalAliasClosureRecord, RepoPathProjection,
    SemanticInputRecord, SemanticInputState, WriteLease, WriteLeaseKind,
};
use lumin_inventory::{WriteTargetError, WriteTargetKind, WriteTargetObservation};
use lumin_model::{PhysicalFileIdentity, RepoPath};

use super::RepositoryCapture;

pub(super) struct DeclaredPathInspection {
    pub(super) observations: Vec<WriteTargetObservation>,
    pub(super) leases: Vec<WriteLease>,
    pub(super) signals: Vec<GateSignal>,
}

pub(super) fn inspect_declared_paths(root: &Path, paths: &[RepoPath]) -> DeclaredPathInspection {
    let mut observations = Vec::new();
    let mut leases = Vec::new();
    let mut signals = Vec::new();
    for path in paths {
        let projection = RepoPathProjection::from(path);
        let Some(portable) = path.portable() else {
            signals.push(unsupported_path(
                projection,
                lumin_evidence::DeclaredPathUnsupportedReason::NotAnalyzedSource,
            ));
            continue;
        };
        if portable == ".lumin" || portable.starts_with(".lumin/") {
            signals.push(unsupported_path(
                projection,
                lumin_evidence::DeclaredPathUnsupportedReason::ReservedState,
            ));
            continue;
        }
        match lumin_inventory::inspect_write_target(root, path) {
            Ok(observation) => {
                if observation.kind == WriteTargetKind::NewFile
                    && !lumin_inventory::is_supported_source_path(path)
                {
                    signals.push(unsupported_path(
                        projection,
                        lumin_evidence::DeclaredPathUnsupportedReason::NotAnalyzedSource,
                    ));
                    continue;
                }
                leases.push(write_lease(&observation));
                observations.push(observation);
            }
            Err(error) => signals.push(write_target_signal(projection, error)),
        }
    }
    leases.sort();
    leases.dedup();
    DeclaredPathInspection {
        observations,
        leases,
        signals,
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
) -> (
    Vec<WriteLease>,
    Vec<PhysicalAliasClosureRecord>,
    Vec<GateSignal>,
) {
    let semantic_paths = match captured_physical_paths(capture) {
        Ok(paths) => paths,
        Err(signal) => return (leases, Vec::new(), vec![signal]),
    };
    let mut seeds = BTreeSet::new();
    let mut signals = Vec::new();
    for observation in observations {
        match observation.kind {
            WriteTargetKind::ExistingFile => {
                if semantic_paths.contains(&observation.path) {
                    seeds.insert(observation.path.clone());
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
    for seed in seeds {
        match lumin_inventory::physical_alias_write_closure(root, &seed, &semantic_paths) {
            Ok(closure) if closure.members.is_empty() => signals.push(unsupported_path(
                RepoPathProjection::from(&seed),
                lumin_evidence::DeclaredPathUnsupportedReason::NotAnalyzedSource,
            )),
            Ok(closure) => {
                groups
                    .entry(closure.physical_identity)
                    .or_default()
                    .extend(closure.members);
            }
            Err(error) => signals.push(GateSignal::AnalysisFailed {
                detail: error.to_string(),
            }),
        }
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
    let source_paths = capture
        .snapshot
        .inputs
        .iter()
        .filter(|input| input.state == SemanticInputState::Source)
        .map(|input| input.path.canonical.as_slice())
        .collect::<BTreeSet<_>>();
    let protect_all_sources = leases
        .iter()
        .any(|lease| lease.kind == WriteLeaseKind::NewFile)
        || leases
            .iter()
            .any(|lease| lease.kind == WriteLeaseKind::Directory);
    let mut selected = if protect_all_sources {
        capture
            .source_paths
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
    } else {
        leases
            .iter()
            .filter(|lease| lease.kind == WriteLeaseKind::ExistingFile)
            .filter_map(|lease| {
                capture
                    .source_paths
                    .iter()
                    .find(|path| path.canonical_bytes() == lease.path.canonical)
                    .cloned()
            })
            .collect::<BTreeSet<_>>()
    };
    let mut frontier = selected.iter().cloned().collect::<Vec<_>>();
    while let Some(path) = frontier.pop() {
        let Some(neighbors) = capture.source_adjacency.get(&path) else {
            continue;
        };
        for neighbor in neighbors {
            if selected.insert(neighbor.clone()) {
                frontier.push(neighbor.clone());
            }
        }
    }
    let selected_keys = selected
        .iter()
        .map(|path| path.canonical_bytes())
        .collect::<BTreeSet<_>>();
    let mut protected = capture
        .snapshot
        .inputs
        .iter()
        .filter(|input| semantic_input_requires_protection(input, &source_paths, &selected_keys))
        .cloned()
        .collect::<Vec<_>>();
    protected.sort();
    protected.dedup();
    protected
}

fn semantic_input_requires_protection(
    input: &SemanticInputRecord,
    source_paths: &BTreeSet<&[u8]>,
    selected_keys: &BTreeSet<&[u8]>,
) -> bool {
    input.physical_redirect_sha256.is_some()
        || !source_paths.contains(input.path.canonical.as_slice())
        || selected_keys.contains(input.path.canonical.as_slice())
}

pub(super) fn close_alias_topology(
    root: &Path,
    gate: &GateRecord,
    capture: &RepositoryCapture,
) -> (Vec<PhysicalAliasClosureRecord>, Vec<GateSignal>) {
    let mut signals = validate_stable_lease_parents(root, &gate.leased_write_set);
    let current_paths = match captured_physical_paths(capture) {
        Ok(paths) => paths,
        Err(signal) => {
            signals.push(signal);
            return (Vec::new(), signals);
        }
    };
    let seeds = current_paths
        .iter()
        .filter(|path| {
            let projection = RepoPathProjection::from(*path);
            gate.leased_write_set
                .iter()
                .any(|lease| lease.covers(&projection))
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut groups = BTreeMap::<PhysicalFileIdentity, BTreeSet<RepoPath>>::new();
    for seed in seeds {
        match lumin_inventory::physical_alias_write_closure(root, &seed, &current_paths) {
            Ok(closure) => {
                for member in &closure.members {
                    let projection = RepoPathProjection::from(member);
                    if !gate
                        .leased_write_set
                        .iter()
                        .any(|lease| lease.covers(&projection))
                    {
                        signals.push(GateSignal::UnplannedWrite {
                            paths: vec![projection],
                        });
                    }
                }
                groups
                    .entry(closure.physical_identity)
                    .or_default()
                    .extend(closure.members);
            }
            Err(error) => signals.push(GateSignal::AnalysisFailed {
                detail: error.to_string(),
            }),
        }
    }
    (alias_closure_records(groups), signals)
}

fn captured_physical_paths(capture: &RepositoryCapture) -> Result<Vec<RepoPath>, GateSignal> {
    let mut paths = capture
        .snapshot
        .inputs
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

fn validate_stable_lease_parents(root: &Path, leases: &[WriteLease]) -> Vec<GateSignal> {
    let mut stale = Vec::new();
    let mut incomplete = Vec::new();
    for lease in leases {
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
    let mut signals = Vec::new();
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
    fn source_backed_redirect_is_protected_without_adjacency_selection()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = RepoPath::from_portable("packages/lib/dist/index.js")?;
        let input = SemanticInputRecord {
            path: RepoPathProjection::from(&path),
            state: SemanticInputState::Source,
            payload_sha256: Some("payload".to_owned()),
            physical_identity: None,
            absence_parent: None,
            physical_redirect_sha256: Some("redirect".to_owned()),
        };
        let source_paths = BTreeSet::from([input.path.canonical.as_slice()]);
        let selected_keys = BTreeSet::new();

        assert!(semantic_input_requires_protection(
            &input,
            &source_paths,
            &selected_keys
        ));
        Ok(())
    }
}
