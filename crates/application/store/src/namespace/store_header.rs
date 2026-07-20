use redb::{
    Database, ReadableDatabase, ReadableTable, TableDefinition, TableError, WriteTransaction,
};
use serde::{Deserialize, Serialize};

use crate::{StoreError, StoreGeneration, backend_error, io_error, serialization_error};

use super::platform::{EntryAccess, EntryKind, HeldEntry};
use super::{NamespaceBinding, NamespaceGuard, entry_exists, require_state_volume};

const STORE_HEADER: TableDefinition<&str, &[u8]> = TableDefinition::new("store-header");
const STORE_HEADER_KEY: &str = "namespace";
const STORE_HEADER_SCHEMA: &str = "lumin-lifecycle-store-header.v3";

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LifecycleStoreHeader {
    schema_version: String,
    binding: NamespaceBinding,
    generation: StoreGeneration,
}

pub(super) fn create_or_verify_store(guard: &NamespaceGuard) -> Result<(), StoreError> {
    let path = guard.state.state_dir.join("lifecycle.store");
    if entry_exists(&path)? {
        let entry = HeldEntry::open(
            &path,
            EntryKind::RegularFile,
            EntryAccess::ReadWrite,
            true,
            "lifecycle.store",
        )?;
        require_state_volume(&entry, &guard.state_directory, "lifecycle.store")?;
        if entry.file().metadata().map_err(io_error)?.len() == 0 {
            return initialize_store(entry, &guard.state.binding, StoreGeneration::INITIAL);
        }
        let database = Database::builder()
            .create_file(entry.file().try_clone().map_err(io_error)?)
            .map_err(backend_error)?;
        verify_store_header(&database, &guard.state.binding)?;
        return Ok(());
    }
    let entry = HeldEntry::create_new(&path, "lifecycle.store")?;
    require_state_volume(&entry, &guard.state_directory, "lifecycle.store")?;
    initialize_store(entry, &guard.state.binding, StoreGeneration::INITIAL)
}

pub(super) fn initialize_store(
    entry: HeldEntry,
    binding: &NamespaceBinding,
    generation: StoreGeneration,
) -> Result<(), StoreError> {
    let database = Database::builder()
        .create_file(entry.file().try_clone().map_err(io_error)?)
        .map_err(backend_error)?;
    let write = database.begin_write().map_err(backend_error)?;
    {
        let mut table = write.open_table(STORE_HEADER).map_err(backend_error)?;
        let bytes = store_header_bytes(binding, generation)?;
        table
            .insert(STORE_HEADER_KEY, bytes.as_slice())
            .map_err(backend_error)?;
    }
    write.commit().map_err(backend_error)?;
    drop(database);
    entry.sync()
}

pub(super) fn verify_store_header(
    database: &Database,
    binding: &NamespaceBinding,
) -> Result<StoreGeneration, StoreError> {
    let read = database.begin_read().map_err(backend_error)?;
    let table = match read.open_table(STORE_HEADER) {
        Ok(table) => table,
        Err(TableError::TableDoesNotExist(_)) => {
            return Err(StoreError::Integrity(
                "lifecycle.store namespace header is missing".to_owned(),
            ));
        }
        Err(error) => return Err(backend_error(error)),
    };
    let value = table
        .get(STORE_HEADER_KEY)
        .map_err(backend_error)?
        .ok_or_else(|| {
            StoreError::Integrity("lifecycle.store namespace header is missing".to_owned())
        })?;
    verify_store_header_bytes(value.value(), binding, None)
}

pub(super) fn verify_store_header_write(
    write: &WriteTransaction,
    binding: &NamespaceBinding,
    generation: StoreGeneration,
) -> Result<(), StoreError> {
    let table = write.open_table(STORE_HEADER).map_err(backend_error)?;
    let value = table
        .get(STORE_HEADER_KEY)
        .map_err(backend_error)?
        .ok_or_else(|| {
            StoreError::Integrity("lifecycle.store namespace header is missing".to_owned())
        })?;
    verify_store_header_bytes(value.value(), binding, Some(generation)).map(|_| ())
}

fn verify_store_header_bytes(
    bytes: &[u8],
    binding: &NamespaceBinding,
    expected_generation: Option<StoreGeneration>,
) -> Result<StoreGeneration, StoreError> {
    let header = serde_json::from_slice::<LifecycleStoreHeader>(bytes).map_err(|error| {
        StoreError::Integrity(format!("lifecycle.store header is malformed: {error}"))
    })?;
    if header.schema_version != STORE_HEADER_SCHEMA {
        return Err(StoreError::Integrity(format!(
            "lifecycle.store schema {} is unsupported; expected {STORE_HEADER_SCHEMA}",
            header.schema_version
        )));
    }
    if header.binding != *binding {
        return Err(StoreError::Integrity(
            "lifecycle.store namespace header disagrees with repository marker".to_owned(),
        ));
    }
    if bytes != store_header_bytes(binding, header.generation)? {
        return Err(StoreError::Integrity(
            "lifecycle.store header bytes are not canonical".to_owned(),
        ));
    }
    if let Some(expected) = expected_generation
        && header.generation != expected
    {
        return Err(StoreError::StoreGenerationChanged {
            expected,
            observed: header.generation,
        });
    }
    Ok(header.generation)
}

fn store_header_bytes(
    binding: &NamespaceBinding,
    generation: StoreGeneration,
) -> Result<Vec<u8>, StoreError> {
    serde_json::to_vec(&LifecycleStoreHeader {
        schema_version: STORE_HEADER_SCHEMA.to_owned(),
        binding: binding.clone(),
        generation,
    })
    .map_err(serialization_error)
}
