use std::fs;

use serde_json::Value;

#[path = "support/publication_barrier.rs"]
mod publication_barrier;
#[path = "support/retention_plan.rs"]
mod retention_plan_support;
mod support;

use publication_barrier::{PausedAudit, Permit, PublicationBarrier, TestResult};
use retention_plan_support::contains_exclusion;
use support::publication::{assert_no_attempt_liveness_files, baseline_repository, json, number};
use support::{ProcessResult, assert_status, field, run, run_with_env};

const TARGET_ATTEMPT: &str = "attempt_0000000000000002";
const TARGET_RUN: &str = "run_0000000000000002";
const BASELINE_RUN: &str = "run_0000000000000001";
const PREPARED_BARRIER_ENV: &str = "LUMIN_TEST_PUBLICATION_PREPARED_BARRIER";
const RETENTION_CRASH_ENV: &str = "LUMIN_TEST_RETENTION_CRASH_POINT";
const RETENTION_CRASH_EXIT_CODE: i32 = 93;

#[test]
fn publication_first_makes_retention_confirmation_stale() -> TestResult {
    let fixture = RaceFixture::new()?;
    let (target, permit) = fixture.pause_target_after_terminal_publication()?;
    fixture.advance_latest_with_failed_attempt()?;
    let plan_id = fixture.prepare_target_plan("publication-first-plan")?;

    permit.release()?;
    let published = target.finish()?;
    assert_status(&published, 0);
    assert_eq!(field(&published.stdout, "runId")?, TARGET_RUN);

    let confirmation = fixture.confirm(&plan_id, "publication-first-confirm")?;
    assert_status(&confirmation, 5);
    assert_eq!(
        json(&confirmation.stdout)?.pointer("/result/status"),
        Some(&Value::String("stale".to_owned()))
    );
    fixture.assert_plan_state(&plan_id, "prepared")?;
    fixture.assert_latest(TARGET_RUN)?;
    fixture.assert_runs(&[TARGET_RUN, BASELINE_RUN])?;
    assert_no_attempt_liveness_files(fixture.root.path())?;
    Ok(())
}

#[test]
fn retention_first_prevents_pointer_publication_with_typed_result() -> TestResult {
    let fixture = RaceFixture::new()?;
    let (target, permit) = fixture.pause_target_after_terminal_publication()?;
    fixture.advance_latest_with_failed_attempt()?;
    let plan_id = fixture.prepare_target_plan("retention-first-plan")?;

    let confirmation = fixture.confirm(&plan_id, "retention-first-confirm")?;
    assert_status(&confirmation, 0);
    assert_eq!(
        json(&confirmation.stdout)?.pointer("/result/status"),
        Some(&Value::String("pruned".to_owned()))
    );

    permit.release()?;
    let rejected = target.finish()?;
    assert_status(&rejected, 2);
    assert!(
        rejected
            .stderr
            .contains(&format!("run is already owned by retention: {TARGET_RUN}")),
        "{}",
        rejected.stderr
    );
    fixture.assert_plan_state(&plan_id, "pruned")?;
    fixture.assert_latest(BASELINE_RUN)?;
    fixture.assert_runs(&[BASELINE_RUN])?;
    assert_no_attempt_liveness_files(fixture.root.path())?;
    Ok(())
}

#[test]
fn retention_cannot_adopt_the_latest_uncatalogued_attempt_as_an_orphan() -> TestResult {
    let fixture = RaceFixture::new()?;
    let (target, permit) = fixture.pause_target_after_terminal_publication()?;
    let plan_id = fixture.prepare_plan("latest-attempt-protection-plan")?;
    let plan = fixture.show_plan(&plan_id)?;
    assert!(contains_exclusion(
        &plan,
        "attempt",
        TARGET_ATTEMPT,
        "latest-attempt"
    ));
    assert!(!contains_orphan(
        &plan,
        &format!("attempts/{TARGET_ATTEMPT}")
    ));
    assert!(!contains_orphan(&plan, &format!("runs/{TARGET_RUN}")));

    let confirmation = fixture.confirm(&plan_id, "latest-attempt-protection-confirm")?;
    assert_status(&confirmation, 0);
    permit.release()?;
    let published = target.finish()?;
    assert_status(&published, 0);
    fixture.assert_latest_state(TARGET_RUN, 2, "completed")?;
    fixture.assert_runs(&[TARGET_RUN, BASELINE_RUN])?;
    assert_no_attempt_liveness_files(fixture.root.path())?;
    Ok(())
}

#[test]
fn retention_rejects_corrupt_latest_uncatalogued_run_payload() -> TestResult {
    let fixture = RaceFixture::new()?;
    let (target, permit) = fixture.pause_target_after_terminal_publication()?;
    fs::write(
        fixture
            .root
            .path()
            .join(".lumin")
            .join("runs")
            .join(TARGET_RUN)
            .join("run.json"),
        b"[]\n",
    )?;

    let plan = run(
        fixture.root.path(),
        &[
            "runs",
            "prune",
            "plan",
            "--before",
            "9000000000000",
            "--operation-id",
            "corrupt-latest-run-plan",
        ],
    )?;
    assert_status(&plan, 1);
    assert!(
        plan.stderr.contains("state serialization failure")
            && plan.stderr.contains("RunCatalogRecord"),
        "{}",
        plan.stderr
    );

    permit.release()?;
    let rejected = target.finish()?;
    assert_status(&rejected, 1);
    assert!(
        rejected.stderr.contains("run publication failed")
            && rejected.stderr.contains("state serialization failure"),
        "{}",
        rejected.stderr
    );
    Ok(())
}

