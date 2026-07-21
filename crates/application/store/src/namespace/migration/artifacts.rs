use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{StoreError, StoreGeneration, io_error, serialization_error};

use super::super::platform::{EntryAccess, EntryKind, HeldEntry};
use super::super::store_header::STORE_HEADER_SCHEMA;
use super::super::{NamespaceGuard, entry_exists, require_state_volume};

const MIGRATION_INTENT_NAME: &str = "lifecycle-migration.json";
const MIGRATION_SOURCE_NAME: &str = "lifecycle.store.migration-source";
const MIGRATION_TARGET_NAME: &str = "lifecycle.store.migration-target";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MigrationIntent {
    pub from_generation: StoreGeneration,
    pub to_generation: StoreGeneration,
    pub source_schema: String,
    pub target_schema: String,
}

pub(super) struct MigrationPaths {
    pub(super) canonical: PathBuf,
    pub(super) intent: PathBuf,
    pub(super) source: PathBuf,
    pub(super) target: PathBuf,
}

impl MigrationPaths {
    pub(super) fn new(guard: &NamespaceGuard) -> Self {
        Self {
            canonical: guard.state.state_dir.join("lifecycle.store"),
            intent: guard.state.state_dir.join(MIGRATION_INTENT_NAME),
            source: guard.state.state_dir.join(MIGRATION_SOURCE_NAME),
            target: guard.state.state_dir.join(MIGRATION_TARGET_NAME),
        }
    }
}

pub(super) fn publish_intent(
    guard: &NamespaceGuard,
    intent: &MigrationIntent,
) -> Result<(), StoreError> {
    validate_intent(intent)?;
    let paths = MigrationPaths::new(guard);
    if entry_exists(&paths.intent)? || entry_exists(&paths.source)? || entry_exists(&paths.target)?
    {
        return Err(StoreError::Integrity(
            "lifecycle migration paths were not empty before intent publication".to_owned(),
        ));
    }
    let entry = HeldEntry::create_new(&paths.intent, "lifecycle migration intent")?;
    require_state_volume(&entry, &guard.state_directory, "lifecycle migration intent")?;
    entry.replace_contents(&intent_bytes(intent)?)?;
    guard.state_directory.sync_directory()
}

pub(super) fn read_intent(guard: &NamespaceGuard) -> Result<Option<MigrationIntent>, StoreError> {
    let path = MigrationPaths::new(guard).intent;
    if !entry_exists(&path)? {
        return Ok(None);
    }
    let entry = HeldEntry::open(
        &path,
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        true,
        "lifecycle migration intent",
    )?;
    require_state_volume(&entry, &guard.state_directory, "lifecycle migration intent")?;
    let bytes = entry.read_all()?;
    let intent = serde_json::from_slice::<MigrationIntent>(&bytes).map_err(|error| {
        StoreError::Integrity(format!("lifecycle migration intent is malformed: {error}"))
    })?;
    validate_intent(&intent)?;
    if bytes != intent_bytes(&intent)? {
        return Err(StoreError::Integrity(
            "lifecycle migration intent bytes are not canonical".to_owned(),
        ));
    }
    Ok(Some(intent))
}

pub(super) fn remove_intent(
    guard: &NamespaceGuard,
    expected: &MigrationIntent,
) -> Result<(), StoreError> {
    let observed = read_intent(guard)?.ok_or_else(|| {
        StoreError::Integrity("lifecycle migration intent disappeared".to_owned())
    })?;
    if &observed != expected {
        return Err(StoreError::Integrity(
            "lifecycle migration intent changed during recovery".to_owned(),
        ));
    }
    fs::remove_file(MigrationPaths::new(guard).intent).map_err(io_error)?;
    guard.state_directory.sync_directory()
}

pub(super) fn remove_private_file(guard: &NamespaceGuard, path: &Path) -> Result<(), StoreError> {
    if !entry_exists(path)? {
        return Ok(());
    }
    let entry = HeldEntry::open(
        path,
        EntryKind::RegularFile,
        EntryAccess::ReadWrite,
        true,
        "private lifecycle migration store",
    )?;
    require_state_volume(
        &entry,
        &guard.state_directory,
        "private lifecycle migration store",
    )?;
    drop(entry);
    fs::remove_file(path).map_err(io_error)
}

pub(super) fn validate_intent(intent: &MigrationIntent) -> Result<(), StoreError> {
    if intent.from_generation.checked_next() != Some(intent.to_generation) {
        return Err(StoreError::Integrity(
            "lifecycle migration generations are not adjacent".to_owned(),
        ));
    }
    if intent.source_schema != STORE_HEADER_SCHEMA || intent.target_schema != STORE_HEADER_SCHEMA {
        return Err(StoreError::Integrity(format!(
            "lifecycle migration schema {} -> {} is unsupported",
            intent.source_schema, intent.target_schema
        )));
    }
    Ok(())
}

fn intent_bytes(intent: &MigrationIntent) -> Result<Vec<u8>, StoreError> {
    let mut bytes = serde_json::to_vec(intent).map_err(serialization_error)?;
    bytes.push(b'\n');
    Ok(bytes)
}
