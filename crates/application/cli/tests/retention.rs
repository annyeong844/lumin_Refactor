use std::fs;
use std::path::Path;

use serde_json::Value;

#[path = "support/retention_plan.rs"]
mod retention_plan_support;
#[path = "support/retention.rs"]
mod retention_support;
mod support;

use retention_plan_support::contains_exclusion;
use retention_support::{audit, json};
use support::{assert_status, run};

const RETENTION_CONFIRM_OPERATION: &str = "public-retention-confirm";
const LATEST_PROTECTION_CONFIRM_OPERATION: &str = "public-latest-protection-confirm";

#[test]
fn retention_truth_survives_public_process_reopen() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("lib.ts"), "export const first = 1;\n")?;
    let first_run = audit(root.path())?;
    fs::write(root.path().join("lib.ts"), "export const second = 2;\n")?;
    let second_run = audit(root.path())?;

    let plan = prepare_plan(root.path())?;
    let plan_id = json(&plan.stdout)?
        .pointer("/result/planId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other("plan response omitted planId"))?;
    let plan_retry = prepare_plan(root.path())?;
    assert_eq!(plan_retry.stdout, plan.stdout);
    assert_prepared_plan(root.path(), &plan_id, &first_run, &second_run)?;

    let confirmed = confirm_plan(root.path(), &plan_id, RETENTION_CONFIRM_OPERATION, 0)?;
    assert_eq!(
        json(&confirmed.stdout)?
            .pointer("/result/status")
            .and_then(Value::as_str),
        Some("pruned")
    );
    let confirm_retry = confirm_plan(root.path(), &plan_id, RETENTION_CONFIRM_OPERATION, 0)?;
    assert_eq!(confirm_retry.stdout, confirmed.stdout);

    assert_pruned_views(root.path(), &plan_id, &first_run, &second_run)?;
    assert_committed_operation(root.path(), &plan_id)?;
    Ok(())
}

#[test]
fn latest_attempt_and_completed_closures_survive_stale_confirmation()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("lib.ts"), "export const first = 1;\n")?;
    let completed_run = audit(root.path())?;
    let completed_overview = overview(root.path())?;
    let completed_attempt = required_string(&completed_overview, "/latestAttempt/attemptId")?;

    fs::write(root.path().join("lumin.json"), b"{\n")?;
    let failed = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&failed, 1);
    let failed_overview = overview(root.path())?;
    let failed_attempt = required_string(&failed_overview, "/latestAttempt/attemptId")?;
    let failed_reason = required_string(&failed_overview, "/latestAttempt/failure")?;
    assert_eq!(
        failed_overview
            .pointer("/latestAttempt/status")
            .and_then(Value::as_str),
        Some("failed")
    );
    assert!(
        failed_reason.contains("malformed lumin.json"),
        "audit failed for an unexpected reason: {failed_reason}"
    );
    assert_eq!(
        failed_overview.pointer("/scope/id").and_then(Value::as_str),
        Some(completed_run.as_str())
    );

    let plan = prepare_plan(root.path())?;
    let plan_id = required_string(&json(&plan.stdout)?, "/result/planId")?;
    assert_latest_exclusions(
        root.path(),
        &plan_id,
        &failed_attempt,
        &completed_attempt,
        &completed_run,
    )?;

    fs::remove_file(root.path().join("lumin.json"))?;
    fs::write(root.path().join("lib.ts"), "export const newest = 2;\n")?;
    let newest_run = audit(root.path())?;
    let newest_overview = overview(root.path())?;
    let newest_attempt = required_string(&newest_overview, "/latestAttempt/attemptId")?;

    let stale = confirm_plan(
        root.path(),
        &plan_id,
        LATEST_PROTECTION_CONFIRM_OPERATION,
        5,
    )?;
    let stale_body = json(&stale.stdout)?;
    assert_eq!(
        stale_body.pointer("/result/status").and_then(Value::as_str),
        Some("stale")
    );
    assert!(
        stale_body
            .pointer("/result/changedInputs")
            .and_then(Value::as_array)
            .is_some_and(|inputs| !inputs.is_empty())
    );

    let stale_retry = confirm_plan(
        root.path(),
        &plan_id,
        LATEST_PROTECTION_CONFIRM_OPERATION,
        5,
    )?;
    assert_eq!(stale_retry.stdout, stale.stdout);

    assert_stale_views(
        root.path(),
        &plan_id,
        &failed_attempt,
        &completed_attempt,
        &completed_run,
        &newest_run,
        &newest_attempt,
    )?;
    Ok(())
}

