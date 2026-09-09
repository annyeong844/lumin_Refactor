use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use crate::StoreError;

const ADDRESS_ENV: &str = "LUMIN_TEST_NAMESPACE_BARRIER";
const STAGE_ENV: &str = "LUMIN_TEST_NAMESPACE_BARRIER_STAGE";
const BARRIER_TIMEOUT: Duration = Duration::from_secs(30);
const RELEASE_FRAME: &[u8; 8] = b"release\n";

const AFTER_PRE_ACQUIRE_VALIDATION: &str = "after-pre-acquire-validation";
const AFTER_COMPLETE_VALIDATION: &str = "after-complete-validation";
const BEFORE_STORE_COMMIT: &str = "before-store-commit";
const BEFORE_LATEST_INDEX_ABORT: &str = "before-latest-index-abort";
const AFTER_LATEST_DERIVATION_OPEN: &str = "after-latest-derivation-open";
const AFTER_LATEST_DERIVATION_OPEN_READ_ERROR: &str = "after-latest-derivation-open-read-error";
const BEFORE_EMPTY_LATEST_RETURN: &str = "before-empty-latest-return";
const BEFORE_EMPTY_LATEST_GENERATION_ERROR: &str = "before-empty-latest-return-generation-error";
const BEFORE_EMPTY_LATEST_RECEIPT_ERROR: &str = "before-empty-latest-return-receipt-error";
const NAMESPACE_LOCK_CONTENDED: &str = "namespace-lock-contended";
const AFTER_ATTEMPT_SESSION_OPEN: &str = "after-attempt-session-open";
const AFTER_ATTEMPT_SESSION_OPEN_READ_ERROR: &str = "after-attempt-session-open-read-error";
const BEFORE_ATTEMPT_SESSION_RETURN: &str = "before-attempt-session-return";
const BEFORE_MIGRATION_STORE_COMMIT: &str = "before-migration-store-commit";
const BEFORE_LATEST_REPLACE: &str = "before-latest-replace";
const BEFORE_RETENTION_COMMIT: &str = "before-retention-commit";
const BEFORE_RUN_RENAME: &str = "before-run-rename";
const BEFORE_RETENTION_MOVE: &str = "before-retention-move";
const BEFORE_CACHE_MOVE: &str = "before-cache-move";
static REACHED: AtomicBool = AtomicBool::new(false);
static SESSION_READ: AtomicUsize = AtomicUsize::new(0);
static LATEST_READ: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn lock_exclusive_for_namespace_test(file: &std::fs::File) -> Result<(), StoreError> {
    use fs2::FileExt;
    if std::env::var_os(STAGE_ENV).as_deref()
        != Some(std::ffi::OsStr::new(NAMESPACE_LOCK_CONTENDED))
    {
        return FileExt::lock_exclusive(file).map_err(crate::io_error);
    }
    match file.try_lock_exclusive() {
        Ok(()) => Ok(()),
        Err(error) if super::lock_contended(&error) => {
            wait(NAMESPACE_LOCK_CONTENDED)?;
            FileExt::lock_exclusive(file).map_err(crate::io_error)
        }
        Err(error) => Err(crate::io_error(error)),
    }
}

pub(crate) fn wait_after_pre_acquire_validation() -> Result<(), StoreError> {
    wait(AFTER_PRE_ACQUIRE_VALIDATION)
}

pub(crate) fn wait_after_complete_validation() -> Result<(), StoreError> {
    wait(AFTER_COMPLETE_VALIDATION)
}

pub(crate) fn wait_before_store_commit() -> Result<(), StoreError> {
    wait(BEFORE_STORE_COMMIT)
}

pub(crate) fn wait_before_latest_index_abort() -> Result<(), StoreError> {
    wait(BEFORE_LATEST_INDEX_ABORT)
}

