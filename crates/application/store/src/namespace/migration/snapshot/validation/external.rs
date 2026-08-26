use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use lumin_evidence::{
    CacheCleanupOperationRecord, CacheCleanupOperationStatus, GateLifecycle, GateOperationKind,
    GateOperationStatus, GateRecord, OperationRecord, SemanticReadReservationBinding, WriteLease,
    WriteLeaseKind,
};
use lumin_model::{AttemptStatus, decode_native_path_component};
use serde::de::DeserializeOwned;

use crate::retention::{MigrationRunPayload, records::StoredRetentionPlan};
use crate::{AttemptEnvelope, RunCatalogRecord, StoreError, io_error};

use super::super::super::super::platform::{EntryAccess, EntryKind, HeldEntry};
use super::super::super::super::{
    NamespaceGuard, records::ManagedStateParentKind, require_state_volume,
};
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
    let mut attempts = validate_attempt_directories(snapshot, guard)?;
    let moved_payloads = validate_retention_payloads(snapshot, guard)?;
    merge_retention_attempts(guard, &mut attempts, &moved_payloads.attempts)?;
    validate_retention_runs(&attempts, &moved_payloads.runs)?;
    validate_run_children(snapshot, guard)?;
    for (key, bytes) in &snapshot.run_catalog {
        validate_run(
            key,
            bytes,
            guard,
            &attempts,
            moved_payloads.runs.get(key).map(|payload| &payload.path),
        )?;
    }
    validate_latest_pointers(snapshot, guard, &attempts)?;
    guard.validate_bound_entries()
}

fn validate_run_children(
    snapshot: &LogicalStoreSnapshot,
    guard: &NamespaceGuard,
) -> Result<(), StoreError> {
    let runs_path = guard.managed_parent_path(ManagedStateParentKind::Runs);
    let mut names = Vec::new();
    for entry in fs::read_dir(&runs_path).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let name = entry.file_name().into_string().map_err(|_| {
            StoreError::Integrity("runs contains a non-UTF-8 child name".to_owned())
        })?;
        if name != "namespace.anchor" {
            names.push(name);
        }
    }
    names.sort();

    let mut maximum_sequence = 0_u64;
    for name in names {
        if name.starts_with("run_") {
            maximum_sequence = maximum_sequence.max(canonical_sequence_id(
                &name,
                "run_",
                "retained run directory",
            )?);
        }
        if snapshot.run_catalog.contains_key(&name) {
            continue;
        }
        let path = runs_path.join(&name);
        let held = guard.open_managed_child_directory(
            ManagedStateParentKind::Runs,
            &name,
            "orphan run directory",
        )?;
        crate::retention::validate_migration_orphan_payload(&path)?;
        held.validate_path(
            &path,
            EntryKind::Directory,
            EntryAccess::ReadOnly,
            false,
            "orphan run directory",
        )?;
    }
    super::validate_allocator_sequence(snapshot, "attempt", maximum_sequence)
}

fn merge_retention_attempts(
    guard: &NamespaceGuard,
    attempts: &mut BTreeMap<String, Option<AttemptEnvelope>>,
    retention_attempts: &BTreeMap<String, PathBuf>,
) -> Result<(), StoreError> {
    for (attempt_id, path) in retention_attempts {
        let sequence = canonical_sequence_id(attempt_id, "attempt_", "retention attempt")?;
        let envelope = read_attempt_envelope(
            guard,
            &path.join("attempt.json"),
            attempt_id,
            sequence,
            "retention attempt envelope",
        )?;
        match attempts.get(attempt_id) {
            Some(Some(existing)) if existing == &envelope => {}
            Some(_) => {
                return Err(StoreError::Integrity(format!(
                    "retention attempt {attempt_id} disagrees with its canonical envelope"
                )));
            }
            None => {
                attempts.insert(attempt_id.clone(), Some(envelope));
            }
        }
    }
    Ok(())
}

