mod artifacts;
mod snapshot;

use std::path::Path;

use crate::{StoreError, StoreGeneration};

pub use self::artifacts::MigrationIntent;
use self::artifacts::{
    MigrationPaths, cleanup_unpublished_intent, publish_intent, read_intent, remove_intent,
    remove_private_file, validate_intent,
};
use self::snapshot::{LogicalStoreSnapshot, create_private, read_canonical, read_private};
use super::platform::replace_file_atomic;
use super::store_header::{STORE_HEADER_SCHEMA, create_or_verify_store};
use super::{NamespaceGuard, NamespaceState, entry_exists};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MigrationCrashPoint {
    PendingIntentCreated,
    IntentPrepared,
    IntentRenamed,
    IntentPublished,
    CopiesValidated,
    CanonicalReplaced,
    ParentFlushed,
    IntentRemoved,
}

impl NamespaceState {
    pub(crate) fn migrate_lifecycle_store(&self) -> Result<StoreGeneration, StoreError> {
        self.with_migration_lock(|guard| migrate_with_hook(guard, &mut |_| Ok(())))
    }
}

pub(super) fn require_idle(guard: &NamespaceGuard) -> Result<(), StoreError> {
    if let Some(intent) = read_intent(guard)? {
        return Err(StoreError::LifecycleMigrationPending {
            from_generation: intent.from_generation,
            to_generation: intent.to_generation,
        });
    }
    let paths = MigrationPaths::new(guard);
    if entry_exists(&paths.source)? || entry_exists(&paths.target)? {
        return Err(StoreError::LifecycleMigrationCleanupPending);
    }
    Ok(())
}

pub(super) fn recover_on_open(guard: &NamespaceGuard) -> Result<(), StoreError> {
    cleanup_unpublished_intent(guard)?;
    if let Some(intent) = read_intent(guard)? {
        recover_intent(guard, &intent, &mut |_| Ok(()))?;
        return Ok(());
    }
    cleanup_without_intent(guard)?;
    create_or_verify_store(guard)
}

pub(super) fn migrate_with_hook(
    guard: &NamespaceGuard,
    hook: &mut impl FnMut(MigrationCrashPoint) -> Result<(), StoreError>,
) -> Result<StoreGeneration, StoreError> {
    cleanup_unpublished_intent(guard)?;
    if let Some(intent) = read_intent(guard)? {
        recover_intent(guard, &intent, hook)?;
        return Ok(intent.to_generation);
    }
    if let Some(recovered_generation) = cleanup_without_intent(guard)? {
        return Ok(recovered_generation);
    }
    create_or_verify_store(guard)?;

    let database = guard.open_database()?;
    let from_generation = database.generation();
    let to_generation = from_generation
        .checked_next()
        .ok_or_else(|| StoreError::Integrity("lifecycle store generation overflow".to_owned()))?;
    drop(database);

    let intent = MigrationIntent {
        from_generation,
        to_generation,
        source_schema: STORE_HEADER_SCHEMA.to_owned(),
        target_schema: STORE_HEADER_SCHEMA.to_owned(),
    };
    publish_intent(guard, &intent, hook)?;
    recover_intent(guard, &intent, hook)?;
    Ok(to_generation)
}

fn recover_intent(
    guard: &NamespaceGuard,
    intent: &MigrationIntent,
    hook: &mut impl FnMut(MigrationCrashPoint) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    validate_intent(intent)?;
    guard.validate_bound_entries()?;
    let paths = MigrationPaths::new(guard);
    if !entry_exists(&paths.canonical)? {
        return Err(StoreError::Integrity(
            "canonical lifecycle.store is missing during migration recovery".to_owned(),
        ));
    }
    let canonical = guard.open_database()?;
    let observed = canonical.generation();
    drop(canonical);

    if observed == intent.from_generation {
        recover_from_source_generation(guard, intent, &paths, hook)
    } else if observed == intent.to_generation {
        recover_from_target_generation(guard, intent, &paths, hook)
    } else {
        Err(StoreError::Integrity(format!(
            "canonical lifecycle.store generation {observed} disagrees with migration intent {} -> {}",
            intent.from_generation, intent.to_generation
        )))
    }
}

