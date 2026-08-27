use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use lumin_model::OperationId;

const ADDRESS_ENV: &str = "LUMIN_TEST_CACHE_CLEANUP_DELIVERY_BARRIER";
const STAGE_ENV: &str = "LUMIN_TEST_CACHE_CLEANUP_DELIVERY_STAGE";
const BARRIER_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Stage {
    Allocation,
    PartialStdout,
    CompleteStdout,
}

impl Stage {
    fn as_str(self) -> &'static str {
        match self {
            Self::Allocation => "after-allocation",
            Self::PartialStdout => "after-partial-stdout",
            Self::CompleteStdout => "after-complete-stdout",
        }
    }
}

pub(super) fn selected(stage: Stage) -> Result<bool, std::io::Error> {
    let Some(value) = std::env::var_os(STAGE_ENV) else {
        return Ok(false);
    };
    let value = value.into_string().map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "cache cleanup delivery barrier stage is not UTF-8",
        )
    })?;
    match value.as_str() {
        "after-allocation" | "after-partial-stdout" | "after-complete-stdout" => {
            Ok(value == stage.as_str())
        }
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("unsupported cache cleanup delivery barrier stage: {value}"),
        )),
    }
}

pub(super) fn wait(
    stage: Stage,
    operation_id: &OperationId,
    sequence: u64,
) -> Result<(), std::io::Error> {
    if !selected(stage)? {
        return Ok(());
    }
    let address = std::env::var(ADDRESS_ENV)
        .map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "cache cleanup delivery barrier omitted its address",
            )
        })?
        .parse::<SocketAddr>()
        .map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("cache cleanup delivery barrier address is invalid: {error}"),
            )
        })?;
    if !address.ip().is_loopback() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "cache cleanup delivery barrier requires a loopback address",
        ));
    }
    let mut stream = TcpStream::connect_timeout(&address, BARRIER_TIMEOUT)?;
    stream.set_write_timeout(Some(BARRIER_TIMEOUT))?;
    writeln!(
        stream,
        "{} {} {sequence}",
        stage.as_str(),
        operation_id.as_str()
    )?;
    stream.flush()?;
    let mut response = String::new();
    BufReader::new(stream).read_line(&mut response)?;
    if response.trim_end() != "release" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "cache cleanup delivery barrier received an invalid release frame",
        ));
    }
    Ok(())
}
