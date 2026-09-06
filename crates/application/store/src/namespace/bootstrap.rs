use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

use fs2::FileExt;

use crate::{StoreError, io_error, nonce_hex};

use super::platform::UnpublishedFile;
use super::platform::{EntryAccess, EntryKind, HeldEntry};
use super::{
    ANCHOR_SCHEMA, CACHE_EVICTION_ANCHOR_SCHEMA, CacheEvictionParentAnchorHeader,
    CacheEvictionParentBinding, GlobalNamespaceBinding, HeldRepository, LOCK_SCHEMA,
    LifecycleLockHeader, MANAGED_KINDS, ManagedParentAnchorHeader, ManagedStateParentBinding,
    ManagedStateParentKind, NamespaceBinding, NamespaceGuard, NamespaceState, REPOSITORY_SCHEMA,
    RepositoryMarker, create_or_verify_store, entry_exists, read_canonical_path,
    require_state_volume, same_volume_and_mount, validate_global_binding, validate_marker,
    verify_canonical_entry, verify_repository_binding, write_canonical_entry,
};

mod crash;

pub(super) use crash::{BootstrapCrashPoint, hit};

const CACHE_EVICTIONS_DIRECTORY: &str = "cache-evictions";
const MARKER_CANDIDATE_PREFIX: &str = ".lumin-unpublished-repository-";

pub(super) fn bootstrap_namespace(
    repository: HeldRepository,
    state_dir: PathBuf,
    state_directory: HeldEntry,
    state_directory_created: bool,
    #[cfg(feature = "audit-store-test-profile")] mut profile: Option<
        &mut crate::audit_profile::StoreProfiler,
    >,
) -> Result<NamespaceState, StoreError> {
    if !state_directory_created {
        return resume_bootstrap(
            repository,
            state_dir,
            state_directory,
            #[cfg(feature = "audit-store-test-profile")]
            profile,
        );
    }
    store_phase_begin!(profile, BootstrapSetup);
    if fs::read_dir(&state_dir).map_err(io_error)?.next().is_some() {
        return Err(StoreError::Integrity(
            "new state directory changed during bootstrap admission".to_owned(),
        ));
    }
    let lock = HeldEntry::create_new(&state_dir.join("lifecycle.lock"), "lifecycle.lock")?;
    hit(BootstrapCrashPoint::AfterLifecycleLockCreated);
    FileExt::lock_exclusive(lock.file()).map_err(io_error)?;
    hit(BootstrapCrashPoint::AfterLifecycleLockAcquired);
    let global = GlobalNamespaceBinding {
        repository_id: repository.binding.repository_id().clone(),
        repository_root_canonical: repository.binding.root().canonical_bytes().to_vec(),
        repository_root_physical_identity: repository.binding.root().physical_identity().clone(),
        state_directory_identity: state_directory.identity().clone(),
        lifecycle_lock_identity: lock.identity().clone(),
        namespace_nonce: nonce_hex()?,
    };
    hit(BootstrapCrashPoint::AfterGlobalBindingAllocated);
    write_canonical_entry(
        &lock,
        &LifecycleLockHeader {
            schema_version: LOCK_SCHEMA.to_owned(),
            global: global.clone(),
        },
    )?;
    hit(BootstrapCrashPoint::AfterLifecycleLockHeaderFlushed);
    store_phase_end!(profile, BootstrapSetup);
    finish_bootstrap(
        repository,
        state_dir,
        state_directory,
        lock,
        global,
        #[cfg(feature = "audit-store-test-profile")]
        profile,
    )
}

