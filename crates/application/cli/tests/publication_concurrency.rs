use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

mod support;

use support::publication::{assert_no_attempt_liveness_files, baseline_repository, json, number};
use support::{ProcessResult, assert_status, field, run};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const BARRIER_ENV: &str = "LUMIN_TEST_PUBLICATION_BARRIER";
const BARRIER_WAIT_LIMIT: Duration = Duration::from_secs(30);

#[test]
fn concurrent_latest_publication_preserves_monotonic_fields() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.advance_to_sequence(9)?;

    let first_barrier = PublicationBarrier::new()?;
    let mut first = fixture.spawn_paused_audit(&first_barrier)?;
    let first_permit = first_barrier.accept(&mut first, "attempt_000000000000000a")?;
    fixture.assert_overview_state(10, "running", "run_0000000000000009")?;

    let second_barrier = PublicationBarrier::new()?;
    let mut second = fixture.spawn_paused_audit(&second_barrier)?;
    let second_permit = second_barrier.accept(&mut second, "attempt_000000000000000b")?;
    fixture.assert_overview_state(11, "running", "run_0000000000000009")?;

    second_permit.release()?;
    let second_result = second.finish()?;
    assert_status(&second_result, 0);
    assert_eq!(number(&second_result.stdout, "sequence")?, 11);
    fixture.assert_overview_state(11, "completed", "run_000000000000000b")?;

    first_permit.release()?;
    let first_result = first.finish()?;
    assert_status(&first_result, 0);
    assert_eq!(number(&first_result.stdout, "sequence")?, 10);
    fixture.assert_overview(11, "completed", "run_000000000000000b")?;
    fixture.assert_catalog_prefix(&[
        "run_000000000000000b",
        "run_000000000000000a",
        "run_0000000000000009",
    ])?;
    Ok(())
}

#[test]
fn concurrent_latest_publication_merges_attempt_and_completed_independently() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.advance_to_sequence(9)?;

    let barrier = PublicationBarrier::new()?;
    let mut older = fixture.spawn_paused_audit(&barrier)?;
    let permit = barrier.accept(&mut older, "attempt_000000000000000a")?;

    fs::write(fixture.root.path().join("lumin.json"), b"{\n")?;
    let newer = run(fixture.root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&newer, 1);
    fixture.assert_overview_state(11, "failed", "run_0000000000000009")?;
    fs::remove_file(fixture.root.path().join("lumin.json"))?;

    permit.release()?;
    let older_result = older.finish()?;
    assert_status(&older_result, 0);
    assert_eq!(number(&older_result.stdout, "sequence")?, 10);
    fixture.assert_overview(11, "failed", "run_000000000000000a")?;
    fixture.assert_catalog_prefix(&["run_000000000000000a", "run_0000000000000009"])?;
    Ok(())
}

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> TestResult<Self> {
        let (root, baseline) = baseline_repository()?;
        assert_eq!(number(&baseline.stdout, "sequence")?, 1);
        assert_eq!(field(&baseline.stdout, "runId")?, "run_0000000000000001");
        let fixture = Self { root };
        assert_no_attempt_liveness_files(fixture.root.path())?;
        Ok(fixture)
    }

    fn advance_to_sequence(&self, target: u64) -> TestResult {
        for expected in 2..=target {
            let output = run(self.root.path(), &["audit", "--jobs", "1"])?;
            assert_status(&output, 0);
            assert_eq!(number(&output.stdout, "sequence")?, expected);
        }
        Ok(())
    }

    fn spawn_paused_audit(&self, barrier: &PublicationBarrier) -> TestResult<PausedAudit> {
        let child = Command::new(env!("CARGO_BIN_EXE_lumin"))
            .current_dir(self.root.path())
            .args(["audit", "--jobs", "1"])
            .env(BARRIER_ENV, barrier.address()?)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        Ok(PausedAudit { child: Some(child) })
    }

    fn assert_overview(&self, sequence: u64, status: &str, selected_run: &str) -> TestResult {
        self.assert_overview_state(sequence, status, selected_run)?;
        assert_no_attempt_liveness_files(self.root.path())
    }

    fn assert_overview_state(&self, sequence: u64, status: &str, selected_run: &str) -> TestResult {
        let overview = run(self.root.path(), &["overview"])?;
        assert_status(&overview, 0);
        let body = json(&overview.stdout)?;
        let attempt = body
            .get("latestAttempt")
            .ok_or_else(|| std::io::Error::other("overview omitted latestAttempt"))?;
        assert_eq!(
            attempt.get("sequence").and_then(Value::as_u64),
            Some(sequence)
        );
        assert_eq!(attempt.get("status").and_then(Value::as_str), Some(status));
        assert_eq!(
            body.pointer("/scope/id").and_then(Value::as_str),
            Some(selected_run)
        );
        Ok(())
    }

    fn assert_catalog_prefix(&self, expected: &[&str]) -> TestResult {
        let runs = run(self.root.path(), &["runs", "list"])?;
        assert_status(&runs, 0);
        let body = json(&runs.stdout)?;
        let observed = body
            .get("runs")
            .and_then(Value::as_array)
            .ok_or_else(|| std::io::Error::other("run catalog omitted runs"))?
            .iter()
            .take(expected.len())
            .map(|run| {
                run.get("runId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| std::io::Error::other("run catalog item omitted runId"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(observed, expected);
        Ok(())
    }
}

struct PublicationBarrier {
    listener: TcpListener,
}

impl PublicationBarrier {
    fn new() -> TestResult<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        Ok(Self { listener })
    }

    fn address(&self) -> TestResult<String> {
        self.listener
            .local_addr()
            .map(|address| address.to_string())
            .map_err(Into::into)
    }

    fn accept(&self, process: &mut PausedAudit, expected_attempt_id: &str) -> TestResult<Permit> {
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
}

struct Permit {
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

    fn release(mut self) -> TestResult {
        self.stream.write_all(b"release\n")?;
        Ok(())
    }
}

struct PausedAudit {
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

    fn finish(mut self) -> TestResult<ProcessResult> {
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
