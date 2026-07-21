mod bootstrap;
pub(crate) mod database;
mod migration;
mod platform;
pub(crate) mod records;
mod store_header;

#[cfg(test)]
mod tests;

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use fs2::FileExt;
use lumin_model::{RepositoryBinding, RepositoryId};

use crate::{StoreError, io_error};
use bootstrap::bootstrap_namespace;
pub(crate) use database::StoreDatabase;
pub use migration::MigrationIntent;
use platform::repository_root_physical_identity;
pub(crate) use platform::{EntryAccess, EntryKind, HeldEntry, same_volume};
use records::*;
use store_header::*;

#[derive(Clone, Debug)]
pub(super) struct NamespaceState {
    repository: HeldRepository,
    state_dir: PathBuf,
    binding: NamespaceBinding,
}

#[derive(Clone, Debug)]
struct HeldRepository {
    path: PathBuf,
    directory: Arc<HeldEntry>,
    binding: RepositoryBinding,
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
    pub(super) fn open(root: &Path, binding: &RepositoryBinding) -> Result<Self, StoreError> {
        let repository = HeldRepository::open(root, binding.clone())?;
        let state_dir = repository.path.join(".lumin");
        let state_directory_created = ensure_state_directory(&state_dir)?;
        let state_directory = HeldEntry::open(
            &state_dir,
            EntryKind::Directory,
            EntryAccess::ReadOnly,
            false,
            ".lumin",
        )?;
        let marker_path = state_dir.join("repository.json");
        if !entry_exists(&marker_path)? {
            return bootstrap_namespace(
                repository,
                state_dir,
                state_directory,
                state_directory_created,
            );
        }

        let marker: RepositoryMarker = read_canonical_path(&marker_path, "repository marker")?;
        validate_marker(&marker)?;
        verify_repository_binding(&marker.binding.global, &repository.binding)?;
        if marker.binding.global.state_directory_identity != *state_directory.identity() {
            return Err(StoreError::Integrity(
                "state directory identity disagrees with repository marker".to_owned(),
            ));
        }
        let state = Self {
            repository,
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
        self.with_lock(true, LockPurpose::Ordinary, operation)
    }

    pub(super) fn with_shared_lock<T>(
        &self,
        operation: impl FnOnce(&NamespaceGuard) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.with_lock(false, LockPurpose::Ordinary, operation)
    }

    fn with_migration_lock<T>(
        &self,
        operation: impl FnOnce(&NamespaceGuard) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.with_lock(true, LockPurpose::Migration, operation)
    }

    fn with_lock<T>(
        &self,
        exclusive: bool,
        purpose: LockPurpose,
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
        let result = match purpose {
            LockPurpose::Ordinary => {
                migration::require_idle(&guard).and_then(|()| operation(&guard))
            }
            LockPurpose::Migration => operation(&guard),
        };
        let final_validation = guard.validate_complete();
        let unlock = FileExt::unlock(guard.lock.file()).map_err(io_error);
        combine_lock_results(result, final_validation, unlock)
    }

    fn validate_global_entries(&self) -> Result<(), StoreError> {
        self.repository.validate()?;
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
            EntryAccess::ReadOnly,
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
        let result = migration::recover_on_open(&guard);
        let final_validation = result.and_then(|()| guard.validate_complete());
        let unlock = FileExt::unlock(guard.lock.file()).map_err(io_error);
        combine_lock_results(final_validation, Ok(()), unlock)
    }
}

#[derive(Clone, Copy)]
enum LockPurpose {
    Ordinary,
    Migration,
}

impl HeldRepository {
    fn open(path: &Path, binding: RepositoryBinding) -> Result<Self, StoreError> {
        let directory = Arc::new(HeldEntry::open(
            path,
            EntryKind::Directory,
            EntryAccess::ReadOnly,
            false,
            "repository root",
        )?);
        let held = Self {
            path: path.to_path_buf(),
            directory,
            binding,
        };
        held.validate()?;
        Ok(held)
    }

    fn validate(&self) -> Result<(), StoreError> {
        self.directory.validate_path(
            &self.path,
            EntryKind::Directory,
            EntryAccess::ReadOnly,
            false,
            "repository root",
        )?;
        let observed = repository_root_physical_identity(self.directory.file())?;
        if &observed != self.binding.root().physical_identity() {
            return Err(StoreError::Integrity(
                "repository root physical identity changed".to_owned(),
            ));
        }
        Ok(())
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

    pub(crate) fn open_or_create_state_file(
        &self,
        name: &str,
        label: &str,
        initial_bytes: &[u8],
    ) -> Result<HeldEntry, StoreError> {
        let path = self.direct_state_file_path(name)?;
        self.mutate(|| {
            let entry = match fs::symlink_metadata(&path) {
                Ok(_) => HeldEntry::open(
                    &path,
                    EntryKind::RegularFile,
                    EntryAccess::ReadWrite,
                    true,
                    label,
                )?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let entry = HeldEntry::create_new(&path, label)?;
                    entry.replace_contents(initial_bytes)?;
                    self.state_directory.sync_directory()?;
                    entry
                }
                Err(error) => return Err(io_error(error)),
            };
            require_state_volume(&entry, &self.state_directory, label)?;
            entry.validate_path(
                &path,
                EntryKind::RegularFile,
                EntryAccess::ReadWrite,
                true,
                label,
            )?;
            Ok(entry)
        })
    }

