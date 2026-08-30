use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::support::{ProcessResult, determinism, finish_process_output, lumin_command};

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const ADDRESS_ENV: &str = "LUMIN_TEST_NAMESPACE_BARRIER";
const STAGE_ENV: &str = "LUMIN_TEST_NAMESPACE_BARRIER_STAGE";
const BARRIER_WAIT_LIMIT: Duration = Duration::from_secs(30);

pub struct NamespaceBarrier {
    listener: TcpListener,
    stage: &'static str,
}

impl NamespaceBarrier {
    pub fn new(stage: &'static str) -> TestResult<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        Ok(Self { listener, stage })
    }

    pub fn spawn(&self, root: &Path, arguments: &[&str]) -> TestResult<PausedProcess> {
        let arguments = arguments.iter().map(OsString::from).collect::<Vec<_>>();
        let effective_arguments = determinism::effective_arguments(&arguments)?;
        let mut command = lumin_command(root)?;
        command
            .args(&effective_arguments)
            .env(ADDRESS_ENV, self.listener.local_addr()?.to_string())
            .env(STAGE_ENV, self.stage)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Ok(PausedProcess {
            child: Some(command.spawn()?),
            root: root.to_path_buf(),
            effective_arguments,
        })
    }

    pub fn accept(&self, process: &mut PausedProcess) -> TestResult<Permit> {
        let started = Instant::now();
        loop {
            match self.listener.accept() {
                Ok((stream, peer)) if peer.ip().is_loopback() => {
                    stream.set_nonblocking(false)?;
                    stream.set_read_timeout(Some(BARRIER_WAIT_LIMIT))?;
                    let mut frame = String::new();
                    BufReader::new(stream.try_clone()?).read_line(&mut frame)?;
                    assert_eq!(frame.trim_end(), self.stage);
                    return Ok(Permit { stream });
                }
                Ok(_) => {
                    return Err(std::io::Error::other(
                        "namespace barrier accepted a non-loopback peer",
                    )
                    .into());
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
            if process.has_exited()? {
                let output = process.take_output()?;
                return Err(std::io::Error::other(format!(
                    "process exited before {} barrier: status={:?}\nstdout={}\nstderr={}",
                    self.stage,
                    output.status.code(),
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr),
                ))
                .into());
            }
            if started.elapsed() >= BARRIER_WAIT_LIMIT {
                return Err(std::io::Error::other(format!(
                    "process did not reach {} barrier",
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

pub struct PausedProcess {
    child: Option<Child>,
    root: PathBuf,
    effective_arguments: Vec<OsString>,
}

impl PausedProcess {
    pub fn has_exited(&mut self) -> Result<bool, std::io::Error> {
        self.child
            .as_mut()
            .ok_or_else(|| std::io::Error::other("paused process already consumed"))?
            .try_wait()
            .map(|status| status.is_some())
    }

    pub fn finish(mut self) -> TestResult<ProcessResult> {
        let output = self.take_output()?;
        finish_process_output(&self.root, &self.effective_arguments, output)
    }

    fn take_output(&mut self) -> Result<std::process::Output, std::io::Error> {
        self.child
            .take()
            .ok_or_else(|| std::io::Error::other("paused process already consumed"))?
            .wait_with_output()
    }
}

impl Drop for PausedProcess {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
