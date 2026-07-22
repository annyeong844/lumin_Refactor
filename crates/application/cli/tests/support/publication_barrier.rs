use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::support::ProcessResult;

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const BARRIER_ENV: &str = "LUMIN_TEST_PUBLICATION_BARRIER";
const BARRIER_WAIT_LIMIT: Duration = Duration::from_secs(30);

pub struct PublicationBarrier {
    listener: TcpListener,
}

impl PublicationBarrier {
    pub fn new() -> TestResult<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        Ok(Self { listener })
    }

    pub fn spawn_audit(&self, root: &Path) -> TestResult<PausedAudit> {
        let child = Command::new(env!("CARGO_BIN_EXE_lumin"))
            .current_dir(root)
            .args(["audit", "--jobs", "1"])
            .env(BARRIER_ENV, self.address()?)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        Ok(PausedAudit { child: Some(child) })
    }

    pub fn accept(
        &self,
        process: &mut PausedAudit,
        expected_attempt_id: &str,
    ) -> TestResult<Permit> {
        let started = Instant::now();
        loop {
            match self.listener.accept() {
                Ok((stream, peer)) => {
                    assert!(peer.ip().is_loopback());
                    return Permit::new(stream, expected_attempt_id);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if process.has_exited()? {
                        return Err(std::io::Error::other(
                            "audit exited before reaching the publication barrier",
                        )
                        .into());
                    }
                    if started.elapsed() >= BARRIER_WAIT_LIMIT {
                        return Err(std::io::Error::other(
                            "audit did not reach the publication barrier",
                        )
                        .into());
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn address(&self) -> TestResult<String> {
        self.listener
            .local_addr()
            .map(|address| address.to_string())
            .map_err(Into::into)
    }
}

pub struct Permit {
    stream: TcpStream,
}

impl Permit {
    fn new(stream: TcpStream, expected_attempt_id: &str) -> TestResult<Self> {
        stream.set_read_timeout(Some(BARRIER_WAIT_LIMIT))?;
        let mut attempt_id = String::new();
        BufReader::new(stream.try_clone()?).read_line(&mut attempt_id)?;
        assert_eq!(attempt_id.trim_end(), expected_attempt_id);
        Ok(Self { stream })
    }

    pub fn release(mut self) -> TestResult {
        self.stream.write_all(b"release\n")?;
        Ok(())
    }
}

pub struct PausedAudit {
    child: Option<Child>,
}

impl PausedAudit {
    fn has_exited(&mut self) -> Result<bool, std::io::Error> {
        self.child
            .as_mut()
            .ok_or_else(|| std::io::Error::other("paused audit child already consumed"))?
            .try_wait()
            .map(|status| status.is_some())
    }

    pub fn finish(mut self) -> TestResult<ProcessResult> {
        let output = self
            .child
            .take()
            .ok_or_else(|| std::io::Error::other("paused audit child already consumed"))?
            .wait_with_output()?;
        Ok(ProcessResult {
            status: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8(output.stdout)?,
            stderr: String::from_utf8(output.stderr)?,
        })
    }
}

impl Drop for PausedAudit {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
