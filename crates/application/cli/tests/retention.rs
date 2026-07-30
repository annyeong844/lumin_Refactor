use std::path::Path;

use serde_json::Value;

use support::{ProcessResult, assert_status, run};

fn required_string(value: &Value, pointer: &str) -> Result<String, Box<dyn std::error::Error>> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other(format!("response omitted {pointer}")).into())
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

fn prepare_pagination_plan(
    root: &Path,
    operation_id: &str,
) -> Result<ProcessResult, Box<dyn std::error::Error>> {
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
) -> Result<ProcessResult, Box<dyn std::error::Error>> {
    let mut arguments = vec!["runs", "prune", "plan", "show", plan_id];
    if let Some(cursor) = cursor {
        arguments.extend(["--cursor", cursor]);
    }
    let output = run(root, &arguments)?;
    assert_status(&output, 0);
    Ok(output)
}

fn required_usize(value: &Value, pointer: &str) -> Result<usize, Box<dyn std::error::Error>> {
    value
        .pointer(pointer)
        .and_then(Value::as_u64)
        .ok_or_else(|| std::io::Error::other(format!("response omitted {pointer}")))?
        .try_into()
        .map_err(Into::into)
}

#[path = "support/retention_plan.rs"]
mod retention_plan_support;
#[path = "support/retention.rs"]
mod retention_support;
mod support;

#[path = "retention/lifecycle.rs"]
mod lifecycle;
#[path = "retention/pagination.rs"]
mod pagination;
#[path = "retention/pins.rs"]
mod pins;