fn resume_bootstrap(
    repository: HeldRepository,
    state_dir: PathBuf,
    state_directory: HeldEntry,
    #[cfg(feature = "audit-store-test-profile")] profile: Option<
        &mut crate::audit_profile::StoreProfiler,
    >,
) -> Result<NamespaceState, StoreError> {
    if fs::read_dir(&state_dir).map_err(io_error)?.next().is_none() {
        return Err(StoreError::Integrity(
            "preexisting state directory has no bound bootstrap state".to_owned(),
        ));
    }
    let lock_path = state_dir.join("lifecycle.lock");
    let lock = HeldEntry::open(
        &lock_path,
        EntryKind::RegularFile,
        EntryAccess::ReadWrite,
        true,
        "lifecycle.lock",
    )?;
    let header: LifecycleLockHeader = read_canonical_path(&lock_path, "lifecycle.lock")?;
    validate_bootstrap_lock(&header, &repository, &state_directory, &lock)?;
    FileExt::lock_exclusive(lock.file()).map_err(io_error)?;
    verify_canonical_bootstrap_lock(&lock, &header)?;

    let marker_path = state_dir.join("repository.json");
    let marker_exists = entry_exists(&marker_path)?;
    let marker_candidate = (!marker_exists)
        .then(|| recoverable_marker_candidate_name(&header.global))
        .flatten();
    reject_unbound_bootstrap_entries(&state_dir, marker_candidate.as_deref())?;
    if marker_exists {
        FileExt::unlock(lock.file()).map_err(io_error)?;
        let marker: RepositoryMarker = read_canonical_path(&marker_path, "repository marker")?;
        validate_marker(&marker)?;
        verify_repository_binding(&marker.binding.global, &repository.binding)?;
        let state = NamespaceState {
            repository,
            state_dir,
            binding: marker.binding,
        };
        state.ensure_store_ready()?;
        return Ok(state);
    }
    if entry_exists(&state_dir.join("lifecycle.store"))? {
        return Err(StoreError::Integrity(
            "pre-marker state cannot contain lifecycle.store".to_owned(),
        ));
    }
    finish_bootstrap(
        repository,
        state_dir,
        state_directory,
        lock,
        header.global,
        #[cfg(feature = "audit-store-test-profile")]
        profile,
    )
}

fn finish_bootstrap(
    repository: HeldRepository,
    state_dir: PathBuf,
    state_directory: HeldEntry,
    lock: HeldEntry,
    global: GlobalNamespaceBinding,
    #[cfg(feature = "audit-store-test-profile")] mut profile: Option<
        &mut crate::audit_profile::StoreProfiler,
    >,
) -> Result<NamespaceState, StoreError> {
    store_phase_begin!(profile, BootstrapParents);
    let mut bindings = Vec::with_capacity(MANAGED_KINDS.len());
    for kind in MANAGED_KINDS {
        let binding = if entry_exists(&state_dir.join(kind.directory_name()))? {
            load_existing_parent(&state_dir, &state_directory, &global, kind)?
        } else {
            create_managed_parent(&state_dir, &state_directory, &global, kind)?
        };
        bindings.push(binding);
    }
    let managed_parents: [ManagedStateParentBinding; 4] = bindings.try_into().map_err(|_| {
        StoreError::Integrity("managed parent initialization was incomplete".to_owned())
    })?;
    let trash_binding = managed_parents
        .iter()
        .find(|binding| binding.kind == ManagedStateParentKind::Trash)
        .ok_or_else(|| StoreError::Integrity("trash parent binding is missing".to_owned()))?;
    let cache_evictions_path = state_dir
        .join(ManagedStateParentKind::Trash.directory_name())
        .join(CACHE_EVICTIONS_DIRECTORY);
    let cache_evictions = if entry_exists(&cache_evictions_path)? {
        load_existing_cache_eviction_parent(&state_dir, &global, trash_binding)?
    } else {
        create_cache_eviction_parent(&state_dir, &global, trash_binding)?
    };
    let binding = NamespaceBinding {
        global,
        managed_parents,
        cache_evictions: Some(cache_evictions),
    };
    state_directory.sync_directory()?;
    hit(BootstrapCrashPoint::AfterAllParentsFlushed);
    store_phase_end!(profile, BootstrapParents);
    store_phase_begin!(profile, BootstrapMarker);
    publish_repository_marker(
        &state_dir,
        &state_directory,
        &RepositoryMarker {
            schema_version: REPOSITORY_SCHEMA.to_owned(),
            binding: binding.clone(),
        },
    )?;

    store_phase_end!(profile, BootstrapMarker);
    let state = NamespaceState {
        repository,
        state_dir,
        binding,
    };
    let guard = NamespaceGuard::acquire_without_store(state.clone(), lock)?;
    hit(BootstrapCrashPoint::BeforeStoreCreation);
    store_phase_begin!(profile, BootstrapStore);
    create_or_verify_store(&guard)?;
    guard.state_directory.sync_directory()?;
    hit(BootstrapCrashPoint::AfterStoreParentFlushed);
    store_phase_end!(profile, BootstrapStore);
    store_phase_begin!(profile, BootstrapValidation);
    guard.validate_complete()?;
    store_phase_end!(profile, BootstrapValidation);
    hit(BootstrapCrashPoint::AfterCompleteValidation);
    FileExt::unlock(guard.lock.file()).map_err(io_error)?;
    Ok(state)
}

