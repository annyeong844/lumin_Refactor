use lumin_model::GateId;
use redb::{ReadableTable, TableDefinition, TableError, WriteTransaction};
use serde::{Serialize, de::DeserializeOwned};

use crate::{SEQUENCES, StoreError, backend_error, namespace::StoreDatabase, serialization_error};

pub(super) fn current_transition_sequence(write: &WriteTransaction) -> Result<u64, StoreError> {
    let table = write.open_table(SEQUENCES).map_err(backend_error)?;
    table
        .get("transition")
        .map_err(backend_error)
        .map(|value| value.map_or(0, |value| value.value()))
}

pub(super) fn next_transition_sequence(write: &WriteTransaction) -> Result<u64, StoreError> {
    let mut table = write.open_table(SEQUENCES).map_err(backend_error)?;
    let current = table
        .get("transition")
        .map_err(backend_error)?
        .map_or(0, |value| value.value());
    let next = current
        .checked_add(1)
        .ok_or_else(|| StoreError::Integrity("transition sequence overflow".to_owned()))?;
    table.insert("transition", next).map_err(backend_error)?;
    Ok(next)
}

pub(crate) fn transition_key(sequence: u64) -> String {
    format!("transition_{sequence:016x}")
}

pub(super) fn next_gate_id(write: &WriteTransaction) -> Result<GateId, StoreError> {
    let next = {
        let mut table = write.open_table(SEQUENCES).map_err(backend_error)?;
        let current = table
            .get("gate")
            .map_err(backend_error)?
            .map_or(0, |value| value.value());
        let next = current
            .checked_add(1)
            .ok_or_else(|| StoreError::Integrity("gate sequence overflow".to_owned()))?;
        table.insert("gate", next).map_err(backend_error)?;
        next
    };
    Ok(GateId::from_string(format!("gate_{next:016x}")))
}

pub(super) fn load_record<T: DeserializeOwned>(
    database: &StoreDatabase<'_>,
    definition: TableDefinition<'static, &str, &[u8]>,
    key: &str,
) -> Result<Option<T>, StoreError> {
    let read = database.begin_read()?;
    let table = match read.open_table(definition) {
        Ok(table) => table,
        Err(TableError::TableDoesNotExist(_)) => return Ok(None),
        Err(error) => return Err(backend_error(error)),
    };
    let bytes = table
        .get(key)
        .map_err(backend_error)?
        .map(|value| value.value().to_vec());
    bytes
        .map(|bytes| serde_json::from_slice(&bytes).map_err(serialization_error))
        .transpose()
}

pub(super) fn read_record<T: DeserializeOwned>(
    write: &WriteTransaction,
    definition: TableDefinition<'static, &str, &[u8]>,
    key: &str,
) -> Result<Option<T>, StoreError> {
    let table = write.open_table(definition).map_err(backend_error)?;
    let bytes = table
        .get(key)
        .map_err(backend_error)?
        .map(|value| value.value().to_vec());
    bytes
        .map(|bytes| serde_json::from_slice(&bytes).map_err(serialization_error))
        .transpose()
}

pub(super) fn read_records<T: DeserializeOwned>(
    write: &WriteTransaction,
    definition: TableDefinition<'static, &str, &[u8]>,
) -> Result<Vec<T>, StoreError> {
    let table = write.open_table(definition).map_err(backend_error)?;
    let mut records = Vec::new();
    for item in table.iter().map_err(backend_error)? {
        let (_, value) = item.map_err(backend_error)?;
        records.push(serde_json::from_slice(value.value()).map_err(serialization_error)?);
    }
    Ok(records)
}

pub(super) fn write_record<T: Serialize>(
    write: &WriteTransaction,
    definition: TableDefinition<'static, &str, &[u8]>,
    key: &str,
    record: &T,
) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec(record).map_err(serialization_error)?;
    let mut table = write.open_table(definition).map_err(backend_error)?;
    table.insert(key, bytes.as_slice()).map_err(backend_error)?;
    Ok(())
}
