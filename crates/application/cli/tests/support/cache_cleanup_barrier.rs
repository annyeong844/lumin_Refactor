use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::support::{ProcessResult, lumin_command};

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const BARRIER_WAIT_LIMIT: Duration = Duration::from_secs(30);

pub struct CacheCleanupBarrier {
    listener: TcpListener,
    environment: &'static str,
    stage: &'static str,
}

impl CacheCleanupBarrier {
    pub fn new(environment: &'static str, stage: &'static str) -> TestResult<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            environment,
            stage,
        })
    }

    pub fn spawn(&self, root: &Path, operation_id: &str) -> TestResult<PausedCleanup> {
        self.spawn_with_barrier(root, operation_id, None)
    }

    pub fn spawn_with_barrier(
        &self,
        root: &Path,
        operation_id: &str,
        additional: Option<&Self>,
    ) -> TestResult<PausedCleanup> {
        let mut command = lumin_command(root)?;
        command
            .args(["cache", "clean", "--operation-id", operation_id])
            .env(self.environment, self.listener.local_addr()?.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(additional) = additional {
            command.env(
                additional.environment,
                additional.listener.local_addr()?.to_string(),
            );
        }
        Ok(PausedCleanup::from_child(command.spawn()?))
    }

    pub fn accept(&self, process: &mut PausedCleanup, operation_id: &str) -> TestResult<Permit> {
        let started = Instant::now();
        loop {
            match self.listener.accept() {
                Ok((stream, peer)) if peer.ip().is_loopback() => {
                    stream.set_nonblocking(false)?;
                    stream.set_read_timeout(Some(BARRIER_WAIT_LIMIT))?;
                    let mut frame = String::new();
                    BufReader::new(stream.try_clone()?).read_line(&mut frame)?;
                    assert_eq!(frame.trim_end(), format!("{} {operation_id} 0", self.stage));
                    return Ok(Permit { stream });
                }
                Ok(_) => {
                    return Err(std::io::Error::other(
                        "cache cleanup barrier accepted a non-loopback peer",
                    )
                    .into());
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
            if process.has_exited()? {
                let output = process.take_output()?;
                return Err(std::io::Error::other(format!(
                    "cleanup exited before {} barrier: status={:?}\nstdout={}\nstderr={}",
                    self.stage,
                    output.status.code(),
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr),
                ))
                .into());
            }
            if started.elapsed() >= BARRIER_WAIT_LIMIT {
                return Err(std::io::Error::other(format!(
                    "cleanup did not reach {} barrier",
                    self.stage
                ))
                .into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

pub struct Permit {
    stream: TcpStream,
}

impl Permit {
    pub fn release(mut self) -> TestResult {
        self.stream.write_all(b"release\n")?;
        Ok(())
    }
}

pub struct PausedCleanup {
    child: Option<Child>,
}

impl PausedCleanup {
    pub fn from_child(child: Child) -> Self {
        Self { child: Some(child) }
    }

    pub fn has_exited(&mut self) -> Result<bool, std::io::Error> {
        self.child
            .as_mut()
            .ok_or_else(|| std::io::Error::other("paused cleanup child already consumed"))?
            .try_wait()
            .map(|status| status.is_some())
    }

    pub fn finish(mut self) -> TestResult<ProcessResult> {
        let output = self.take_output()?;
        Ok(ProcessResult {
            status: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8(output.stdout)?,
            stderr: String::from_utf8(output.stderr)?,
        })
    }

    pub fn take_output(&mut self) -> Result<std::process::Output, std::io::Error> {
        self.child
            .take()
            .ok_or_else(|| std::io::Error::other("paused cleanup child already consumed"))?
            .wait_with_output()
    }
}

impl Drop for PausedCleanup {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
