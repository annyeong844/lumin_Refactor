use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::support::{ProcessResult, lumin_command};

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const ADDRESS_ENV: &str = "LUMIN_TEST_LIFECYCLE_MIGRATION_BARRIER";
const STAGE_ENV: &str = "LUMIN_TEST_LIFECYCLE_MIGRATION_STAGE";
const BARRIER_WAIT_LIMIT: Duration = Duration::from_secs(30);

pub struct LifecycleMigrationBarrier {
    listener: TcpListener,
    stage: &'static str,
}

impl LifecycleMigrationBarrier {
    pub fn new(stage: &'static str) -> TestResult<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        Ok(Self { listener, stage })
    }

    pub fn spawn(&self, root: &Path) -> TestResult<PausedMigration> {
        let mut command = lumin_command(root)?;
        command
            .args(["store", "migrate"])
            .env(ADDRESS_ENV, self.listener.local_addr()?.to_string())
            .env(STAGE_ENV, self.stage)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Ok(PausedMigration::from_child(command.spawn()?))
    }

    pub fn accept(&self, process: &mut PausedMigration) -> TestResult<MigrationPermit> {
        let started = Instant::now();
        loop {
            match self.listener.accept() {
                Ok((stream, peer)) if peer.ip().is_loopback() => {
                    stream.set_nonblocking(false)?;
                    stream.set_read_timeout(Some(BARRIER_WAIT_LIMIT))?;
                    let mut frame = String::new();
                    BufReader::new(stream.try_clone()?).read_line(&mut frame)?;
                    assert_eq!(frame.trim_end(), self.stage);
                    return Ok(MigrationPermit { stream });
                }
                Ok(_) => {
                    return Err(std::io::Error::other(
                        "migration barrier accepted a non-loopback peer",
                    )
                    .into());
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
            if process.has_exited()? {
                let output = process.take_output()?;
                return Err(std::io::Error::other(format!(
                    "migration exited before {} barrier: status={:?}\nstdout={}\nstderr={}",
                    self.stage,
                    output.status.code(),
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr),
                ))
                .into());
            }
            if started.elapsed() >= BARRIER_WAIT_LIMIT {
                return Err(std::io::Error::other(format!(
                    "migration did not reach {} barrier",
                    self.stage
                ))
                .into());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

pub struct MigrationPermit {
    stream: TcpStream,
}

impl MigrationPermit {
    pub fn release(mut self) -> TestResult {
        self.stream.write_all(b"release\n")?;
        Ok(())
    }
}

pub struct PausedMigration {
    child: Option<Child>,
}

impl PausedMigration {
    fn from_child(child: Child) -> Self {
        Self { child: Some(child) }
    }

    fn has_exited(&mut self) -> Result<bool, std::io::Error> {
        self.child
            .as_mut()
            .ok_or_else(|| std::io::Error::other("paused migration child already consumed"))?
            .try_wait()
            .map(|status| status.is_some())
    }

    pub fn terminate(mut self) -> TestResult<ProcessResult> {
        self.child
            .as_mut()
            .ok_or_else(|| std::io::Error::other("paused migration child already consumed"))?
            .kill()?;
        let output = self.take_output()?;
        process_result(output)
    }

    pub fn finish(mut self) -> TestResult<ProcessResult> {
        let output = self.take_output()?;
        process_result(output)
    }

    fn take_output(&mut self) -> Result<std::process::Output, std::io::Error> {
        self.child
            .take()
            .ok_or_else(|| std::io::Error::other("paused migration child already consumed"))?
            .wait_with_output()
    }
}

impl Drop for PausedMigration {
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
