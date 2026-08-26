mod artifacts;
#[cfg(feature = "lifecycle-migration-test-fault")]
mod barrier;
mod snapshot;

#[cfg(all(feature = "lifecycle-migration-test-fault", not(debug_assertions)))]
compile_error!("lifecycle-migration-test-fault is restricted to debug test builds");

use std::ffi::OsStr;
use std::fs;

#[cfg(feature = "lifecycle-migration-test-fault")]
use redb::ReadableTable;

use crate::{StoreError, StoreGeneration, io_error};

pub use self::artifacts::MigrationIntent;
#[cfg(feature = "lifecycle-migration-test-fault")]
use self::artifacts::remove_root_authorization_for_test;
use self::artifacts::{
    MigrationArtifactBinding, MigrationBindingEvent, MigrationBindingState, MigrationJournal,
    MigrationPhase, MigrationRootAuthorization, append_revision, append_root_authorization,
    file_sha256, next_authorization_sequence, publish_root, read_journal, read_root_authorizations,
    revalidate_journal, root_core_sha256, source_binding, source_retirement_name, target_binding,
    target_name, validate_binding_at, validate_root_authority,
};
use self::snapshot::{
    CurrentStore, LegacyStore, LogicalStoreSnapshot, create_unpublished_target,
    open_current_canonical, open_legacy_at, open_legacy_canonical, open_legacy_entry,
    read_current_entry,
};
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
use super::platform::exchange_entries;
#[cfg(windows)]
use super::platform::move_entry_noreplace;
use super::platform::{EntryAccess, EntryKind, HeldEntry, UnpublishedFile};
use super::store_header::{
    MigrationProvenanceAnchor, PRIOR_STORE_HEADER_SCHEMA, STORE_HEADER_SCHEMA,
    create_or_verify_store, store_schema,
};
use super::{
    NamespaceGuard, NamespaceState, detached_database, entry_exists, require_state_volume,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MigrationCrashPoint {
    PendingIntentCreated,
    RootAuthorizationCommitted,
    IntentPrepared,
    RootCandidateWriteStarted,
    RootCandidatePartiallyWritten,
    RootCandidateWritten,
    RootNamePublished,
    RootReopened,
    RootFileFlushed,
    RootParentFlushed,
    IntentRenamed,
    IntentPublished,
    RevisionCandidateCreated,
    RevisionCandidateWriteStarted,
    RevisionCandidatePartiallyWritten,
    RevisionCandidateWritten,
    RevisionNamePublished,
    RevisionReopened,
    RevisionFileFlushed,
    RevisionParentFlushed,
    CopiesValidated,
    TargetNamePublished,
    TargetReopened,
    TargetFileFlushed,
    TargetParentFlushed,
    TargetPublished,
    BeforeExchange,
    ExchangeInputsOpened,
    ExchangeExternalReferencesValidated,
    #[cfg(windows)]
    SourceRetired,
    #[cfg(windows)]
    CanonicalMoveExternalReferencesValidated,
    CanonicalReplaced,
    ParentFlushed,
    IntentRemoved,
    TerminalSourceValidated,
}

impl NamespaceState {
    pub(crate) fn migrate_lifecycle_store(&self) -> Result<StoreGeneration, StoreError> {
        self.with_migration_lock(|guard| {
            migrate_with_hook(guard, &mut |point| {
                #[cfg(feature = "lifecycle-migration-test-fault")]
                barrier::wait(point.as_str())?;
                #[cfg(not(feature = "lifecycle-migration-test-fault"))]
                let _ = point;
                Ok(())
            })
        })
    }
}

impl MigrationCrashPoint {
    #[cfg(feature = "lifecycle-migration-test-fault")]
    fn as_str(self) -> &'static str {
        match self {
            Self::PendingIntentCreated => "after-pending-intent-create",
            Self::RootAuthorizationCommitted => "after-root-authorization",
            Self::IntentPrepared => "after-pending-intent-sync",
            Self::RootCandidateWriteStarted => "after-root-write-start",
            Self::RootCandidatePartiallyWritten => "after-root-partial-write",
            Self::RootCandidateWritten => "after-root-write",
            Self::RootNamePublished => "after-root-name-publication",
            Self::RootReopened => "after-root-reopen",
            Self::RootFileFlushed => "after-root-file-flush",
            Self::RootParentFlushed => "after-root-parent-flush",
            Self::IntentRenamed => "after-intent-rename",
            Self::IntentPublished => "after-intent",
            Self::RevisionCandidateCreated => "after-revision-candidate-create",
            Self::RevisionCandidateWriteStarted => "after-revision-write-start",
            Self::RevisionCandidatePartiallyWritten => "after-revision-partial-write",
            Self::RevisionCandidateWritten => "after-revision-write",
            Self::RevisionNamePublished => "after-revision-name-publication",
            Self::RevisionReopened => "after-revision-reopen",
            Self::RevisionFileFlushed => "after-revision-file-flush",
            Self::RevisionParentFlushed => "after-revision-parent-flush",
            Self::CopiesValidated => "after-validated-replacement",
            Self::TargetNamePublished => "after-target-name-publication",
            Self::TargetReopened => "after-target-reopen",
            Self::TargetFileFlushed => "after-target-file-flush",
            Self::TargetParentFlushed => "after-target-parent-flush",
            Self::TargetPublished => "after-target-publication",
            Self::BeforeExchange => "before-exchange",
            Self::ExchangeInputsOpened => "after-exchange-input-open",
            Self::ExchangeExternalReferencesValidated => "after-exchange-external-validation",
            #[cfg(windows)]
            Self::SourceRetired => "after-source-retirement",
            #[cfg(windows)]
            Self::CanonicalMoveExternalReferencesValidated => {
                "after-canonical-move-external-validation"
            }
            Self::CanonicalReplaced => "after-replace",
            Self::ParentFlushed => "after-parent-flush",
            Self::IntentRemoved => "after-intent-removal",
            Self::TerminalSourceValidated => "after-terminal-source-validation",
        }
    }
}

pub(super) fn require_idle(guard: &NamespaceGuard) -> Result<(), StoreError> {
    match read_journal(guard)? {
        Some(journal) if journal.phase == MigrationPhase::Terminal => {
            validate_terminal(guard, &journal, &mut |_| Ok(())).map(|_| ())
        }
        Some(journal) => {
            validate_nonterminal_envelope(guard, &journal)?;
            Err(StoreError::LifecycleMigrationRequired)
        }
        None => {
            reject_orphan_migration_artifacts(guard)?;
            Ok(())
        }
    }
}