fn prepare_plan(root: &Path) -> Result<support::ProcessResult, Box<dyn std::error::Error>> {
    let output = run(
        root,
        &[
            "runs",
            "prune",
            "plan",
            "--before",
            "9000000000000",
            "--operation-id",
            "public-retention-plan",
        ],
    )?;
    assert_status(&output, 0);
    Ok(output)
}

fn confirm_plan(
    root: &Path,
    plan_id: &str,
    operation_id: &str,
    expected_status: i32,
) -> Result<support::ProcessResult, Box<dyn std::error::Error>> {
    let output = run(
        root,
        &[
            "runs",
            "prune",
            "confirm",
            plan_id,
            "--operation-id",
            operation_id,
        ],
    )?;
    assert_status(&output, expected_status);
    Ok(output)
}

fn assert_prepared_plan(
    root: &Path,
    plan_id: &str,
    first_run: &str,
    second_run: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let shown = run(root, &["runs", "prune", "plan", "show", plan_id])?;
    assert_status(&shown, 0);
    let body = json(&shown.stdout)?;
    assert_eq!(body.get("state").and_then(Value::as_str), Some("prepared"));
    assert!(contains_record(&body, "items", "run", first_run));
    assert!(contains_record(&body, "exclusions", "run", second_run));
    Ok(())
}

fn assert_pruned_views(
    root: &Path,
    plan_id: &str,
    first_run: &str,
    second_run: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let runs = run(root, &["runs", "list"])?;
    assert_status(&runs, 0);
    let runs_body = json(&runs.stdout)?;
    let run_ids = runs_body
        .get("runs")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("run catalog omitted runs"))?
        .iter()
        .filter_map(|run| run.get("runId").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert_eq!(run_ids, [second_run]);

    for arguments in [
        vec!["overview", "--run", first_run],
        vec!["findings", "--run", first_run, "--area", "dead-code"],
    ] {
        let lookup = run(root, &arguments)?;
        assert_status(&lookup, 0);
        let body = json(&lookup.stdout)?;
        assert_eq!(body.get("status").and_then(Value::as_str), Some("pruned"));
        assert_eq!(
            body.pointer("/tombstone/planId").and_then(Value::as_str),
            Some(plan_id)
        );
    }

    let shown = run(root, &["runs", "prune", "plan", "show", plan_id])?;
    assert_status(&shown, 0);
    let body = json(&shown.stdout)?;
    assert_eq!(body.get("state").and_then(Value::as_str), Some("pruned"));
    assert_eq!(
        body.get("physicalReclamationPending")
            .and_then(Value::as_bool),
        Some(false)
    );
    Ok(())
}

fn assert_committed_operation(
    root: &Path,
    plan_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let operation = run(root, &["operation", "show", "public-retention-confirm"])?;
    assert_status(&operation, 0);
    let body = json(&operation.stdout)?;
    assert_eq!(
        body.pointer("/operation/status").and_then(Value::as_str),
        Some("committed")
    );
    assert_eq!(
        body.pointer("/operation/result/result/planId")
            .and_then(Value::as_str),
        Some(plan_id)
    );
    assert_eq!(
        body.pointer("/operation/result/result/physicalReclamationPending")
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        body.get("currentPhysicalReclamationPending")
            .and_then(Value::as_bool),
        Some(false)
    );
    Ok(())
}

