use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;

use crate::retention::records::StoredRetentionPlan;
use crate::{AttemptEnvelope, RunCatalogRecord, StoreError, digest_hex};

use super::super::super::super::platform::{EntryAccess, EntryKind, HeldEntry};
use super::super::super::super::{NamespaceGuard, require_state_volume};
use super::super::LogicalStoreSnapshot;
use super::parse_record;

pub(super) fn validate_external_references(
    snapshot: &LogicalStoreSnapshot,
    guard: &NamespaceGuard,
) -> Result<(), StoreError> {
    guard.validate_bound_entries()?;
    validate_latest_attempt(snapshot, guard)?;
    let moved_runs = validate_retention_payloads(snapshot, guard)?;
    for (key, bytes) in &snapshot.run_catalog {
        validate_run(key, bytes, guard, moved_runs.get(key))?;
    }
    guard.validate_bound_entries()
}

fn validate_latest_attempt(
    snapshot: &LogicalStoreSnapshot,
    guard: &NamespaceGuard,
) -> Result<(), StoreError> {
    let Some(attempt_id) = snapshot.pointers.get("latest-attempt") else {
        return Ok(());
    };
    let attempt_id = std::str::from_utf8(attempt_id).map_err(|error| {
        StoreError::Integrity(format!("latest-attempt pointer is not UTF-8: {error}"))
    })?;
    let sequence = canonical_sequence_id(attempt_id, "attempt_", "latest attempt")?;
    let attempt_dir = guard.state.state_dir.join("attempts").join(attempt_id);
    let held_dir = open_state_entry(
        guard,
        &attempt_dir,
        EntryKind::Directory,
        false,
        "latest attempt directory",
    )?;
    let envelope: AttemptEnvelope = read_state_json(
        guard,
        &attempt_dir.join("attempt.json"),
        "latest attempt envelope",
    )?;
    held_dir.validate_path(
        &attempt_dir,
        EntryKind::Directory,
        EntryAccess::ReadOnly,
        false,
        "latest attempt directory",
    )?;
    if envelope.attempt_id.as_str() != attempt_id || envelope.sequence != sequence {
        return Err(StoreError::Integrity(
            "latest-attempt pointer disagrees with its envelope".to_owned(),
        ));
    }
    Ok(())
}

fn validate_run(
    key: &str,
    bytes: &[u8],
    guard: &NamespaceGuard,
    moved_path: Option<&PathBuf>,
) -> Result<(), StoreError> {
    let record = parse_record::<RunCatalogRecord>("run-catalog", key, bytes)?;
    let run_sequence = canonical_sequence_id(record.run_id.as_str(), "run_", "run")?;
    let attempt_sequence =
        canonical_sequence_id(record.attempt_id.as_str(), "attempt_", "run attempt")?;
    if run_sequence != record.sequence || attempt_sequence != record.sequence {
        return Err(StoreError::Integrity(format!(
            "run catalog entry {key} has incoherent sequence identities"
        )));
    }

    let canonical_run_dir = guard
        .state
        .state_dir
        .join("runs")
        .join(record.run_id.as_str());
    let run_dir = moved_path.unwrap_or(&canonical_run_dir);
    let held_dir = open_state_entry(guard, run_dir, EntryKind::Directory, false, "run directory")?;
    let envelope =
        read_state_json::<RunCatalogRecord>(guard, &run_dir.join("run.json"), "run envelope")?;
    if envelope.run_id != record.run_id
        || envelope.attempt_id != record.attempt_id
        || envelope.sequence != record.sequence
        || envelope.evidence_store_sha256 != record.evidence_store_sha256
        || envelope.evidence_store_size != record.evidence_store_size
    {
        return Err(StoreError::Integrity(format!(
            "run catalog entry {key} disagrees with its durable run envelope"
        )));
    }
    let evidence_path = run_dir.join("evidence.store");
    let evidence = read_state_file(guard, &evidence_path, "run evidence store")?;
    held_dir.validate_path(
        run_dir,
        EntryKind::Directory,
        EntryAccess::ReadOnly,
        false,
        "run directory",
    )?;
    if evidence.len() as u64 != record.evidence_store_size
        || digest_hex(&evidence) != record.evidence_store_sha256
    {
        return Err(StoreError::Integrity(format!(
            "run catalog entry {key} disagrees with its evidence store"
        )));
    }
    Ok(())
}

fn validate_retention_payloads(
    snapshot: &LogicalStoreSnapshot,
    guard: &NamespaceGuard,
) -> Result<BTreeMap<String, PathBuf>, StoreError> {
    let mut moved_runs = BTreeMap::new();
    for (key, bytes) in &snapshot.retention_plans {
        let plan = parse_record::<StoredRetentionPlan>("retention-plans", key, bytes)?;
        if plan.progress.is_none() {
            continue;
        }
        moved_runs.extend(crate::retention::validate_migration_payloads(guard, &plan)?);
    }
    Ok(moved_runs)
}

fn canonical_sequence_id(value: &str, prefix: &str, label: &str) -> Result<u64, StoreError> {
    let suffix = value.strip_prefix(prefix).ok_or_else(|| {
        StoreError::Integrity(format!("{label} ID is outside its canonical grammar"))
    })?;
    if suffix.len() != 16
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(StoreError::Integrity(format!(
            "{label} ID is outside its canonical grammar"
        )));
    }
    let sequence = u64::from_str_radix(suffix, 16).map_err(|error| {
        StoreError::Integrity(format!("{label} ID sequence is malformed: {error}"))
    })?;
    if sequence == 0 {
        return Err(StoreError::Integrity(format!(
            "{label} ID sequence must be nonzero"
        )));
    }
    Ok(sequence)
}

fn read_state_json<T: DeserializeOwned>(
    guard: &NamespaceGuard,
    path: &Path,
    label: &str,
) -> Result<T, StoreError> {
    let bytes = read_state_file(guard, path, label)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| StoreError::Integrity(format!("{label} is malformed: {error}")))
}

fn read_state_file(
    guard: &NamespaceGuard,
    path: &Path,
    label: &str,
) -> Result<Vec<u8>, StoreError> {
    let entry = open_state_entry(guard, path, EntryKind::RegularFile, true, label)?;
    let bytes = entry.read_all()?;
    entry.validate_path(
        path,
        EntryKind::RegularFile,
        EntryAccess::ReadOnly,
        true,
        label,
    )?;
    Ok(bytes)
}

fn open_state_entry(
    guard: &NamespaceGuard,
    path: &Path,
    kind: EntryKind,
    one_link: bool,
    label: &str,
) -> Result<HeldEntry, StoreError> {
    let entry = HeldEntry::open(path, kind, EntryAccess::ReadOnly, one_link, label)?;
    require_state_volume(&entry, &guard.state_directory, label)?;
    Ok(entry)
}
