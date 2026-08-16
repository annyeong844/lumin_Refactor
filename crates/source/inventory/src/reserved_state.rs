use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lumin_model::{PhysicalFileIdentity, RepoPath, RepoPathError};

use super::{InventoryError, native_relative};
use crate::physical_path::physical_file_identity_and_links;

type IdentityCheck =
    dyn Fn(&PhysicalFileIdentity) -> Result<bool, InventoryError> + Send + Sync + 'static;

/// Store-owned identity membership is evaluated lazily. Ordinary one-link
/// evidence candidates never require enumerating the retained state tree;
/// redirected paths are rejected separately through canonical containment.
#[derive(Clone)]
pub struct ReservedStateIdentityLookup {
    check: Arc<IdentityCheck>,
    queried: Arc<AtomicBool>,
}

impl ReservedStateIdentityLookup {
    pub fn new(
        check: impl Fn(&PhysicalFileIdentity) -> Result<bool, InventoryError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            check: Arc::new(check),
            queried: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn from_identities(identities: BTreeSet<PhysicalFileIdentity>) -> Self {
        Self::new(move |identity| Ok(identities.contains(identity)))
    }

    pub fn empty() -> Self {
        Self::from_identities(BTreeSet::new())
    }

    pub(crate) fn contains_shared(
        &self,
        identity: &PhysicalFileIdentity,
        links: u64,
    ) -> Result<bool, InventoryError> {
        if links <= 1 {
            return Ok(false);
        }
        self.queried.store(true, Ordering::Release);
        (self.check)(identity)
    }

    /// Freeze a lookup for a final validation that runs under the store lock.
    /// If analysis never needed the state index, a newly shared input is itself
    /// stale topology and must fail without trying to acquire the store lock.
    pub fn for_final_validation(&self) -> Self {
        if self.queried.load(Ordering::Acquire) {
            return self.clone();
        }
        Self::new(|_| {
            Err(InventoryError::PhysicalIdentity(
                "semantic input acquired an additional physical link after capture".to_owned(),
            ))
        })
    }
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
        match fs::symlink_metadata(&native) {
            Ok(_) => {
                let (identity, links) = physical_file_identity_and_links(&native)?;
                if reserved_state_lookup.contains_shared(&identity, links)? {
                    return Err(InventoryError::ReservedEntryPath(entry.display_escaped()));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(InventoryError::PhysicalIdentity(format!(
                    "cannot inspect entry {}: {error}",
                    entry.display_escaped()
                )));
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_semantic_input_identity(
    path: &RepoPath,
    physical_identity: &PhysicalFileIdentity,
    links: u64,
    reserved_state_lookup: &ReservedStateIdentityLookup,
) -> Result<(), InventoryError> {
    if reserved_state_lookup.contains_shared(physical_identity, links)? {
        Err(InventoryError::ReservedSemanticInputPath(
            path.display_escaped(),
        ))
    } else {
        Ok(())
    }
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
