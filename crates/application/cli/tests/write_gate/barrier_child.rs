use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, ExitStatus, Output};
use std::thread;
use std::time::{Duration, Instant};

const ARRIVAL_WATCHDOG: Duration = Duration::from_secs(30);

/// Own the child through barrier failures and assertion unwinding. Diagnostics
/// are libtest output only; neither timing nor process identity is an oracle.
pub(super) struct BarrierChild {
    child: Option<Child>,
    description: String,
    started: Instant,
    #[cfg(test)]
    collected_output: Option<std::rc::Rc<std::cell::RefCell<Option<Output>>>>,
}

impl BarrierChild {
    pub(super) fn spawn(command: &mut Command, label: &str) -> io::Result<Self> {
        let child = command.spawn()?;
        let started = Instant::now();
        let description = format!(
            "{label}: pid={} command={:?} arguments={:?}",
            child.id(),
            command.get_program(),
            command.get_args().collect::<Vec<_>>(),
        );
        eprintln!("[gate-barrier] spawned {description}");
        Ok(Self {
            child: Some(child),
            description,
            started,
            #[cfg(test)]
            collected_output: None,
        })
    }

    pub(super) fn accept(&mut self, listener: &TcpListener) -> io::Result<(TcpStream, SocketAddr)> {
        self.accept_until(listener, Instant::now() + ARRIVAL_WATCHDOG)
    }