#[cfg(test)]
pub(super) fn validate_journal_payload_recheck_for_test(
    guard: &NamespaceGuard,
    hook: &mut impl FnMut() -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    artifacts::read_journal_with_after_fold_for_test(guard, hook)?
        .ok_or_else(|| StoreError::Integrity("test migration journal is missing".to_owned()))?;
    Ok(())
}

#[cfg(test)]
pub(super) fn validate_rebound_target_for_test(guard: &NamespaceGuard) -> Result<(), StoreError> {
    let mut journal = read_journal(guard)?
        .ok_or_else(|| StoreError::Integrity("test migration journal is missing".to_owned()))?;
    let original = journal.target()?.binding.clone();
    let path = guard.state.state_dir.join(&original.pre_exchange_name);
    let entry = HeldEntry::open(
        &path,
        EntryKind::RegularFile,
        EntryAccess::ReadWrite,
        true,
        "test rebound migration target",
    )?;
    if entry.identity() != &original.physical_identity {
        return Err(StoreError::Integrity(
            "test rebound migration target changed identity".to_owned(),
        ));
    }
    let byte_sha256 = file_sha256(&entry)?;
    drop(entry);
    let target = journal
        .targets
        .get_mut(&original.binding_id)
        .ok_or_else(|| StoreError::Integrity("test migration target is missing".to_owned()))?;
    target.binding.byte_sha256 = byte_sha256;
    let rebound = target.binding.clone();
    validate_typed_target_at(
        guard,
        &journal,
        &rebound,
        &rebound.pre_exchange_name,
        "rebound migration target",
    )
}

#[cfg(feature = "lifecycle-migration-test-fault")]
pub(super) fn corrupt_bound_cleanup_operation_for_test(
    guard: &NamespaceGuard,
    operation_id: &lumin_model::OperationId,
) -> Result<(), StoreError> {
    let journal = read_journal(guard)?.ok_or_else(|| {
        StoreError::Integrity("test corruption requires a live journal".to_owned())
    })?;
    if journal.phase != MigrationPhase::ObservedSource {
        return Err(StoreError::Integrity(
            "test corruption requires the observed-source migration phase".to_owned(),
        ));
    }
    validate_source_envelope_at(guard, &journal, "lifecycle.store")?;
    let path = guard.state.state_dir.join("lifecycle.store");
    let entry = HeldEntry::open(
        &path,
        EntryKind::RegularFile,
        EntryAccess::ReadWrite,
        true,
        "test prior lifecycle.store corruption",
    )?;
    let database = redb::Database::builder()
        .create_file(entry.file().try_clone().map_err(io_error)?)
        .map_err(crate::backend_error)?;
    let write = database.begin_write().map_err(crate::backend_error)?;
    {
        let mut table = write
            .open_table(crate::cache::CACHE_CLEANUP_OPERATIONS)
            .map_err(crate::backend_error)?;
        if table
            .get(operation_id.as_str())
            .map_err(crate::backend_error)?
            .is_none()
        {
            return Err(StoreError::OperationNotFound(
                operation_id.as_str().to_owned(),
            ));
        }
        table
            .insert(operation_id.as_str(), b"{}".as_slice())
            .map_err(crate::backend_error)?;
    }
    write.commit().map_err(crate::backend_error)?;
    drop(database);
    entry.sync()
}

#[cfg(feature = "lifecycle-migration-test-fault")]
pub(super) fn remove_bound_root_authorization_for_test(
    guard: &NamespaceGuard,
) -> Result<(), StoreError> {
    let journal = read_journal(guard)?
        .ok_or_else(|| StoreError::Integrity("test mutation requires a live journal".to_owned()))?;
    if journal.phase != MigrationPhase::ObservedSource {
        return Err(StoreError::Integrity(
            "test mutation requires the observed-source migration phase".to_owned(),
        ));
    }
    validate_source_envelope_at(guard, &journal, "lifecycle.store")?;
    let path = guard.state.state_dir.join("lifecycle.store");
    let entry = HeldEntry::open(
        &path,
        EntryKind::RegularFile,
        EntryAccess::ReadWrite,
        true,
        "test prior lifecycle.store authorization removal",
    )?;
    let database = redb::Database::builder()
        .create_file(entry.file().try_clone().map_err(io_error)?)
        .map_err(crate::backend_error)?;
    remove_root_authorization_for_test(
        &database,
        journal.root_authorization.authorization_sequence,
    )?;
    drop(database);
    entry.sync()
}

pub(super) fn admit_ordinary(guard: &NamespaceGuard) -> Result<(), StoreError> {
    match read_journal(guard)? {
        Some(journal) if journal.phase == MigrationPhase::Terminal => {
            validate_terminal(guard, &journal, &mut |_| Ok(())).map(|_| ())
        }
        Some(journal) => {
            validate_nonterminal_envelope(guard, &journal)?;
            Err(StoreError::LifecycleMigrationRequired)
        }
        None => {
            reject_orphan_migration_artifacts(guard)?;
            create_or_verify_store(guard)?;
            let current = open_current_canonical(guard)?;
            if current.anchor.is_some() {
                return Err(StoreError::Integrity(
                    "migrated lifecycle.store omitted its permanent journal".to_owned(),
                ));
            }
            Ok(())
        }
    }
}

pub(super) fn migrate_with_hook(
    guard: &NamespaceGuard,
    hook: &mut impl FnMut(MigrationCrashPoint) -> Result<(), StoreError>,
) -> Result<StoreGeneration, StoreError> {
    guard.validate_bound_entries()?;
    if let Some(journal) = read_journal(guard)? {
        reject_unbound_migration_artifacts(guard, &journal)?;
        return recover_journal(guard, journal, hook);
    }
    reject_orphan_migration_artifacts(guard)?;
    let path = guard.state.state_dir.join("lifecycle.store");
    if !entry_exists(&path)? {
        return Err(StoreError::Integrity(
            "initialized state namespace omitted lifecycle.store".to_owned(),
        ));
    }
    let entry = HeldEntry::open(
        &path,
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        true,
        "lifecycle.store",
    )?;
    require_state_volume(&entry, &guard.state_directory, "lifecycle.store")?;
    let database = detached_database(guard, &entry)?;
    let schema = store_schema(&database)?;
    drop(database);
    drop(entry);
    match schema.as_str() {
        STORE_HEADER_SCHEMA => validate_native_current(guard),
        PRIOR_STORE_HEADER_SCHEMA => begin_migration(guard, hook),
        _ => Err(StoreError::IncompatibleStateSchema(format!(
            "lifecycle.store schema {schema} is unsupported for migration"
        ))),
    }
}

