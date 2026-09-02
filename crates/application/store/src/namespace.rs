#[cfg(feature = "namespace-test-crash")]
pub(crate) mod barrier;
mod bootstrap;
pub(crate) mod database;
mod migration;
mod platform;
pub(crate) mod records;
mod reserved_state;
mod store_header;

#[cfg(test)]
mod tests;

use std::fs;
use std::ops::Deref;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use fs2::FileExt;
use lumin_model::{RepositoryBinding, RepositoryId};

use crate::{StoreError, io_error};
use bootstrap::{BootstrapCrashPoint, bootstrap_namespace, hit as bootstrap_hit};
pub(crate) use database::StoreDatabase;
pub use migration::MigrationIntent;
use platform::repository_root_physical_identity;
pub(crate) use platform::{
    EntryAccess, EntryKind, HeldEntry, UnpublishedFile, move_entry_noreplace, replace_entry_atomic,
    same_volume_and_mount, validate_active_unpublished_name,
};

#[cfg(any(feature = "namespace-test-crash", feature = "retention-test-crash"))]
pub(crate) fn current_logical_snapshot_for_test(
    guard: &NamespaceGuard,
) -> Result<Vec<u8>, StoreError> {
    migration::current_logical_snapshot_for_test(guard)
}
use records::*;
use store_header::*;

pub(super) enum MigrationDatabase {
    Direct(redb::Database),
    Detached {
        database: redb::Database,
        _unpublished: platform::UnpublishedFile,
    },
}

impl Deref for MigrationDatabase {
    type Target = redb::Database;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Direct(database) | Self::Detached { database, .. } => database,
        }
    }
}

fn detached_database(
    guard: &NamespaceGuard,
    entry: &HeldEntry,
) -> Result<MigrationDatabase, StoreError> {
    let bytes = entry.read_all()?;
    let unpublished =
        platform::UnpublishedFile::create(&guard.state.state_dir, &guard.state_directory)?;
    unpublished.entry().replace_contents(&bytes)?;
    let database = redb::Database::builder()
        .create_file(unpublished.entry().file().try_clone().map_err(io_error)?)
        .map_err(crate::backend_error)?;
    Ok(MigrationDatabase::Detached {
        database,
        _unpublished: unpublished,
    })
}

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
    cache_evictions: HeldCacheEvictionParent,
}

struct HeldManagedParent {
    binding: ManagedStateParentBinding,
    directory: HeldEntry,
    anchor: HeldEntry,
}

struct HeldCacheEvictionParent {
    binding: CacheEvictionParentBinding,
    directory: HeldEntry,
    anchor: HeldEntry,
}

impl NamespaceState {
    #[cfg(any(test, feature = "lifecycle-migration-test-fault"))]
    pub(crate) fn rewrite_current_store_header_as_prior_for_test(&self) -> Result<(), StoreError> {
        self.with_migration_lock(|_| {
            store_header::rewrite_current_store_header_as_prior_for_test(
                &self.state_dir.join("lifecycle.store"),
                &self.binding,
            )
        })
    }

    #[cfg(feature = "lifecycle-migration-test-fault")]
    pub(crate) fn corrupt_migrating_cleanup_operation_for_test(
        &self,
        operation_id: &lumin_model::OperationId,
    ) -> Result<(), StoreError> {
        self.with_migration_lock(|guard| {
            migration::corrupt_bound_cleanup_operation_for_test(guard, operation_id)
        })
    }

    #[cfg(feature = "lifecycle-migration-test-fault")]
    pub(crate) fn corrupt_migration_anchor_for_test(&self) -> Result<(), StoreError> {
        self.with_migration_lock(|_| {
            store_header::corrupt_migration_anchor_for_test(
                &self.state_dir.join("lifecycle.store"),
                &self.binding,
            )
        })
    }

    #[cfg(feature = "lifecycle-migration-test-fault")]
    pub(crate) fn remove_bound_root_authorization_for_test(&self) -> Result<(), StoreError> {
        self.with_migration_lock(migration::remove_bound_root_authorization_for_test)
    }

    #[cfg(feature = "namespace-test-crash")]
    pub(crate) fn remove_cache_eviction_binding_for_test(&self) -> Result<(), StoreError> {
        self.with_migration_lock(|_| {
            store_header::remove_cache_eviction_binding_for_test(
                &self.state_dir.join("lifecycle.store"),
                &self.binding,
            )
        })
    }