fn overview(root: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    let output = run(root, &["overview"])?;
    assert_status(&output, 0);
    json(&output.stdout).map_err(Into::into)
}

fn required_string(value: &Value, pointer: &str) -> Result<String, Box<dyn std::error::Error>> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other(format!("response omitted {pointer}")).into())
}

fn assert_latest_exclusions(
    root: &Path,
    plan_id: &str,
    failed_attempt: &str,
    completed_attempt: &str,
    completed_run: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let shown = run(root, &["runs", "prune", "plan", "show", plan_id])?;
    assert_status(&shown, 0);
    let body = json(&shown.stdout)?;
    assert_eq!(body.get("state").and_then(Value::as_str), Some("prepared"));
    assert!(
        body.get("items")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
    );
    assert!(contains_exclusion(
        &body,
        "attempt",
        failed_attempt,
        "latest-attempt"
    ));
    assert!(contains_exclusion(
        &body,
        "attempt",
        completed_attempt,
        "latest-completed"
    ));
    assert!(contains_exclusion(
        &body,
        "run",
        completed_run,
        "latest-completed"
    ));
    Ok(())
}

fn assert_stale_views(
    root: &Path,
    plan_id: &str,
    failed_attempt: &str,
    completed_attempt: &str,
    completed_run: &str,
    newest_run: &str,
    newest_attempt: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_latest_exclusions(
        root,
        plan_id,
        failed_attempt,
        completed_attempt,
        completed_run,
    )?;
    for attempt_id in [failed_attempt, completed_attempt] {
        let path = root
            .join(".lumin")
            .join("attempts")
            .join(attempt_id)
            .join("attempt.json");
        assert!(
            path.is_file(),
            "stale confirmation removed excluded attempt {attempt_id}"
        );
    }

    let operation = run(
        root,
        &["operation", "show", LATEST_PROTECTION_CONFIRM_OPERATION],
    )?;
    assert_status(&operation, 0);
    let operation = json(&operation.stdout)?;
    assert_eq!(
        operation
            .pointer("/operation/status")
            .and_then(Value::as_str),
        Some("stale")
    );

    let latest = overview(root)?;
    assert_eq!(
        latest.pointer("/scope/id").and_then(Value::as_str),
        Some(newest_run)
    );
    assert_eq!(
        latest
            .pointer("/latestAttempt/status")
            .and_then(Value::as_str),
        Some("completed")
    );
    assert_eq!(
        latest
            .pointer("/latestAttempt/attemptId")
            .and_then(Value::as_str),
        Some(newest_attempt)
    );

    let retained = run(root, &["overview", "--run", completed_run])?;
    assert_status(&retained, 0);
    assert_eq!(
        json(&retained.stdout)?
            .pointer("/scope/id")
            .and_then(Value::as_str),
        Some(completed_run)
    );
    Ok(())
}

fn contains_record(body: &Value, collection: &str, kind: &str, record_id: &str) -> bool {
    body.get(collection)
        .and_then(Value::as_array)
        .is_some_and(|records| {
            records.iter().any(|record| {
                record.get("kind").and_then(Value::as_str) == Some(kind)
                    && record.get("recordId").and_then(Value::as_str) == Some(record_id)
            })
        })
}

