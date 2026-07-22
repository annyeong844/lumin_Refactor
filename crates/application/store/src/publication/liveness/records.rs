use lumin_model::{AttemptId, PhysicalFileIdentity};
use redb::{ReadableTable, TableError};
use serde::{Deserialize, Serialize};

use crate::namespace::{HeldEntry, NamespaceGuard};
use crate::{
    ATTEMPT_LEASES, SEQUENCES, StoreError, StoreGeneration, backend_error, serialization_error,
};

const LEASE_SCHEMA: &str = "lumin-attempt-lease.v1";
const LOCK_SCHEMA: &str = "lumin-attempt-lock.v1";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum AttemptLeaseState {
    Active,
    Releasing,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AttemptLeaseRecord {
    pub(super) schema_version: String,
    pub(super) attempt_id: AttemptId,
    pub(super) sequence: u64,
    pub(super) lease_nonce: String,
    pub(super) owner_process_id: u32,
    pub(super) lock_name: String,
    pub(super) lock_physical_identity: PhysicalFileIdentity,
    pub(super) generation: StoreGeneration,
    pub(super) state: AttemptLeaseState,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AttemptLockBinding<'record> {
    schema_version: &'static str,
    attempt_id: &'record AttemptId,
    sequence: u64,
    lease_nonce: &'record str,
    owner_process_id: u32,
    lock_name: &'record str,
    lock_physical_identity: &'record PhysicalFileIdentity,
    generation: StoreGeneration,
}

pub(super) fn allocate(
    guard: &NamespaceGuard,
    lock_file: &HeldEntry,
    lock_name: String,
    lease_nonce: String,
    owner_process_id: u32,
) -> Result<AttemptLeaseRecord, StoreError> {
    let database = guard.open_database()?;
    let generation = database.generation();
    let write = database.begin_write()?;
    let sequence = {
        let mut sequences = write.open_table(SEQUENCES).map_err(backend_error)?;
        let current = sequences
            .get("attempt")
            .map_err(backend_error)?
            .map_or(0, |value| value.value());
        let next = current
            .checked_add(1)
            .ok_or_else(|| StoreError::Integrity("attempt sequence overflow".to_owned()))?;
        sequences.insert("attempt", next).map_err(backend_error)?;
        next
    };
    let lease = AttemptLeaseRecord {
        schema_version: LEASE_SCHEMA.to_owned(),
        attempt_id: AttemptId::from_string(format!("attempt_{sequence:016x}")),
        sequence,
        lease_nonce,
        owner_process_id,
        lock_name,
        lock_physical_identity: lock_file.identity().clone(),
        generation,
        state: AttemptLeaseState::Active,
    };
    validate_record(&lease)?;
    let bytes = record_bytes(&lease)?;
    lock_file.replace_contents(&lock_bytes(&lease)?)?;
    {
        let mut leases = write.open_table(ATTEMPT_LEASES).map_err(backend_error)?;
        if leases
            .get(lease.attempt_id.as_str())
            .map_err(backend_error)?
            .is_some()
        {
            return Err(StoreError::Integrity(format!(
                "attempt process-liveness lease already exists: {}",
                lease.attempt_id.as_str()
            )));
        }
        leases
            .insert(lease.attempt_id.as_str(), bytes.as_slice())
            .map_err(backend_error)?;
    }
    guard.commit(write)?;
    Ok(lease)
}

pub(super) fn mark_releasing(
    guard: &NamespaceGuard,
    expected: &AttemptLeaseRecord,
) -> Result<AttemptLeaseRecord, StoreError> {
    if expected.state != AttemptLeaseState::Active {
        return Err(StoreError::Integrity(format!(
            "attempt lease is not active: {}",
            expected.attempt_id.as_str()
        )));
    }
    let mut releasing = expected.clone();
    releasing.state = AttemptLeaseState::Releasing;
    // Live sessions are generation-fenced before this transition. Recovery must
    // clear an old-generation lease from the current canonical store.
    let database = guard.open_database()?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(ATTEMPT_LEASES).map_err(backend_error)?;
        let current = table
            .get(expected.attempt_id.as_str())
            .map_err(backend_error)?
            .map(|value| value.value().to_vec())
            .ok_or_else(|| {
                StoreError::Integrity(format!(
                    "attempt process-liveness lease is missing: {}",
                    expected.attempt_id.as_str()
                ))
            })?;
        if current != record_bytes(expected)? {
            return Err(StoreError::Integrity(format!(
                "attempt process-liveness lease changed: {}",
                expected.attempt_id.as_str()
            )));
        }
        let bytes = record_bytes(&releasing)?;
        table
            .insert(releasing.attempt_id.as_str(), bytes.as_slice())
            .map_err(backend_error)?;
    }
    guard.commit(write)?;
    Ok(releasing)
}

pub(super) fn read(
    guard: &NamespaceGuard,
    attempt_id: &AttemptId,
) -> Result<Option<AttemptLeaseRecord>, StoreError> {
    let database = guard.open_database()?;
    let read = database.begin_read()?;
    let table = match read.open_table(ATTEMPT_LEASES) {
        Ok(table) => table,
        Err(TableError::TableDoesNotExist(_)) => return Ok(None),
        Err(error) => return Err(backend_error(error)),
    };
    table
        .get(attempt_id.as_str())
        .map_err(backend_error)?
        .map(|value| parse_record(value.value(), Some(attempt_id)))
        .transpose()
}

