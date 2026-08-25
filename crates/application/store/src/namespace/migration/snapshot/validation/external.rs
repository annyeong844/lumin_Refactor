use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use lumin_evidence::{
    CacheCleanupOperationRecord, CacheCleanupOperationStatus, GateLifecycle, GateOperationKind,
    GateOperationStatus, GateRecord, OperationRecord, SemanticReadReservationBinding, WriteLease,
    WriteLeaseKind,
};
use lumin_model::decode_native_path_component;
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
    crate::cache::validate_external_snapshot(
        guard,
        &snapshot.cache_cleanup_operations,
        &snapshot.cache_eviction_authorizations,
    )?;
    validate_pending_cleanup_liveness(snapshot, guard)?;
    validate_pending_operation_liveness(snapshot, guard)?;
    validate_pending_pre_write_leases(snapshot, guard)?;
    validate_pending_semantic_read_bindings(snapshot, guard)?;
    validate_active_gate_write_prefixes(snapshot, guard)?;
    validate_latest_attempt(snapshot, guard)?;
    let moved_runs = validate_retention_payloads(snapshot, guard)?;
    for (key, bytes) in &snapshot.run_catalog {
        validate_run(key, bytes, guard, moved_runs.get(key))?;
    }
    guard.validate_bound_entries()
}

fn validate_pending_semantic_read_bindings(
    snapshot: &LogicalStoreSnapshot,
    guard: &NamespaceGuard,
) -> Result<(), StoreError> {
    for (key, bytes) in &snapshot.operations {
        let operation = parse_record::<OperationRecord>("operations", key, bytes)?;
        if operation.status != GateOperationStatus::Pending {
            continue;
        }
        for binding in &operation.semantic_read_reservation_bindings {
            validate_pending_semantic_read_binding(key, binding, guard)?;
        }
    }
    Ok(())
}

fn validate_pending_semantic_read_binding(
    operation_key: &str,
    binding: &SemanticReadReservationBinding,
    guard: &NamespaceGuard,
) -> Result<(), StoreError> {
    let label = format!(
        "pending operation {operation_key} semantic-read reservation {}",
        binding.path.display
    );
    let native = worktree_path(guard, &binding.path, &label)?;
    match (&binding.physical_identity, &binding.absence_parent) {
        (Some(expected), None) => {
            let held = HeldResolvedWorktreeEntry::open(guard, &native, &label)?;
            if held.entry.identity() != expected {
                return Err(StoreError::Integrity(format!(
                    "{label} physical identity changed"
                )));
            }
            held.validate(&native, &label)
        }
        (None, Some(parent)) => {
            let parent_native = worktree_path(guard, &parent.path, &label)?;
            let held_parent = HeldResolvedWorktreeEntry::open(guard, &parent_native, &label)?;
            if held_parent.entry.identity() != &parent.physical_identity {
                return Err(StoreError::Integrity(format!(
                    "{label} absence-parent identity changed"
                )));
            }
            validate_absent_worktree_path(&native, &label)?;
            held_parent.validate(&parent_native, &label)?;
            validate_absent_worktree_path(&native, &label)
        }
        (None, None) => validate_broken_redirect(guard, &native, &label),
        (Some(_), Some(_)) => Err(StoreError::Integrity(format!(
            "{label} carries both direct and absence identities"
        ))),
    }
}

struct HeldResolvedWorktreeEntry {
    canonical_path: PathBuf,
    kind: EntryKind,
    entry: HeldEntry,
}

impl HeldResolvedWorktreeEntry {
    fn open(guard: &NamespaceGuard, path: &Path, label: &str) -> Result<Self, StoreError> {
        let canonical_root = fs::canonicalize(&guard.state.repository.path)
            .map_err(|error| StoreError::Io(error.to_string()))?;
        let canonical_path =
            fs::canonicalize(path).map_err(|error| StoreError::Io(error.to_string()))?;
        if !canonical_path.starts_with(&canonical_root) {
            return Err(StoreError::Integrity(format!(
                "{label} resolves outside the bound repository root"
            )));
        }
        let metadata =
            fs::metadata(&canonical_path).map_err(|error| StoreError::Io(error.to_string()))?;
        let kind = if metadata.is_dir() {
            EntryKind::Directory
        } else if metadata.is_file() {
            EntryKind::RegularFile
        } else {
            return Err(StoreError::Integrity(format!(
                "{label} has an unsupported physical entry kind"
            )));
        };
        let entry = HeldEntry::open(&canonical_path, kind, EntryAccess::ReadOnly, false, label)?;
        Ok(Self {
            canonical_path,
            kind,
            entry,
        })
    }

    fn validate(&self, path: &Path, label: &str) -> Result<(), StoreError> {
        self.validate_target(path, label)?;
        self.entry.validate_path(
            &self.canonical_path,
            self.kind,
            EntryAccess::ReadOnly,
            false,
            label,
        )?;
        self.validate_target(path, label)
    }