fn validate_attempt_directories(
    snapshot: &LogicalStoreSnapshot,
    guard: &NamespaceGuard,
) -> Result<BTreeMap<String, Option<AttemptEnvelope>>, StoreError> {
    let attempts_path = guard.managed_parent_path(ManagedStateParentKind::Attempts);
    let mut names = Vec::new();
    for entry in fs::read_dir(&attempts_path).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| StoreError::Integrity("attempt directory name is not UTF-8".to_owned()))?;
        if name != "namespace.anchor" {
            names.push(name);
        }
    }
    names.sort();

    let mut attempts = BTreeMap::new();
    let mut pending_attempts = BTreeSet::new();
    let mut maximum_sequence = 0_u64;
    for attempt_id in names {
        let sequence = canonical_sequence_id(&attempt_id, "attempt_", "retained attempt")?;
        maximum_sequence = maximum_sequence.max(sequence);
        let attempt_dir = attempts_path.join(&attempt_id);
        let held_dir = guard.open_managed_child_directory(
            ManagedStateParentKind::Attempts,
            &attempt_id,
            "retained attempt directory",
        )?;

        let mut contents = Vec::new();
        for entry in fs::read_dir(&attempt_dir).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let name = entry.file_name().into_string().map_err(|_| {
                StoreError::Integrity(format!(
                    "retained attempt {attempt_id} contains a non-UTF-8 entry"
                ))
            })?;
            if name != "attempt.json" && name != "attempt.json.pending" {
                return Err(StoreError::Integrity(format!(
                    "retained attempt {attempt_id} contains an unknown entry {name}"
                )));
            }
            contents.push(name);
        }
        contents.sort();

        let envelope = if contents
            .binary_search_by(|name| name.as_str().cmp("attempt.json"))
            .is_ok()
        {
            Some(read_attempt_envelope(
                guard,
                &attempt_dir.join("attempt.json"),
                &attempt_id,
                sequence,
                "retained attempt envelope",
            )?)
        } else {
            None
        };
        if contents
            .binary_search_by(|name| name.as_str().cmp("attempt.json.pending"))
            .is_ok()
        {
            read_attempt_envelope(
                guard,
                &attempt_dir.join("attempt.json.pending"),
                &attempt_id,
                sequence,
                "retained pending attempt envelope",
            )?;
            pending_attempts.insert(attempt_id.clone());
        }
        held_dir.validate_path(
            &attempt_dir,
            EntryKind::Directory,
            EntryAccess::ReadOnly,
            false,
            "retained attempt directory",
        )?;
        attempts.insert(attempt_id, envelope);
    }

    crate::publication::validate_migration_attempt_links(
        &snapshot.attempt_leases,
        &attempts,
        &pending_attempts,
    )?;
    super::validate_allocator_sequence(snapshot, "attempt", maximum_sequence)?;
    Ok(attempts)
}