fn publish_repository_marker(
    state_dir: &Path,
    state_directory: &HeldEntry,
    marker: &RepositoryMarker,
) -> Result<(), StoreError> {
    hit(BootstrapCrashPoint::BeforeMarkerCandidate);
    let candidate_name = marker_candidate_name(&marker.binding.global);
    let unpublished =
        UnpublishedFile::create_with_named_fallback(state_dir, state_directory, &candidate_name)?;
    hit(BootstrapCrashPoint::AfterMarkerCandidateCreated);
    if unpublished.entry().read_all()?.is_empty() {
        write_canonical_entry(unpublished.entry(), marker)?;
    } else {
        verify_canonical_entry(unpublished.entry(), marker, "repository marker candidate")?;
        unpublished.entry().sync()?;
    }
    hit(BootstrapCrashPoint::AfterMarkerCandidateFlushed);
    verify_canonical_entry(unpublished.entry(), marker, "repository marker candidate")?;
    let published = unpublished.publish_noreplace(
        state_directory,
        state_dir,
        OsStr::new("repository.json"),
        "repository marker",
        || {
            hit(BootstrapCrashPoint::AfterMarkerPublished);
            Ok(())
        },
    )?;
    drop(published);
    state_directory.sync_directory()?;
    hit(BootstrapCrashPoint::AfterMarkerParentFlushed);
    Ok(())
}

fn create_managed_parent(
    state_dir: &Path,
    state_directory: &HeldEntry,
    global: &GlobalNamespaceBinding,
    kind: ManagedStateParentKind,
) -> Result<ManagedStateParentBinding, StoreError> {
    let name = kind.directory_name();
    let directory_path = state_dir.join(name);
    fs::create_dir(&directory_path).map_err(io_error)?;
    hit(managed_parent_point(
        kind,
        ManagedParentStage::DirectoryCreated,
    ));
    let directory = open_parent_directory(&directory_path, name)?;
    require_state_volume(&directory, state_directory, name)?;
    let anchor = HeldEntry::create_new(
        &directory_path.join("namespace.anchor"),
        &format!("managed state anchor {name}"),
    )?;
    hit(managed_parent_point(
        kind,
        ManagedParentStage::AnchorCreated,
    ));
    let binding = ManagedStateParentBinding {
        kind,
        directory_physical_identity: directory.identity().clone(),
        anchor_physical_identity: anchor.identity().clone(),
        parent_nonce: nonce_hex()?,
    };
    hit(managed_parent_point(
        kind,
        ManagedParentStage::BindingAllocated,
    ));
    write_canonical_entry(
        &anchor,
        &ManagedParentAnchorHeader {
            schema_version: ANCHOR_SCHEMA.to_owned(),
            global: global.clone(),
            binding: binding.clone(),
        },
    )?;
    hit(managed_parent_point(
        kind,
        ManagedParentStage::AnchorFlushed,
    ));
    directory.sync_directory()?;
    hit(managed_parent_point(
        kind,
        ManagedParentStage::ParentFlushed,
    ));
    Ok(binding)
}

#[derive(Clone, Copy)]
enum ManagedParentStage {
    DirectoryCreated,
    AnchorCreated,
    BindingAllocated,
    AnchorFlushed,
    ParentFlushed,
}

