mod bootstrap;
mod platform;
mod records;
mod store_header;

#[cfg(test)]
mod tests;

use std::fs;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use redb::{Database, WriteTransaction};

use crate::{StoreError, backend_error, io_error};
use bootstrap::bootstrap_namespace;
use platform::{EntryAccess, EntryKind, HeldEntry, same_volume};
use records::*;
use store_header::*;

#[derive(Clone, Debug)]
pub(super) struct NamespaceState {
    state_dir: PathBuf,
    binding: NamespaceBinding,
}

pub(super) struct NamespaceGuard {
    state: NamespaceState,
    state_directory: HeldEntry,
    lock: HeldEntry,
    managed_parents: Vec<HeldManagedParent>,
}

struct HeldManagedParent {
    binding: ManagedStateParentBinding,
    directory: HeldEntry,
    anchor: HeldEntry,
}

impl NamespaceState {
    pub(super) fn open(root: &Path) -> Result<Self, StoreError> {
        let state_dir = root.join(".lumin");
        ensure_state_directory(&state_dir)?;
        let state_directory = HeldEntry::open(
            &state_dir,
            EntryKind::Directory,
            EntryAccess::ReadOnly,
            false,
            ".lumin",
        )?;
        let marker_path = state_dir.join("repository.json");
        if !entry_exists(&marker_path)? {
            return bootstrap_namespace(state_dir, state_directory);
        }

        let marker: RepositoryMarker = read_canonical_path(&marker_path, "repository marker")?;
        validate_marker(&marker)?;
        if marker.binding.global.state_directory_identity != *state_directory.identity() {
            return Err(StoreError::Integrity(
                "state directory identity disagrees with repository marker".to_owned(),
            ));
        }
        let state = Self {
            state_dir,
            binding: marker.binding,
        };
        state.ensure_store_ready()?;
        Ok(state)
    }

    pub(super) fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    pub(super) fn with_exclusive_lock<T>(
        &self,
        operation: impl FnOnce(&NamespaceGuard) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.with_lock(true, operation)
    }

    pub(super) fn with_shared_lock<T>(
        &self,
        operation: impl FnOnce(&NamespaceGuard) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.with_lock(false, operation)
    }

    fn with_lock<T>(
        &self,
        exclusive: bool,
        operation: impl FnOnce(&NamespaceGuard) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.validate_global_entries()?;
        let lock = self.open_bound_lock()?;
        if exclusive {
            FileExt::lock_exclusive(lock.file()).map_err(io_error)?;
        } else {
            FileExt::lock_shared(lock.file()).map_err(io_error)?;
        }
        let guard = NamespaceGuard::acquire(self.clone(), lock)?;
        let result = operation(&guard);
        let final_validation = guard.validate_complete();
        let unlock = FileExt::unlock(guard.lock.file()).map_err(io_error);
        combine_lock_results(result, final_validation, unlock)
    }

    fn validate_global_entries(&self) -> Result<(), StoreError> {
        let state = HeldEntry::open(
            &self.state_dir,
            EntryKind::Directory,
            EntryAccess::ReadOnly,
            false,
            ".lumin",
        )?;
        if state.identity() != &self.binding.global.state_directory_identity {
            return Err(StoreError::Integrity(
                "state directory physical identity changed".to_owned(),
            ));
        }
        let lock = self.open_bound_lock()?;
        verify_lock_header(&lock, &self.binding.global)
    }

    fn open_bound_lock(&self) -> Result<HeldEntry, StoreError> {
        let lock = HeldEntry::open(
            &self.state_dir.join("lifecycle.lock"),
            EntryKind::RegularFile,
            EntryAccess::ReadWrite,
            true,
            "lifecycle.lock",
        )?;
        if lock.identity() != &self.binding.global.lifecycle_lock_identity {
            return Err(StoreError::Integrity(
                "lifecycle.lock physical identity changed".to_owned(),
            ));
        }
        Ok(lock)
    }

    fn ensure_store_ready(&self) -> Result<(), StoreError> {
        self.validate_global_entries()?;
        let lock = self.open_bound_lock()?;
        FileExt::lock_exclusive(lock.file()).map_err(io_error)?;
        let guard = NamespaceGuard::acquire_without_store(self.clone(), lock)?;
        let result = create_or_verify_store(&guard);
        let final_validation = result.and_then(|()| guard.validate_complete());
        let unlock = FileExt::unlock(guard.lock.file()).map_err(io_error);
        combine_lock_results(final_validation, Ok(()), unlock)
    }
}

impl NamespaceGuard {
    fn acquire(state: NamespaceState, lock: HeldEntry) -> Result<Self, StoreError> {
        let guard = Self::acquire_without_store(state, lock)?;
        guard.validate_complete()?;
        Ok(guard)
    }

    fn acquire_without_store(state: NamespaceState, lock: HeldEntry) -> Result<Self, StoreError> {
        let state_directory = HeldEntry::open(
            &state.state_dir,
            EntryKind::Directory,
            EntryAccess::ReadOnly,
            false,
            ".lumin",
        )?;
        let mut managed_parents = Vec::with_capacity(MANAGED_KINDS.len());
        for binding in &state.binding.managed_parents {
            managed_parents.push(open_managed_parent(&state, binding)?);
        }
        let guard = Self {
            state,
            state_directory,
            lock,
            managed_parents,
        };
        guard.validate_bound_entries()?;
        Ok(guard)
    }