pub(crate) fn wait_after_latest_derivation_open(
    database: &super::StoreDatabase<'_>,
) -> Result<(), StoreError> {
    let ordinal = LATEST_READ.fetch_add(1, Ordering::SeqCst) + 1;
    wait_for_latest(AFTER_LATEST_DERIVATION_OPEN, ordinal, database)?;
    if wait_for_latest(AFTER_LATEST_DERIVATION_OPEN_READ_ERROR, ordinal, database)? {
        return Err(StoreError::Integrity(
            "injected latest index read failure".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn wait_before_empty_latest_return(
    database: &super::StoreDatabase<'_>,
) -> Result<(), StoreError> {
    for stage in [
        BEFORE_EMPTY_LATEST_RETURN,
        BEFORE_EMPTY_LATEST_GENERATION_ERROR,
        BEFORE_EMPTY_LATEST_RECEIPT_ERROR,
    ] {
        wait_for_latest(stage, LATEST_READ.load(Ordering::SeqCst), database)?;
    }
    Ok(())
}

// Faults are applied only at the SECOND real owner proof, never as the barrier
// callback's result. The public fixture leaves every durable byte/row intact;
// raw-header owner tests independently prove the actual generation/receipt checks.
pub(crate) fn fail_latest_generation_validation_for_test(
    generation: crate::StoreGeneration,
) -> Result<(), StoreError> {
    if latest_final_fault_selected(BEFORE_EMPTY_LATEST_GENERATION_ERROR) {
        return Err(StoreError::StoreGenerationChanged {
            expected: generation,
            observed: generation
                .checked_next()
                .ok_or_else(|| StoreError::Integrity("test generation exhausted".to_owned()))?,
        });
    }
    Ok(())
}

pub(crate) fn fail_latest_receipt_validation_for_test() -> Result<(), StoreError> {
    if latest_final_fault_selected(BEFORE_EMPTY_LATEST_RECEIPT_ERROR) {
        return Err(StoreError::Integrity(
            "injected latest validation receipt failure".to_owned(),
        ));
    }
    Ok(())
}

fn latest_final_fault_selected(stage: &str) -> bool {
    let stage = match LATEST_READ.load(Ordering::SeqCst) {
        1 => stage.to_owned(),
        2 => format!("{stage}:attempt"),
        _ => return false,
    };
    REACHED.load(Ordering::SeqCst)
        && std::env::var_os(STAGE_ENV).as_deref() == Some(std::ffi::OsStr::new(&stage))
}

fn wait_for_latest(
    stage: &str,
    ordinal: usize,
    database: &super::StoreDatabase<'_>,
) -> Result<bool, StoreError> {
    let stage = match ordinal {
        1 => stage.to_owned(),
        2 => format!("{stage}:attempt"),
        _ => return Ok(false),
    };
    wait_with_observation(&stage, || {
        database.complete_logical_observation_for_test().map(Some)
    })
}

pub(crate) fn wait_after_attempt_session_open(
    database: &super::StoreDatabase<'_>,
) -> Result<(), StoreError> {
    let ordinal = SESSION_READ.fetch_add(1, Ordering::SeqCst) + 1;
    wait_for_session(AFTER_ATTEMPT_SESSION_OPEN, ordinal, database)?;
    if wait_for_session(AFTER_ATTEMPT_SESSION_OPEN_READ_ERROR, ordinal, database)? {
        return Err(StoreError::Integrity(
            "injected attempt-session lease read failure".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn wait_before_attempt_session_return(
    database: &super::StoreDatabase<'_>,
) -> Result<(), StoreError> {
    wait_for_session(
        BEFORE_ATTEMPT_SESSION_RETURN,
        SESSION_READ.load(Ordering::SeqCst),
        database,
    )
    .map(|_| ())
}

fn wait_for_session(
    stage: &str,
    ordinal: usize,
    database: &super::StoreDatabase<'_>,
) -> Result<bool, StoreError> {
    let stage = match ordinal {
        1 => stage.to_owned(),
        2 => format!("{stage}:finalize"),
        3 => format!("{stage}:release"),
        _ => return Ok(false),
    };
    wait_with_observation(&stage, || {
        database.complete_logical_observation_for_test().map(Some)
    })
}

pub(crate) fn wait_before_migration_store_commit() -> Result<(), StoreError> {
    wait(BEFORE_MIGRATION_STORE_COMMIT)
}

pub(crate) fn wait_before_latest_replace() -> Result<(), StoreError> {
    wait(BEFORE_LATEST_REPLACE)
}

pub(crate) fn wait_before_retention_commit() -> Result<(), StoreError> {
    wait(BEFORE_RETENTION_COMMIT)
}

pub(crate) fn wait_before_run_rename() -> Result<(), StoreError> {
    wait(BEFORE_RUN_RENAME)
}

pub(crate) fn wait_before_retention_move() -> Result<(), StoreError> {
    wait(BEFORE_RETENTION_MOVE)
}

pub(crate) fn wait_before_cache_move() -> Result<(), StoreError> {
    wait(BEFORE_CACHE_MOVE)
}

fn wait(stage: &str) -> Result<(), StoreError> {
    wait_with_observation(stage, || Ok(None)).map(|_| ())
}

fn wait_with_observation(
    stage: &str,
    observation: impl FnOnce() -> Result<Option<Vec<u8>>, StoreError>,
) -> Result<bool, StoreError> {
    let (address, selected) = match (std::env::var_os(ADDRESS_ENV), std::env::var_os(STAGE_ENV)) {
        (None, None) => return Ok(false),
        (Some(address), Some(selected)) => (address, selected),
        _ => {
            return Err(StoreError::Integrity(format!(
                "{ADDRESS_ENV} and {STAGE_ENV} must both be set or both unset"
            )));
        }
    };
    let selected = selected.into_string().map_err(|_| {
        StoreError::Integrity("namespace test barrier stage is not UTF-8".to_owned())
    })?;
    if !is_supported_stage(&selected) {
        return Err(StoreError::Integrity(format!(
            "unsupported namespace test barrier stage: {selected}"
        )));
    }
    if selected != stage || REACHED.swap(true, Ordering::SeqCst) {
        return Ok(false);
    }

    let address = address.into_string().map_err(|_| {
        StoreError::Integrity("namespace test barrier address is not UTF-8".to_owned())
    })?;
    let address = address.parse::<SocketAddr>().map_err(|error| {
        StoreError::Integrity(format!(
            "namespace test barrier address is malformed: {error}"
        ))
    })?;
    if !address.ip().is_loopback() {
        return Err(StoreError::Integrity(
            "namespace test barrier must use a loopback address".to_owned(),
        ));
    }

    let observation = observation()?;
    let mut stream = TcpStream::connect(address).map_err(io_error)?;
    stream
        .set_read_timeout(Some(BARRIER_TIMEOUT))
        .map_err(io_error)?;
    stream
        .set_write_timeout(Some(BARRIER_TIMEOUT))
        .map_err(io_error)?;
    stream.write_all(stage.as_bytes()).map_err(io_error)?;
    stream.write_all(b"\n").map_err(io_error)?;
    if let Some(bytes) = observation {
        let length = u64::try_from(bytes.len()).map_err(|_| {
            StoreError::Integrity("namespace observation length overflow".to_owned())
        })?;
        stream.write_all(&length.to_be_bytes()).map_err(io_error)?;
        stream.write_all(&bytes).map_err(io_error)?;
    }
    stream.flush().map_err(io_error)?;

    let mut release = [0_u8; RELEASE_FRAME.len()];
    stream.read_exact(&mut release).map_err(io_error)?;
    if &release != RELEASE_FRAME {
        return Err(StoreError::Integrity(
            "namespace test barrier returned an invalid release frame".to_owned(),
        ));
    }
    Ok(true)
}

fn is_supported_stage(stage: &str) -> bool {
    if let Some((base, phase)) = stage.split_once(':') {
        if phase == "attempt" {
            return matches!(
                base,
                AFTER_LATEST_DERIVATION_OPEN
                    | AFTER_LATEST_DERIVATION_OPEN_READ_ERROR
                    | BEFORE_EMPTY_LATEST_RETURN
                    | BEFORE_EMPTY_LATEST_GENERATION_ERROR
                    | BEFORE_EMPTY_LATEST_RECEIPT_ERROR
            );
        }
        return matches!(phase, "finalize" | "release")
            && matches!(
                base,
                AFTER_ATTEMPT_SESSION_OPEN
                    | AFTER_ATTEMPT_SESSION_OPEN_READ_ERROR
                    | BEFORE_ATTEMPT_SESSION_RETURN
            );
    }
    matches!(
        stage,
        AFTER_PRE_ACQUIRE_VALIDATION
            | AFTER_COMPLETE_VALIDATION
            | BEFORE_STORE_COMMIT
            | BEFORE_LATEST_INDEX_ABORT
            | AFTER_LATEST_DERIVATION_OPEN
            | AFTER_LATEST_DERIVATION_OPEN_READ_ERROR
            | BEFORE_EMPTY_LATEST_RETURN
            | BEFORE_EMPTY_LATEST_GENERATION_ERROR
            | BEFORE_EMPTY_LATEST_RECEIPT_ERROR
            | NAMESPACE_LOCK_CONTENDED
            | AFTER_ATTEMPT_SESSION_OPEN
            | AFTER_ATTEMPT_SESSION_OPEN_READ_ERROR
            | BEFORE_ATTEMPT_SESSION_RETURN
            | BEFORE_MIGRATION_STORE_COMMIT
            | BEFORE_LATEST_REPLACE
            | BEFORE_RETENTION_COMMIT
            | BEFORE_RUN_RENAME
            | BEFORE_RETENTION_MOVE
            | BEFORE_CACHE_MOVE
    )
}

fn io_error(error: std::io::Error) -> StoreError {
    StoreError::Io(format!("namespace test barrier failed: {error}"))
}