    fn validate_target(&self, path: &Path, label: &str) -> Result<(), StoreError> {
        let current = fs::canonicalize(path).map_err(|error| StoreError::Io(error.to_string()))?;
        if current != self.canonical_path {
            return Err(StoreError::Integrity(format!(
                "{label} physical identity changed"
            )));
        }
        Ok(())
    }
}

fn validate_broken_redirect(
    guard: &NamespaceGuard,
    path: &Path,
    label: &str,
) -> Result<(), StoreError> {
    let parent = path.parent().ok_or_else(|| {
        StoreError::Integrity(format!("{label} unresolved redirect omitted its parent"))
    })?;
    validate_contained_worktree_path(guard, parent, label)?;
    let validate = || match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() && fs::canonicalize(path).is_err() => {
            Ok(())
        }
        Ok(_) => Err(StoreError::Integrity(format!(
            "{label} no longer names an unresolved redirect"
        ))),
        Err(error) => Err(StoreError::Io(error.to_string())),
    };
    validate()?;
    validate()
}

fn validate_pending_pre_write_leases(
    snapshot: &LogicalStoreSnapshot,
    guard: &NamespaceGuard,
) -> Result<(), StoreError> {
    for (key, bytes) in &snapshot.operations {
        let operation = parse_record::<OperationRecord>("operations", key, bytes)?;
        if operation.status != GateOperationStatus::Pending
            || operation.kind != GateOperationKind::PreWrite
        {
            continue;
        }
        for lease in &operation.leased_write_set {
            validate_pending_pre_write_lease(key, lease, guard)?;
        }
    }
    Ok(())
}

fn validate_pending_pre_write_lease(
    operation_key: &str,
    lease: &WriteLease,
    guard: &NamespaceGuard,
) -> Result<(), StoreError> {
    let label = format!(
        "pending pre-write {operation_key} lease {}",
        lease.path.display
    );
    let held_prefixes = open_worktree_prefixes(lease, guard, &label)?;
    let native = worktree_path(guard, &lease.path, &label)?;
    match lease.kind {
        WriteLeaseKind::ExistingFile => {
            validate_contained_worktree_path(guard, &native, &label)?;
            let held = HeldEntry::open_following_file(&native, &label)?;
            if lease.physical_identity.as_ref() != Some(held.identity()) {
                return Err(StoreError::Integrity(format!(
                    "{label} physical identity changed"
                )));
            }
            validate_contained_worktree_path(guard, &native, &label)?;
            validate_held_prefixes(&held_prefixes, &label)?;
            held.validate_following_file_path(&native, &label)?;
            validate_contained_worktree_path(guard, &native, &label)?;
        }
        WriteLeaseKind::Directory => {
            validate_contained_worktree_path(guard, &native, &label)?;
            let held = HeldEntry::open(
                &native,
                EntryKind::Directory,
                EntryAccess::ReadOnly,
                false,
                &label,
            )?;
            if lease.physical_identity.as_ref() != Some(held.identity()) {
                return Err(StoreError::Integrity(format!(
                    "{label} physical identity changed"
                )));
            }
            validate_held_prefixes(&held_prefixes, &label)?;
            held.validate_path(
                &native,
                EntryKind::Directory,
                EntryAccess::ReadOnly,
                false,
                &label,
            )?;
            validate_contained_worktree_path(guard, &native, &label)?;
        }
        WriteLeaseKind::NewFile => {
            validate_absent_worktree_path(&native, &label)?;
            validate_held_prefixes(&held_prefixes, &label)?;
            validate_absent_worktree_path(&native, &label)?;
        }
    }
    Ok(())
}

fn validate_held_prefixes(
    held_prefixes: &[(PathBuf, HeldEntry)],
    label: &str,
) -> Result<(), StoreError> {
    for (native, held) in held_prefixes {
        held.validate_path(
            native,
            EntryKind::Directory,
            EntryAccess::ReadOnly,
            false,
            label,
        )?;
    }
    Ok(())
}

fn validate_absent_worktree_path(path: &Path, label: &str) -> Result<(), StoreError> {
    match fs::symlink_metadata(path) {
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(())
        }
        Ok(_) => Err(StoreError::Integrity(format!(
            "{label} no longer names an absent path"
        ))),
        Err(error) => Err(StoreError::Io(error.to_string())),
    }
}

fn open_worktree_prefixes(
    lease: &WriteLease,
    guard: &NamespaceGuard,
    label: &str,
) -> Result<Vec<(PathBuf, HeldEntry)>, StoreError> {
    let mut held_prefixes = Vec::with_capacity(lease.prefix_identities.len());
    for prefix in &lease.prefix_identities {
        let native = worktree_path(guard, &prefix.path, label)?;
        validate_contained_worktree_path(guard, &native, label)?;
        let held = HeldEntry::open(
            &native,
            EntryKind::Directory,
            EntryAccess::ReadOnly,
            false,
            label,
        )?;
        if held.identity() != &prefix.physical_identity {
            return Err(StoreError::Integrity(format!(
                "{label} prefix identity changed"
            )));
        }
        held_prefixes.push((native, held));
    }
    Ok(held_prefixes)
}