#[test]
fn pruning_crash_and_publisher_death_cannot_recover_a_pointer() -> TestResult {
    let fixture = RaceFixture::new()?;
    let (target, permit) = fixture.pause_target_after_terminal_publication()?;
    fixture.advance_latest_with_failed_attempt()?;
    let plan_id = fixture.prepare_target_plan("retention-recovery-plan")?;

    let confirmation = run_with_env(
        fixture.root.path(),
        &[
            "runs",
            "prune",
            "confirm",
            &plan_id,
            "--operation-id",
            "retention-recovery-confirm",
        ],
        &[(RETENTION_CRASH_ENV, "after-pruning-commit")],
    )?;
    assert_status(&confirmation, RETENTION_CRASH_EXIT_CODE);

    drop(target);
    drop(permit);
    fixture.assert_plan_state(&plan_id, "pruning")?;
    fixture.assert_latest(BASELINE_RUN)?;
    fixture.assert_runs(&[BASELINE_RUN])?;
    assert_no_attempt_liveness_files(fixture.root.path())?;
    Ok(())
}

struct RaceFixture {
    root: tempfile::TempDir,
}

impl RaceFixture {
    fn new() -> TestResult<Self> {
        let (root, baseline) = baseline_repository()?;
        assert_eq!(number(&baseline.stdout, "sequence")?, 1);
        assert_eq!(field(&baseline.stdout, "runId")?, BASELINE_RUN);
        Ok(Self { root })
    }

    fn pause_target_after_terminal_publication(&self) -> TestResult<(PausedAudit, Permit)> {
        let barrier = PublicationBarrier::new(PREPARED_BARRIER_ENV, "prepared")?;
        let mut target = barrier.spawn_audit(self.root.path(), &[])?;
        let permit = barrier.accept(&mut target, TARGET_ATTEMPT)?;
        Ok((target, permit))
    }

    fn advance_latest_with_failed_attempt(&self) -> TestResult {
        fs::write(self.root.path().join("lumin.json"), b"{\n")?;
        let newer = run(self.root.path(), &["audit", "--jobs", "1"])?;
        assert_status(&newer, 1);
        fs::remove_file(self.root.path().join("lumin.json"))?;
        let overview = run(self.root.path(), &["overview"])?;
        assert_status(&overview, 0);
        let body = json(&overview.stdout)?;
        assert_eq!(
            body.pointer("/latestAttempt/sequence"),
            Some(&Value::from(3))
        );
        assert_eq!(
            body.pointer("/latestAttempt/status"),
            Some(&Value::String("failed".to_owned()))
        );
        Ok(())
    }

    fn prepare_target_plan(&self, operation_id: &str) -> TestResult<String> {
        let plan_id = self.prepare_plan(operation_id)?;
        let shown = self.show_plan(&plan_id)?;
        assert!(contains_orphan(
            &shown,
            &format!("attempts/{TARGET_ATTEMPT}")
        ));
        assert!(contains_orphan(&shown, &format!("runs/{TARGET_RUN}")));
        Ok(plan_id)
    }

    fn prepare_plan(&self, operation_id: &str) -> TestResult<String> {
        let plan = run(
            self.root.path(),
            &[
                "runs",
                "prune",
                "plan",
                "--before",
                "9000000000000",
                "--operation-id",
                operation_id,
            ],
        )?;
        assert_status(&plan, 0);
        let plan_id = json(&plan.stdout)?
            .pointer("/result/planId")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| std::io::Error::other("plan response omitted planId"))?;
        Ok(plan_id)
    }

    fn confirm(&self, plan_id: &str, operation_id: &str) -> TestResult<ProcessResult> {
        run(
            self.root.path(),
            &[
                "runs",
                "prune",
                "confirm",
                plan_id,
                "--operation-id",
                operation_id,
            ],
        )
    }

    fn assert_plan_state(&self, plan_id: &str, expected: &str) -> TestResult {
        let plan = self.show_plan(plan_id)?;
        assert_eq!(plan.get("state").and_then(Value::as_str), Some(expected));
        Ok(())
    }

    fn show_plan(&self, plan_id: &str) -> TestResult<Value> {
        let shown = run(
            self.root.path(),
            &["runs", "prune", "plan", "show", plan_id],
        )?;
        assert_status(&shown, 0);
        json(&shown.stdout)
    }

    fn assert_latest(&self, expected_run: &str) -> TestResult {
        self.assert_latest_state(expected_run, 3, "failed")
    }

    fn assert_latest_state(
        &self,
        expected_run: &str,
        expected_sequence: u64,
        expected_status: &str,
    ) -> TestResult {
        let overview = run(self.root.path(), &["overview"])?;
        assert_status(&overview, 0);
        let body = json(&overview.stdout)?;
        assert_eq!(
            body.pointer("/latestAttempt/sequence"),
            Some(&Value::from(expected_sequence))
        );
        assert_eq!(
            body.pointer("/latestAttempt/status"),
            Some(&Value::String(expected_status.to_owned()))
        );
        assert_eq!(
            body.pointer("/scope/id").and_then(Value::as_str),
            Some(expected_run)
        );
        Ok(())
    }

    fn assert_runs(&self, expected: &[&str]) -> TestResult {
        let runs = run(self.root.path(), &["runs", "list"])?;
        assert_status(&runs, 0);
        let body = json(&runs.stdout)?;
        let observed = body["runs"]
            .as_array()
            .ok_or_else(|| std::io::Error::other("run catalog omitted runs"))?
            .iter()
            .filter_map(|run| run.get("runId").and_then(Value::as_str))
            .collect::<Vec<_>>();
        assert_eq!(observed, expected);
        Ok(())
    }
}

fn contains_orphan(plan: &Value, record_id: &str) -> bool {
    plan["items"].as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item.get("kind").and_then(Value::as_str) == Some("orphan-payload")
                && item.get("recordId").and_then(Value::as_str) == Some(record_id)
        })
    })
}
