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
        match lumin_inventory::is_reserved_state_path(path) {
            Ok(true) => {
                signals.push(unsupported_path(
                    projection,
                    lumin_evidence::DeclaredPathUnsupportedReason::ReservedState,
                ));
                continue;
            }
            Err(_) => {
                signals.push(unsupported_path(
                    projection,
                    lumin_evidence::DeclaredPathUnsupportedReason::NotAnalyzedSource,
                ));
                continue;
            }
            Ok(false) => {}
        }
        match lumin_inventory::inspect_write_target(root, path) {
            Ok(observation) => {
                let supported_source = lumin_inventory::is_supported_source_path(path);
                let unsupported_native_file = observation.kind == WriteTargetKind::ExistingFile
                    && path.file_name_portable().is_none()
                    && !supported_source;
                if unsupported_native_file
                    || (observation.kind == WriteTargetKind::NewFile && !supported_source)
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

pub(super) fn revalidate_write_domain(
    root: &Path,
    expected_leases: &[WriteLease],
    expected_alias_closures: &[PhysicalAliasClosureRecord],
    current_source_paths: &[RepoPath],
) -> Vec<GateSignal> {
    let (leases, alias_closures) =
        match observe_write_domain(root, expected_leases, current_source_paths) {
            Ok(domain) => domain,
            Err(signals) => return signals,
        };

    let mut expected_leases = expected_leases.to_vec();
    expected_leases.sort();
    expected_leases.dedup();
    let expected_alias_closures = normalized_alias_closures(expected_alias_closures.to_vec());
    if leases == expected_leases && alias_closures == expected_alias_closures {
        return Vec::new();
    }

    let mut changed = expected_leases
        .iter()
        .chain(&leases)
        .map(|lease| lease.path.clone())
        .chain(
            expected_alias_closures
                .iter()
                .chain(&alias_closures)
                .flat_map(|closure| closure.members.iter().cloned()),
        )
        .collect::<Vec<_>>();
    changed.sort();
    changed.dedup();
    vec![GateSignal::ProtectedInputChanged { paths: changed }]
}

fn observe_write_domain(
    root: &Path,
    lease_paths: &[WriteLease],
    current_source_paths: &[RepoPath],
) -> Result<(Vec<WriteLease>, Vec<PhysicalAliasClosureRecord>), Vec<GateSignal>> {
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
                        seeds.insert(observation.path.clone());
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
    for seed in seeds {
        match lumin_inventory::physical_alias_write_closure(root, &seed, &semantic_paths) {
            Ok(closure) => {
                groups
                    .entry(closure.physical_identity)
                    .or_default()
                    .extend(closure.members);
            }
            Err(error) => classify_disappeared_domain_path(
                root,
                &seed,
                error.to_string(),
                &mut drift_paths,
                &mut failures,
            ),
        }
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

    if !failures.is_empty() || !drift_paths.is_empty() {
        let mut signals = Vec::new();
        failures.sort();
        failures.dedup();
        signals.extend(
            failures
                .into_iter()
                .map(|detail| GateSignal::AnalysisFailed { detail }),
        );
        drift_paths.sort();
        drift_paths.dedup();
        if !drift_paths.is_empty() {
            signals.push(GateSignal::ProtectedInputChanged { paths: drift_paths });
        }
        return Err(signals);
    }

    leases.sort();
    leases.dedup();
    let alias_closures = normalized_alias_closures(alias_closure_records(groups));
    Ok((leases, alias_closures))
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
) -> (
    Vec<WriteLease>,
    Vec<PhysicalAliasClosureRecord>,
    Vec<GateSignal>,
) {
    let mut signals = validate_stable_lease_parents(root, &gate.leased_write_set);
    let semantic_paths = match captured_input_physical_paths(&capture.snapshot.inputs) {
        Ok(paths) => paths,
        Err(signal) => {
            signals.push(signal);
            return (Vec::new(), Vec::new(), signals);
        }
    };
    let (leases, alias_closures) =
        match observe_write_domain(root, &gate.leased_write_set, &semantic_paths) {
            Ok(domain) => domain,
            Err(domain_signals) => {
                signals.extend(domain_signals);
                return (Vec::new(), Vec::new(), signals);
            }
        };
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
    fn nonportable_descendant_of_reserved_state_is_rejected_before_inspection()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let mut native = std::path::PathBuf::from(".lumin");
        native.push(nonportable_component());
        native.push("mod.ts");
        let path = RepoPath::from_native_relative(&native)?;

        let inspection = inspect_declared_paths(root.path(), std::slice::from_ref(&path));

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
            expand_write_domain(root.path(), &[], Vec::new(), &capture);
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
            expand_write_domain(root.path(), &[], Vec::new(), &capture);
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

    #[test]
    fn final_write_domain_revalidation_detects_a_new_physical_alias()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        std::fs::create_dir(root.path().join("src"))?;
        let source = RepoPath::from_portable("src/main.ts")?;
        let planned = RepoPath::from_portable("src/new.ts")?;
        std::fs::write(root.path().join("src/main.ts"), "export const main = 1;\n")?;
        let inspection = inspect_declared_paths(root.path(), std::slice::from_ref(&planned));
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
        )
        .map_err(|signals| format!("unchanged semantic paths were rejected: {signals:?}"))?;

        assert_eq!(observed.0, [expected_lease]);
        assert_eq!(observed.1.len(), 1);
        assert_eq!(observed.1[0].members, [RepoPathProjection::from(&source)]);
        Ok(())
    }
}