fn validate_native_current(guard: &NamespaceGuard) -> Result<StoreGeneration, StoreError> {
    validate_native_current_with_after_external(guard, &mut || Ok(()))
}

#[cfg(test)]
pub(super) fn validate_native_current_recheck_for_test(
    guard: &NamespaceGuard,
    hook: &mut impl FnMut() -> Result<(), StoreError>,
) -> Result<StoreGeneration, StoreError> {
    validate_native_current_with_after_external(guard, hook)
}

fn validate_native_current_with_after_external(
    guard: &NamespaceGuard,
    hook: &mut impl FnMut() -> Result<(), StoreError>,
) -> Result<StoreGeneration, StoreError> {
    let current = open_current_canonical(guard)?;
    if current.anchor.is_some() {
        return Err(StoreError::Integrity(
            "migrated lifecycle.store omitted its permanent journal".to_owned(),
        ));
    }
    current.snapshot.validate_external_references(guard)?;
    hook()?;
    let current = revalidate_current_canonical(guard, &current)?;
    current.snapshot.validate_external_references(guard)?;
    revalidate_current_canonical(guard, &current).map(|current| current.generation)
}

fn revalidate_current_canonical(
    guard: &NamespaceGuard,
    expected: &CurrentStore,
) -> Result<CurrentStore, StoreError> {
    let observed = open_current_canonical(guard)?;
    if observed.entry.identity() != expected.entry.identity()
        || observed.generation != expected.generation
        || observed.anchor != expected.anchor
        || observed.snapshot != expected.snapshot
    {
        return Err(StoreError::Integrity(
            "lifecycle.store changed after external reference validation".to_owned(),
        ));
    }
    Ok(observed)
}

fn begin_migration(
    guard: &NamespaceGuard,
    hook: &mut impl FnMut(MigrationCrashPoint) -> Result<(), StoreError>,
) -> Result<StoreGeneration, StoreError> {
    let legacy = open_legacy_canonical(guard)?;
    let target_generation = legacy
        .generation
        .checked_next()
        .ok_or_else(|| StoreError::Integrity("lifecycle store generation overflow".to_owned()))?;
    let source_logical = legacy.snapshot.logical_sha256()?;
    let transformed = legacy.snapshot.clone().transformed_from_v12(guard)?;
    transformed.validate_external_references(guard)?;

    let root = UnpublishedFile::create(&guard.state.state_dir, &guard.state_directory)?;
    hook(MigrationCrashPoint::PendingIntentCreated)?;
    let authorization_sequence = next_authorization_sequence(&legacy.database)?;
    let root_core = root_core_sha256(
        root.entry().identity(),
        legacy.generation,
        target_generation,
        legacy.entry.identity(),
        &source_logical,
    )?;
    let anchor = MigrationProvenanceAnchor {
        authorization_sequence,
        root_name: artifacts::MIGRATION_ROOT_NAME.to_owned(),
        root_physical_identity: root.entry().identity().clone(),
        root_core_sha256: root_core.clone(),
        source_generation: legacy.generation,
        source_schema: PRIOR_STORE_HEADER_SCHEMA.to_owned(),
        source_physical_identity: legacy.entry.identity().clone(),
        source_user_logical_sha256: source_logical.clone(),
    };
    let target_logical = transformed.anchored_logical_sha256(&anchor)?;
    let authorization = MigrationRootAuthorization {
        authorization_sequence,
        source_generation: legacy.generation,
        target_generation,
        source_schema: PRIOR_STORE_HEADER_SCHEMA.to_owned(),
        target_schema: STORE_HEADER_SCHEMA.to_owned(),
        root_name: artifacts::MIGRATION_ROOT_NAME.to_owned(),
        root_physical_identity: root.entry().identity().clone(),
        root_core_sha256: root_core,
        source_user_logical_sha256: source_logical.clone(),
        target_user_logical_sha256: target_logical,
    };
    append_root_authorization(&legacy.database, &authorization)?;
    hook(MigrationCrashPoint::RootAuthorizationCommitted)?;
    legacy.entry.sync()?;
    hook(MigrationCrashPoint::IntentPrepared)?;
    drop(legacy.database);
    drop(legacy.entry);

    let source = open_legacy_at(guard, "lifecycle.store", "authorized prior lifecycle.store")?;
    if source.snapshot.logical_sha256()? != source_logical {
        return Err(StoreError::Integrity(
            "source logical state changed while authorizing migration root".to_owned(),
        ));
    }
    validate_authorization_history(&source, &authorization)?;
    let target_slot = target_name(&format!("{authorization_sequence:016}-0001"));
    let retired_name =
        source_retirement_name(&target_slot, &format!("{authorization_sequence:016}"));
    let source_identity = source.entry.identity().clone();
    let source_generation = source.generation;
    let source_entry = source.entry;
    drop(source.database);
    source_entry.sync()?;
    let source_binding = source_binding(
        source_identity,
        retired_name,
        source_generation,
        file_sha256(&source_entry)?,
        source_logical,
    )?;
    drop(source_entry);
    let journal = publish_root(guard, root, &authorization, source_binding, hook)?;
    hook(MigrationCrashPoint::IntentRenamed)?;
    hook(MigrationCrashPoint::IntentPublished)?;
    recover_journal(guard, journal, hook)
}

fn recover_journal(
    guard: &NamespaceGuard,
    mut journal: MigrationJournal,
    hook: &mut impl FnMut(MigrationCrashPoint) -> Result<(), StoreError>,
) -> Result<StoreGeneration, StoreError> {
    loop {
        journal = match journal.phase {
            MigrationPhase::ObservedSource | MigrationPhase::TargetPending => {
                ensure_target_published(guard, journal, hook)?
            }
            MigrationPhase::TargetPublished => exchange_store(guard, journal, hook)?,
            MigrationPhase::Exchanged => terminalize(guard, journal, hook)?,
            MigrationPhase::Terminal => {
                return validate_terminal(guard, &journal, hook).map(|current| current.generation);
            }
        };
    }
}