    fn accept_until(
        &mut self,
        listener: &TcpListener,
        deadline: Instant,
    ) -> io::Result<(TcpStream, SocketAddr)> {
        loop {
            match listener.accept() {
                Ok(accepted) => {
                    eprintln!(
                        "[gate-barrier] arrived {} elapsed={:?}",
                        self.description,
                        self.started.elapsed(),
                    );
                    return Ok(accepted);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(self.failure(&format!("cannot accept barrier: {error}"))),
            }
            let status = self.child_mut()?.try_wait();
            match status {
                Ok(Some(_)) => return Err(self.failure("child exited before reaching barrier")),
                Ok(None) => {}
                Err(error) => {
                    return Err(self.failure(&format!("cannot inspect child: {error}")));
                }
            }
            if Instant::now() >= deadline {
                return Err(self.failure("child did not reach barrier before arrival watchdog"));
            }
            // Poll pacing only: the accepted TCP frame controls the test turn.
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn failure(&mut self, reason: &str) -> io::Error {
        let context = format!(
            "{}: {reason}; elapsed={:?}; watchdog={ARRIVAL_WATCHDOG:?}",
            self.description,
            self.started.elapsed(),
        );
        match self.stop_and_collect() {
            Ok((before_stop, output)) => io::Error::other(format!(
                "{context}; statusBeforeStop={:?}; statusCode={:?}\nstdout={}\nstderr={}",
                before_stop.map(|status| status.code()),
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            )),
            Err(error) => io::Error::other(format!(
                "{context}; cannot terminate/reap and collect child output: {error}"
            )),
        }
    }

    fn child_mut(&mut self) -> io::Result<&mut Child> {
        self.child
            .as_mut()
            .ok_or_else(|| io::Error::other("gate barrier child already consumed"))
    }

    fn stop_and_collect(&mut self) -> io::Result<(Option<ExitStatus>, Output)> {
        let child = self.child_mut()?;
        let before_stop = child.try_wait()?;
        if before_stop.is_none()
            && let Err(error) = child.kill()
            && child.try_wait()?.is_none()
        {
            return Err(error);
        }
        Ok((before_stop, self.take_output()?))
    }

    fn take_output(&mut self) -> io::Result<Output> {
        let output = self
            .child
            .take()
            .ok_or_else(|| io::Error::other("gate barrier child already consumed"))?
            .wait_with_output()?;
        #[cfg(test)]
        if let Some(receipt) = &self.collected_output {
            *receipt.borrow_mut() = Some(output.clone());
        }
        Ok(output)
    }

    pub(super) fn wait_with_output(mut self) -> io::Result<Output> {
        let output = self.take_output()?;
        eprintln!(
            "[gate-barrier] finished {} elapsed={:?} statusCode={:?}",
            self.description,
            self.started.elapsed(),
            output.status.code(),
        );
        Ok(output)
    }
}

impl Drop for BarrierChild {
    fn drop(&mut self) {
        if self.child.is_some() {
            let diagnostic = self.failure("scope ended before child completion");
            eprintln!("[gate-barrier] {diagnostic}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::lumin_command;
    use std::io::{BufRead, BufReader, Read};
    use std::net::Ipv4Addr;
    use std::process::Stdio;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn listener() -> io::Result<TcpListener> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        Ok(listener)
    }

    #[test]
    fn early_error_exit_retains_the_request_and_stderr() -> TestResult {
        let root = tempfile::tempdir()?;
        let mut command = lumin_command(root.path())?;
        command
            .args([
                "pre-write",
                "--operation-id",
                "op-barrier-malformed",
                "--path",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut process = BarrierChild::spawn(&mut command, "early error")?;
        let pid = process.child_mut()?.id();
        let failure = process
            .accept(&listener()?)
            .err()
            .ok_or("missing failure")?;
        let message = failure.to_string();
        assert!(
            message.contains("child exited before reaching barrier"),
            "{message}"
        );
        assert!(message.contains("op-barrier-malformed"), "{message}");
        assert!(message.contains(&format!("pid={pid}")), "{message}");
        assert!(message.contains("statusCode=Some(2)"), "{message}");
        assert!(
            message.contains("stderr=lumin: missing value for --path\n"),
            "{message}",
        );
        assert!(process.child.is_none());
        assert!(!root.path().join(".lumin").exists());
        Ok(())
    }

    #[test]
    fn early_success_is_a_barrier_failure_with_stdout() -> TestResult {
        let root = tempfile::tempdir()?;
        let mut command = lumin_command(root.path())?;
        command
            .arg("help-agent")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut process = BarrierChild::spawn(&mut command, "early success")?;
        let failure = process
            .accept(&listener()?)
            .err()
            .ok_or("missing failure")?;
        let message = failure.to_string();
        assert!(
            message.contains("child exited before reaching barrier"),
            "{message}"
        );
        assert!(message.contains("statusCode=Some(0)"), "{message}");
        assert!(
            message.contains("stdout=Lumin agent workflow\n"),
            "{message}"
        );
        assert!(process.child.is_none());
        assert!(!root.path().join(".lumin").exists());
        Ok(())
    }

    #[test]
    fn expired_watchdog_reaps_the_child_held_at_an_exact_barrier() -> TestResult {
        let (_root, mut process, mut reader) = held_child()?;

        // The child is now held by the explicit admission frame, not by timing.
        // Expire only the test clock input for an unconnected final barrier.
        let failure = process
            .accept_until(&listener()?, Instant::now())
            .err()
            .ok_or("missing failure")?;
        let message = failure.to_string();
        assert!(
            message.contains("child did not reach barrier before arrival watchdog"),
            "{message}",
        );
        assert!(message.contains("statusBeforeStop=None"), "{message}");
        assert!(message.contains("op-watchdog"), "{message}");
        assert!(message.ends_with("stdout=\nstderr="), "{message}");
        assert!(process.child.is_none());
        require_closed_socket(&mut reader)
    }

    #[test]
    fn early_scope_exit_reaps_the_child_without_releasing_its_barrier() -> TestResult {
        let (_root, mut process, mut reader) = held_child()?;
        let receipt = std::rc::Rc::new(std::cell::RefCell::new(None));
        process.collected_output = Some(std::rc::Rc::clone(&receipt));
        drop(process);
        let output = receipt.borrow_mut().take().ok_or("child was not reaped")?;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        // A child that exits on its own protocol timeout emits a diagnostic;
        // that must not masquerade as immediate owned-child cleanup.
        assert!(output.stderr.is_empty(), "{output:?}");
        require_closed_socket(&mut reader)
    }

    fn held_child()
    -> Result<(tempfile::TempDir, BarrierChild, BufReader<TcpStream>), Box<dyn std::error::Error>>
    {
        let root = crate::fixture()?;
        let admission = listener()?;
        let mut command = lumin_command(root.path())?;
        command
            .args([
                "pre-write",
                "--operation-id",
                "op-watchdog",
                "--path",
                "src/main.ts",
                "--jobs",
                "1",
            ])
            .env(
                "LUMIN_TEST_GATE_ADMISSION_BARRIER",
                admission.local_addr()?.to_string(),
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut process = BarrierChild::spawn(&mut command, "held admission")?;
        let (stream, peer) = process.accept(&admission)?;
        assert!(peer.ip().is_loopback());
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(ARRIVAL_WATCHDOG))?;
        let mut reader = BufReader::new(stream);
        let mut frame = String::new();
        reader.read_line(&mut frame)?;
        assert_eq!(frame, "reserved op-watchdog gate_0000000000000001\n");
        Ok((root, process, reader))
    }

    fn require_closed_socket(reader: &mut BufReader<TcpStream>) -> TestResult {
        let mut remaining = Vec::new();
        match reader.read_to_end(&mut remaining) {
            Ok(0) => {}
            Err(error) if error.kind() == io::ErrorKind::ConnectionReset => {}
            result => {
                return Err(io::Error::other(format!(
                    "child retained its barrier socket: {result:?}"
                ))
                .into());
            }
        }
        assert!(remaining.is_empty());
        Ok(())
    }
}
