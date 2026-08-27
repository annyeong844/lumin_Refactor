use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use crate::{StoreError, io_error};

const ADDRESS_ENV: &str = "LUMIN_TEST_LIFECYCLE_MIGRATION_BARRIER";
const STAGE_ENV: &str = "LUMIN_TEST_LIFECYCLE_MIGRATION_STAGE";
const BARRIER_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) fn wait(stage: &str) -> Result<(), StoreError> {
    let Some(selected) = std::env::var_os(STAGE_ENV) else {
        return Ok(());
    };
    let selected = selected.into_string().map_err(|_| {
        StoreError::Integrity("lifecycle migration barrier stage is not UTF-8".to_owned())
    })?;
    if selected != stage {
        return Ok(());
    }
    let address = std::env::var(ADDRESS_ENV)
        .map_err(|_| {
            StoreError::Integrity("lifecycle migration barrier omitted its address".to_owned())
        })?
        .parse::<SocketAddr>()
        .map_err(|error| {
            StoreError::Integrity(format!(
                "lifecycle migration barrier address is invalid: {error}"
            ))
        })?;
    if !address.ip().is_loopback() {
        return Err(StoreError::Integrity(
            "lifecycle migration barrier requires a loopback address".to_owned(),
        ));
    }
    let mut stream = TcpStream::connect_timeout(&address, BARRIER_TIMEOUT).map_err(io_error)?;
    stream
        .set_read_timeout(Some(BARRIER_TIMEOUT))
        .map_err(io_error)?;
    stream
        .set_write_timeout(Some(BARRIER_TIMEOUT))
        .map_err(io_error)?;
    writeln!(stream, "{stage}").map_err(io_error)?;
    stream.flush().map_err(io_error)?;
    let mut response = String::new();
    BufReader::new(stream)
        .read_line(&mut response)
        .map_err(io_error)?;
    if response.trim_end() != "release" {
        return Err(StoreError::Integrity(
            "lifecycle migration barrier received an invalid release frame".to_owned(),
        ));
    }
    Ok(())
}