fn ensure_target_published(
    guard: &NamespaceGuard,
    mut journal: MigrationJournal,
    hook: &mut impl FnMut(MigrationCrashPoint) -> Result<(), StoreError>,
) -> Result<MigrationJournal, StoreError> {
    let mut prepared = None;
    if journal.phase == MigrationPhase::TargetPending {
        let target = journal.target()?.binding.clone();
        let path = guard.state.state_dir.join(&target.pre_exchange_name);
        if entry_exists(&path)? {
            validate_binding_at(
                guard,
                &target,
                &target.pre_exchange_name,
                true,
                "published migration target",
            )?;
            return append_revision(
                guard,
                &journal,
                MigrationPhase::TargetPublished,
                vec![MigrationBindingEvent::Published {
                    binding_id: target.binding_id,
                }],
                hook,
            );
        }
        let attempt = journal.next_target_publication_attempt()?;
        let candidate = prepare_target_candidate(guard, &journal, attempt)?;
        let new_binding = candidate.1.clone();
        journal = append_revision(
            guard,
            &journal,
            MigrationPhase::TargetPending,
            vec![
                MigrationBindingEvent::SupersededUnpublished {
                    binding_id: target.binding_id,
                },
                MigrationBindingEvent::PendingPublication {
                    binding: new_binding,
                },
            ],
            hook,
        )?;
        prepared = Some(candidate);
    }

    let (unpublished, binding) = if let Some(prepared) = prepared {
        prepared
    } else {
        let attempt = journal.next_target_publication_attempt()?;
        let prepared = prepare_target_candidate(guard, &journal, attempt)?;
        journal = append_revision(
            guard,
            &journal,
            MigrationPhase::TargetPending,
            vec![MigrationBindingEvent::PendingPublication {
                binding: prepared.1.clone(),
            }],
            hook,
        )?;
        prepared
    };
    hook(MigrationCrashPoint::CopiesValidated)?;
    let published = unpublished.publish_noreplace(
        &guard.state_directory,
        &guard.state.state_dir,
        OsStr::new(&binding.pre_exchange_name),
        "migration target",
        || hook(MigrationCrashPoint::TargetNamePublished),
    )?;
    hook(MigrationCrashPoint::TargetReopened)?;
    if published.identity() != &binding.physical_identity
        || file_sha256(&published)? != binding.byte_sha256
    {
        return Err(StoreError::Integrity(
            "migration target changed during publication".to_owned(),
        ));
    }
    published.sync()?;
    hook(MigrationCrashPoint::TargetFileFlushed)?;
    guard.state_directory.sync_directory()?;
    hook(MigrationCrashPoint::TargetParentFlushed)?;
    hook(MigrationCrashPoint::TargetPublished)?;
    validate_binding_at(
        guard,
        &binding,
        &binding.pre_exchange_name,
        true,
        "published migration target",
    )?;
    append_revision(
        guard,
        &journal,
        MigrationPhase::TargetPublished,
        vec![MigrationBindingEvent::Published {
            binding_id: binding.binding_id,
        }],
        hook,
    )
}

