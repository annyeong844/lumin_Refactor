use redb::{
    Database, ReadTransaction, ReadableDatabase, ReadableTable, TableDefinition, TableError,
    WriteTransaction,
};
use serde::{Deserialize, Serialize};

use crate::{StoreError, StoreGeneration, backend_error, io_error, serialization_error};

use super::platform::{EntryAccess, EntryKind, HeldEntry};
use super::{NamespaceBinding, NamespaceGuard, entry_exists, require_state_volume};

const STORE_HEADER: TableDefinition<&str, &[u8]> = TableDefinition::new("store-header");
const STORE_HEADER_KEY: &str = "namespace";
const VALIDATION_RECEIPT_SET_FRAME: &[u8] = b"lumin-gate-validation-receipt-set.v1";
pub(super) const STORE_HEADER_SCHEMA: &str = "lumin-lifecycle-store-header.v8";
pub(super) const STORE_HEADER_TABLE_NAME: &str = "store-header";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LifecycleStoreHeader {
    schema_version: String,
    binding: NamespaceBinding,
    generation: StoreGeneration,
    validation_receipt_set_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LifecycleStoreHeaderEnvelope {
    schema_version: String,
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
            return initialize_store(&entry, &guard.state.binding, StoreGeneration::INITIAL);
        }
        let database = Database::builder()
            .create_file(entry.file().try_clone().map_err(io_error)?)
            .map_err(backend_error)?;
        verify_store_header(&database, &guard.state.binding)?;
        return Ok(());
    }
    let entry = HeldEntry::create_new(&path, "lifecycle.store")?;
    require_state_volume(&entry, &guard.state_directory, "lifecycle.store")?;
    initialize_store(&entry, &guard.state.binding, StoreGeneration::INITIAL)
}

