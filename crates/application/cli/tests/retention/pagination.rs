use std::fs;

use serde_json::Value;

use super::retention_support::{audit, json};
use super::support::{assert_status, run};
use super::{prepare_pagination_plan, required_string, required_usize, show_pagination_plan};

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
    let mut ordered_runs = Vec::new();
    let mut latest_attempt_id = None;
    for catalog_run in catalog_runs {
        let attempt_id = required_string(catalog_run, "/attemptId")?;
        let run_id = required_string(catalog_run, "/runId")?;
        let sequence = catalog_run
            .get("sequence")
            .and_then(Value::as_u64)
            .ok_or_else(|| std::io::Error::other("run catalog item omitted sequence"))?;
        if run_id == latest_run_id {
            latest_attempt_id = Some(attempt_id.clone());
        }
        assert!(catalogued_runs.insert(run_id.clone()));
        ordered_runs.push((sequence, attempt_id.clone(), run_id.clone()));
        assert!(expected_attempts_and_runs.insert(("attempt".to_owned(), attempt_id)));
        assert!(expected_attempts_and_runs.insert(("run".to_owned(), run_id)));
    }
    let latest_attempt_id = latest_attempt_id
        .ok_or_else(|| std::io::Error::other("latest run has no catalogued attempt"))?;
    ordered_runs.sort_by_key(|record| record.0);
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

    let mut observed_record_identities = std::collections::BTreeSet::new();
    let mut observed_record_order = Vec::new();
    append_plan_page_records(
        &first_page,
        &mut observed_record_identities,
        &mut observed_record_order,
    )?;

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
        append_plan_page_records(
            &page,
            &mut observed_record_identities,
            &mut observed_record_order,
        )?;
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
    assert_eq!(
        observed_record_order,
        expected_plan_record_order(&ordered_runs, &latest_attempt_id, &latest_run_id),
        "retention pages did not preserve retention-plan-items.v1 adjacency",
    );
    assert_plan_record_truth(
        &observed_record_identities,
        &expected_attempts_and_runs,
        &latest_attempt_id,
        &latest_run_id,
        COMPLETED_RUN_COUNT,
    );
    Ok(())
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
type PlanRecordOrder = (String, String, Option<u64>, String, Option<String>);

fn append_plan_page_records(
    page: &Value,
    observed_identities: &mut std::collections::BTreeSet<PlanRecordIdentity>,
    observed_order: &mut Vec<PlanRecordOrder>,
) -> Result<(), Box<dyn std::error::Error>> {
    for (identity, order) in plan_page_records(page)? {
        assert!(
            observed_identities.insert(identity.clone()),
            "retention plan record appeared more than once: {identity:?}"
        );
        observed_order.push(order);
    }
    Ok(())
}

fn plan_page_records(
    page: &Value,
) -> Result<Vec<(PlanRecordIdentity, PlanRecordOrder)>, Box<dyn std::error::Error>> {
    let mut records = Vec::new();
    for collection in ["items", "exclusions"] {
        let values = page
            .get(collection)
            .and_then(Value::as_array)
            .ok_or_else(|| std::io::Error::other(format!("plan page omitted {collection}")))?;
        for value in values {
            let kind = required_string(value, "/kind")?;
            let record_id = required_string(value, "/recordId")?;
            let (owning_sequence, discriminator, reason) = match collection {
                "items" => (
                    Some(
                        value
                            .get("owningSequence")
                            .and_then(Value::as_u64)
                            .ok_or_else(|| {
                                std::io::Error::other("plan item omitted owningSequence")
                            })?,
                    ),
                    required_string(value, "/identitySha256")?,
                    None,
                ),
                "exclusions" => {
                    let reason = required_string(value, "/reason/reason")?;
                    (None, reason.clone(), Some(reason))
                }
                _ => unreachable!(),
            };
            records.push((
                (
                    collection.to_owned(),
                    kind.clone(),
                    record_id.clone(),
                    discriminator,
                ),
                (
                    collection.to_owned(),
                    kind,
                    owning_sequence,
                    record_id,
                    reason,
                ),
            ));
        }
    }
    Ok(records)
}

fn expected_plan_record_order(
    ordered_runs: &[(u64, String, String)],
    latest_attempt_id: &str,
    latest_run_id: &str,
) -> Vec<PlanRecordOrder> {
    let mut expected = Vec::new();
    for kind in ["attempt", "run", "evidence"] {
        for (sequence, attempt_id, run_id) in ordered_runs {
            if run_id == latest_run_id {
                continue;
            }
            let record_id = match kind {
                "attempt" => attempt_id.clone(),
                "run" => run_id.clone(),
                "evidence" => format!("run:{run_id}/evidence"),
                _ => unreachable!(),
            };
            expected.push((
                "items".to_owned(),
                kind.to_owned(),
                Some(*sequence),
                record_id,
                None,
            ));
        }
    }
    expected.extend([
        (
            "exclusions".to_owned(),
            "attempt".to_owned(),
            None,
            latest_attempt_id.to_owned(),
            Some("latest-attempt".to_owned()),
        ),
        (
            "exclusions".to_owned(),
            "attempt".to_owned(),
            None,
            latest_attempt_id.to_owned(),
            Some("latest-completed".to_owned()),
        ),
        (
            "exclusions".to_owned(),
            "run".to_owned(),
            None,
            latest_run_id.to_owned(),
            Some("latest-completed".to_owned()),
        ),
    ]);
    expected
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