    pub(super) fn open_database(&self) -> Result<Database, StoreError> {
        self.validate_bound_entries()?;
        let entry = HeldEntry::open(
            &self.state.state_dir.join("lifecycle.store"),
            EntryKind::RegularFile,
            EntryAccess::ReadWrite,
            true,
            "lifecycle.store",
        )?;
        require_state_volume(&entry, &self.state_directory, "lifecycle.store")?;
        let database = Database::builder()
            .create_file(entry.file().try_clone().map_err(io_error)?)
            .map_err(backend_error)?;
        verify_store_header(&database, &self.state.binding)?;
        self.validate_bound_entries()?;
        Ok(database)
    }

    pub(super) fn commit(
        &self,
        database: &Database,
        write: WriteTransaction,
    ) -> Result<(), StoreError> {
        self.validate_bound_entries()?;
        verify_store_header_write(&write, &self.state.binding)?;
        write.commit().map_err(backend_error)?;
        self.validate_bound_entries()?;
        verify_store_header(database, &self.state.binding)?;
        self.validate_bound_entries()
    }

    pub(super) fn mutate<T>(
        &self,
        mutation: impl FnOnce() -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.validate_complete()?;
        let result = mutation();
        let validation = self.validate_complete();
        match (result, validation) {
            (_, Err(error)) => Err(error),
            (result, Ok(())) => result,
        }
    }

    fn validate_complete(&self) -> Result<(), StoreError> {
        self.validate_bound_entries()?;
        let database = self.open_database()?;
        verify_store_header(&database, &self.state.binding)?;
        drop(database);
        self.validate_bound_entries()
    }

    fn validate_bound_entries(&self) -> Result<(), StoreError> {
        self.state_directory.validate_path(
            &self.state.state_dir,
            EntryKind::Directory,
            EntryAccess::ReadOnly,
            false,
            ".lumin",
        )?;
        if self.state_directory.identity() != &self.state.binding.global.state_directory_identity {
            return Err(StoreError::Integrity(
                "held state directory disagrees with repository marker".to_owned(),
            ));
        }
        self.lock.validate_path(
            &self.state.state_dir.join("lifecycle.lock"),
            EntryKind::RegularFile,
            EntryAccess::ReadWrite,
            true,
            "lifecycle.lock",
        )?;
        verify_lock_header(&self.lock, &self.state.binding.global)?;
        verify_marker(&self.state)?;
        for held in &self.managed_parents {
            validate_managed_parent(&self.state, &self.state_directory, held)?;
        }
        Ok(())
    }
}

fn open_managed_parent(
    state: &NamespaceState,
    binding: &ManagedStateParentBinding,
) -> Result<HeldManagedParent, StoreError> {
    let name = binding.kind.directory_name();
    let directory = HeldEntry::open(
        &state.state_dir.join(name),
        EntryKind::Directory,
        EntryAccess::ReadOnly,
        false,
        &format!("managed state parent {name}"),
    )?;
    let anchor = HeldEntry::open(
        &state.state_dir.join(name).join("namespace.anchor"),
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        true,
        &format!("managed state anchor {name}"),
    )?;
    Ok(HeldManagedParent {
        binding: binding.clone(),
        directory,
        anchor,
    })
}

fn validate_managed_parent(
    state: &NamespaceState,
    state_directory: &HeldEntry,
    held: &HeldManagedParent,
) -> Result<(), StoreError> {
    let name = held.binding.kind.directory_name();
    let directory_path = state.state_dir.join(name);
    held.directory.validate_path(
        &directory_path,
        EntryKind::Directory,
        EntryAccess::ReadOnly,
        false,
        &format!("managed state parent {name}"),
    )?;
    require_state_volume(&held.directory, state_directory, name)?;
    if held.directory.identity() != &held.binding.directory_physical_identity {
        return Err(StoreError::Integrity(format!(
            "managed state parent {name} identity disagrees with marker"
        )));
    }
    held.anchor.validate_path(
        &directory_path.join("namespace.anchor"),
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        true,
        &format!("managed state anchor {name}"),
    )?;
    if held.anchor.identity() != &held.binding.anchor_physical_identity {
        return Err(StoreError::Integrity(format!(
            "managed state anchor {name} identity disagrees with marker"
        )));
    }
    verify_canonical_entry(
        &held.anchor,
        &ManagedParentAnchorHeader {
            schema_version: "lumin-managed-parent-anchor.v1".to_owned(),
            global: state.binding.global.clone(),
            binding: held.binding.clone(),
        },
        &format!("managed state anchor {name}"),
    )
}

fn ensure_state_directory(path: &Path) -> Result<(), StoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(StoreError::Integrity(
            ".lumin must be a real directory".to_owned(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(io_error)
        }
        Err(error) => Err(io_error(error)),
    }
}

fn entry_exists(path: &Path) -> Result<bool, StoreError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error(error)),
    }
}

fn require_state_volume(
    entry: &HeldEntry,
    state_directory: &HeldEntry,
    label: &str,
) -> Result<(), StoreError> {
    if !same_volume(entry.identity(), state_directory.identity()) {
        return Err(StoreError::Integrity(format!(
            "{label} crosses the state filesystem or volume"
        )));
    }
    Ok(())
}

fn combine_lock_results<T>(
    operation: Result<T, StoreError>,
    validation: Result<(), StoreError>,
    unlock: Result<(), StoreError>,
) -> Result<T, StoreError> {
    validation?;
    unlock?;
    operation
}
