use std::fs;
use std::path::PathBuf;

use fs2::FileExt;
use lumin_evidence::{GateOperationStatus, OperationLivenessLease, OperationRecord};
use lumin_model::{OperationId, PhysicalFileIdentity, digest_hex};

use crate::{RepositoryStore, StoreError, StoreGeneration, io_error, namespace, nonce_hex};

use super::records::{read_records, write_record};
use super::{OPERATIONS, validate_reservation_binding_set};

pub struct OperationSession<'store> {
    pub(super) store: &'store RepositoryStore,
    pub(super) operation_id: OperationId,
    pub(super) liveness: OperationLivenessLease,
    generation: StoreGeneration,
    lock_file: namespace::HeldEntry,
}

impl RepositoryStore {
    pub fn begin_operation(
        &self,
        operation_id: &OperationId,
    ) -> Result<OperationSession<'_>, StoreError> {
        let (lock_path, lock_file) = self.ensure_operation_lock_file(operation_id)?;
        match lock_file.file().try_lock_exclusive() {
            Ok(()) => {}
            Err(error) if namespace::lock_contended(&error) => {
                return Err(StoreError::OperationBusy(operation_id.as_str().to_owned()));
            }
            Err(error) => return Err(io_error(error)),
        }
        validate_operation_lock_file(&lock_file, &lock_path, operation_id)?;
        self.recover_interrupted_operations(Some((operation_id, &lock_file)))?;
        let generation = self.current_store_generation()?;
        Ok(OperationSession {
            store: self,
            operation_id: operation_id.clone(),
            liveness: OperationLivenessLease {
                lease_nonce: nonce_hex()?,
                owner_process_id: std::process::id(),
                lock_physical_identity: Some(lock_file.identity().clone()),
            },
            generation,
            lock_file,
        })
    }

    pub(super) fn recover_interrupted_operations(
        &self,
        current_operation: Option<(&OperationId, &namespace::HeldEntry)>,
    ) -> Result<(), StoreError> {
        self.with_exclusive_lock(|guard| {
            let database = guard.open_database()?;
            let write = database.begin_write()?;
            let mut recovered_locks = Vec::new();
            for mut operation in read_records::<OperationRecord>(&write, OPERATIONS)? {
                if operation.status != GateOperationStatus::Pending {
                    continue;
                }
                validate_reservation_binding_set(&operation)?;
                if !operation_lock_is_interrupted(
                    guard,
                    &operation,
                    current_operation,
                    &mut recovered_locks,
                )? {
                    continue;
                }
                operation.status = GateOperationStatus::Interrupted;
                operation.interruption_count =
                    operation.interruption_count.checked_add(1).ok_or_else(|| {
                        StoreError::Integrity("operation interruption count overflow".to_owned())
                    })?;
                operation.leased_write_set.clear();
                operation.semantic_read_reservations.clear();
                operation.semantic_read_reservation_bindings.clear();
                operation.operation_liveness = None;
                write_record(
                    &write,
                    OPERATIONS,
                    operation.operation_id.as_str(),
                    &operation,
                )?;
            }
            guard.commit(write)?;
            drop(recovered_locks);
            Ok(())
        })
    }

    fn ensure_operation_lock_file(
        &self,
        operation_id: &OperationId,
    ) -> Result<(PathBuf, namespace::HeldEntry), StoreError> {
        self.with_exclusive_lock(|guard| {
            let name = operation_lock_name(operation_id);
            let path = self.state_dir.join(&name);
            let file = guard.open_or_create_state_file(
                &name,
                "operation liveness lock",
                operation_id.as_str().as_bytes(),
            )?;
            Ok((path, file))
        })
    }

    fn current_store_generation(&self) -> Result<StoreGeneration, StoreError> {
        self.with_shared_lock(|guard| Ok(guard.open_database()?.generation()))
    }
}

impl OperationSession<'_> {
    pub(super) fn open_database<'guard>(
        &self,
        guard: &'guard namespace::NamespaceGuard,
    ) -> Result<namespace::StoreDatabase<'guard>, StoreError> {
        guard.open_database_for_generation(self.generation)
    }

    pub(super) fn bind_pending_operation(
        &self,
        operation: &mut OperationRecord,
    ) -> Result<(), StoreError> {
        self.validate_live_lock()?;
        if operation.operation_id != self.operation_id {
            return Err(StoreError::Integrity(format!(
                "operation session identity mismatch: {}",
                operation.operation_id.as_str()
            )));
        }
        if operation.status == GateOperationStatus::Committed {
            return Err(StoreError::Integrity(format!(
                "committed operation cannot acquire a liveness binding: {}",
                operation.operation_id.as_str()
            )));
        }
        operation.status = GateOperationStatus::Pending;
        operation.operation_liveness = Some(self.liveness.clone());
        Ok(())
    }

    pub(super) fn validate_pending_operation(
        &self,
        operation: &OperationRecord,
    ) -> Result<(), StoreError> {
        self.validate_live_lock()?;
        if operation.status != GateOperationStatus::Pending
            || operation.operation_liveness.as_ref() != Some(&self.liveness)
        {
            return Err(StoreError::Integrity(format!(
                "operation is not bound to the current live session: {}",
                operation.operation_id.as_str()
            )));
        }
        Ok(())
    }

    fn validate_live_lock(&self) -> Result<(), StoreError> {
        let path = operation_lock_path(self.store.state_dir.as_path(), &self.operation_id);
        validate_operation_lock_file(&self.lock_file, &path, &self.operation_id)?;
        if self.liveness.lock_physical_identity.as_ref() != Some(self.lock_file.identity()) {
            return Err(StoreError::Integrity(format!(
                "operation liveness lock binding changed: {}",
                self.operation_id.as_str()
            )));
        }
        Ok(())
    }
}