#[test]
fn retention_plan_pages_survive_unrelated_repository_mutation_and_reject_cross_plan_cursor()
-> Result<(), Box<dyn std::error::Error>> {
    const COMPLETED_RUN_COUNT: usize = 51;
    const RETENTION_RECORDS_PER_COMPLETION: usize = 3;
    const PLAN_PAGE_SIZE: usize = 100;

    let root = tempfile::tempdir()?;
    let mut authored_runs = std::collections::BTreeSet::new();
    let mut latest_run_id = None;
    for index in 0..COMPLETED_RUN_COUNT {
        fs::write(
            root.path().join("lib.ts"),
            format!("export const value{index} = {index};\n"),
        )?;
        let run_id = audit(root.path())?;
        latest_run_id = Some(run_id.clone());
        assert!(authored_runs.insert(run_id), "audit reused a run ID");
    }
    let latest_run_id =
        latest_run_id.ok_or_else(|| std::io::Error::other("fixture produced no completed run"))?;

    let catalog = run(root.path(), &["runs", "list"])?;
    assert_status(&catalog, 0);
    let catalog = json(&catalog.stdout)?;
    let catalog_runs = catalog
        .get("runs")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("run catalog omitted runs"))?;
    assert_eq!(catalog_runs.len(), COMPLETED_RUN_COUNT);
    assert!(
        !catalog
            .get("truncated")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    );

    let mut expected_attempts_and_runs = std::collections::BTreeSet::new();
    let mut catalogued_runs = std::collections::BTreeSet::new();
    let mut latest_attempt_id = None;
    for catalog_run in catalog_runs {
        let attempt_id = required_string(catalog_run, "/attemptId")?;
        let run_id = required_string(catalog_run, "/runId")?;
        if run_id == latest_run_id {
            latest_attempt_id = Some(attempt_id.clone());
        }
        assert!(catalogued_runs.insert(run_id.clone()));
        assert!(expected_attempts_and_runs.insert(("attempt".to_owned(), attempt_id)));
        assert!(expected_attempts_and_runs.insert(("run".to_owned(), run_id)));
    }
    let latest_attempt_id = latest_attempt_id
        .ok_or_else(|| std::io::Error::other("latest run has no catalogued attempt"))?;
    assert_eq!(catalogued_runs, authored_runs);
    assert_eq!(expected_attempts_and_runs.len(), COMPLETED_RUN_COUNT * 2);
    let expected_total = COMPLETED_RUN_COUNT * RETENTION_RECORDS_PER_COMPLETION;

    let first_plan = prepare_pagination_plan(root.path(), "pagination-plan-first")?;
    let first_plan = json(&first_plan.stdout)?;
    let first_plan_id = required_string(&first_plan, "/result/planId")?;
    let first_content_identity = required_string(&first_plan, "/result/contentIdentity")?;
    let first_page = show_pagination_plan(root.path(), &first_plan_id, None)?;
    let first_page = json(&first_page.stdout)?;
    assert_plan_page_scope(
        &first_page,
        &first_plan_id,
        &first_content_identity,
        expected_total,
    )?;
    assert_eq!(required_usize(&first_page, "/returned")?, PLAN_PAGE_SIZE);
    assert_eq!(
        first_page.get("truncated").and_then(Value::as_bool),
        Some(true)
    );
    let first_cursor = required_string(&first_page, "/nextCursor")?;

    let mut observed_records = std::collections::BTreeSet::new();
    insert_plan_page_records(&first_page, &mut observed_records)?;

    fs::write(
        root.path().join("lib.ts"),
        "export const unrelatedMutation = true;\n",
    )?;
    let unrelated_run = audit(root.path())?;
    assert!(!authored_runs.contains(&unrelated_run));

    let second_plan = prepare_pagination_plan(root.path(), "pagination-plan-second")?;
    let second_plan = json(&second_plan.stdout)?;
    let second_plan_id = required_string(&second_plan, "/result/planId")?;
    let second_content_identity = required_string(&second_plan, "/result/contentIdentity")?;
    assert_ne!(second_plan_id, first_plan_id);
    assert_ne!(second_content_identity, first_content_identity);

    let cross_plan = run(
        root.path(),
        &[
            "runs",
            "prune",
            "plan",
            "show",
            &second_plan_id,
            "--cursor",
            &first_cursor,
        ],
    )?;
    assert_status(&cross_plan, 2);
    assert!(cross_plan.stdout.trim().is_empty());
    assert!(
        cross_plan.stderr.contains("cursor scope"),
        "cross-plan cursor failed for an unexpected reason: {}",
        cross_plan.stderr
    );

    let second_plan_after_rejection = show_pagination_plan(root.path(), &second_plan_id, None)?;
    let second_plan_after_rejection = json(&second_plan_after_rejection.stdout)?;
    assert_eq!(
        required_string(&second_plan_after_rejection, "/planId")?,
        second_plan_id
    );
    assert_eq!(
        required_string(&second_plan_after_rejection, "/contentIdentity")?,
        second_content_identity
    );
    assert_eq!(
        second_plan_after_rejection
            .get("state")
            .and_then(Value::as_str),
        Some("prepared")
    );

    let mut seen_cursors = std::collections::BTreeSet::new();
    let mut cursor = Some(first_cursor);
    let mut page_count = 1usize;
    while let Some(current_cursor) = cursor {
        assert!(
            seen_cursors.insert(current_cursor.clone()),
            "retention pagination repeated a cursor"
        );
        let page = show_pagination_plan(root.path(), &first_plan_id, Some(&current_cursor))?;
        let page = json(&page.stdout)?;
        assert_plan_page_scope(
            &page,
            &first_plan_id,
            &first_content_identity,
            expected_total,
        )?;
        insert_plan_page_records(&page, &mut observed_records)?;
        cursor = page
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        assert_eq!(
            page.get("truncated").and_then(Value::as_bool),
            Some(cursor.is_some())
        );
        page_count += 1;
    }

    assert_eq!(page_count, 2);
    assert_plan_record_truth(
        &observed_records,
        &expected_attempts_and_runs,
        &latest_attempt_id,
        &latest_run_id,
        COMPLETED_RUN_COUNT,
    );
    Ok(())
}

