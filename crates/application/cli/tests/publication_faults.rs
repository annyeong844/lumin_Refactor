use std::fs;

use serde_json::Value;

mod support;

use support::publication::{assert_no_attempt_liveness_files, baseline_repository, json, number};
use support::{ProcessResult, assert_status, field, run, run_with_env};

const CRASH_POINT_ENV: &str = "LUMIN_TEST_PUBLICATION_CRASH_POINT";
const CRASH_EXIT_CODE: i32 = 95;
const INVALID_SELECTOR_EXIT_CODE: i32 = 96;

#[test]
fn pre_start_crashes_preserve_sequence_rules_without_inventing_attempts()
-> Result<(), Box<dyn std::error::Error>> {
    for (point, next_sequence) in [
        ("before-attempt-catalog-allocation", 2),
        ("after-catalog-allocation", 3),
    ] {
        let fixture = Fixture::new()?;
        fixture.crash(point)?;
        fixture.assert_overview(1, "completed", &fixture.baseline_run)?;
        assert!(
            !fixture
                .root
                .path()
                .join(".lumin/attempts/attempt_0000000000000002")
                .exists(),
            "pre-start crash created an attempt envelope"
        );
        let next = fixture.audit()?;
        assert_eq!(number(&next.stdout, "sequence")?, next_sequence);
    }
    Ok(())
}

#[test]
fn running_crashes_become_interrupted_without_replacing_the_completed_run()
-> Result<(), Box<dyn std::error::Error>> {
    for point in [
        "after-running-envelope",
        "after-latest-running",
        "after-run-rename",
    ] {
        let fixture = Fixture::new()?;
        fixture.crash(point)?;
        fixture.assert_overview(2, "interrupted", &fixture.baseline_run)?;
        fixture.assert_only_catalogued_run(&fixture.baseline_run)?;
        if point == "after-run-rename" {
            assert!(
                fixture
                    .root
                    .path()
                    .join(".lumin/runs/run_0000000000000002")
                    .is_dir()
            );
        }
    }
    Ok(())
}

#[test]
fn terminal_crashes_recover_the_completed_run_and_monotonic_pointers()
-> Result<(), Box<dyn std::error::Error>> {
    for point in [
        "after-terminal-attempt",
        "after-latest-temp",
        "after-latest-replace",
    ] {
        let fixture = Fixture::new()?;
        fixture.crash(point)?;
        fixture.assert_overview(2, "completed", "run_0000000000000002")?;
        fixture.assert_catalogued_runs(&["run_0000000000000002", fixture.baseline_run.as_str()])?;
    }
    Ok(())
}

#[test]
fn explicit_run_overview_does_not_mix_in_repository_latest_attempt()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    fixture.crash("after-running-envelope")?;
    let overview = run(
        fixture.root.path(),
        &["overview", "--run", &fixture.baseline_run],
    )?;
    assert_status(&overview, 0);
    assert!(
        json(&overview.stdout)?
            .get("latestAttempt")
            .is_some_and(Value::is_null)
    );
    Ok(())
}

#[test]
fn unknown_crash_selector_fails_before_attempt_allocation() -> Result<(), Box<dyn std::error::Error>>
{
    let fixture = Fixture::new()?;
    let crashed = run_with_env(
        fixture.root.path(),
        &["audit", "--jobs", "1"],
        &[(CRASH_POINT_ENV, "not-a-publication-crash-point")],
    )?;
    assert_status(&crashed, INVALID_SELECTOR_EXIT_CODE);
    fixture.assert_overview(1, "completed", &fixture.baseline_run)?;
    assert_eq!(number(&fixture.audit()?.stdout, "sequence")?, 2);
    Ok(())
}

#[test]
fn malformed_latest_pending_file_is_not_silently_discarded()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let pending = fixture.root.path().join(".lumin/latest.json.pending");
    fs::write(&pending, b"{\n")?;

    let overview = run(fixture.root.path(), &["overview"])?;
    assert_status(&overview, 1);
    assert!(pending.is_file(), "invalid pending evidence was discarded");
    Ok(())
}

#[test]
fn analysis_failure_publishes_a_terminal_attempt_and_releases_liveness()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    fs::write(fixture.root.path().join("lumin.json"), b"{\n")?;

    let audit = run(fixture.root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 1);
    fixture.assert_overview(2, "failed", &fixture.baseline_run)?;
    Ok(())
}

struct Fixture {
    root: tempfile::TempDir,
    baseline_run: String,
}

impl Fixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let (root, baseline) = baseline_repository()?;
        let baseline_run = field(&baseline.stdout, "runId")?;
        let fixture = Self { root, baseline_run };
        assert_no_attempt_liveness_files(fixture.root.path())?;
        Ok(fixture)
    }

    fn crash(&self, point: &str) -> Result<(), Box<dyn std::error::Error>> {
        fs::write(
            self.root.path().join("src/lib.ts"),
            format!(
                "export const {point_ident} = 2;\n",
                point_ident = point.replace('-', "_")
            ),
        )?;
        let crashed = run_with_env(
            self.root.path(),
            &["audit", "--jobs", "1"],
            &[(CRASH_POINT_ENV, point)],
        )?;
        assert_status(&crashed, CRASH_EXIT_CODE);
        Ok(())
    }

    fn audit(&self) -> Result<ProcessResult, Box<dyn std::error::Error>> {
        let output = run(self.root.path(), &["audit", "--jobs", "1"])?;
        assert_status(&output, 0);
        Ok(output)
    }

    fn assert_overview(
        &self,
        attempt_sequence: u64,
        status: &str,
        selected_run: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let overview = run(self.root.path(), &["overview"])?;
        assert_status(&overview, 0);
        let body = json(&overview.stdout)?;
        assert_eq!(
            body.pointer("/scope/id").and_then(Value::as_str),
            Some(selected_run)
        );
        assert_eq!(
            body.pointer("/latestAttempt/sequence")
                .and_then(Value::as_u64),
            Some(attempt_sequence)
        );
        assert_eq!(
            body.pointer("/latestAttempt/status")
                .and_then(Value::as_str),
            Some(status)
        );
        if status == "interrupted" {
            assert!(
                body.pointer("/latestAttempt/failure")
                    .and_then(Value::as_str)
                    .is_some_and(|failure| failure.contains("process exited"))
            );
        } else if status == "failed" {
            assert!(
                body.pointer("/latestAttempt/failure")
                    .and_then(Value::as_str)
                    .is_some_and(|failure| !failure.is_empty())
            );
        }
        assert_no_attempt_liveness_files(self.root.path())?;
        Ok(())
    }

    fn assert_only_catalogued_run(&self, run_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.assert_catalogued_runs(&[run_id])
    }

    fn assert_catalogued_runs(&self, expected: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
        let runs = run(self.root.path(), &["runs", "list"])?;
        assert_status(&runs, 0);
        let body = json(&runs.stdout)?;
        let observed = body
            .get("runs")
            .and_then(Value::as_array)
            .ok_or_else(|| std::io::Error::other("run catalog omitted runs"))?
            .iter()
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
