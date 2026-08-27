use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::support::{ProcessResult, lumin_command};

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const ADDRESS_ENV: &str = "LUMIN_TEST_CACHE_CLEANUP_DELIVERY_BARRIER";
const STAGE_ENV: &str = "LUMIN_TEST_CACHE_CLEANUP_DELIVERY_STAGE";
const BARRIER_WAIT_LIMIT: Duration = Duration::from_secs(30);

pub struct CacheCleanupDeliveryBarrier {
    listener: TcpListener,
    stage: &'static str,
}

impl CacheCleanupDeliveryBarrier {
    pub fn new(stage: &'static str) -> TestResult<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        Ok(Self { listener, stage })
    }

    pub fn spawn(&self, root: &Path, operation_id: &str) -> TestResult<PausedDelivery> {
        let mut command = lumin_command(root)?;
        command
            .args(["cache", "clean", "--operation-id", operation_id])
            .env(ADDRESS_ENV, self.listener.local_addr()?.to_string())
            .env(STAGE_ENV, self.stage)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Ok(PausedDelivery::from_child(command.spawn()?))
    }

    pub fn accept(
        &self,
        process: &mut PausedDelivery,
        operation_id: &str,
    ) -> TestResult<(u64, DeliveryPermit)> {
        let started = Instant::now();
        loop {
            match self.listener.accept() {
                Ok((stream, peer)) if peer.ip().is_loopback() => {
                    stream.set_nonblocking(false)?;
                    stream.set_read_timeout(Some(BARRIER_WAIT_LIMIT))?;
                    let mut frame = String::new();
                    BufReader::new(stream.try_clone()?).read_line(&mut frame)?;
                    let mut fields = frame.split_whitespace();
                    assert_eq!(fields.next(), Some(self.stage));
                    assert_eq!(fields.next(), Some(operation_id));
                    let sequence = fields
                        .next()
                        .ok_or_else(|| std::io::Error::other("delivery barrier omitted sequence"))?
                        .parse::<u64>()?;
                    assert!(fields.next().is_none());
                    return Ok((sequence, DeliveryPermit { stream }));
                }
                Ok(_) => {
                    return Err(std::io::Error::other(
                        "delivery barrier accepted a non-loopback peer",
                    )
                    .into());
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
            if process.has_exited()? {
                let output = process.take_output()?;
                return Err(std::io::Error::other(format!(
                    "cleanup exited before {} delivery barrier: status={:?}\nstdout={}\nstderr={}",
                    self.stage,
                    output.status.code(),
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr),
                ))
                .into());
            }
            if started.elapsed() >= BARRIER_WAIT_LIMIT {
                return Err(std::io::Error::other(format!(
                    "cleanup did not reach {} delivery barrier",
                    self.stage
                ))
                .into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

pub struct DeliveryPermit {
    stream: TcpStream,
}

impl DeliveryPermit {
    pub fn release(mut self) -> TestResult {
        self.stream.write_all(b"release\n")?;
        Ok(())
    }
}

pub struct PausedDelivery {
    child: Option<Child>,
}

impl PausedDelivery {
    fn from_child(child: Child) -> Self {
        Self { child: Some(child) }
    }

    fn has_exited(&mut self) -> Result<bool, std::io::Error> {
        self.child
            .as_mut()
            .ok_or_else(|| std::io::Error::other("paused delivery child already consumed"))?
            .try_wait()
            .map(|status| status.is_some())
    }

    pub fn finish(mut self) -> TestResult<ProcessResult> {
        let output = self.take_output()?;
        process_result(output)
    }

    #[allow(dead_code)]
    pub fn terminate(mut self) -> TestResult<ProcessResult> {
        self.child
            .as_mut()
            .ok_or_else(|| std::io::Error::other("paused delivery child already consumed"))?
            .kill()?;
        let output = self.take_output()?;
        process_result(output)
    }

    fn take_output(&mut self) -> Result<std::process::Output, std::io::Error> {
        self.child
            .take()
            .ok_or_else(|| std::io::Error::other("paused delivery child already consumed"))?
            .wait_with_output()
    }
}

impl Drop for PausedDelivery {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn process_result(output: std::process::Output) -> TestResult<ProcessResult> {
    Ok(ProcessResult {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8(output.stdout)?,
        stderr: String::from_utf8(output.stderr)?,
    })
}