fn operation_lock_is_interrupted(
    guard: &namespace::NamespaceGuard,
    operation: &OperationRecord,
    current_operation: Option<(&OperationId, &namespace::HeldEntry)>,
    recovered_locks: &mut Vec<namespace::HeldEntry>,
) -> Result<bool, StoreError> {
    let expected_identity = operation
        .operation_liveness
        .as_ref()
        .and_then(|liveness| liveness.lock_physical_identity.as_ref())
        .ok_or_else(|| {
            StoreError::Integrity(format!(
                "pending operation omitted its lock identity binding: {}",
                operation.operation_id.as_str()
            ))
        })?;
    let lock_name = operation_lock_name(&operation.operation_id);
    if let Some((operation_id, file)) = current_operation
        && operation.operation_id == *operation_id
    {
        validate_locked_operation_lock(
            guard,
            file,
            &lock_name,
            &operation.operation_id,
            expected_identity,
        )?;
        return Ok(true);
    }

    let file = guard.open_state_file(&lock_name, "operation liveness lock")?;
    validate_operation_lock_identity(
        guard,
        &file,
        &lock_name,
        &operation.operation_id,
        expected_identity,
    )?;
    match file.file().try_lock_exclusive() {
        Ok(()) => {
            validate_locked_operation_lock(
                guard,
                &file,
                &lock_name,
                &operation.operation_id,
                expected_identity,
            )?;
            recovered_locks.push(file);
            Ok(true)
        }
        Err(error) if namespace::lock_contended(&error) => Ok(false),
        Err(error) => Err(io_error(error)),
    }
}

fn operation_lock_name(operation_id: &OperationId) -> String {
    format!(
        "operation-liveness-{}.lock",
        digest_hex(operation_id.as_str().as_bytes())
    )
}

fn operation_lock_path(state_dir: &std::path::Path, operation_id: &OperationId) -> PathBuf {
    state_dir.join(operation_lock_name(operation_id))
}

fn validate_operation_lock_identity(
    guard: &namespace::NamespaceGuard,
    file: &namespace::HeldEntry,
    lock_name: &str,
    operation_id: &OperationId,
    expected_identity: &PhysicalFileIdentity,
) -> Result<(), StoreError> {
    guard.validate_state_file(file, lock_name, "operation liveness lock")?;
    if file.identity() != expected_identity {
        return Err(StoreError::Integrity(format!(
            "operation liveness lock physical identity changed: {}",
            operation_id.as_str()
        )));
    }
    Ok(())
}

fn validate_locked_operation_lock(
    guard: &namespace::NamespaceGuard,
    file: &namespace::HeldEntry,
    lock_name: &str,
    operation_id: &OperationId,
    expected_identity: &PhysicalFileIdentity,
) -> Result<(), StoreError> {
    validate_operation_lock_identity(guard, file, lock_name, operation_id, expected_identity)?;
    verify_operation_lock_file(file, operation_id)
}

fn validate_operation_lock_file(
    file: &namespace::HeldEntry,
    path: &std::path::Path,
    operation_id: &OperationId,
) -> Result<(), StoreError> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(StoreError::Integrity(
            "operation liveness lock must be a real file".to_owned(),
        ));
    }
    file.validate_path(
        path,
        namespace::EntryKind::RegularFile,
        namespace::EntryAccess::ReadOnly,
        true,
        "operation liveness lock",
    )?;
    verify_operation_lock_file(file, operation_id)
}

fn verify_operation_lock_file(
    file: &namespace::HeldEntry,
    operation_id: &OperationId,
) -> Result<(), StoreError> {
    let bytes = file.read_all()?;
    if bytes != operation_id.as_str().as_bytes() {
        return Err(StoreError::Integrity(format!(
            "operation liveness lock identity mismatch: {}",
            operation_id.as_str()
        )));
    }
    Ok(())
}