pub(super) fn initialize_store(
    entry: &HeldEntry,
    binding: &NamespaceBinding,
    generation: StoreGeneration,
) -> Result<(), StoreError> {
    let database = Database::builder()
        .create_file(entry.file().try_clone().map_err(io_error)?)
        .map_err(backend_error)?;
    let write = database.begin_write().map_err(backend_error)?;
    {
        let mut table = write.open_table(STORE_HEADER).map_err(backend_error)?;
        let bytes =
            store_header_bytes(binding, generation, &validation_receipt_set_id(Vec::new()))?;
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
    verify_store_header_bytes(value.value(), binding, None).map(|header| header.generation)
}

pub(super) fn verify_store_header_write(
    write: &WriteTransaction,
    binding: &NamespaceBinding,
    generation: StoreGeneration,
) -> Result<(), StoreError> {
    let bytes = read_store_header_bytes_from_write(write)?;
    let header = verify_store_header_bytes(&bytes, binding, Some(generation))?;
    let observed = validation_receipt_set_id_from_write(write)?;
    if observed != header.validation_receipt_set_id {
        return Err(StoreError::Integrity(
            "store-owned validation receipt set disagrees with lifecycle.store header".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn verify_validation_receipt_set_read(
    read: &ReadTransaction,
    binding: &NamespaceBinding,
    generation: StoreGeneration,
) -> Result<(), StoreError> {
    let bytes = read_store_header_bytes_from_read(read)?;
    let header = verify_store_header_bytes(&bytes, binding, Some(generation))?;
    let observed = validation_receipt_set_id_from_read(read)?;
    if observed != header.validation_receipt_set_id {
        return Err(StoreError::Integrity(
            "store-owned validation receipt set disagrees with lifecycle.store header".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn refresh_validation_receipt_set_id(
    write: &WriteTransaction,
) -> Result<(), StoreError> {
    let bytes = read_store_header_bytes_from_write(write)?;
    let mut header = decode_store_header_bytes(&bytes)?;
    header.validation_receipt_set_id = validation_receipt_set_id_from_write(write)?;
    let bytes = store_header_bytes(
        &header.binding,
        header.generation,
        &header.validation_receipt_set_id,
    )?;
    let mut table = write.open_table(STORE_HEADER).map_err(backend_error)?;
    table
        .insert(STORE_HEADER_KEY, bytes.as_slice())
        .map_err(backend_error)?;
    Ok(())
}

fn verify_store_header_bytes(
    bytes: &[u8],
    binding: &NamespaceBinding,
    expected_generation: Option<StoreGeneration>,
) -> Result<LifecycleStoreHeader, StoreError> {
    let header = decode_store_header_bytes(bytes)?;
    if header.binding.cache_evictions.is_none() {
        return Err(StoreError::IncompatibleStateSchema(
            "lifecycle.store omitted the cache-eviction parent binding".to_owned(),
        ));
    }
    if header.binding != *binding {
        return Err(StoreError::Integrity(
            "lifecycle.store namespace header disagrees with repository marker".to_owned(),
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
    Ok(header)
}

fn store_header_bytes(
    binding: &NamespaceBinding,
    generation: StoreGeneration,
    validation_receipt_set_id: &str,
) -> Result<Vec<u8>, StoreError> {
    serde_json::to_vec(&LifecycleStoreHeader {
        schema_version: STORE_HEADER_SCHEMA.to_owned(),
        binding: binding.clone(),
        generation,
        validation_receipt_set_id: validation_receipt_set_id.to_owned(),
    })
    .map_err(serialization_error)
}

fn decode_store_header_bytes(bytes: &[u8]) -> Result<LifecycleStoreHeader, StoreError> {
    let envelope =
        serde_json::from_slice::<LifecycleStoreHeaderEnvelope>(bytes).map_err(|error| {
            StoreError::Integrity(format!("lifecycle.store header is malformed: {error}"))
        })?;
    if envelope.schema_version != STORE_HEADER_SCHEMA {
        return Err(StoreError::IncompatibleStateSchema(format!(
            "lifecycle.store schema {} is unsupported; expected {STORE_HEADER_SCHEMA}",
            envelope.schema_version
        )));
    }
    let header = serde_json::from_slice::<LifecycleStoreHeader>(bytes).map_err(|error| {
        StoreError::Integrity(format!("lifecycle.store header is malformed: {error}"))
    })?;
    if !is_canonical_sha256(&header.validation_receipt_set_id) {
        return Err(StoreError::Integrity(
            "lifecycle.store validation receipt set identity is malformed".to_owned(),
        ));
    }
    if bytes
        != store_header_bytes(
            &header.binding,
            header.generation,
            &header.validation_receipt_set_id,
        )?
    {
        return Err(StoreError::Integrity(
            "lifecycle.store header bytes are not canonical".to_owned(),
        ));
    }
    Ok(header)
}

fn read_store_header_bytes_from_read(read: &ReadTransaction) -> Result<Vec<u8>, StoreError> {
    let table = read.open_table(STORE_HEADER).map_err(backend_error)?;
    table
        .get(STORE_HEADER_KEY)
        .map_err(backend_error)?
        .map(|value| value.value().to_vec())
        .ok_or_else(|| {
            StoreError::Integrity("lifecycle.store namespace header is missing".to_owned())
        })
}

fn read_store_header_bytes_from_write(write: &WriteTransaction) -> Result<Vec<u8>, StoreError> {
    let table = write.open_table(STORE_HEADER).map_err(backend_error)?;
    table
        .get(STORE_HEADER_KEY)
        .map_err(backend_error)?
        .map(|value| value.value().to_vec())
        .ok_or_else(|| {
            StoreError::Integrity("lifecycle.store namespace header is missing".to_owned())
        })
}

fn validation_receipt_set_id_from_read(read: &ReadTransaction) -> Result<String, StoreError> {
    let table = match read.open_table(crate::gate::VALIDATION_RECEIPTS) {
        Ok(table) => table,
        Err(TableError::TableDoesNotExist(_)) => return Ok(validation_receipt_set_id(Vec::new())),
        Err(error) => return Err(backend_error(error)),
    };
    let mut rows = Vec::new();
    for row in table.iter().map_err(backend_error)? {
        let (key, value) = row.map_err(backend_error)?;
        rows.push((key.value().as_bytes().to_vec(), value.value().to_vec()));
    }
    Ok(validation_receipt_set_id(rows))
}

fn validation_receipt_set_id_from_write(write: &WriteTransaction) -> Result<String, StoreError> {
    let table = write
        .open_table(crate::gate::VALIDATION_RECEIPTS)
        .map_err(backend_error)?;
    let mut rows = Vec::new();
    for row in table.iter().map_err(backend_error)? {
        let (key, value) = row.map_err(backend_error)?;
        rows.push((key.value().as_bytes().to_vec(), value.value().to_vec()));
    }
    Ok(validation_receipt_set_id(rows))
}

fn validation_receipt_set_id(rows: Vec<(Vec<u8>, Vec<u8>)>) -> String {
    let mut framed = Vec::new();
    append_length_prefixed(&mut framed, VALIDATION_RECEIPT_SET_FRAME);
    framed.extend_from_slice(&(rows.len() as u64).to_be_bytes());
    for (key, value) in rows {
        append_length_prefixed(&mut framed, &key);
        append_length_prefixed(&mut framed, &value);
    }
    crate::digest_hex(&framed)
}

fn append_length_prefixed(output: &mut Vec<u8>, field: &[u8]) {
    output.extend_from_slice(&(field.len() as u64).to_be_bytes());
    output.extend_from_slice(field);
}

fn is_canonical_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prior_store_schema_is_reported_as_explicitly_incompatible()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let admission = lumin_inventory::repository_admission(root.path())?;
        drop(crate::RepositoryStore::open(
            &admission.canonical_root,
            &admission.binding,
        )?);
        let marker: super::super::RepositoryMarker = super::super::read_canonical_path(
            &root.path().join(".lumin/repository.json"),
            "repository marker",
        )?;
        let prior = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": "lumin-lifecycle-store-header.v7",
            "binding": marker.binding,
            "generation": StoreGeneration::INITIAL,
        }))?;

        assert!(matches!(
            verify_store_header_bytes(&prior, &marker.binding, None),
            Err(StoreError::IncompatibleStateSchema(_))
        ));
        Ok(())
    }
}