fn recover_from_source_generation(
    guard: &NamespaceGuard,
    intent: &MigrationIntent,
    paths: &MigrationPaths,
    hook: &mut impl FnMut(MigrationCrashPoint) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    let source = read_canonical(guard, intent.from_generation)?;
    source.validate_external_references(guard)?;
    ensure_private_snapshot(guard, &paths.source, intent.from_generation, &source)?;
    ensure_private_snapshot(guard, &paths.target, intent.to_generation, &source)?;
    hook(MigrationCrashPoint::CopiesValidated)?;

    let retained_source = read_private(guard, &paths.source, intent.from_generation)?;
    let replacement = read_private(guard, &paths.target, intent.to_generation)?;
    if retained_source != source || replacement != source {
        return Err(StoreError::Integrity(
            "validated lifecycle migration copies disagree before replacement".to_owned(),
        ));
    }
    retained_source.validate_external_references(guard)?;
    guard.validate_bound_entries()?;
    replace_file_atomic(&paths.canonical, &paths.target)?;
    hook(MigrationCrashPoint::CanonicalReplaced)?;
    guard.state_directory.sync_directory()?;
    hook(MigrationCrashPoint::ParentFlushed)?;

    let current = read_canonical(guard, intent.to_generation)?;
    if current != retained_source {
        return Err(StoreError::Integrity(
            "replaced lifecycle.store changed the canonical logical snapshot".to_owned(),
        ));
    }
    current.validate_external_references(guard)?;
    finish_migration(guard, intent, paths, hook)
}

fn recover_from_target_generation(
    guard: &NamespaceGuard,
    intent: &MigrationIntent,
    paths: &MigrationPaths,
    hook: &mut impl FnMut(MigrationCrashPoint) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    if entry_exists(&paths.target)? {
        return Err(StoreError::Integrity(
            "migration target still exists after canonical replacement".to_owned(),
        ));
    }
    if !entry_exists(&paths.source)? {
        return Err(StoreError::Integrity(
            "migration source snapshot is missing after canonical replacement".to_owned(),
        ));
    }
    let source = read_private(guard, &paths.source, intent.from_generation)?;
    let current = read_canonical(guard, intent.to_generation)?;
    if current != source {
        return Err(StoreError::Integrity(
            "canonical target generation disagrees with the retained source snapshot".to_owned(),
        ));
    }
    current.validate_external_references(guard)?;
    guard.state_directory.sync_directory()?;
    hook(MigrationCrashPoint::ParentFlushed)?;
    finish_migration(guard, intent, paths, hook)
}

fn finish_migration(
    guard: &NamespaceGuard,
    intent: &MigrationIntent,
    paths: &MigrationPaths,
    hook: &mut impl FnMut(MigrationCrashPoint) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    remove_intent(guard, intent)?;
    hook(MigrationCrashPoint::IntentRemoved)?;
    remove_private_file(guard, &paths.source)?;
    if entry_exists(&paths.target)? {
        return Err(StoreError::Integrity(
            "migration target reappeared during cleanup".to_owned(),
        ));
    }
    guard.state_directory.sync_directory()
}

fn ensure_private_snapshot(
    guard: &NamespaceGuard,
    path: &Path,
    generation: StoreGeneration,
    expected: &LogicalStoreSnapshot,
) -> Result<(), StoreError> {
    if entry_exists(path)? {
        match read_private(guard, path, generation) {
            Ok(snapshot) if &snapshot == expected => return Ok(()),
            Ok(_) | Err(_) => remove_private_file(guard, path)?,
        }
    }
    create_private(guard, path, generation, expected)
}

fn cleanup_without_intent(guard: &NamespaceGuard) -> Result<Option<StoreGeneration>, StoreError> {
    let paths = MigrationPaths::new(guard);
    if entry_exists(&paths.target)? {
        return Err(StoreError::Integrity(
            "migration target exists without a durable migration intent".to_owned(),
        ));
    }
    if !entry_exists(&paths.source)? {
        return Ok(None);
    }
    let database = guard.open_database()?;
    let current_generation = database.generation();
    drop(database);
    let source_generation = current_generation.checked_previous().ok_or_else(|| {
        StoreError::Integrity(
            "migration source exists beside the initial lifecycle generation".to_owned(),
        )
    })?;
    let source = read_private(guard, &paths.source, source_generation)?;
    let current = read_canonical(guard, current_generation)?;
    if current != source {
        return Err(StoreError::Integrity(
            "completed migration cleanup payload disagrees with lifecycle.store".to_owned(),
        ));
    }
    remove_private_file(guard, &paths.source)?;
    guard.state_directory.sync_directory()?;
    Ok(Some(current_generation))
}