    pub(super) fn open_if_bound(
        root: &Path,
        binding: &RepositoryBinding,
    ) -> Result<Option<Self>, StoreError> {
        let repository = HeldRepository::open(root, binding.clone())?;
        let state_dir = repository.path.join(".lumin");
        match fs::symlink_metadata(&state_dir) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                return Err(StoreError::Integrity(
                    ".lumin must be a real directory".to_owned(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error(error)),
        }
        let marker_path = state_dir.join("repository.json");
        if !entry_exists(&marker_path)? {
            return Ok(None);
        }
        let state_directory = HeldEntry::open(
            &state_dir,
            EntryKind::Directory,
            EntryAccess::ReadOnly,
            false,
            ".lumin",
        )?;
        require_state_volume(&state_directory, repository.directory.as_ref(), ".lumin")?;
        Self::open_bound(repository, state_dir, state_directory, marker_path).map(Some)
    }

    pub(super) fn open(root: &Path, binding: &RepositoryBinding) -> Result<Self, StoreError> {
        let repository = HeldRepository::open(root, binding.clone())?;
        let state_dir = repository.path.join(".lumin");
        bootstrap_hit(BootstrapCrashPoint::BeforeStateDirectory);
        let state_directory_created = ensure_state_directory(&state_dir)?;
        if state_directory_created {
            bootstrap_hit(BootstrapCrashPoint::AfterStateDirectoryCreated);
            repository.directory.sync_directory()?;
            bootstrap_hit(BootstrapCrashPoint::AfterStateDirectoryFlushed);
        }
        let state_directory = HeldEntry::open(
            &state_dir,
            EntryKind::Directory,
            EntryAccess::ReadOnly,
            false,
            ".lumin",
        )?;
        require_state_volume(&state_directory, repository.directory.as_ref(), ".lumin")?;
        let marker_path = state_dir.join("repository.json");
        if !entry_exists(&marker_path)? {
            return bootstrap_namespace(
                repository,
                state_dir,
                state_directory,
                state_directory_created,
            );
        }

        Self::open_bound(repository, state_dir, state_directory, marker_path)
    }

    pub(super) fn open_for_migration(
        root: &Path,
        binding: &RepositoryBinding,
    ) -> Result<Option<Self>, StoreError> {
        let repository = HeldRepository::open(root, binding.clone())?;
        let state_dir = repository.path.join(".lumin");
        match fs::symlink_metadata(&state_dir) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                return Err(StoreError::Integrity(
                    ".lumin must be a real directory".to_owned(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error(error)),
        }
        let marker_path = state_dir.join("repository.json");
        if !entry_exists(&marker_path)? {
            return Err(StoreError::Integrity(
                "initialized state namespace omitted repository.json".to_owned(),
            ));
        }
        let state_directory = HeldEntry::open(
            &state_dir,
            EntryKind::Directory,
            EntryAccess::ReadOnly,
            false,
            ".lumin",
        )?;
        require_state_volume(&state_directory, repository.directory.as_ref(), ".lumin")?;
        Self::bind_existing(repository, state_dir, state_directory, marker_path).map(Some)
    }

    fn open_bound(
        repository: HeldRepository,
        state_dir: PathBuf,
        state_directory: HeldEntry,
        marker_path: PathBuf,
    ) -> Result<Self, StoreError> {
        let state = Self::bind_existing(repository, state_dir, state_directory, marker_path)?;
        state.ensure_store_ready()?;
        Ok(state)
    }

    fn bind_existing(
        repository: HeldRepository,
        state_dir: PathBuf,
        state_directory: HeldEntry,
        marker_path: PathBuf,
    ) -> Result<Self, StoreError> {
        let marker: RepositoryMarker = read_canonical_path(&marker_path, "repository marker")?;
        validate_marker(&marker)?;
        verify_repository_binding(&marker.binding.global, &repository.binding)?;
        if marker.binding.global.state_directory_identity != *state_directory.identity() {
            return Err(StoreError::Integrity(
                "state directory identity disagrees with repository marker".to_owned(),
            ));
        }
        Ok(Self {
            repository,
            state_dir,
            binding: marker.binding,
        })
    }

    pub(super) fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    pub(super) fn reserved_state_identities(
        &self,
    ) -> Result<std::collections::BTreeSet<lumin_model::PhysicalFileIdentity>, StoreError> {
        self.with_shared_lock(reserved_state::collect_identities)
    }

    pub(super) fn with_exclusive_lock<T>(
        &self,
        operation: impl FnOnce(&NamespaceGuard) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.with_lock(
            true,
            LockPurpose::Ordinary,
            None::<fn() -> Result<(), StoreError>>,
            operation,
        )
    }

    #[cfg(feature = "publication-test-crash")]
    pub(super) fn with_exclusive_lock_after_contention<T>(
        &self,
        on_contention: impl FnOnce() -> Result<(), StoreError>,
        operation: impl FnOnce(&NamespaceGuard) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.with_lock(true, LockPurpose::Ordinary, Some(on_contention), operation)
    }

    pub(super) fn with_shared_lock<T>(
        &self,
        operation: impl FnOnce(&NamespaceGuard) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.with_lock(
            false,
            LockPurpose::Ordinary,
            None::<fn() -> Result<(), StoreError>>,
            operation,
        )
    }

    fn with_migration_lock<T>(
        &self,
        operation: impl FnOnce(&NamespaceGuard) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.with_lock(
            true,
            LockPurpose::Migration,
            None::<fn() -> Result<(), StoreError>>,
            operation,
        )
    }

    fn with_lock<T, C>(
        &self,
        exclusive: bool,
        purpose: LockPurpose,
        on_contention: Option<C>,
        operation: impl FnOnce(&NamespaceGuard) -> Result<T, StoreError>,
    ) -> Result<T, StoreError>
    where
        C: FnOnce() -> Result<(), StoreError>,
    {
        let lock = self.open_prevalidated_lock()?;
        if exclusive {
            match on_contention {
                Some(on_contention) => match lock.file().try_lock_exclusive() {
                    Ok(()) => {}
                    Err(error) if lock_contended(&error) => {
                        on_contention()?;
                        FileExt::lock_exclusive(lock.file()).map_err(io_error)?;
                    }
                    Err(error) => return Err(io_error(error)),
                },
                None => FileExt::lock_exclusive(lock.file()).map_err(io_error)?,
            }
        } else {
            FileExt::lock_shared(lock.file()).map_err(io_error)?;
        }
        let guard = NamespaceGuard::acquire_without_store(self.clone(), lock)?;
        let result = match purpose {
            LockPurpose::Ordinary => migration::require_idle(&guard)
                .and_then(|()| guard.validate_complete())
                .and_then(|()| {
                    #[cfg(feature = "namespace-test-crash")]
                    barrier::wait_after_complete_validation()?;
                    guard.validate_complete()
                })
                .and_then(|()| operation(&guard)),
            LockPurpose::Migration => operation(&guard),
        };
        let final_validation = match purpose {
            LockPurpose::Ordinary => {
                migration::require_idle(&guard).and_then(|()| guard.validate_complete())
            }
            LockPurpose::Migration => guard.validate_bound_entries(),
        };
        let unlock = FileExt::unlock(guard.lock.file()).map_err(io_error);
        combine_lock_results(result, final_validation, unlock)
    }

    fn open_prevalidated_lock(&self) -> Result<HeldEntry, StoreError> {
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
        // Windows mandatory range locking can reject a header read while another
        // process owns the lock. Prove object identity here; NamespaceGuard performs
        // the complete header and namespace proof after this handle acquires the lock.
        let lock = self.open_bound_lock()?;
        #[cfg(feature = "namespace-test-crash")]
        barrier::wait_after_pre_acquire_validation()?;
        self.repository.validate()?;
        state.validate_path(
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
        lock.validate_path(
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
        let lock = self.open_prevalidated_lock()?;
        FileExt::lock_exclusive(lock.file()).map_err(io_error)?;
        let guard = NamespaceGuard::acquire_without_store(self.clone(), lock)?;
        let result = migration::admit_ordinary(&guard);
        let final_validation = result
            .and_then(|()| guard.validate_complete())
            .and_then(|()| {
                #[cfg(feature = "namespace-test-crash")]
                barrier::wait_after_complete_validation()?;
                guard.validate_complete()
            });
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
        let cache_evictions = open_cache_eviction_parent(&state)?;
        let guard = Self {
            state,
            state_directory,
            lock,
            managed_parents,
            cache_evictions,
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

    pub(crate) fn create_state_file(
        &self,
        name: &str,
        label: &str,
    ) -> Result<HeldEntry, StoreError> {
        let path = self.direct_state_file_path(name)?;
        self.mutate(|| {
            if entry_exists(&path)? {
                return Err(StoreError::Integrity(format!(
                    "{label} already exists before allocation"
                )));
            }
            let entry = HeldEntry::create_new(&path, label)?;
            require_state_volume(&entry, &self.state_directory, label)?;
            entry.validate_path(
                &path,
                EntryKind::RegularFile,
                EntryAccess::ReadWrite,
                true,
                label,
            )?;
            self.state_directory.sync_directory()?;
            Ok(entry)
        })
    }

    pub(crate) fn repository_id(&self) -> &RepositoryId {
        &self.state.binding.global.repository_id
    }

    pub(crate) fn state_directory_entry(&self) -> &HeldEntry {
        &self.state_directory
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

    pub(crate) fn cache_eviction_parent_path(&self) -> PathBuf {
        self.managed_parent_path(ManagedStateParentKind::Trash)
            .join("cache-evictions")
    }

    pub(crate) fn cache_eviction_parent_entry(&self) -> &HeldEntry {
        &self.cache_evictions.directory
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
        if !same_volume_and_mount(&entry, parent) {
            return Err(StoreError::Integrity(format!(
                "{label} must remain on its managed parent volume and mount"
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

    pub(super) fn direct_state_file_path(&self, name: &str) -> Result<PathBuf, StoreError> {
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
        validate_cache_eviction_parent(&self.state, &self.managed_parents, &self.cache_evictions)?;
        Ok(())
    }

    pub(crate) fn reserved_state_identities(
        &self,
    ) -> Result<std::collections::BTreeSet<lumin_model::PhysicalFileIdentity>, StoreError> {
        reserved_state::collect_identities(self)
    }
}

fn open_cache_eviction_parent(
    state: &NamespaceState,
) -> Result<HeldCacheEvictionParent, StoreError> {
    let binding = state.binding.cache_evictions.as_ref().ok_or_else(|| {
        StoreError::IncompatibleStateSchema(
            "repository marker omitted the cache-eviction parent binding".to_owned(),
        )
    })?;
    let path = state
        .state_dir
        .join(ManagedStateParentKind::Trash.directory_name())
        .join("cache-evictions");
    let directory = HeldEntry::open(
        &path,
        EntryKind::Directory,
        EntryAccess::ReadOnly,
        false,
        "cache-eviction parent",
    )?;
    let anchor = HeldEntry::open(
        &path.join("namespace.anchor"),
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        true,
        "cache-eviction parent anchor",
    )?;
    Ok(HeldCacheEvictionParent {
        binding: binding.clone(),
        directory,
        anchor,
    })
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

fn validate_cache_eviction_parent(
    state: &NamespaceState,
    managed_parents: &[HeldManagedParent],
    held: &HeldCacheEvictionParent,
) -> Result<(), StoreError> {
    let trash = managed_parents
        .iter()
        .find(|parent| parent.binding.kind == ManagedStateParentKind::Trash)
        .ok_or_else(|| StoreError::Integrity("trash parent handle is missing".to_owned()))?;
    let path = state
        .state_dir
        .join(ManagedStateParentKind::Trash.directory_name())
        .join("cache-evictions");
    held.directory.validate_path(
        &path,
        EntryKind::Directory,
        EntryAccess::ReadOnly,
        false,
        "cache-eviction parent",
    )?;
    if !same_volume_and_mount(&held.directory, &trash.directory)
        || held.directory.identity() != &held.binding.directory_physical_identity
    {
        return Err(StoreError::Integrity(
            "cache-eviction parent binding changed".to_owned(),
        ));
    }
    held.anchor.validate_path(
        &path.join("namespace.anchor"),
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        true,
        "cache-eviction parent anchor",
    )?;
    if held.anchor.identity() != &held.binding.anchor_physical_identity {
        return Err(StoreError::Integrity(
            "cache-eviction parent anchor binding changed".to_owned(),
        ));
    }
    verify_canonical_entry(
        &held.anchor,
        &CacheEvictionParentAnchorHeader {
            schema_version: CACHE_EVICTION_ANCHOR_SCHEMA.to_owned(),
            global: state.binding.global.clone(),
            trash_binding: trash.binding.clone(),
            binding: held.binding.clone(),
        },
        "cache-eviction parent anchor",
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

pub(crate) fn lock_contended(error: &std::io::Error) -> bool {
    let expected = fs2::lock_contended_error();
    error.raw_os_error() == expected.raw_os_error() || error.kind() == expected.kind()
}

fn require_state_volume(
    entry: &HeldEntry,
    state_directory: &HeldEntry,
    label: &str,
) -> Result<(), StoreError> {
    if !same_volume_and_mount(entry, state_directory) {
        return Err(StoreError::Integrity(format!(
            "{label} crosses the state filesystem, volume, or mount"
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