fn read_attempt_envelope(
    guard: &NamespaceGuard,
    path: &Path,
    expected_id: &str,
    expected_sequence: u64,
    label: &str,
) -> Result<AttemptEnvelope, StoreError> {
    let envelope = read_state_json::<AttemptEnvelope>(guard, path, label)?;
    crate::publication::validate_attempt_envelope(&envelope)?;
    if envelope.attempt_id.as_str() != expected_id || envelope.sequence != expected_sequence {
        return Err(StoreError::Integrity(format!(
            "{label} disagrees with its directory"
        )));
    }
    Ok(envelope)
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

fn validate_latest_pointers(
    snapshot: &LogicalStoreSnapshot,
    guard: &NamespaceGuard,
    attempts: &BTreeMap<String, Option<AttemptEnvelope>>,
) -> Result<(), StoreError> {
    let table_attempt = snapshot
        .pointers
        .get("latest-attempt")
        .map(|bytes| {
            std::str::from_utf8(bytes)
                .map(str::to_owned)
                .map_err(|error| {
                    StoreError::Integrity(format!("latest-attempt pointer is not UTF-8: {error}"))
                })
        })
        .transpose()?;
    let table_completed = snapshot
        .pointers
        .get("latest-completed")
        .map(|bytes| {
            std::str::from_utf8(bytes)
                .map(str::to_owned)
                .map_err(|error| {
                    StoreError::Integrity(format!("latest-completed pointer is not UTF-8: {error}"))
                })
        })
        .transpose()?;
    let mut read_run = |run_id: &lumin_model::RunId| {
        let bytes = snapshot.run_catalog.get(run_id.as_str()).ok_or_else(|| {
            StoreError::Integrity(format!(
                "latest-completed pointer references missing run {}",
                run_id.as_str()
            ))
        })?;
        parse_record::<RunCatalogRecord>("run-catalog", run_id.as_str(), bytes)
    };
    let mut has_active_lease = |attempt_id: &lumin_model::AttemptId| {
        crate::publication::migration_has_active_lease(&snapshot.attempt_leases, attempt_id)
    };
    let (document_attempt, document_completed) = crate::publication::migration_pointer_ids(
        &guard.state.state_dir,
        guard,
        &mut read_run,
        &mut has_active_lease,
    )?;
    if table_attempt.as_deref() != document_attempt.as_ref().map(|id| id.as_str())
        || table_completed.as_deref() != document_completed.as_ref().map(|id| id.as_str())
    {
        return Err(StoreError::Integrity(
            "lifecycle-store pointer table disagrees with durable latest document".to_owned(),
        ));
    }

    if let Some(attempt_id) = document_attempt.as_ref() {
        let sequence = canonical_sequence_id(attempt_id.as_str(), "attempt_", "latest attempt")?;
        let envelope = attempts
            .get(attempt_id.as_str())
            .and_then(Option::as_ref)
            .ok_or_else(|| {
                StoreError::Integrity(
                    "latest-attempt pointer references a missing complete envelope".to_owned(),
                )
            })?;
        if envelope.attempt_id != *attempt_id || envelope.sequence != sequence {
            return Err(StoreError::Integrity(
                "latest-attempt pointer disagrees with its envelope".to_owned(),
            ));
        }
    }
    validate_latest_frontier(
        snapshot,
        attempts,
        document_attempt.as_ref(),
        document_completed.as_ref(),
    )
}

fn validate_latest_frontier(
    snapshot: &LogicalStoreSnapshot,
    attempts: &BTreeMap<String, Option<AttemptEnvelope>>,
    latest_attempt: Option<&lumin_model::AttemptId>,
    latest_completed: Option<&lumin_model::RunId>,
) -> Result<(), StoreError> {
    let latest_attempt_sequence = latest_attempt
        .map(|attempt_id| canonical_sequence_id(attempt_id.as_str(), "attempt_", "latest attempt"))
        .transpose()?
        .unwrap_or_default();
    let mut newer_attempt = None;
    for envelope in attempts.values().filter_map(Option::as_ref) {
        if envelope.sequence <= latest_attempt_sequence {
            continue;
        }
        if newer_attempt.replace(&envelope.attempt_id).is_some() {
            return Err(StoreError::Integrity(
                "durable latestAttempt regresses behind authenticated attempt history".to_owned(),
            ));
        }
    }
    if let Some(attempt_id) = newer_attempt
        && !crate::publication::migration_has_active_lease(&snapshot.attempt_leases, attempt_id)?
    {
        return Err(StoreError::Integrity(
            "durable latestAttempt regresses behind authenticated attempt history".to_owned(),
        ));
    }

    let latest_completed_sequence = latest_completed
        .map(|run_id| canonical_sequence_id(run_id.as_str(), "run_", "latest completed run"))
        .transpose()?
        .unwrap_or_default();
    let mut newer_run_owner = None;
    for (key, bytes) in &snapshot.run_catalog {
        let record = parse_record::<RunCatalogRecord>("run-catalog", key, bytes)?;
        if record.sequence <= latest_completed_sequence {
            continue;
        }
        if newer_run_owner.replace(record.attempt_id).is_some() {
            return Err(StoreError::Integrity(
                "durable latestCompleted regresses behind authenticated run history".to_owned(),
            ));
        }
    }
    if let Some(attempt_id) = newer_run_owner
        && !crate::publication::migration_has_active_lease(&snapshot.attempt_leases, &attempt_id)?
    {
        return Err(StoreError::Integrity(
            "durable latestCompleted regresses behind authenticated run history".to_owned(),
        ));
    }
    Ok(())
}

fn validate_run(
    key: &str,
    bytes: &[u8],
    guard: &NamespaceGuard,
    attempts: &BTreeMap<String, Option<AttemptEnvelope>>,
    moved_path: Option<&PathBuf>,
) -> Result<(), StoreError> {
    let record = parse_record::<RunCatalogRecord>("run-catalog", key, bytes)?;
    validate_run_record(key, &record, attempts)?;

    let canonical_run_dir = guard
        .state
        .state_dir
        .join("runs")
        .join(record.run_id.as_str());
    let run_dir = moved_path.unwrap_or(&canonical_run_dir);
    let held_dir = open_state_entry(guard, run_dir, EntryKind::Directory, false, "run directory")?;
    crate::publication::validate_run_directory(run_dir, &held_dir, &record)
}

fn validate_retention_runs(
    attempts: &BTreeMap<String, Option<AttemptEnvelope>>,
    runs: &BTreeMap<String, MigrationRunPayload>,
) -> Result<(), StoreError> {
    for (key, payload) in runs {
        validate_run_record(key, &payload.record, attempts)?;
    }
    Ok(())
}

fn validate_run_record(
    key: &str,
    record: &RunCatalogRecord,
    attempts: &BTreeMap<String, Option<AttemptEnvelope>>,
) -> Result<(), StoreError> {
    let run_sequence = canonical_sequence_id(record.run_id.as_str(), "run_", "run")?;
    let attempt_sequence =
        canonical_sequence_id(record.attempt_id.as_str(), "attempt_", "run attempt")?;
    if key != record.run_id.as_str()
        || run_sequence != record.sequence
        || attempt_sequence != record.sequence
    {
        return Err(StoreError::Integrity(format!(
            "run catalog entry {key} has incoherent sequence identities"
        )));
    }
    let attempt = attempts
        .get(record.attempt_id.as_str())
        .and_then(Option::as_ref);
    if attempt.is_none_or(|attempt| {
        attempt.state != AttemptStatus::Completed
            || attempt.sequence != record.sequence
            || attempt.run_id.as_ref() != Some(&record.run_id)
    }) {
        return Err(StoreError::Integrity(format!(
            "run catalog entry {key} is not owned by its completed attempt"
        )));
    }
    Ok(())
}

fn validate_retention_payloads(
    snapshot: &LogicalStoreSnapshot,
    guard: &NamespaceGuard,
) -> Result<crate::retention::MigrationPayloadPaths, StoreError> {
    let mut payloads = crate::retention::MigrationPayloadPaths::default();
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
        let validated = crate::retention::validate_migration_payloads(guard, &plan)?;
        payloads.attempts.extend(validated.attempts);
        payloads.runs.extend(validated.runs);
    }
    Ok(payloads)
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