fn worktree_path(
    guard: &NamespaceGuard,
    path: &lumin_evidence::RepoPathProjection,
    label: &str,
) -> Result<PathBuf, StoreError> {
    let mut relative = PathBuf::new();
    for component in &path.components {
        let native = decode_native_path_component(component).map_err(|error| {
            StoreError::Integrity(format!("{label} path is not canonical: {error}"))
        })?;
        relative.push(native);
    }
    Ok(guard.state.repository.path.join(relative))
}

fn validate_contained_worktree_path(
    guard: &NamespaceGuard,
    path: &Path,
    label: &str,
) -> Result<(), StoreError> {
    let root = fs::canonicalize(&guard.state.repository.path)
        .map_err(|error| StoreError::Io(error.to_string()))?;
    let target = fs::canonicalize(path).map_err(|error| StoreError::Io(error.to_string()))?;
    if !target.starts_with(&root) {
        return Err(StoreError::Integrity(format!(
            "{label} resolves outside the bound repository root"
        )));
    }
    Ok(())
}

fn validate_pending_operation_liveness(
    snapshot: &LogicalStoreSnapshot,
    guard: &NamespaceGuard,
) -> Result<(), StoreError> {
    for (key, bytes) in &snapshot.operations {
        let operation = parse_record::<OperationRecord>("operations", key, bytes)?;
        if operation.status == GateOperationStatus::Pending {
            crate::gate::validate_migration_operation_liveness(guard, &operation)?;
        }
    }
    Ok(())
}

fn validate_pending_cleanup_liveness(
    snapshot: &LogicalStoreSnapshot,
    guard: &NamespaceGuard,
) -> Result<(), StoreError> {
    for (key, bytes) in &snapshot.cache_cleanup_operations {
        let operation =
            parse_record::<CacheCleanupOperationRecord>("cache-cleanup-operations", key, bytes)?;
        if operation.status != CacheCleanupOperationStatus::Pending {
            continue;
        }
        let liveness = &operation
            .execution_lease
            .as_ref()
            .ok_or_else(|| {
                StoreError::Integrity(format!(
                    "pending cache cleanup {key} omitted its execution lease"
                ))
            })?
            .liveness;
        crate::gate::validate_migration_liveness_lease(guard, &operation.operation_id, liveness)?;
    }
    Ok(())
}

fn validate_active_gate_write_prefixes(
    snapshot: &LogicalStoreSnapshot,
    guard: &NamespaceGuard,
) -> Result<(), StoreError> {
    for (key, bytes) in &snapshot.gates {
        let gate = parse_record::<GateRecord>("gates", key, bytes)?;
        if gate.lifecycle != GateLifecycle::Active {
            continue;
        }
        let baseline = gate.baseline.as_ref().ok_or_else(|| {
            StoreError::Integrity(format!("active gate {key} omitted its sealed baseline"))
        })?;
        for lease in baseline
            .leased_write_set
            .iter()
            .filter(|lease| lease.kind == WriteLeaseKind::NewFile)
        {
            let mut held_prefixes = Vec::with_capacity(lease.prefix_identities.len());
            for prefix in &lease.prefix_identities {
                let mut relative = PathBuf::new();
                for component in &prefix.path.components {
                    let native = decode_native_path_component(component).map_err(|error| {
                        StoreError::Integrity(format!(
                            "gate {key} new-file prefix is not canonical: {error}"
                        ))
                    })?;
                    relative.push(native);
                }
                let native = guard.state.repository.path.join(relative);
                let held = HeldEntry::open(
                    &native,
                    EntryKind::Directory,
                    EntryAccess::ReadOnly,
                    false,
                    "active gate new-file prefix",
                )?;
                if held.identity() != &prefix.physical_identity {
                    return Err(StoreError::Integrity(format!(
                        "gate {key} new-file lease {} prefix identity changed",
                        lease.path.display
                    )));
                }
                held_prefixes.push((native, held));
            }
            for (native, held) in &held_prefixes {
                held.validate_path(
                    native,
                    EntryKind::Directory,
                    EntryAccess::ReadOnly,
                    false,
                    "active gate new-file prefix",
                )?;
            }
        }
    }
    Ok(())
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
    crate::publication::validate_attempt_envelope(&envelope)?;
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
        if &plan.record.repository_id != guard.repository_id() {
            return Err(StoreError::Integrity(format!(
                "retention plan {key} changed repository ownership"
            )));
        }
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
