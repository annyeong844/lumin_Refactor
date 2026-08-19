use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use lumin_model::OperationId;

use crate::StoreError;

const INTERRUPTED_BARRIER_ENV: &str = "LUMIN_TEST_CACHE_CLEANUP_INTERRUPTED_BARRIER";
const MOVE_BARRIER_ENV: &str = "LUMIN_TEST_CACHE_CLEANUP_MOVE_BARRIER";
const DURABILITY_BARRIER_ENV: &str = "LUMIN_TEST_CACHE_CLEANUP_DURABILITY_BARRIER";
const BARRIER_TIMEOUT: Duration = Duration::from_secs(30);
const RELEASE_FRAME: &[u8; 8] = b"release\n";
static MOVE_BARRIER_USED: AtomicBool = AtomicBool::new(false);
static DURABILITY_BARRIER_USED: AtomicBool = AtomicBool::new(false);

pub(super) fn wait_interrupted(operation_id: &OperationId) -> Result<(), StoreError> {
    wait(INTERRUPTED_BARRIER_ENV, "interrupted", operation_id, None)
}

pub(super) fn wait_before_move(operation_id: &OperationId, ordinal: u64) -> Result<(), StoreError> {
    if MOVE_BARRIER_USED.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    wait(MOVE_BARRIER_ENV, "before-move", operation_id, Some(ordinal))
}

pub(super) fn wait_after_initial_flush(
    operation_id: &OperationId,
    ordinal: u64,
) -> Result<(), StoreError> {
    if DURABILITY_BARRIER_USED.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    wait(
        DURABILITY_BARRIER_ENV,
        "after-initial-flush",
        operation_id,
        Some(ordinal),
    )
}

fn wait(
    environment: &str,
    stage: &str,
    operation_id: &OperationId,
    ordinal: Option<u64>,
) -> Result<(), StoreError> {
    let Some(raw_address) = std::env::var_os(environment) else {
        return Ok(());
    };
    let raw_address = raw_address.into_string().map_err(|_| {
        StoreError::Integrity("cache cleanup test barrier address is not UTF-8".to_owned())
    })?;
    let address = raw_address.parse::<SocketAddr>().map_err(|error| {
        StoreError::Integrity(format!(
            "cache cleanup test barrier address is malformed: {error}"
        ))
    })?;
    if !address.ip().is_loopback() {
        return Err(StoreError::Integrity(
            "cache cleanup test barrier must use a loopback address".to_owned(),
        ));
    }

    let mut stream = TcpStream::connect(address).map_err(io_error)?;
    stream
        .set_read_timeout(Some(BARRIER_TIMEOUT))
        .map_err(io_error)?;
    stream
        .set_write_timeout(Some(BARRIER_TIMEOUT))
        .map_err(io_error)?;
    write!(stream, "{stage} {}", operation_id.as_str()).map_err(io_error)?;
    if let Some(ordinal) = ordinal {
        write!(stream, " {ordinal}").map_err(io_error)?;
    }
    stream.write_all(b"\n").map_err(io_error)?;
    stream.flush().map_err(io_error)?;

    let mut release = [0_u8; RELEASE_FRAME.len()];
    stream.read_exact(&mut release).map_err(io_error)?;
    if &release != RELEASE_FRAME {
        return Err(StoreError::Integrity(
            "cache cleanup test barrier returned an invalid release frame".to_owned(),
        ));
    }
    Ok(())
}

fn io_error(error: std::io::Error) -> StoreError {
    StoreError::Io(format!("cache cleanup test barrier failed: {error}"))
}