fn managed_parent_point(
    kind: ManagedStateParentKind,
    stage: ManagedParentStage,
) -> BootstrapCrashPoint {
    match (kind, stage) {
        (ManagedStateParentKind::Attempts, ManagedParentStage::DirectoryCreated) => {
            BootstrapCrashPoint::AfterAttemptsDirectoryCreated
        }
        (ManagedStateParentKind::Attempts, ManagedParentStage::AnchorCreated) => {
            BootstrapCrashPoint::AfterAttemptsAnchorCreated
        }
        (ManagedStateParentKind::Attempts, ManagedParentStage::BindingAllocated) => {
            BootstrapCrashPoint::AfterAttemptsBindingAllocated
        }
        (ManagedStateParentKind::Attempts, ManagedParentStage::AnchorFlushed) => {
            BootstrapCrashPoint::AfterAttemptsAnchorFlushed
        }
        (ManagedStateParentKind::Attempts, ManagedParentStage::ParentFlushed) => {
            BootstrapCrashPoint::AfterAttemptsParentFlushed
        }
        (ManagedStateParentKind::Runs, ManagedParentStage::DirectoryCreated) => {
            BootstrapCrashPoint::AfterRunsDirectoryCreated
        }
        (ManagedStateParentKind::Runs, ManagedParentStage::AnchorCreated) => {
            BootstrapCrashPoint::AfterRunsAnchorCreated
        }
        (ManagedStateParentKind::Runs, ManagedParentStage::BindingAllocated) => {
            BootstrapCrashPoint::AfterRunsBindingAllocated
        }
        (ManagedStateParentKind::Runs, ManagedParentStage::AnchorFlushed) => {
            BootstrapCrashPoint::AfterRunsAnchorFlushed
        }
        (ManagedStateParentKind::Runs, ManagedParentStage::ParentFlushed) => {
            BootstrapCrashPoint::AfterRunsParentFlushed
        }
        (ManagedStateParentKind::Trash, ManagedParentStage::DirectoryCreated) => {
            BootstrapCrashPoint::AfterTrashDirectoryCreated
        }
        (ManagedStateParentKind::Trash, ManagedParentStage::AnchorCreated) => {
            BootstrapCrashPoint::AfterTrashAnchorCreated
        }
        (ManagedStateParentKind::Trash, ManagedParentStage::BindingAllocated) => {
            BootstrapCrashPoint::AfterTrashBindingAllocated
        }
        (ManagedStateParentKind::Trash, ManagedParentStage::AnchorFlushed) => {
            BootstrapCrashPoint::AfterTrashAnchorFlushed
        }
        (ManagedStateParentKind::Trash, ManagedParentStage::ParentFlushed) => {
            BootstrapCrashPoint::AfterTrashParentFlushed
        }
        (ManagedStateParentKind::Cache, ManagedParentStage::DirectoryCreated) => {
            BootstrapCrashPoint::AfterCacheDirectoryCreated
        }
        (ManagedStateParentKind::Cache, ManagedParentStage::AnchorCreated) => {
            BootstrapCrashPoint::AfterCacheAnchorCreated
        }
        (ManagedStateParentKind::Cache, ManagedParentStage::BindingAllocated) => {
            BootstrapCrashPoint::AfterCacheBindingAllocated
        }
        (ManagedStateParentKind::Cache, ManagedParentStage::AnchorFlushed) => {
            BootstrapCrashPoint::AfterCacheAnchorFlushed
        }
        (ManagedStateParentKind::Cache, ManagedParentStage::ParentFlushed) => {
            BootstrapCrashPoint::AfterCacheParentFlushed
        }
    }
}

fn load_existing_parent(
    state_dir: &Path,
    state_directory: &HeldEntry,
    global: &GlobalNamespaceBinding,
    kind: ManagedStateParentKind,
) -> Result<ManagedStateParentBinding, StoreError> {
    let name = kind.directory_name();
    let path = state_dir.join(name);
    require_parent_bootstrap_entries(&path, kind)?;
    let directory = open_parent_directory(&path, name)?;
    require_state_volume(&directory, state_directory, name)?;
    let anchor_path = path.join("namespace.anchor");
    let anchor = HeldEntry::open(
        &anchor_path,
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        true,
        &format!("managed state anchor {name}"),
    )?;
    let header: ManagedParentAnchorHeader =
        read_canonical_path(&anchor_path, &format!("managed state anchor {name}"))?;
    if header.schema_version != ANCHOR_SCHEMA
        || &header.global != global
        || header.binding.kind != kind
        || header.binding.directory_physical_identity != *directory.identity()
        || header.binding.anchor_physical_identity != *anchor.identity()
        || !valid_nonce(&header.binding.parent_nonce)
    {
        return Err(StoreError::Integrity(format!(
            "managed state parent {name} is not a matching bootstrap remnant"
        )));
    }
    Ok(header.binding)
}

