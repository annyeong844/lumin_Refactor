use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::StoreError;

const ADDRESS_ENV: &str = "LUMIN_TEST_NAMESPACE_BARRIER";
const STAGE_ENV: &str = "LUMIN_TEST_NAMESPACE_BARRIER_STAGE";
const BARRIER_TIMEOUT: Duration = Duration::from_secs(30);
const RELEASE_FRAME: &[u8; 8] = b"release\n";

const AFTER_PRE_ACQUIRE_VALIDATION: &str = "after-pre-acquire-validation";
const AFTER_COMPLETE_VALIDATION: &str = "after-complete-validation";
const BEFORE_STORE_COMMIT: &str = "before-store-commit";
const BEFORE_MIGRATION_STORE_COMMIT: &str = "before-migration-store-commit";
const BEFORE_LATEST_REPLACE: &str = "before-latest-replace";
const BEFORE_RETENTION_COMMIT: &str = "before-retention-commit";
const BEFORE_RUN_RENAME: &str = "before-run-rename";
const BEFORE_RETENTION_MOVE: &str = "before-retention-move";
const BEFORE_CACHE_MOVE: &str = "before-cache-move";
static REACHED: AtomicBool = AtomicBool::new(false);

pub(crate) fn wait_after_pre_acquire_validation() -> Result<(), StoreError> {
    wait(AFTER_PRE_ACQUIRE_VALIDATION)
}

pub(crate) fn wait_after_complete_validation() -> Result<(), StoreError> {
    wait(AFTER_COMPLETE_VALIDATION)
}

pub(crate) fn wait_before_store_commit() -> Result<(), StoreError> {
    wait(BEFORE_STORE_COMMIT)
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
    let (address, selected) = match (std::env::var_os(ADDRESS_ENV), std::env::var_os(STAGE_ENV)) {
        (None, None) => return Ok(()),
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
        return Ok(());
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

    let mut stream = TcpStream::connect(address).map_err(io_error)?;
    stream
        .set_read_timeout(Some(BARRIER_TIMEOUT))
        .map_err(io_error)?;
    stream
        .set_write_timeout(Some(BARRIER_TIMEOUT))
        .map_err(io_error)?;
    stream.write_all(stage.as_bytes()).map_err(io_error)?;
    stream.write_all(b"\n").map_err(io_error)?;
    stream.flush().map_err(io_error)?;

    let mut release = [0_u8; RELEASE_FRAME.len()];
    stream.read_exact(&mut release).map_err(io_error)?;
    if &release != RELEASE_FRAME {
        return Err(StoreError::Integrity(
            "namespace test barrier returned an invalid release frame".to_owned(),
        ));
    }
    Ok(())
}

fn is_supported_stage(stage: &str) -> bool {
    matches!(
        stage,
        AFTER_PRE_ACQUIRE_VALIDATION
            | AFTER_COMPLETE_VALIDATION
            | BEFORE_STORE_COMMIT
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