pub(super) fn read_all(guard: &NamespaceGuard) -> Result<Vec<AttemptLeaseRecord>, StoreError> {
    let database = guard.open_database()?;
    let read = database.begin_read()?;
    let table = match read.open_table(ATTEMPT_LEASES) {
        Ok(table) => table,
        Err(TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
        Err(error) => return Err(backend_error(error)),
    };
    let mut records = Vec::new();
    for row in table.iter().map_err(backend_error)? {
        let (key, value) = row.map_err(backend_error)?;
        let attempt_id = AttemptId::from_string(key.value().to_owned());
        records.push(parse_record(value.value(), Some(&attempt_id))?);
    }
    records.sort_by(|left, right| {
        left.sequence
            .cmp(&right.sequence)
            .then_with(|| left.attempt_id.cmp(&right.attempt_id))
    });
    Ok(records)
}

pub(super) fn validate_snapshot(
    rows: &std::collections::BTreeMap<String, Vec<u8>>,
) -> Result<(), StoreError> {
    for (key, bytes) in rows {
        let attempt_id = AttemptId::from_string(key.clone());
        parse_record(bytes, Some(&attempt_id))?;
    }
    Ok(())
}

pub(super) fn remove(
    guard: &NamespaceGuard,
    expected: &AttemptLeaseRecord,
) -> Result<(), StoreError> {
    let database = guard.open_database()?;
    let write = database.begin_write()?;
    {
        let mut table = write.open_table(ATTEMPT_LEASES).map_err(backend_error)?;
        let current = table
            .get(expected.attempt_id.as_str())
            .map_err(backend_error)?
            .map(|value| value.value().to_vec())
            .ok_or_else(|| {
                StoreError::Integrity(format!(
                    "attempt process-liveness lease is missing: {}",
                    expected.attempt_id.as_str()
                ))
            })?;
        if current != record_bytes(expected)? {
            return Err(StoreError::Integrity(format!(
                "attempt process-liveness lease changed: {}",
                expected.attempt_id.as_str()
            )));
        }
        table
            .remove(expected.attempt_id.as_str())
            .map_err(backend_error)?;
    }
    guard.commit(write)
}

pub(super) fn validate_lock(
    guard: &NamespaceGuard,
    file: &HeldEntry,
    lease: &AttemptLeaseRecord,
) -> Result<(), StoreError> {
    validate_lock_identity(guard, file, lease)?;
    if file.read_all()? != lock_bytes(lease)? {
        return Err(StoreError::Integrity(format!(
            "attempt process-liveness lock contents changed: {}",
            lease.attempt_id.as_str()
        )));
    }
    Ok(())
}

pub(super) fn validate_lock_identity(
    guard: &NamespaceGuard,
    file: &HeldEntry,
    lease: &AttemptLeaseRecord,
) -> Result<(), StoreError> {
    validate_record(lease)?;
    guard.validate_state_file(file, &lease.lock_name, "attempt process-liveness lock")?;
    if file.identity() != &lease.lock_physical_identity {
        return Err(StoreError::Integrity(format!(
            "attempt process-liveness lock binding changed: {}",
            lease.attempt_id.as_str()
        )));
    }
    Ok(())
}

fn parse_record(
    bytes: &[u8],
    expected_id: Option<&AttemptId>,
) -> Result<AttemptLeaseRecord, StoreError> {
    let record: AttemptLeaseRecord = serde_json::from_slice(bytes).map_err(serialization_error)?;
    validate_record(&record)?;
    if expected_id.is_some_and(|attempt_id| attempt_id != &record.attempt_id) {
        return Err(StoreError::Integrity(
            "attempt lease table key disagrees with its record".to_owned(),
        ));
    }
    if bytes != record_bytes(&record)? {
        return Err(StoreError::Integrity(
            "attempt process-liveness lease bytes are not canonical".to_owned(),
        ));
    }
    Ok(record)
}

fn validate_record(record: &AttemptLeaseRecord) -> Result<(), StoreError> {
    if record.schema_version != LEASE_SCHEMA
        || record.attempt_id.as_str() != format!("attempt_{:016x}", record.sequence)
        || record.lease_nonce.is_empty()
        || record.lock_name != format!("attempt-liveness-{}.lock", record.lease_nonce)
    {
        return Err(StoreError::Integrity(format!(
            "attempt process-liveness lease is malformed: {}",
            record.attempt_id.as_str()
        )));
    }
    Ok(())
}

fn record_bytes(record: &AttemptLeaseRecord) -> Result<Vec<u8>, StoreError> {
    serde_json::to_vec(record).map_err(serialization_error)
}

fn lock_bytes(record: &AttemptLeaseRecord) -> Result<Vec<u8>, StoreError> {
    serde_json::to_vec(&AttemptLockBinding {
        schema_version: LOCK_SCHEMA,
        attempt_id: &record.attempt_id,
        sequence: record.sequence,
        lease_nonce: &record.lease_nonce,
        owner_process_id: record.owner_process_id,
        lock_name: &record.lock_name,
        lock_physical_identity: &record.lock_physical_identity,
        generation: record.generation,
    })
    .map_err(serialization_error)
}
