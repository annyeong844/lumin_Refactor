use std::fs;
use std::path::Path;

use serde_json::Value;

use super::retention_support::{audit, json};
use super::support::{assert_status, run};
use super::{
    contains_record, prepare_pagination_plan, required_string, required_usize, show_pagination_plan,
};

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
