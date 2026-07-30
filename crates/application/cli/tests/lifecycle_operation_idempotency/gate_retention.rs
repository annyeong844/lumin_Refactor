use super::*;

#[test]
fn gate_retention_mutations_recover_post_commit_delivery_failure_without_duplication()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let gate_id = open_gate(root.path(), "lifecycle-gate-retention-open", "src/lib.ts")?;
    fs::write(root.path().join("src/lib.ts"), "export const value = 3;\n")?;
    let closed = run(
        root.path(),
        &[
            "post-write",
            gate_id.as_str(),
            "--operation-id",
            "lifecycle-gate-retention-close",
        ],
    )?;
    assert_status(&closed, 0);

    let plan_args = [
        "gate",
        "prune",
        "plan",
        "--terminal-before",
        "9000000000000",
        "--operation-id",
        "lifecycle-gate-plan",
    ];
    assert_delivery_failure(root.path(), &plan_args, "lifecycle-gate-plan")?;
    let plan_operation =
        recovered_retention_result(root.path(), "lifecycle-gate-plan", "gate-prune-plan")?;
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
            "gate",
            "prune",
            "plan",
            "--terminal-before",
            "8999999999999",
            "--operation-id",
            "lifecycle-gate-plan",
        ],
    )?);
    let shown_plan = show_gate_plan(root.path(), &plan_id)?;
    assert_plan_contains_record(&shown_plan, "items", "gate", &gate_id)?;

    let second_plan_output = run(
        root.path(),
        &[
            "gate",
            "prune",
            "plan",
            "--terminal-before",
            "9000000000000",
            "--operation-id",
            "lifecycle-gate-plan-secondary",
        ],
    )?;
    assert_status(&second_plan_output, 0);
    let second_plan = required_string(&json(&second_plan_output.stdout)?, "/result/planId")?;

    let confirm_args = [
        "gate",
        "prune",
        "confirm",
        plan_id.as_str(),
        "--operation-id",
        "lifecycle-gate-confirm",
    ];
    assert_delivery_failure(root.path(), &confirm_args, "lifecycle-gate-confirm")?;
    let confirm_operation =
        recovered_retention_result(root.path(), "lifecycle-gate-confirm", "gate-prune-confirm")?;
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
            "gate",
            "prune",
            "confirm",
            second_plan.as_str(),
            "--operation-id",
            "lifecycle-gate-confirm",
        ],
    )?);
    assert_eq!(
        required_string(&show_gate_plan(root.path(), &plan_id)?, "/state")?,
        "pruned"
    );
    assert_eq!(
        required_string(&show_gate_plan(root.path(), &second_plan)?, "/state")?,
        "prepared"
    );
    assert_tombstone(root.path(), &["gate", "show", gate_id.as_str()], &plan_id)?;
    Ok(())
}
