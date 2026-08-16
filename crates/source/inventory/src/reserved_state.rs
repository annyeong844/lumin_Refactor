use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use lumin_model::{PhysicalFileIdentity, RepoPath, RepoPathError};

use super::{InventoryError, native_relative, physical_file_identity};

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
    for entry in entries {
        let native = root.join(native_relative(entry)?);
        match fs::symlink_metadata(&native) {
            Ok(_) => {
                if reserved_state_identities.contains(&physical_file_identity(&native)?) {
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