fn create_cache_eviction_parent(
    state_dir: &Path,
    global: &GlobalNamespaceBinding,
    trash_binding: &ManagedStateParentBinding,
) -> Result<CacheEvictionParentBinding, StoreError> {
    let trash_path = state_dir.join(ManagedStateParentKind::Trash.directory_name());
    let trash = open_parent_directory(&trash_path, "trash")?;
    let directory_path = trash_path.join(CACHE_EVICTIONS_DIRECTORY);
    fs::create_dir(&directory_path).map_err(io_error)?;
    hit(BootstrapCrashPoint::AfterCacheEvictionsDirectoryCreated);
    let directory = open_parent_directory(&directory_path, CACHE_EVICTIONS_DIRECTORY)?;
    if !same_volume_and_mount(&directory, &trash) {
        return Err(StoreError::Integrity(
            "cache-eviction parent crossed the trash parent volume or mount".to_owned(),
        ));
    }
    let anchor = HeldEntry::create_new(
        &directory_path.join("namespace.anchor"),
        "cache-eviction parent anchor",
    )?;
    hit(BootstrapCrashPoint::AfterCacheEvictionsAnchorCreated);
    let binding = CacheEvictionParentBinding {
        directory_physical_identity: directory.identity().clone(),
        anchor_physical_identity: anchor.identity().clone(),
        parent_nonce: nonce_hex()?,
    };
    hit(BootstrapCrashPoint::AfterCacheEvictionsBindingAllocated);
    write_canonical_entry(
        &anchor,
        &CacheEvictionParentAnchorHeader {
            schema_version: CACHE_EVICTION_ANCHOR_SCHEMA.to_owned(),
            global: global.clone(),
            trash_binding: trash_binding.clone(),
            binding: binding.clone(),
        },
    )?;
    hit(BootstrapCrashPoint::AfterCacheEvictionsAnchorFlushed);
    directory.sync_directory()?;
    hit(BootstrapCrashPoint::AfterCacheEvictionsParentFlushed);
    trash.sync_directory()?;
    hit(BootstrapCrashPoint::AfterTrashParentFlushedForCacheEvictions);
    Ok(binding)
}

fn load_existing_cache_eviction_parent(
    state_dir: &Path,
    global: &GlobalNamespaceBinding,
    trash_binding: &ManagedStateParentBinding,
) -> Result<CacheEvictionParentBinding, StoreError> {
    let trash_path = state_dir.join(ManagedStateParentKind::Trash.directory_name());
    let trash = open_parent_directory(&trash_path, "trash")?;
    let path = trash_path.join(CACHE_EVICTIONS_DIRECTORY);
    require_anchor_only(&path, CACHE_EVICTIONS_DIRECTORY)?;
    let directory = open_parent_directory(&path, CACHE_EVICTIONS_DIRECTORY)?;
    if !same_volume_and_mount(&directory, &trash) {
        return Err(StoreError::Integrity(
            "cache-eviction parent crossed the trash parent volume or mount".to_owned(),
        ));
    }
    let anchor_path = path.join("namespace.anchor");
    let anchor = HeldEntry::open(
        &anchor_path,
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        true,
        "cache-eviction parent anchor",
    )?;
    let header: CacheEvictionParentAnchorHeader =
        read_canonical_path(&anchor_path, "cache-eviction parent anchor")?;
    if header.schema_version != CACHE_EVICTION_ANCHOR_SCHEMA
        || &header.global != global
        || &header.trash_binding != trash_binding
        || header.binding.directory_physical_identity != *directory.identity()
        || header.binding.anchor_physical_identity != *anchor.identity()
        || !valid_nonce(&header.binding.parent_nonce)
    {
        return Err(StoreError::Integrity(
            "cache-eviction parent is not a matching bootstrap remnant".to_owned(),
        ));
    }
    Ok(header.binding)
}