    pub(crate) fn repository_id(&self) -> &RepositoryId {
        &self.state.binding.global.repository_id
    }

    pub(crate) fn managed_parent_binding(
        &self,
        kind: ManagedStateParentKind,
    ) -> Result<&ManagedStateParentBinding, StoreError> {
        self.managed_parents
            .iter()
            .find(|parent| parent.binding.kind == kind)
            .map(|parent| &parent.binding)
            .ok_or_else(|| {
                StoreError::Integrity(format!(
                    "managed parent binding is missing for {}",
                    kind.directory_name()
                ))
            })
    }

    pub(crate) fn managed_parent_path(&self, kind: ManagedStateParentKind) -> PathBuf {
        self.state.state_dir.join(kind.directory_name())
    }

    pub(crate) fn managed_child_path(
        &self,
        kind: ManagedStateParentKind,
        child: &str,
    ) -> Result<PathBuf, StoreError> {
        let mut components = Path::new(child).components();
        if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
            return Err(StoreError::Integrity(format!(
                "managed child for {} must be one normal component",
                kind.directory_name()
            )));
        }
        Ok(self.managed_parent_path(kind).join(child))
    }

    pub(crate) fn managed_parent_entry(
        &self,
        kind: ManagedStateParentKind,
    ) -> Result<&HeldEntry, StoreError> {
        self.managed_parents
            .iter()
            .find(|parent| parent.binding.kind == kind)
            .map(|parent| &parent.directory)
            .ok_or_else(|| {
                StoreError::Integrity(format!(
                    "managed parent handle is missing for {}",
                    kind.directory_name()
                ))
            })
    }

    pub(crate) fn open_managed_child_directory(
        &self,
        kind: ManagedStateParentKind,
        child: &str,
        label: &str,
    ) -> Result<HeldEntry, StoreError> {
        let path = self.managed_child_path(kind, child)?;
        let entry = HeldEntry::open(
            &path,
            EntryKind::Directory,
            EntryAccess::ReadOnly,
            false,
            label,
        )?;
        let parent = self.managed_parent_entry(kind)?;
        if !same_volume(entry.identity(), parent.identity()) {
            return Err(StoreError::Integrity(format!(
                "{label} must remain on its managed parent volume"
            )));
        }
        Ok(entry)
    }

    pub(crate) fn open_state_file(&self, name: &str, label: &str) -> Result<HeldEntry, StoreError> {
        let path = self.direct_state_file_path(name)?;
        let entry = match fs::symlink_metadata(&path) {
            Ok(_) => HeldEntry::open(
                &path,
                EntryKind::RegularFile,
                EntryAccess::ReadWrite,
                true,
                label,
            )?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(StoreError::Integrity(format!("{label} is missing")));
            }
            Err(error) => return Err(io_error(error)),
        };
        require_state_volume(&entry, &self.state_directory, label)?;
        Ok(entry)
    }

    pub(crate) fn validate_state_file(
        &self,
        entry: &HeldEntry,
        name: &str,
        label: &str,
    ) -> Result<(), StoreError> {
        let path = self.direct_state_file_path(name)?;
        entry.validate_path(
            &path,
            EntryKind::RegularFile,
            EntryAccess::ReadOnly,
            true,
            label,
        )?;
        require_state_volume(entry, &self.state_directory, label)
    }

    fn direct_state_file_path(&self, name: &str) -> Result<PathBuf, StoreError> {
        let mut components = Path::new(name).components();
        if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
            return Err(StoreError::Integrity(
                "state file name must be one direct normal component".to_owned(),
            ));
        }
        Ok(self.state.state_dir.join(name))
    }

    fn validate_complete(&self) -> Result<(), StoreError> {
        self.validate_bound_entries()?;
        let database = self.open_database()?;
        drop(database);
        self.validate_bound_entries()
    }

    pub(crate) fn validate_bound_entries(&self) -> Result<(), StoreError> {
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
            schema_version: ANCHOR_SCHEMA.to_owned(),
            global: state.binding.global.clone(),
            binding: held.binding.clone(),
        },
        &format!("managed state anchor {name}"),
    )
}

fn verify_repository_binding(
    global: &GlobalNamespaceBinding,
    repository: &RepositoryBinding,
) -> Result<(), StoreError> {
    if &global.repository_id != repository.repository_id()
        || global.repository_root_canonical != repository.root().canonical_bytes()
        || &global.repository_root_physical_identity != repository.root().physical_identity()
    {
        return Err(StoreError::Integrity(
            "repository marker belongs to a different canonical root".to_owned(),
        ));
    }
    Ok(())
}

fn ensure_state_directory(path: &Path) -> Result<bool, StoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(false),
        Ok(_) => Err(StoreError::Integrity(
            ".lumin must be a real directory".to_owned(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(io_error)?;
            Ok(true)
        }
        Err(error) => Err(io_error(error)),
    }
}

pub(crate) fn entry_exists(path: &Path) -> Result<bool, StoreError> {
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
