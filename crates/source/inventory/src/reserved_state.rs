use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lumin_model::{PhysicalFileIdentity, RepoPath, RepoPathError};

use super::{InventoryError, native_relative};
use crate::capture::{PhysicalFileObservation, physical_file_observation};

type IdentityCheck =
    dyn Fn(&PhysicalFileIdentity) -> Result<bool, InventoryError> + Send + Sync + 'static;

/// Store-owned identity membership is evaluated lazily. Ordinary one-link
/// evidence candidates never require enumerating the retained state tree;
/// redirected paths use canonical containment and Linux mount crossings force
/// an identity lookup even when their link count remains one.
#[derive(Clone)]
pub struct ReservedStateIdentityLookup {
    check: Arc<IdentityCheck>,
    observations: Arc<Mutex<LookupObservations>>,
    frozen: bool,
}

#[derive(Default)]
struct LookupObservations {
    root_mount: Option<(PathBuf, Option<u64>)>,
    candidates: BTreeMap<PhysicalFileIdentity, CandidateObservation>,
}

#[derive(Clone, Copy)]
struct CandidateObservation {
    links: u64,
    mount_id: Option<u64>,
    reserved: Option<bool>,
}

impl ReservedStateIdentityLookup {
    pub fn new(
        check: impl Fn(&PhysicalFileIdentity) -> Result<bool, InventoryError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            check: Arc::new(check),
            observations: Arc::new(Mutex::new(LookupObservations::default())),
            frozen: false,
        }
    }

    pub fn from_identities(identities: BTreeSet<PhysicalFileIdentity>) -> Self {
        Self::new(move |identity| Ok(identities.contains(identity)))
    }

    pub fn empty() -> Self {
        Self::from_identities(BTreeSet::new())
    }

    pub(crate) fn contains_candidate(
        &self,
        root: &Path,
        observation: &PhysicalFileObservation,
    ) -> Result<bool, InventoryError> {
        self.contains_candidate_with_link_aliases(root, observation, true)
    }

    fn contains_candidate_with_link_aliases(
        &self,
        root: &Path,
        observation: &PhysicalFileObservation,
        link_aliases_possible: bool,
    ) -> Result<bool, InventoryError> {
        let root_mount = self.root_mount(root)?;
        let requires_lookup = (link_aliases_possible && observation.links > 1)
            || matches!(
                (root_mount, observation.mount_id),
                (Some(root_mount), Some(candidate_mount)) if root_mount != candidate_mount
            );
        {
            let mut observed = self.observations.lock().map_err(|_| {
                InventoryError::PhysicalIdentity(
                    "reserved-state candidate observation lock failed".to_owned(),
                )
            })?;
            if let Some(previous) = observed.candidates.get(&observation.identity) {
                if previous.links != observation.links || previous.mount_id != observation.mount_id
                {
                    return Err(candidate_topology_changed());
                }
                if let Some(reserved) = previous.reserved {
                    return Ok(reserved);
                }
                if !requires_lookup {
                    return Ok(false);
                }
            } else {
                if self.frozen {
                    return Err(candidate_topology_changed());
                }
                observed.candidates.insert(
                    observation.identity.clone(),
                    CandidateObservation {
                        links: observation.links,
                        mount_id: observation.mount_id,
                        reserved: None,
                    },
                );
                if !requires_lookup {
                    return Ok(false);
                }
            }
        }

        let reserved = (self.check)(&observation.identity)?;
        let mut observed = self.observations.lock().map_err(|_| {
            InventoryError::PhysicalIdentity(
                "reserved-state candidate observation lock failed".to_owned(),
            )
        })?;
        let candidate = observed
            .candidates
            .get_mut(&observation.identity)
            .ok_or_else(candidate_topology_changed)?;
        if candidate.links != observation.links || candidate.mount_id != observation.mount_id {
            return Err(candidate_topology_changed());
        }
        candidate.reserved = Some(reserved);
        Ok(reserved)
    }

    /// Freeze a lookup for a final validation that runs under the store lock.
    /// Final validation accepts only the exact link and mount topology observed
    /// during capture, so it never reacquires the store lock or trusts a stale
    /// identity index after a new alias appears.
    pub fn for_final_validation(&self) -> Self {
        Self {
            check: Arc::clone(&self.check),
            observations: Arc::clone(&self.observations),
            frozen: true,
        }
    }

    fn root_mount(&self, root: &Path) -> Result<Option<u64>, InventoryError> {
        let mut observed = self.observations.lock().map_err(|_| {
            InventoryError::PhysicalIdentity(
                "reserved-state candidate observation lock failed".to_owned(),
            )
        })?;
        if let Some((observed_root, mount_id)) = &observed.root_mount {
            if observed_root != root {
                return Err(InventoryError::RepositoryIdentity(
                    "reserved-state lookup was reused for another repository".to_owned(),
                ));
            }
            return Ok(*mount_id);
        }
        let mount_id = repository_mount_id(root)?;
        observed.root_mount = Some((root.to_owned(), mount_id));
        Ok(mount_id)
    }
}