fn validate_bootstrap_lock(
    header: &LifecycleLockHeader,
    repository: &HeldRepository,
    state_directory: &HeldEntry,
    lock: &HeldEntry,
) -> Result<(), StoreError> {
    if header.schema_version != LOCK_SCHEMA
        || validate_global_binding(&header.global).is_err()
        || verify_repository_binding(&header.global, &repository.binding).is_err()
        || header.global.state_directory_identity != *state_directory.identity()
        || header.global.lifecycle_lock_identity != *lock.identity()
        || !valid_nonce(&header.global.namespace_nonce)
    {
        return Err(StoreError::Integrity(
            "lifecycle.lock is not a matching bootstrap remnant".to_owned(),
        ));
    }
    Ok(())
}

fn verify_canonical_bootstrap_lock(
    lock: &HeldEntry,
    header: &LifecycleLockHeader,
) -> Result<(), StoreError> {
    let current: LifecycleLockHeader =
        serde_json::from_slice(&lock.read_all()?).map_err(|error| {
            StoreError::Integrity(format!("lifecycle.lock header is malformed: {error}"))
        })?;
    if &current != header {
        return Err(StoreError::Integrity(
            "lifecycle.lock changed before bootstrap resumed".to_owned(),
        ));
    }
    Ok(())
}

fn reject_unbound_bootstrap_entries(
    state_dir: &Path,
    marker_candidate: Option<&OsStr>,
) -> Result<(), StoreError> {
    for entry in fs::read_dir(state_dir).map_err(io_error)? {
        let name = entry.map_err(io_error)?.file_name();
        let allowed = name == OsStr::new("lifecycle.lock")
            || name == OsStr::new("repository.json")
            || name == OsStr::new("lifecycle.store")
            || marker_candidate.is_some_and(|candidate| name == candidate)
            || MANAGED_KINDS
                .iter()
                .any(|kind| name == OsStr::new(kind.directory_name()));
        if !allowed {
            return Err(StoreError::Integrity(
                "unbound pre-marker state contains a foreign entry".to_owned(),
            ));
        }
    }
    Ok(())
}

fn marker_candidate_name(global: &GlobalNamespaceBinding) -> OsString {
    OsString::from(format!(
        "{MARKER_CANDIDATE_PREFIX}{}",
        global.namespace_nonce
    ))
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn recoverable_marker_candidate_name(global: &GlobalNamespaceBinding) -> Option<OsString> {
    Some(marker_candidate_name(global))
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn recoverable_marker_candidate_name(_global: &GlobalNamespaceBinding) -> Option<OsString> {
    None
}

fn require_anchor_only(path: &Path, name: &str) -> Result<(), StoreError> {
    let mut entries = fs::read_dir(path).map_err(io_error)?;
    let anchor = entries
        .next()
        .transpose()
        .map_err(io_error)?
        .ok_or_else(|| {
            StoreError::Integrity(format!("managed state parent {name} omitted its anchor"))
        })?;
    if anchor.file_name() != OsStr::new("namespace.anchor") || entries.next().is_some() {
        return Err(StoreError::Integrity(format!(
            "managed state parent {name} contains foreign pre-marker state"
        )));
    }
    Ok(())
}

fn require_parent_bootstrap_entries(
    path: &Path,
    kind: ManagedStateParentKind,
) -> Result<(), StoreError> {
    let mut names = fs::read_dir(path)
        .map_err(io_error)?
        .map(|entry| entry.map(|entry| entry.file_name()).map_err(io_error))
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    let mut expected = vec![OsStr::new("namespace.anchor").to_os_string()];
    if kind == ManagedStateParentKind::Trash
        && names
            .iter()
            .any(|name| name == OsStr::new(CACHE_EVICTIONS_DIRECTORY))
    {
        expected.push(OsStr::new(CACHE_EVICTIONS_DIRECTORY).to_os_string());
        expected.sort();
    }
    if names != expected {
        return Err(StoreError::Integrity(format!(
            "managed state parent {} contains foreign pre-marker state",
            kind.directory_name()
        )));
    }
    Ok(())
}

fn open_parent_directory(path: &Path, name: &str) -> Result<HeldEntry, StoreError> {
    HeldEntry::open(
        path,
        EntryKind::Directory,
        EntryAccess::ReadOnly,
        false,
        &format!("managed state parent {name}"),
    )
}

fn valid_nonce(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
