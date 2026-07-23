use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use lumin_model::AttemptId;

use crate::StoreError;

const PREPARED_BARRIER_ENV: &str = "LUMIN_TEST_PUBLICATION_PREPARED_BARRIER";
const CONTENDED_BARRIER_ENV: &str = "LUMIN_TEST_PUBLICATION_CONTENDED_BARRIER";
const GUARDED_BARRIER_ENV: &str = "LUMIN_TEST_PUBLICATION_GUARDED_BARRIER";
const BARRIER_TIMEOUT: Duration = Duration::from_secs(30);
const RELEASE_FRAME: &[u8; 8] = b"release\n";

pub(super) fn wait_prepared(attempt_id: &AttemptId) -> Result<(), StoreError> {
    wait(PREPARED_BARRIER_ENV, "prepared", attempt_id)
}

pub(super) fn wait_guarded(attempt_id: &AttemptId) -> Result<(), StoreError> {
    wait(GUARDED_BARRIER_ENV, "guarded", attempt_id)
}

pub(super) fn wait_contended(attempt_id: &AttemptId) -> Result<(), StoreError> {
    wait(CONTENDED_BARRIER_ENV, "contended", attempt_id)
}

fn wait(environment: &str, stage: &str, attempt_id: &AttemptId) -> Result<(), StoreError> {
    let Some(raw_address) = std::env::var_os(environment) else {
        return Ok(());
    };
    let raw_address = raw_address.into_string().map_err(|_| {
        StoreError::Integrity("publication test barrier address is not UTF-8".to_owned())
    })?;
    let address = raw_address.parse::<SocketAddr>().map_err(|error| {
        StoreError::Integrity(format!(
            "publication test barrier address is malformed: {error}"
        ))
    })?;
    if !address.ip().is_loopback() {
        return Err(StoreError::Integrity(
            "publication test barrier must use a loopback address".to_owned(),
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
    stream.write_all(b" ").map_err(io_error)?;
    stream
        .write_all(attempt_id.as_str().as_bytes())
        .map_err(io_error)?;
    stream.write_all(b"\n").map_err(io_error)?;
    stream.flush().map_err(io_error)?;

    let mut release = [0_u8; RELEASE_FRAME.len()];
    stream.read_exact(&mut release).map_err(io_error)?;
    if &release != RELEASE_FRAME {
        return Err(StoreError::Integrity(
            "publication test barrier returned an invalid release frame".to_owned(),
        ));
    }
    Ok(())
}

fn io_error(error: std::io::Error) -> StoreError {
    StoreError::Io(format!("publication test barrier failed: {error}"))
}
