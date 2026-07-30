use super::*;

#[test]
fn retention_mutations_recover_post_commit_delivery_failure_without_duplication()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let first_audited = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&first_audited, 0);
    let prunable_run_id = field(&first_audited.stdout, "runId")?;
    fs::write(root.path().join("src/lib.ts"), "export const value = 2;\n")?;
    let audited = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audited, 0);
    let run_id = field(&audited.stdout, "runId")?;
    assert_ne!(prunable_run_id, run_id);

    let pin_args = [
        "runs",
        "pin",
        run_id.as_str(),
        "--operation-id",
        "lifecycle-pin",
        "--reason",
        "primary review",
    ];
    let pin_started_unix_millis = unix_millis()?;
    assert_delivery_failure(root.path(), &pin_args, "lifecycle-pin")?;
    let pin_finished_unix_millis = unix_millis()?;
    let pin_operation = recovered_retention_result(root.path(), "lifecycle-pin", "run-pin")?;
    let created_unix_millis = required_u64(&pin_operation, "/pin/createdUnixMillis")?;
    assert!(
        (pin_started_unix_millis..=pin_finished_unix_millis).contains(&created_unix_millis),
        "pin timestamp {created_unix_millis} fell outside {pin_started_unix_millis}..={pin_finished_unix_millis}"
    );
    let first_pin = required_string(&pin_operation, "/pin/pinId")?;
    let pin_retry = run(root.path(), &pin_args)?;
    assert_status(&pin_retry, 0);
    assert_eq!(
        json(&pin_retry.stdout)?.pointer("/pin").cloned(),
        pin_operation.pointer("/pin").cloned()
    );
    assert_conflict(run(
        root.path(),
        &[
            "runs",
            "pin",
            run_id.as_str(),
            "--operation-id",
            "lifecycle-pin",
            "--reason",
            "different review",
        ],
    )?);

    let second_pin_output = run(
        root.path(),
        &[
            "runs",
            "pin",
            run_id.as_str(),
            "--operation-id",
            "lifecycle-pin-secondary",
            "--reason",
            "secondary review",
        ],
    )?;
    assert_status(&second_pin_output, 0);
    let second_pin = required_string(&json(&second_pin_output.stdout)?, "/pin/pinId")?;
    assert_ne!(first_pin, second_pin);

    let unpin_args = [
        "runs",
        "unpin",
        first_pin.as_str(),
        "--operation-id",
        "lifecycle-unpin",
    ];
    assert_delivery_failure(root.path(), &unpin_args, "lifecycle-unpin")?;
    let unpin_operation = recovered_retention_result(root.path(), "lifecycle-unpin", "run-unpin")?;
    assert_eq!(required_string(&unpin_operation, "/pinId")?, first_pin);
    let unpin_retry = run(root.path(), &unpin_args)?;
    assert_status(&unpin_retry, 0);
    let unpin_retry = json(&unpin_retry.stdout)?;
    assert_eq!(required_string(&unpin_retry, "/pin/pinId")?, first_pin);
    assert_eq!(
        required_string(&unpin_retry, "/pin/removedOperationId")?,
        "lifecycle-unpin"
    );
    assert_conflict(run(
        root.path(),
        &[
            "runs",
            "unpin",
            second_pin.as_str(),
            "--operation-id",
            "lifecycle-unpin",
        ],
    )?);

    let plan_args = [
        "runs",
        "prune",
        "plan",
        "--before",
        "9000000000000",
        "--operation-id",
        "lifecycle-plan",
    ];
    assert_delivery_failure(root.path(), &plan_args, "lifecycle-plan")?;
    let plan_operation =
        recovered_retention_result(root.path(), "lifecycle-plan", "run-prune-plan")?;
    let plan_result = required_value(&plan_operation, "/result")?;
    let plan_id = required_string(plan_result, "/planId")?;
    let plan_retry = run(root.path(), &plan_args)?;
    assert_status(&plan_retry, 0);
    assert_eq!(
        json(&plan_retry.stdout)?.pointer("/result"),
        Some(plan_result)
    );
    assert_conflict(run(
        root.path(),
        &[
            "runs",
            "prune",
            "plan",
            "--before",
            "8999999999999",
            "--operation-id",
            "lifecycle-plan",
        ],
    )?);
    let shown_plan = show_run_plan(root.path(), &plan_id)?;
    assert_eq!(
        active_pin_ids(&shown_plan, "run", &run_id)?,
        vec![second_pin]
    );
    assert_plan_contains_record(&shown_plan, "items", "run", &prunable_run_id)?;

    let second_plan_output = run(
        root.path(),
        &[
            "runs",
            "prune",
            "plan",
            "--before",
            "9000000000000",
            "--operation-id",
            "lifecycle-plan-secondary",
        ],
    )?;
    assert_status(&second_plan_output, 0);
    let second_plan = required_string(&json(&second_plan_output.stdout)?, "/result/planId")?;

    let confirm_args = [
        "runs",
        "prune",
        "confirm",
        plan_id.as_str(),
        "--operation-id",
        "lifecycle-confirm",
    ];
    assert_delivery_failure(root.path(), &confirm_args, "lifecycle-confirm")?;
    let confirm_operation =
        recovered_retention_result(root.path(), "lifecycle-confirm", "run-prune-confirm")?;
    let confirm_result = required_value(&confirm_operation, "/result")?;
    assert_eq!(required_string(confirm_result, "/status")?, "pruned");
    assert_eq!(required_string(confirm_result, "/planId")?, plan_id);
    let confirm_retry = run(root.path(), &confirm_args)?;
    assert_status(&confirm_retry, 0);
    assert_eq!(
        json(&confirm_retry.stdout)?.pointer("/result"),
        Some(confirm_result)
    );
    assert_conflict(run(
        root.path(),
        &[
            "runs",
            "prune",
            "confirm",
            second_plan.as_str(),
            "--operation-id",
            "lifecycle-confirm",
        ],
    )?);
    assert_eq!(
        required_string(&show_run_plan(root.path(), &plan_id)?, "/state")?,
        "pruned"
    );
    assert_eq!(
        required_string(&show_run_plan(root.path(), &second_plan)?, "/state")?,
        "prepared"
    );
    assert_tombstone(
        root.path(),
        &["overview", "--run", prunable_run_id.as_str()],
        &plan_id,
    )?;
    Ok(())
}