fn repository_mount_id(root: &Path) -> Result<Option<u64>, InventoryError> {
    #[cfg(target_os = "linux")]
    {
        physical_file_observation(root).map(|observation| observation.mount_id)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = root;
        Ok(None)
    }
}

fn candidate_topology_changed() -> InventoryError {
    InventoryError::PhysicalIdentity(
        "semantic input link or mount topology changed after capture".to_owned(),
    )
}

impl fmt::Debug for ReservedStateIdentityLookup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReservedStateIdentityLookup")
            .finish_non_exhaustive()
    }
}

pub fn is_reserved_state_path(path: &RepoPath) -> Result<bool, RepoPathError> {
    let relative = path.to_native_relative()?;
    Ok(relative.iter().next().is_some_and(reserved_state_component))
}

/// Reject caller-owned lexical paths in the reserved namespace without observing
/// the current filesystem. This check is safe before committed-operation replay.
pub fn validate_caller_paths_lexically(entries: &[RepoPath]) -> Result<(), InventoryError> {
    for entry in entries {
        if is_reserved_state_path(entry).map_err(|source| InventoryError::InvalidRepoPath {
            path: entry.display_escaped(),
            source,
        })? {
            return Err(InventoryError::ReservedEntryPath(entry.display_escaped()));
        }
    }
    Ok(())
}

/// Reject caller paths that currently escape the root or enter `.lumin` through
/// a redirected existing path or parent.
pub fn validate_caller_entries(root: &Path, entries: &[RepoPath]) -> Result<(), InventoryError> {
    validate_caller_paths_lexically(entries)?;
    let canonical_root = fs::canonicalize(root)
        .map_err(|error| InventoryError::RepositoryIdentity(error.to_string()))?;
    let canonical_state = canonical_reserved_state(root)?;
    for entry in entries {
        validate_entry_containment(root, &canonical_root, canonical_state.as_deref(), entry)?;
    }
    Ok(())
}

/// Reject existing caller paths whose object identity belongs to the store-owned
/// reserved namespace. Inventory compares only identities supplied by the store.
pub fn validate_caller_entry_identities(
    root: &Path,
    entries: &[RepoPath],
    reserved_state_identities: &BTreeSet<PhysicalFileIdentity>,
) -> Result<(), InventoryError> {
    let lookup = ReservedStateIdentityLookup::from_identities(reserved_state_identities.clone());
    validate_caller_entry_identity_lookup(root, entries, &lookup)
}

pub fn validate_caller_entry_identity_lookup(
    root: &Path,
    entries: &[RepoPath],
    reserved_state_lookup: &ReservedStateIdentityLookup,
) -> Result<(), InventoryError> {
    for entry in entries {
        let native = root.join(native_relative(entry)?);
        let existing = nearest_existing_path(root, &native, entry)?;
        let observation = physical_file_observation(&existing)?;
        let is_directory = fs::metadata(&existing)
            .map_err(|error| InventoryError::PhysicalIdentity(error.to_string()))?
            .is_dir();
        if reserved_state_lookup.contains_candidate_with_link_aliases(
            root,
            &observation,
            !is_directory,
        )? {
            return Err(InventoryError::ReservedEntryPath(entry.display_escaped()));
        }
    }
    Ok(())
}

fn nearest_existing_path(
    root: &Path,
    native: &Path,
    entry: &RepoPath,
) -> Result<PathBuf, InventoryError> {
    let mut candidate = native.to_owned();
    loop {
        match fs::symlink_metadata(&candidate) {
            Ok(_) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if candidate == root || !candidate.pop() {
                    return Err(InventoryError::RepositoryIdentity(format!(
                        "cannot locate an existing parent for entry {}",
                        entry.display_escaped()
                    )));
                }
            }
            Err(error) => {
                return Err(InventoryError::PhysicalIdentity(format!(
                    "cannot inspect entry {}: {error}",
                    entry.display_escaped()
                )));
            }
        }
    }
}

