use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use fs2::FileExt;
use lumin_evidence::{GateOperationStatus, OperationLivenessLease, OperationRecord};
use lumin_model::{OperationId, digest_hex};

use crate::{
    RepositoryStore, StoreError, StoreGeneration, ensure_real_file, io_error, namespace, nonce_hex,
};

use super::records::{read_records, write_record};
use super::{OPERATIONS, validate_reservation_binding_set};

pub struct OperationSession<'store> {
    pub(super) store: &'store RepositoryStore,
    pub(super) operation_id: OperationId,
    pub(super) liveness: OperationLivenessLease,
    generation: StoreGeneration,
    _lock_file: File,
}

impl RepositoryStore {
    pub fn begin_operation(
        &self,
        operation_id: &OperationId,
    ) -> Result<OperationSession<'_>, StoreError> {
        let lock_path = self.ensure_operation_lock_file(operation_id)?;
        let lock_file = open_operation_lock_file(&lock_path, operation_id)?;
        match lock_file.try_lock_exclusive() {
            Ok(()) => {}
            Err(error) if lock_contended(&error) => {
                return Err(StoreError::OperationBusy(operation_id.as_str().to_owned()));
            }
            Err(error) => return Err(io_error(error)),
        }
        verify_operation_lock_file(&lock_file, operation_id)?;
        self.recover_interrupted_operations(Some(operation_id))?;
        let generation = self.current_store_generation()?;
        Ok(OperationSession {
            store: self,
            operation_id: operation_id.clone(),
            liveness: OperationLivenessLease {
                lease_nonce: nonce_hex()?,
                owner_process_id: std::process::id(),
            },
            generation,
            _lock_file: lock_file,
        })
    }

    pub(super) fn recover_interrupted_operations(
        &self,
        current_operation_id: Option<&OperationId>,
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
                if operation.operation_liveness.is_none() {
                    return Err(StoreError::Integrity(format!(
                        "pending operation omitted its liveness binding: {}",
                        operation.operation_id.as_str()
                    )));
                }
                let interrupted = if current_operation_id
                    .is_some_and(|operation_id| operation.operation_id == *operation_id)
                {
                    true
                } else {
                    let lock_path = operation_lock_path(&self.state_dir, &operation.operation_id);
                    let file = open_operation_lock_file(&lock_path, &operation.operation_id)?;
                    match file.try_lock_exclusive() {
                        Ok(()) => {
                            verify_operation_lock_file(&file, &operation.operation_id)?;
                            recovered_locks.push(file);
                            true
                        }
                        Err(error) if lock_contended(&error) => false,
                        Err(error) => return Err(io_error(error)),
                    }
                };
                if interrupted {
                    operation.status = GateOperationStatus::Interrupted;
                    operation.interruption_count =
                        operation.interruption_count.checked_add(1).ok_or_else(|| {
                            StoreError::Integrity(
                                "operation interruption count overflow".to_owned(),
                            )
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
            }
            guard.commit(write)?;
            drop(recovered_locks);
            Ok(())
        })
    }

    fn ensure_operation_lock_file(
        &self,
        operation_id: &OperationId,
    ) -> Result<PathBuf, StoreError> {
        self.with_exclusive_lock(|guard| {
            let path = operation_lock_path(&self.state_dir, operation_id);
            guard.mutate(|| {
                ensure_real_file(&path, "operation liveness lock", || {
                    Ok(operation_id.as_str().as_bytes().to_vec())
                })
            })?;
            Ok(path)
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
}

fn operation_lock_path(state_dir: &std::path::Path, operation_id: &OperationId) -> PathBuf {
    state_dir.join(format!(
        "operation-liveness-{}.lock",
        digest_hex(operation_id.as_str().as_bytes())
    ))
}

fn open_operation_lock_file(
    path: &std::path::Path,
    _operation_id: &OperationId,
) -> Result<File, StoreError> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(StoreError::Integrity(
            "operation liveness lock must be a real file".to_owned(),
        ));
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(io_error)
}

fn verify_operation_lock_file(
    mut file: &File,
    operation_id: &OperationId,
) -> Result<(), StoreError> {
    file.seek(SeekFrom::Start(0)).map_err(io_error)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(io_error)?;
    if bytes != operation_id.as_str().as_bytes() {
        return Err(StoreError::Integrity(format!(
            "operation liveness lock identity mismatch: {}",
            operation_id.as_str()
        )));
    }
    Ok(())
}

fn lock_contended(error: &std::io::Error) -> bool {
    let expected = fs2::lock_contended_error();
    error.raw_os_error() == expected.raw_os_error() || error.kind() == expected.kind()
}