fn prepare_target_candidate(
    guard: &NamespaceGuard,
    journal: &MigrationJournal,
    attempt: u64,
) -> Result<(UnpublishedFile, MigrationArtifactBinding), StoreError> {
    let source = open_bound_source(guard, journal)?;
    let transformed = source.snapshot.clone().transformed_from_v12(guard)?;
    let anchor = anchor_for(journal);
    let logical = transformed.anchored_logical_sha256(&anchor)?;
    if logical != journal.root_authorization.target_user_logical_sha256 {
        return Err(StoreError::Integrity(
            "reconstructed migration target disagrees with root authorization".to_owned(),
        ));
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    let name = journal.source.binding.post_exchange_name.clone();
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    let name = target_name(&format!(
        "{:016}-{attempt:04}",
        journal.root_authorization.authorization_sequence
    ));
    if entry_exists(&guard.state.state_dir.join(&name))? {
        return Err(StoreError::Integrity(
            "unbound migration target name already exists".to_owned(),
        ));
    }
    let unpublished = UnpublishedFile::create(&guard.state.state_dir, &guard.state_directory)?;
    create_unpublished_target(
        guard,
        &unpublished,
        journal.root_authorization.target_generation,
        &transformed,
        &anchor,
    )?;
    let binding = target_binding(
        unpublished.entry().identity().clone(),
        attempt,
        name,
        journal.root_authorization.target_generation,
        file_sha256(unpublished.entry())?,
        logical,
    )?;
    Ok((unpublished, binding))
}

fn exchange_store(
    guard: &NamespaceGuard,
    journal: MigrationJournal,
    hook: &mut impl FnMut(MigrationCrashPoint) -> Result<(), StoreError>,
) -> Result<MigrationJournal, StoreError> {
    validate_target_published_envelope(guard, &journal)?;
    let transformed = open_exchange_source(guard, &journal)?
        .snapshot
        .transformed_from_v12(guard)?;
    transformed.validate_external_references(guard)?;
    let source = journal.source.binding.clone();
    let target = journal.target()?.binding.clone();
    validate_exchange_names(&source, &target)?;
    exchange_or_recover(guard, &journal, &transformed, hook)?;
    validate_binding_at(
        guard,
        &source,
        &source.post_exchange_name,
        true,
        "retained migration source",
    )?;
    validate_binding_at(
        guard,
        &target,
        &target.post_exchange_name,
        true,
        "canonical migration target",
    )?;
    hook(MigrationCrashPoint::CanonicalReplaced)?;
    guard.state_directory.sync_directory()?;
    hook(MigrationCrashPoint::ParentFlushed)?;
    append_revision(
        guard,
        &journal,
        MigrationPhase::Exchanged,
        vec![
            MigrationBindingEvent::Exchanged {
                binding_id: source.binding_id,
            },
            MigrationBindingEvent::Exchanged {
                binding_id: target.binding_id,
            },
        ],
        hook,
    )
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn exchange_or_recover(
    guard: &NamespaceGuard,
    journal: &MigrationJournal,
    transformed: &LogicalStoreSnapshot,
    hook: &mut impl FnMut(MigrationCrashPoint) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let source = &journal.source.binding;
    let target = &journal.target()?.binding;
    let canonical = HeldEntry::open(
        &guard.state.state_dir.join("lifecycle.store"),
        EntryKind::RegularFile,
        EntryAccess::Move,
        true,
        "migration exchange canonical entry",
    )?;
    let private = HeldEntry::open(
        &guard.state.state_dir.join(&target.pre_exchange_name),
        EntryKind::RegularFile,
        EntryAccess::Move,
        true,
        "migration exchange private entry",
    )?;
    let before = canonical.identity() == &source.physical_identity
        && private.identity() == &target.physical_identity;
    let after = canonical.identity() == &target.physical_identity
        && private.identity() == &source.physical_identity;
    if before {
        drop(canonical);
        drop(private);
        hook(MigrationCrashPoint::BeforeExchange)?;
        let canonical = open_binding_for_move(
            guard,
            source,
            "lifecycle.store",
            "migration source before exchange",
        )?;
        let private = open_binding_for_move(
            guard,
            target,
            &target.pre_exchange_name,
            "migration target before exchange",
        )?;
        hook(MigrationCrashPoint::ExchangeInputsOpened)?;
        transformed.validate_external_references(guard)?;
        hook(MigrationCrashPoint::ExchangeExternalReferencesValidated)?;
        revalidate_journal(guard, journal)?;
        revalidate_binding_for_move(
            guard,
            &canonical,
            source,
            "lifecycle.store",
            "migration source before exchange",
        )?;
        revalidate_binding_for_move(
            guard,
            &private,
            target,
            &target.pre_exchange_name,
            "migration target before exchange",
        )?;
        exchange_entries(
            &guard.state_directory,
            OsStr::new("lifecycle.store"),
            &canonical,
            OsStr::new(&target.pre_exchange_name),
            &private,
        )?;
    } else if !after {
        return Err(StoreError::Integrity(
            "Linux migration exchange entries have unknown identities".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn exchange_or_recover(
    guard: &NamespaceGuard,
    journal: &MigrationJournal,
    transformed: &LogicalStoreSnapshot,
    hook: &mut impl FnMut(MigrationCrashPoint) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let source = &journal.source.binding;
    let target = &journal.target()?.binding;
    let canonical_path = guard.state.state_dir.join("lifecycle.store");
    let source_path = guard.state.state_dir.join(&source.post_exchange_name);
    let target_path = guard.state.state_dir.join(&target.pre_exchange_name);
    let canonical_exists = entry_exists(&canonical_path)?;
    let source_exists = entry_exists(&source_path)?;
    let target_exists = entry_exists(&target_path)?;

    if canonical_exists {
        let canonical = HeldEntry::open(
            &canonical_path,
            EntryKind::RegularFile,
            EntryAccess::Move,
            true,
            "migration exchange canonical entry",
        )?;
        if canonical.identity() == &source.physical_identity {
            if source_exists || !target_exists {
                return Err(StoreError::Integrity(
                    "Windows migration source-retirement placement is incoherent".to_owned(),
                ));
            }
            drop(canonical);
            hook(MigrationCrashPoint::BeforeExchange)?;
            if entry_exists(&source_path)? || !entry_exists(&target_path)? {
                return Err(StoreError::Integrity(
                    "Windows migration source-retirement placement changed before exchange"
                        .to_owned(),
                ));
            }
            let canonical = open_binding_for_move(
                guard,
                source,
                "lifecycle.store",
                "migration source before exchange",
            )?;
            let target_entry = open_binding_for_move(
                guard,
                target,
                &target.pre_exchange_name,
                "migration target before exchange",
            )?;
            hook(MigrationCrashPoint::ExchangeInputsOpened)?;
            transformed.validate_external_references(guard)?;
            hook(MigrationCrashPoint::ExchangeExternalReferencesValidated)?;
            revalidate_journal(guard, journal)?;
            revalidate_binding_for_move(
                guard,
                &canonical,
                source,
                "lifecycle.store",
                "migration source before exchange",
            )?;
            revalidate_binding_for_move(
                guard,
                &target_entry,
                target,
                &target.pre_exchange_name,
                "migration target before exchange",
            )?;
            move_entry_noreplace(
                &guard.state_directory,
                OsStr::new("lifecycle.store"),
                &canonical,
                &guard.state_directory,
                OsStr::new(&source.post_exchange_name),
            )?;
            guard.state_directory.sync_directory()?;
            hook(MigrationCrashPoint::SourceRetired)?;
            transformed.validate_external_references(guard)?;
            hook(MigrationCrashPoint::CanonicalMoveExternalReferencesValidated)?;
            revalidate_journal(guard, journal)?;
            revalidate_binding_for_move(
                guard,
                &canonical,
                source,
                &source.post_exchange_name,
                "retired migration source",
            )?;
            revalidate_binding_for_move(
                guard,
                &target_entry,
                target,
                &target.pre_exchange_name,
                "migration target before canonical move",
            )?;
            move_entry_noreplace(
                &guard.state_directory,
                OsStr::new(&target.pre_exchange_name),
                &target_entry,
                &guard.state_directory,
                OsStr::new("lifecycle.store"),
            )?;
        } else if canonical.identity() != &target.physical_identity
            || !source_exists
            || target_exists
        {
            return Err(StoreError::Integrity(
                "Windows migration exchange entries have unknown identities".to_owned(),
            ));
        }
    } else {
        if !source_exists || !target_exists {
            return Err(StoreError::Integrity(
                "canonical-absent migration exchange omitted a bound object".to_owned(),
            ));
        }
        let source_entry = open_binding_for_move(
            guard,
            source,
            &source.post_exchange_name,
            "retired migration source",
        )?;
        let target_entry = open_binding_for_move(
            guard,
            target,
            &target.pre_exchange_name,
            "migration target before canonical move",
        )?;
        hook(MigrationCrashPoint::ExchangeInputsOpened)?;
        transformed.validate_external_references(guard)?;
        hook(MigrationCrashPoint::CanonicalMoveExternalReferencesValidated)?;
        revalidate_journal(guard, journal)?;
        revalidate_binding_for_move(
            guard,
            &source_entry,
            source,
            &source.post_exchange_name,
            "retired migration source",
        )?;
        revalidate_binding_for_move(
            guard,
            &target_entry,
            target,
            &target.pre_exchange_name,
            "migration target before canonical move",
        )?;
        move_entry_noreplace(
            &guard.state_directory,
            OsStr::new(&target.pre_exchange_name),
            &target_entry,
            &guard.state_directory,
            OsStr::new("lifecycle.store"),
        )?;
    }
    Ok(())
}

#[cfg(not(any(all(target_os = "linux", target_arch = "x86_64"), windows)))]
fn exchange_or_recover(
    _guard: &NamespaceGuard,
    _journal: &MigrationJournal,
    _transformed: &LogicalStoreSnapshot,
    _hook: &mut impl FnMut(MigrationCrashPoint) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    Err(StoreError::Integrity(
        "lifecycle-store migration exchange supports Windows and Linux x64".to_owned(),
    ))
}

fn terminalize(
    guard: &NamespaceGuard,
    journal: MigrationJournal,
    hook: &mut impl FnMut(MigrationCrashPoint) -> Result<(), StoreError>,
) -> Result<MigrationJournal, StoreError> {
    let source = journal.source.binding.clone();
    let target = journal.target()?.binding.clone();
    let source_entry = validate_binding_at(
        guard,
        &source,
        &source.post_exchange_name,
        true,
        "retained migration source",
    )?;
    let legacy = open_legacy_entry(
        guard,
        source_entry,
        &source.post_exchange_name,
        "retained migration source",
    )?;
    validate_source(&legacy, &journal, &source)?;
    let canonical = validate_binding_at(
        guard,
        &target,
        &target.post_exchange_name,
        true,
        "canonical migration target",
    )?;
    let snapshot = read_current_entry(
        guard,
        &canonical,
        journal.root_authorization.target_generation,
        Some(&anchor_for(&journal)),
    )?;
    if snapshot.anchored_logical_sha256(&anchor_for(&journal))?
        != journal.root_authorization.target_user_logical_sha256
    {
        return Err(StoreError::Integrity(
            "canonical migration target changed before terminalization".to_owned(),
        ));
    }
    snapshot.validate_external_references(guard)?;
    let terminal = append_revision(
        guard,
        &journal,
        MigrationPhase::Terminal,
        vec![
            MigrationBindingEvent::RetainedImmutable {
                binding_id: source.binding_id,
            },
            MigrationBindingEvent::CanonicalMutable {
                binding_id: target.binding_id,
            },
        ],
        hook,
    )?;
    hook(MigrationCrashPoint::IntentRemoved)?;
    Ok(terminal)
}

fn validate_terminal(
    guard: &NamespaceGuard,
    journal: &MigrationJournal,
    hook: &mut impl FnMut(MigrationCrashPoint) -> Result<(), StoreError>,
) -> Result<CurrentStore, StoreError> {
    if journal.phase != MigrationPhase::Terminal
        || journal.source.state != MigrationBindingState::Retained
        || journal.target()?.state != MigrationBindingState::Canonical
    {
        return Err(StoreError::Integrity(
            "migration journal is not terminal".to_owned(),
        ));
    }
    reject_unbound_migration_artifacts(guard, journal)?;
    let source_binding = &journal.source.binding;
    let source = validate_binding_at(
        guard,
        source_binding,
        &source_binding.post_exchange_name,
        true,
        "retained migration source",
    )?;
    let source_database = detached_database(guard, &source)?;
    let source_generation =
        super::store_header::verify_prior_store_header(&source_database, &guard.state.binding)?;
    if source_generation != journal.root_authorization.source_generation {
        return Err(StoreError::Integrity(
            "retained migration source generation changed".to_owned(),
        ));
    }
    validate_root_authority(&source_database, journal)?;
    drop(source_database);

    let current = open_current_canonical(guard)?;
    let target = journal.target()?;
    if current.entry.identity() != &target.binding.physical_identity
        || current.generation != journal.root_authorization.target_generation
        || current.anchor.as_ref() != Some(&anchor_for(journal))
    {
        return Err(StoreError::Integrity(
            "mutable migrated lifecycle.store lost its terminal provenance".to_owned(),
        ));
    }
    current.snapshot.validate_external_references(guard)?;
    hook(MigrationCrashPoint::TerminalSourceValidated)?;
    revalidate_retained_source(guard, &source, source_binding)?;
    let current = revalidate_current_canonical(guard, &current)?;
    revalidate_journal(guard, journal)?;
    current.snapshot.validate_external_references(guard)?;
    revalidate_retained_source(guard, &source, source_binding)?;
    let current = revalidate_current_canonical(guard, &current)?;
    revalidate_journal(guard, journal)?;
    Ok(current)
}

fn revalidate_retained_source(
    guard: &NamespaceGuard,
    source: &HeldEntry,
    binding: &MigrationArtifactBinding,
) -> Result<(), StoreError> {
    source.validate_path(
        &guard.state.state_dir.join(&binding.post_exchange_name),
        EntryKind::RegularFile,
        EntryAccess::ReadWrite,
        true,
        "retained migration source",
    )?;
    if file_sha256(source)? != binding.byte_sha256 {
        return Err(StoreError::Integrity(
            "retained migration source payload changed during terminal validation".to_owned(),
        ));
    }
    Ok(())
}

fn validate_nonterminal_envelope(
    guard: &NamespaceGuard,
    journal: &MigrationJournal,
) -> Result<(), StoreError> {
    reject_unbound_migration_artifacts(guard, journal)?;
    match journal.phase {
        MigrationPhase::ObservedSource => {
            validate_source_envelope_at(guard, journal, "lifecycle.store")?;
            require_missing_private_source(guard, journal)?;
        }
        MigrationPhase::TargetPending => {
            validate_source_envelope_at(guard, journal, "lifecycle.store")?;
            let target = journal.target()?;
            validate_optional_binding_at(
                guard,
                &target.binding,
                &target.binding.pre_exchange_name,
                "pending migration target",
            )?;
            if journal.source.binding.post_exchange_name != target.binding.pre_exchange_name {
                require_missing_entry(
                    guard,
                    &journal.source.binding.post_exchange_name,
                    "retired migration source appeared before exchange",
                )?;
            }
        }
        MigrationPhase::TargetPublished => {
            validate_target_published_envelope(guard, journal)?;
        }
        MigrationPhase::Exchanged => {
            let target = journal.target()?;
            validate_source_envelope_at(
                guard,
                journal,
                &journal.source.binding.post_exchange_name,
            )?;
            validate_binding_at(
                guard,
                &target.binding,
                &target.binding.post_exchange_name,
                true,
                "canonical migration target",
            )?;
            if target.binding.pre_exchange_name != journal.source.binding.post_exchange_name {
                require_missing_entry(
                    guard,
                    &target.binding.pre_exchange_name,
                    "migration target staging entry remained after exchange",
                )?;
            }
        }
        MigrationPhase::Terminal => {
            return Err(StoreError::Integrity(
                "terminal migration journal reached nonterminal validation".to_owned(),
            ));
        }
    }
    Ok(())
}

fn validate_source_envelope_at(
    guard: &NamespaceGuard,
    journal: &MigrationJournal,
    name: &str,
) -> Result<(), StoreError> {
    let entry = validate_binding_at(
        guard,
        &journal.source.binding,
        name,
        false,
        "bound migration source",
    )?;
    let database = detached_database(guard, &entry)?;
    let generation =
        super::store_header::verify_prior_store_header(&database, &guard.state.binding)?;
    if generation != journal.root_authorization.source_generation {
        return Err(StoreError::Integrity(
            "bound migration source generation changed".to_owned(),
        ));
    }
    validate_root_authority(&database, journal)
}

fn validate_optional_binding_at(
    guard: &NamespaceGuard,
    binding: &MigrationArtifactBinding,
    name: &str,
    label: &str,
) -> Result<(), StoreError> {
    if entry_exists(&guard.state.state_dir.join(name))? {
        validate_binding_at(guard, binding, name, true, label)?;
    }
    Ok(())
}

fn require_missing_private_source(
    guard: &NamespaceGuard,
    journal: &MigrationJournal,
) -> Result<(), StoreError> {
    require_missing_entry(
        guard,
        &journal.source.binding.post_exchange_name,
        "retired migration source appeared before target publication",
    )
}

fn require_missing_entry(
    guard: &NamespaceGuard,
    name: &str,
    message: &str,
) -> Result<(), StoreError> {
    if entry_exists(&guard.state.state_dir.join(name))? {
        return Err(StoreError::Integrity(message.to_owned()));
    }
    Ok(())
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn validate_target_published_envelope(
    guard: &NamespaceGuard,
    journal: &MigrationJournal,
) -> Result<(), StoreError> {
    let source = &journal.source.binding;
    let target = &journal.target()?.binding;
    validate_exchange_names(source, target)?;
    let canonical = HeldEntry::open(
        &guard.state.state_dir.join("lifecycle.store"),
        EntryKind::RegularFile,
        EntryAccess::ReadWrite,
        true,
        "migration exchange canonical entry",
    )?;
    let private = HeldEntry::open(
        &guard.state.state_dir.join(&target.pre_exchange_name),
        EntryKind::RegularFile,
        EntryAccess::ReadWrite,
        true,
        "migration exchange private entry",
    )?;
    let before = canonical.identity() == &source.physical_identity
        && private.identity() == &target.physical_identity;
    let after = canonical.identity() == &target.physical_identity
        && private.identity() == &source.physical_identity;
    drop(canonical);
    drop(private);
    if before {
        validate_source_envelope_at(guard, journal, "lifecycle.store")?;
        validate_typed_target_at(
            guard,
            journal,
            target,
            &target.pre_exchange_name,
            "published migration target",
        )?;
    } else if after {
        validate_source_envelope_at(guard, journal, &source.post_exchange_name)?;
        validate_typed_target_at(
            guard,
            journal,
            target,
            &target.post_exchange_name,
            "canonical migration target",
        )?;
    } else {
        return Err(StoreError::Integrity(
            "Linux migration exchange entries have unknown identities".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn validate_target_published_envelope(
    guard: &NamespaceGuard,
    journal: &MigrationJournal,
) -> Result<(), StoreError> {
    let source = &journal.source.binding;
    let target = &journal.target()?.binding;
    validate_exchange_names(source, target)?;
    let canonical = guard.state.state_dir.join("lifecycle.store");
    let source_private = guard.state.state_dir.join(&source.post_exchange_name);
    let target_private = guard.state.state_dir.join(&target.pre_exchange_name);
    let canonical_exists = entry_exists(&canonical)?;
    let source_exists = entry_exists(&source_private)?;
    let target_exists = entry_exists(&target_private)?;

    match (canonical_exists, source_exists, target_exists) {
        (true, false, true) => {
            validate_source_envelope_at(guard, journal, "lifecycle.store")?;
            validate_typed_target_at(
                guard,
                journal,
                target,
                &target.pre_exchange_name,
                "published migration target",
            )?;
        }
        (false, true, true) => {
            validate_source_envelope_at(guard, journal, &source.post_exchange_name)?;
            validate_typed_target_at(
                guard,
                journal,
                target,
                &target.pre_exchange_name,
                "published migration target",
            )?;
        }
        (true, true, false) => {
            validate_source_envelope_at(guard, journal, &source.post_exchange_name)?;
            validate_typed_target_at(
                guard,
                journal,
                target,
                &target.post_exchange_name,
                "canonical migration target",
            )?;
        }
        _ => {
            return Err(StoreError::Integrity(
                "Windows migration exchange entries have unknown placement".to_owned(),
            ));
        }
    }
    Ok(())
}

fn validate_typed_target_at(
    guard: &NamespaceGuard,
    journal: &MigrationJournal,
    binding: &MigrationArtifactBinding,
    name: &str,
    label: &str,
) -> Result<(), StoreError> {
    let entry = validate_binding_at(guard, binding, name, true, label)?;
    let anchor = anchor_for(journal);
    let snapshot = read_current_entry(
        guard,
        &entry,
        journal.root_authorization.target_generation,
        Some(&anchor),
    )?;
    let logical = snapshot.anchored_logical_sha256(&anchor)?;
    if logical != journal.root_authorization.target_user_logical_sha256
        || logical != binding.logical_sha256
    {
        return Err(StoreError::Integrity(format!(
            "{label} logical identity disagrees with its authorized target"
        )));
    }
    snapshot.validate_external_references(guard)?;
    entry.validate_path(
        &guard.state.state_dir.join(name),
        EntryKind::RegularFile,
        EntryAccess::ReadWrite,
        true,
        label,
    )?;
    if file_sha256(&entry)? != binding.byte_sha256 {
        return Err(StoreError::Integrity(format!(
            "{label} payload changed during typed validation"
        )));
    }
    Ok(())
}

#[cfg(not(any(all(target_os = "linux", target_arch = "x86_64"), windows)))]
fn validate_target_published_envelope(
    _guard: &NamespaceGuard,
    _journal: &MigrationJournal,
) -> Result<(), StoreError> {
    Err(StoreError::Integrity(
        "lifecycle-store migration exchange supports Windows and Linux x64".to_owned(),
    ))
}

fn open_bound_source(
    guard: &NamespaceGuard,
    journal: &MigrationJournal,
) -> Result<LegacyStore, StoreError> {
    let binding = &journal.source.binding;
    let name = match journal.source.state {
        MigrationBindingState::Observed => &binding.pre_exchange_name,
        MigrationBindingState::Exchanged | MigrationBindingState::Retained => {
            &binding.post_exchange_name
        }
        _ => {
            return Err(StoreError::Integrity(
                "migration source has an impossible folded state".to_owned(),
            ));
        }
    };
    let entry = validate_binding_at(guard, binding, name, true, "bound migration source")?;
    let source = open_legacy_entry(guard, entry, name, "bound migration source")?;
    validate_source(&source, journal, binding)?;
    Ok(source)
}

fn open_exchange_source(
    guard: &NamespaceGuard,
    journal: &MigrationJournal,
) -> Result<LegacyStore, StoreError> {
    let binding = &journal.source.binding;
    let mut previous_name = None;
    for name in [&binding.pre_exchange_name, &binding.post_exchange_name] {
        if previous_name == Some(name.as_str()) {
            continue;
        }
        previous_name = Some(name.as_str());
        let path = guard.state.state_dir.join(name);
        if !entry_exists(&path)? {
            continue;
        }
        let candidate = HeldEntry::open(
            &path,
            EntryKind::RegularFile,
            EntryAccess::ReadOnly,
            true,
            "migration exchange source candidate",
        )?;
        if candidate.identity() != &binding.physical_identity {
            continue;
        }
        drop(candidate);
        let entry = validate_binding_at(
            guard,
            binding,
            name,
            true,
            "bound migration exchange source",
        )?;
        let source = open_legacy_entry(guard, entry, name, "bound migration exchange source")?;
        validate_source(&source, journal, binding)?;
        return Ok(source);
    }
    Err(StoreError::Integrity(
        "bound migration source is missing".to_owned(),
    ))
}

fn validate_source(
    source: &LegacyStore,
    journal: &MigrationJournal,
    binding: &MigrationArtifactBinding,
) -> Result<(), StoreError> {
    if source.generation != journal.root_authorization.source_generation
        || source.entry.identity() != &binding.physical_identity
        || source.snapshot.logical_sha256()?
            != journal.root_authorization.source_user_logical_sha256
    {
        return Err(StoreError::Integrity(
            "bound migration source changed".to_owned(),
        ));
    }
    validate_authorization_history(source, &journal.root_authorization)
}

fn validate_authorization_history(
    source: &LegacyStore,
    expected: &MigrationRootAuthorization,
) -> Result<(), StoreError> {
    let authorizations = read_root_authorizations(&source.database)?;
    if authorizations.last_key_value().map(|(_, row)| row) != Some(expected) {
        return Err(StoreError::Integrity(
            "prior lifecycle.store migration authorization changed".to_owned(),
        ));
    }
    Ok(())
}

fn anchor_for(journal: &MigrationJournal) -> MigrationProvenanceAnchor {
    let authorization = &journal.root_authorization;
    MigrationProvenanceAnchor {
        authorization_sequence: authorization.authorization_sequence,
        root_name: authorization.root_name.clone(),
        root_physical_identity: authorization.root_physical_identity.clone(),
        root_core_sha256: authorization.root_core_sha256.clone(),
        source_generation: authorization.source_generation,
        source_schema: authorization.source_schema.clone(),
        source_physical_identity: journal.source.binding.physical_identity.clone(),
        source_user_logical_sha256: authorization.source_user_logical_sha256.clone(),
    }
}

fn validate_exchange_names(
    source: &MigrationArtifactBinding,
    target: &MigrationArtifactBinding,
) -> Result<(), StoreError> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    if source.post_exchange_name != target.pre_exchange_name {
        return Err(StoreError::Integrity(
            "Linux migration exchange bindings disagree on the private slot".to_owned(),
        ));
    }
    #[cfg(windows)]
    if source.post_exchange_name == target.pre_exchange_name {
        return Err(StoreError::Integrity(
            "Windows migration exchange requires distinct source and target placements".to_owned(),
        ));
    }
    Ok(())
}

fn open_binding_for_move(
    guard: &NamespaceGuard,
    binding: &MigrationArtifactBinding,
    name: &str,
    label: &str,
) -> Result<HeldEntry, StoreError> {
    let entry = HeldEntry::open(
        &guard.state.state_dir.join(name),
        EntryKind::RegularFile,
        EntryAccess::Move,
        true,
        label,
    )?;
    revalidate_binding_for_move(guard, &entry, binding, name, label)?;
    Ok(entry)
}

fn revalidate_binding_for_move(
    guard: &NamespaceGuard,
    entry: &HeldEntry,
    binding: &MigrationArtifactBinding,
    name: &str,
    label: &str,
) -> Result<(), StoreError> {
    entry.validate_path(
        &guard.state.state_dir.join(name),
        EntryKind::RegularFile,
        EntryAccess::Move,
        true,
        label,
    )?;
    require_state_volume(entry, &guard.state_directory, label)?;
    if entry.identity() != &binding.physical_identity || file_sha256(entry)? != binding.byte_sha256
    {
        return Err(StoreError::Integrity(format!(
            "{label} changed before handle-bound movement"
        )));
    }
    Ok(())
}

fn migration_artifact_name(name: &OsStr) -> Result<Option<&str>, StoreError> {
    let name = name.to_str().ok_or_else(|| {
        StoreError::Integrity(
            "state namespace contains a non-UTF-8 entry during migration artifact validation"
                .to_owned(),
        )
    })?;
    Ok(name
        .starts_with("lifecycle.store.migration-")
        .then_some(name))
}

fn reject_orphan_migration_artifacts(guard: &NamespaceGuard) -> Result<(), StoreError> {
    for item in fs::read_dir(&guard.state.state_dir).map_err(io_error)? {
        let item = item.map_err(io_error)?;
        if migration_artifact_name(&item.file_name())?.is_some() {
            return Err(StoreError::Integrity(
                "private lifecycle migration artifact exists without a journal".to_owned(),
            ));
        }
    }
    Ok(())
}

fn reject_unbound_migration_artifacts(
    guard: &NamespaceGuard,
    journal: &MigrationJournal,
) -> Result<(), StoreError> {
    let mut allowed = std::collections::BTreeSet::new();
    match journal.phase {
        MigrationPhase::ObservedSource => {}
        MigrationPhase::TargetPending => {
            allowed.insert(journal.target()?.binding.pre_exchange_name.as_str());
        }
        MigrationPhase::TargetPublished => {
            allowed.insert(journal.source.binding.post_exchange_name.as_str());
            allowed.insert(journal.target()?.binding.pre_exchange_name.as_str());
        }
        MigrationPhase::Exchanged | MigrationPhase::Terminal => {
            allowed.insert(journal.source.binding.post_exchange_name.as_str());
        }
    }
    for item in fs::read_dir(&guard.state.state_dir).map_err(io_error)? {
        let item = item.map_err(io_error)?;
        let native_name = item.file_name();
        let Some(name) = migration_artifact_name(&native_name)? else {
            continue;
        };
        if !allowed.contains(name) {
            return Err(StoreError::Integrity(format!(
                "unbound lifecycle migration artifact is present: {name}"
            )));
        }
    }
    Ok(())
}
