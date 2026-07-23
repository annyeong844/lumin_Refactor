use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::support::ProcessResult;

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const BARRIER_WAIT_LIMIT: Duration = Duration::from_secs(30);

pub struct PublicationBarrier {
    listener: TcpListener,
    environment: &'static str,
    label: &'static str,
}

impl PublicationBarrier {
    pub fn new(environment: &'static str, label: &'static str) -> TestResult<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            environment,
            label,
        })
    }

    pub fn spawn_audit(
        &self,
        root: &Path,
        additional: &[&PublicationBarrier],
    ) -> TestResult<PausedAudit> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_lumin"));
        command
            .current_dir(root)
            .args(["audit", "--jobs", "1"])
            .env(self.environment, self.address()?)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for barrier in additional {
            command.env(barrier.environment, barrier.address()?);
        }
        let child = command.spawn()?;
        Ok(PausedAudit { child: Some(child) })
    }

    pub fn accept(
        &self,
        process: &mut PausedAudit,
        expected_attempt_id: &str,
    ) -> TestResult<Permit> {
        let started = Instant::now();
        loop {
            match self.try_accept()? {
                Some(stream) => {
                    return Permit::new(stream, self.label, expected_attempt_id);
                }
                None => {
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
            }
        }
    }

    pub fn try_accept(&self) -> Result<Option<TcpStream>, std::io::Error> {
        match self.listener.accept() {
            Ok((stream, peer)) if peer.ip().is_loopback() => Ok(Some(stream)),
            Ok(_) => Err(std::io::Error::other(
                "publication barrier accepted a non-loopback peer",
            )),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error),
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
    fn new(stream: TcpStream, label: &str, expected_attempt_id: &str) -> TestResult<Self> {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(BARRIER_WAIT_LIMIT))?;
        let mut frame = String::new();
        BufReader::new(stream.try_clone()?).read_line(&mut frame)?;
        assert_eq!(frame.trim_end(), format!("{label} {expected_attempt_id}"));
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
    pub fn has_exited(&mut self) -> Result<bool, std::io::Error> {
        self.child
            .as_mut()
            .ok_or_else(|| std::io::Error::other("paused audit child already consumed"))?
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
            .ok_or_else(|| std::io::Error::other("paused audit child already consumed"))?
            .wait_with_output()
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
