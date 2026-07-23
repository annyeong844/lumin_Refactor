use std::fs;

use serde_json::Value;

#[path = "support/publication_barrier.rs"]
mod publication_barrier;
mod support;

use publication_barrier::{PublicationBarrier, TestResult};
use support::publication::{assert_no_attempt_liveness_files, baseline_repository, json, number};
use support::{assert_status, field, run};

const PREPARED_BARRIER_ENV: &str = "LUMIN_TEST_PUBLICATION_PREPARED_BARRIER";
const CONTENDED_BARRIER_ENV: &str = "LUMIN_TEST_PUBLICATION_CONTENDED_BARRIER";
const GUARDED_BARRIER_ENV: &str = "LUMIN_TEST_PUBLICATION_GUARDED_BARRIER";

#[test]
fn concurrent_latest_publication_preserves_monotonic_fields() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.advance_to_sequence(9)?;

    let prepared = PublicationBarrier::new(PREPARED_BARRIER_ENV, "prepared")?;
    let contended = PublicationBarrier::new(CONTENDED_BARRIER_ENV, "contended")?;
    let guarded = PublicationBarrier::new(GUARDED_BARRIER_ENV, "guarded")?;
    let mut first = prepared.spawn_audit(fixture.root.path(), &[&contended, &guarded])?;
    let first_prepared = prepared.accept(&mut first, "attempt_000000000000000a")?;
    fixture.assert_overview_state(10, "completed", "run_0000000000000009")?;

    let mut second = prepared.spawn_audit(fixture.root.path(), &[&contended, &guarded])?;
    let second_prepared = prepared.accept(&mut second, "attempt_000000000000000b")?;
    fixture.assert_overview_state(11, "completed", "run_0000000000000009")?;

    second_prepared.release()?;
    let second_guarded = guarded.accept(&mut second, "attempt_000000000000000b")?;
    first_prepared.release()?;
    let first_contended = contended.accept(&mut first, "attempt_000000000000000a")?;
    first_contended.release()?;
    second_guarded.release()?;
    let first_guarded = guarded.accept(&mut first, "attempt_000000000000000a")?;
    first_guarded.release()?;

    let second_result = second.finish()?;
    assert_status(&second_result, 0);
    assert_eq!(number(&second_result.stdout, "sequence")?, 11);

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

    let barrier = PublicationBarrier::new(PREPARED_BARRIER_ENV, "prepared")?;
    let mut older = barrier.spawn_audit(fixture.root.path(), &[])?;
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
