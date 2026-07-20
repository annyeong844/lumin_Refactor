use redb::{
    Database, ReadableDatabase, ReadableTable, TableDefinition, TableError, WriteTransaction,
};
use serde::Serialize;

use crate::{StoreError, backend_error, io_error, serialization_error};

use super::platform::{EntryAccess, EntryKind, HeldEntry};
use super::{NamespaceBinding, NamespaceGuard, entry_exists, require_state_volume};

const STORE_HEADER: TableDefinition<&str, &[u8]> = TableDefinition::new("store-header");
const STORE_HEADER_KEY: &str = "namespace";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LifecycleStoreHeader {
    schema_version: String,
    binding: NamespaceBinding,
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
            return initialize_store(entry, &guard.state.binding);
        }
        let database = Database::builder()
            .create_file(entry.file().try_clone().map_err(io_error)?)
            .map_err(backend_error)?;
        return verify_store_header(&database, &guard.state.binding);
    }
    let entry = HeldEntry::create_new(&path, "lifecycle.store")?;
    require_state_volume(&entry, &guard.state_directory, "lifecycle.store")?;
    initialize_store(entry, &guard.state.binding)
}

fn initialize_store(entry: HeldEntry, binding: &NamespaceBinding) -> Result<(), StoreError> {
    let database = Database::builder()
        .create_file(entry.file().try_clone().map_err(io_error)?)
        .map_err(backend_error)?;
    let write = database.begin_write().map_err(backend_error)?;
    {
        let mut table = write.open_table(STORE_HEADER).map_err(backend_error)?;
        let bytes = store_header_bytes(binding)?;
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
) -> Result<(), StoreError> {
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
    verify_store_header_bytes(value.value(), binding)
}

pub(super) fn verify_store_header_write(
    write: &WriteTransaction,
    binding: &NamespaceBinding,
) -> Result<(), StoreError> {
    let table = write.open_table(STORE_HEADER).map_err(backend_error)?;
    let value = table
        .get(STORE_HEADER_KEY)
        .map_err(backend_error)?
        .ok_or_else(|| {
            StoreError::Integrity("lifecycle.store namespace header is missing".to_owned())
        })?;
    verify_store_header_bytes(value.value(), binding)
}

fn verify_store_header_bytes(bytes: &[u8], binding: &NamespaceBinding) -> Result<(), StoreError> {
    if bytes != store_header_bytes(binding)? {
        return Err(StoreError::Integrity(
            "lifecycle.store namespace header disagrees with repository marker".to_owned(),
        ));
    }
    Ok(())
}

fn store_header_bytes(binding: &NamespaceBinding) -> Result<Vec<u8>, StoreError> {
    serde_json::to_vec(&LifecycleStoreHeader {
        schema_version: "lumin-lifecycle-store-header.v2".to_owned(),
        binding: binding.clone(),
    })
    .map_err(serialization_error)
}
