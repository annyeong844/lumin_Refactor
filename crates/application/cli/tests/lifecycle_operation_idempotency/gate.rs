use super::*;

#[test]
fn gate_mutations_recover_post_commit_delivery_failure_without_duplication()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let pre_args = [
        "pre-write",
        "--operation-id",
        "lifecycle-pre",
        "--path",
        "src/lib.ts",
        "--jobs",
        "1",
    ];
    assert_delivery_failure(root.path(), &pre_args, "lifecycle-pre")?;
    let pre_result = recovered_gate_result(root.path(), "lifecycle-pre", "pre-write")?;
    let gate_id = required_string(&pre_result, "/gateId")?;
    let pre_retry = run(root.path(), &pre_args)?;
    assert_status(&pre_retry, 0);
    assert_eq!(json(&pre_retry.stdout)?, pre_result);
    assert_conflict(run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "lifecycle-pre",
            "--path",
            "src/other.ts",
            "--jobs",
            "1",
        ],
    )?);
    assert_gate_history(root.path(), &gate_id, "active", &["lifecycle-pre"])?;

    fs::write(root.path().join("src/lib.ts"), "export const value = 2;\n")?;
    let post_args = [
        "post-write",
        gate_id.as_str(),
        "--operation-id",
        "lifecycle-post",
    ];
    assert_delivery_failure(root.path(), &post_args, "lifecycle-post")?;
    let post_result = recovered_gate_result(root.path(), "lifecycle-post", "post-write")?;
    assert_eq!(required_u64(&post_result, "/revision")?, 1);
    let post_retry = run(root.path(), &post_args)?;
    assert_status(&post_retry, 0);
    assert_eq!(json(&post_retry.stdout)?, post_result);
    assert_gate_history(
        root.path(),
        &gate_id,
        "closed",
        &["lifecycle-pre", "lifecycle-post"],
    )?;

    let abandoned_gate = open_gate(root.path(), "lifecycle-abandon-pre", "src/other.ts")?;
    assert_conflict(run(
        root.path(),
        &[
            "post-write",
            abandoned_gate.as_str(),
            "--operation-id",
            "lifecycle-post",
        ],
    )?);
    let abandon_args = [
        "gate",
        "abandon",
        abandoned_gate.as_str(),
        "--operation-id",
        "lifecycle-abandon",
        "--reason",
        "cancelled edit",
    ];
    assert_delivery_failure(root.path(), &abandon_args, "lifecycle-abandon")?;
    let abandon_result = recovered_gate_result(root.path(), "lifecycle-abandon", "gate-abandon")?;
    assert_eq!(
        required_string(&abandon_result, "/reason")?,
        "cancelled edit"
    );
    let abandon_retry = run(root.path(), &abandon_args)?;
    assert_status(&abandon_retry, 0);
    assert_eq!(json(&abandon_retry.stdout)?, abandon_result);
    assert_conflict(run(
        root.path(),
        &[
            "gate",
            "abandon",
            abandoned_gate.as_str(),
            "--operation-id",
            "lifecycle-abandon",
            "--reason",
            "different reason",
        ],
    )?);
    assert_gate_history(
        root.path(),
        &abandoned_gate,
        "abandoned",
        &["lifecycle-abandon-pre", "lifecycle-abandon"],
    )?;
    Ok(())
}