fn prepare_pagination_plan(
    root: &Path,
    operation_id: &str,
) -> Result<support::ProcessResult, Box<dyn std::error::Error>> {
    let output = run(
        root,
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
    assert_status(&output, 0);
    Ok(output)
}

fn show_pagination_plan(
    root: &Path,
    plan_id: &str,
    cursor: Option<&str>,
) -> Result<support::ProcessResult, Box<dyn std::error::Error>> {
    let mut arguments = vec!["runs", "prune", "plan", "show", plan_id];
    if let Some(cursor) = cursor {
        arguments.extend(["--cursor", cursor]);
    }
    let output = run(root, &arguments)?;
    assert_status(&output, 0);
    Ok(output)
}

fn assert_plan_page_scope(
    page: &Value,
    plan_id: &str,
    content_identity: &str,
    total: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(required_string(page, "/planId")?, plan_id);
    assert_eq!(required_string(page, "/contentIdentity")?, content_identity);
    assert_eq!(
        page.get("ordering").and_then(Value::as_str),
        Some("retention-plan-items.v1")
    );
    assert_eq!(page.get("state").and_then(Value::as_str), Some("prepared"));
    assert_eq!(required_usize(page, "/total")?, total);
    assert_eq!(
        required_usize(page, "/returned")?,
        plan_page_records(page)?.len()
    );
    Ok(())
}

type PlanRecordIdentity = (String, String, String, String);

fn insert_plan_page_records(
    page: &Value,
    observed: &mut std::collections::BTreeSet<PlanRecordIdentity>,
) -> Result<(), Box<dyn std::error::Error>> {
    for record in plan_page_records(page)? {
        assert!(
            observed.insert(record.clone()),
            "retention plan record appeared more than once: {record:?}"
        );
    }
    Ok(())
}

fn plan_page_records(page: &Value) -> Result<Vec<PlanRecordIdentity>, Box<dyn std::error::Error>> {
    let mut records = Vec::new();
    for collection in ["items", "exclusions"] {
        let values = page
            .get(collection)
            .and_then(Value::as_array)
            .ok_or_else(|| std::io::Error::other(format!("plan page omitted {collection}")))?;
        for value in values {
            let discriminator = match collection {
                "items" => required_string(value, "/identitySha256")?,
                "exclusions" => required_string(value, "/reason/reason")?,
                _ => unreachable!(),
            };
            records.push((
                collection.to_owned(),
                required_string(value, "/kind")?,
                required_string(value, "/recordId")?,
                discriminator,
            ));
        }
    }
    Ok(records)
}

fn assert_plan_record_truth(
    records: &std::collections::BTreeSet<PlanRecordIdentity>,
    expected_attempts_and_runs: &std::collections::BTreeSet<(String, String)>,
    latest_attempt_id: &str,
    latest_run_id: &str,
    completed_run_count: usize,
) {
    let observed_attempts_and_runs = records
        .iter()
        .filter(|record| record.1 == "attempt" || record.1 == "run")
        .map(|record| (record.1.clone(), record.2.clone()))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(observed_attempts_and_runs, *expected_attempts_and_runs);

    let count = |collection: &str, kind: &str| {
        records
            .iter()
            .filter(|record| record.0 == collection && record.1 == kind)
            .count()
    };
    let prune_eligible_count = completed_run_count - 1;
    assert_eq!(count("items", "attempt"), prune_eligible_count);
    assert_eq!(count("items", "run"), prune_eligible_count);
    assert_eq!(count("items", "evidence"), prune_eligible_count);
    assert_eq!(count("exclusions", "attempt"), 2);
    assert_eq!(count("exclusions", "run"), 1);
    assert_eq!(records.len(), completed_run_count * 3);

    for reason in ["latest-attempt", "latest-completed"] {
        assert!(records.contains(&(
            "exclusions".to_owned(),
            "attempt".to_owned(),
            latest_attempt_id.to_owned(),
            reason.to_owned(),
        )));
    }
    assert!(records.contains(&(
        "exclusions".to_owned(),
        "run".to_owned(),
        latest_run_id.to_owned(),
        "latest-completed".to_owned(),
    )));
}

fn required_usize(value: &Value, pointer: &str) -> Result<usize, Box<dyn std::error::Error>> {
    value
        .pointer(pointer)
        .and_then(Value::as_u64)
        .ok_or_else(|| std::io::Error::other(format!("response omitted {pointer}")))?
        .try_into()
        .map_err(Into::into)
}

#[test]
fn independent_public_pins_keep_a_run_protected_until_the_last_unpin()
-> Result<(), Box<dyn std::error::Error>> {
    const FIRST_PIN_OPERATION: &str = "public-independent-pin-release";
    const SECOND_PIN_OPERATION: &str = "public-independent-pin-investigation";
    const FIRST_UNPIN_OPERATION: &str = "public-independent-unpin-release";
    const SECOND_UNPIN_OPERATION: &str = "public-independent-unpin-investigation";

    let root = tempfile::tempdir()?;
    fs::write(root.path().join("lib.ts"), "export const first = 1;\n")?;
    let protected_run = audit(root.path())?;
    fs::write(root.path().join("lib.ts"), "export const latest = 2;\n")?;
    let latest_run = audit(root.path())?;
    assert_ne!(protected_run, latest_run);
    let protected_attempt = catalogued_attempt_for_run(root.path(), &protected_run)?;

    let first_pin = run(
        root.path(),
        &[
            "runs",
            "pin",
            &protected_run,
            "--operation-id",
            FIRST_PIN_OPERATION,
            "--reason",
            "release baseline",
        ],
    )?;
    assert_status(&first_pin, 0);
    let first_pin_id = assert_run_pin_response(
        &json(&first_pin.stdout)?,
        &protected_run,
        "release baseline",
        FIRST_PIN_OPERATION,
        None,
    )?;

    let second_pin = run(
        root.path(),
        &[
            "runs",
            "pin",
            &protected_run,
            "--operation-id",
            SECOND_PIN_OPERATION,
            "--reason",
            "active investigation",
        ],
    )?;
    assert_status(&second_pin, 0);
    let second_pin_id = assert_run_pin_response(
        &json(&second_pin.stdout)?,
        &protected_run,
        "active investigation",
        SECOND_PIN_OPERATION,
        None,
    )?;
    assert_ne!(first_pin_id, second_pin_id);

    let mut both_pin_ids = vec![first_pin_id.clone(), second_pin_id.clone()];
    both_pin_ids.sort();
    let both_plan =
        prepare_and_show_independent_pin_plan(root.path(), "public-independent-pin-plan-both")?;
    assert_pin_protected_plan(
        &both_plan,
        &protected_attempt,
        &protected_run,
        &both_pin_ids,
    )?;

    let first_unpin = run(
        root.path(),
        &[
            "runs",
            "unpin",
            &first_pin_id,
            "--operation-id",
            FIRST_UNPIN_OPERATION,
        ],
    )?;
    assert_status(&first_unpin, 0);
    assert_eq!(
        assert_run_pin_response(
            &json(&first_unpin.stdout)?,
            &protected_run,
            "release baseline",
            FIRST_PIN_OPERATION,
            Some(FIRST_UNPIN_OPERATION),
        )?,
        first_pin_id
    );

    let one_plan =
        prepare_and_show_independent_pin_plan(root.path(), "public-independent-pin-plan-one")?;
    assert_pin_protected_plan(
        &one_plan,
        &protected_attempt,
        &protected_run,
        std::slice::from_ref(&second_pin_id),
    )?;

    let second_unpin = run(
        root.path(),
        &[
            "runs",
            "unpin",
            &second_pin_id,
            "--operation-id",
            SECOND_UNPIN_OPERATION,
        ],
    )?;
    assert_status(&second_unpin, 0);
    assert_eq!(
        assert_run_pin_response(
            &json(&second_unpin.stdout)?,
            &protected_run,
            "active investigation",
            SECOND_PIN_OPERATION,
            Some(SECOND_UNPIN_OPERATION),
        )?,
        second_pin_id
    );

    let unpinned_plan =
        prepare_and_show_independent_pin_plan(root.path(), "public-independent-pin-plan-none")?;
    assert_eq!(
        active_pin_ids(&unpinned_plan, "attempt", &protected_attempt)?,
        None
    );
    assert_eq!(active_pin_ids(&unpinned_plan, "run", &protected_run)?, None);
    for (kind, record_id) in [
        ("attempt", protected_attempt.as_str()),
        ("run", protected_run.as_str()),
        ("evidence", format!("run:{protected_run}/evidence").as_str()),
        ("pin-or-reference", first_pin_id.as_str()),
        ("pin-or-reference", second_pin_id.as_str()),
    ] {
        assert!(
            contains_record(&unpinned_plan, "items", kind, record_id),
            "final unpin did not make {kind} {record_id} eligible"
        );
    }
    assert_eq!(required_usize(&unpinned_plan, "/total")?, 8);
    assert_eq!(required_usize(&unpinned_plan, "/returned")?, 8);
    assert_eq!(
        unpinned_plan.get("truncated").and_then(Value::as_bool),
        Some(false)
    );
    Ok(())
}

fn catalogued_attempt_for_run(
    root: &Path,
    run_id: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let catalog = run(root, &["runs", "list"])?;
    assert_status(&catalog, 0);
    let catalog = json(&catalog.stdout)?;
    assert_eq!(
        catalog.get("truncated").and_then(Value::as_bool),
        Some(false)
    );
    catalog
        .get("runs")
        .and_then(Value::as_array)
        .and_then(|runs| {
            runs.iter()
                .find(|candidate| candidate.get("runId").and_then(Value::as_str) == Some(run_id))
        })
        .and_then(|run| run.get("attemptId"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            std::io::Error::other(format!("run catalog omitted attempt for {run_id}")).into()
        })
}

fn assert_run_pin_response(
    response: &Value,
    run_id: &str,
    reason: &str,
    created_operation_id: &str,
    removed_operation_id: Option<&str>,
) -> Result<String, Box<dyn std::error::Error>> {
    assert_eq!(
        response.get("schemaVersion").and_then(Value::as_str),
        Some("lumin.run-pin.v1")
    );
    assert_eq!(
        response
            .pointer("/pin/schemaVersion")
            .and_then(Value::as_str),
        Some("lumin-run-pin.v1")
    );
    assert_eq!(
        response.pointer("/pin/runId").and_then(Value::as_str),
        Some(run_id)
    );
    assert_eq!(
        response.pointer("/pin/reason").and_then(Value::as_str),
        Some(reason)
    );
    assert_eq!(
        response
            .pointer("/pin/createdOperationId")
            .and_then(Value::as_str),
        Some(created_operation_id)
    );
    assert_eq!(
        response
            .pointer("/pin/removedOperationId")
            .and_then(Value::as_str),
        removed_operation_id
    );
    required_string(response, "/pin/pinId")
}

fn prepare_and_show_independent_pin_plan(
    root: &Path,
    operation_id: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let prepared = prepare_pagination_plan(root, operation_id)?;
    let prepared = json(&prepared.stdout)?;
    let plan_id = required_string(&prepared, "/result/planId")?;
    let content_identity = required_string(&prepared, "/result/contentIdentity")?;
    let shown = show_pagination_plan(root, &plan_id, None)?;
    let shown = json(&shown.stdout)?;
    assert_eq!(required_string(&shown, "/planId")?, plan_id);
    assert_eq!(
        required_string(&shown, "/contentIdentity")?,
        content_identity
    );
    assert_eq!(shown.get("state").and_then(Value::as_str), Some("prepared"));
    assert_eq!(
        shown.get("ordering").and_then(Value::as_str),
        Some("retention-plan-items.v1")
    );
    Ok(shown)
}

fn assert_pin_protected_plan(
    plan: &Value,
    attempt_id: &str,
    run_id: &str,
    expected_pin_ids: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    assert_active_pin_exclusion(plan, "attempt", attempt_id, expected_pin_ids)?;
    assert_active_pin_exclusion(plan, "run", run_id, expected_pin_ids)?;
    for (kind, record_id) in [
        ("attempt", attempt_id),
        ("run", run_id),
        ("evidence", format!("run:{run_id}/evidence").as_str()),
    ] {
        assert!(
            !contains_record(plan, "items", kind, record_id),
            "active pin exposed {kind} {record_id} as prune eligible"
        );
    }
    assert_eq!(required_usize(plan, "/total")?, 5);
    assert_eq!(required_usize(plan, "/returned")?, 5);
    assert_eq!(plan.get("truncated").and_then(Value::as_bool), Some(false));
    Ok(())
}

fn assert_active_pin_exclusion(
    plan: &Value,
    kind: &str,
    record_id: &str,
    expected_pin_ids: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let actual = active_pin_ids(plan, kind, record_id)?.ok_or_else(|| {
        std::io::Error::other(format!(
            "plan omitted active-pin exclusion for {kind} {record_id}"
        ))
    })?;
    assert_eq!(actual, expected_pin_ids);
    Ok(())
}

fn active_pin_ids(
    plan: &Value,
    kind: &str,
    record_id: &str,
) -> Result<Option<Vec<String>>, Box<dyn std::error::Error>> {
    let exclusions = plan
        .get("exclusions")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("retention plan omitted exclusions"))?;
    let mut matching = exclusions.iter().filter(|exclusion| {
        exclusion.get("kind").and_then(Value::as_str) == Some(kind)
            && exclusion.get("recordId").and_then(Value::as_str) == Some(record_id)
            && exclusion.pointer("/reason/reason").and_then(Value::as_str) == Some("active-pin")
    });
    let Some(exclusion) = matching.next() else {
        return Ok(None);
    };
    assert!(
        matching.next().is_none(),
        "plan emitted duplicate active-pin exclusions for {kind} {record_id}"
    );
    let pin_ids = exclusion
        .pointer("/reason/pinIds")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("active-pin exclusion omitted pinIds"))?
        .iter()
        .map(|pin_id| {
            pin_id
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| std::io::Error::other("active-pin pinId was not a string"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(pin_ids))
}