pub(crate) fn validate_semantic_input_identity(
    root: &Path,
    path: &RepoPath,
    observation: &PhysicalFileObservation,
    reserved_state_lookup: &ReservedStateIdentityLookup,
) -> Result<(), InventoryError> {
    if reserved_state_lookup.contains_candidate(root, observation)? {
        Err(InventoryError::ReservedSemanticInputPath(
            path.display_escaped(),
        ))
    } else {
        Ok(())
    }
}

pub fn validate_captured_semantic_input_topology(
    root: &Path,
    path: &RepoPath,
    expected_identity: &PhysicalFileIdentity,
    reserved_state_lookup: &ReservedStateIdentityLookup,
) -> Result<(), InventoryError> {
    validate_semantic_input_path(root, path)?;
    let observation = physical_file_observation(&root.join(native_relative(path)?))?;
    if &observation.identity != expected_identity {
        return Err(InventoryError::PhysicalIdentity(format!(
            "semantic input changed physical identity after capture: {}",
            path.display_escaped()
        )));
    }
    validate_semantic_input_identity(root, path, &observation, reserved_state_lookup)
}

pub(crate) fn validate_semantic_input_path(
    root: &Path,
    path: &RepoPath,
) -> Result<(), InventoryError> {
    if semantic_input_resolves_into_reserved_state(root, path)? {
        return Err(InventoryError::ReservedSemanticInputPath(
            path.display_escaped(),
        ));
    }
    Ok(())
}

pub(crate) fn semantic_input_resolves_into_reserved_state(
    root: &Path,
    path: &RepoPath,
) -> Result<bool, InventoryError> {
    let Some(canonical_state) = canonical_reserved_state(root)? else {
        return Ok(false);
    };
    let native = root.join(native_relative(path)?);
    match fs::canonicalize(&native) {
        Ok(canonical) => Ok(canonical.starts_with(canonical_state)),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(InventoryError::PhysicalIdentity(format!(
            "cannot resolve semantic input {}: {error}",
            path.display_escaped()
        ))),
    }
}

fn canonical_reserved_state(root: &Path) -> Result<Option<PathBuf>, InventoryError> {
    let state = root.join(".lumin");
    match fs::symlink_metadata(&state) {
        Ok(_) => fs::canonicalize(&state).map(Some).map_err(|error| {
            InventoryError::PhysicalIdentity(format!(
                "cannot resolve reserved .lumin namespace: {error}"
            ))
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(InventoryError::PhysicalIdentity(format!(
            "cannot inspect reserved .lumin namespace: {error}"
        ))),
    }
}

fn reserved_state_component(component: &OsStr) -> bool {
    #[cfg(windows)]
    {
        component
            .to_str()
            .is_some_and(|component| component.eq_ignore_ascii_case(".lumin"))
    }
    #[cfg(not(windows))]
    {
        component == ".lumin"
    }
}

fn validate_entry_containment(
    root: &Path,
    canonical_root: &Path,
    canonical_state: Option<&Path>,
    entry: &RepoPath,
) -> Result<(), InventoryError> {
    let mut candidate = root.join(native_relative(entry)?);
    loop {
        match fs::symlink_metadata(&candidate) {
            Ok(_) => {
                let physical = fs::canonicalize(&candidate).map_err(|error| {
                    InventoryError::PhysicalIdentity(format!(
                        "cannot resolve entry {}: {error}",
                        entry.display_escaped()
                    ))
                })?;
                if !physical.starts_with(canonical_root) {
                    return Err(InventoryError::EntryEscapesRoot(entry.display_escaped()));
                }
                if canonical_state.is_some_and(|state| physical.starts_with(state)) {
                    return Err(InventoryError::ReservedEntryPath(entry.display_escaped()));
                }
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if candidate == root || !candidate.pop() {
                    return Err(InventoryError::PhysicalIdentity(format!(
                        "cannot find an existing parent for entry {}",
                        entry.display_escaped()
                    )));
                }
            }
            Err(error) => {
                return Err(InventoryError::PhysicalIdentity(format!(
                    "cannot inspect entry {}: {error}",
                    entry.display_escaped()
                )));
            }
        }
    }
}
