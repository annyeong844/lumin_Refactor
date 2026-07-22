use std::fs;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

#[test]
fn first_failed_attempt_remains_visible_without_a_completed_run()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(
        root.path().join("src/lib.ts"),
        "export const visible = 1;\n",
    )?;
    fs::write(root.path().join("lumin.json"), b"{\n")?;

    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 1);

    let overview = run(root.path(), &["overview"])?;
    assert_status(&overview, 0);
    assert_eq!(
        field(&overview.stdout, "schemaVersion")?,
        "lumin.attempt-overview.v1"
    );
    let body: Value = serde_json::from_str(&overview.stdout)?;
    assert_eq!(
        body.pointer("/scope/kind").and_then(Value::as_str),
        Some("attempt")
    );
    assert_eq!(
        body.pointer("/latestAttempt/sequence")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        body.pointer("/latestAttempt/status")
            .and_then(Value::as_str),
        Some("failed")
    );
    assert!(
        body.pointer("/latestAttempt/failure")
            .and_then(Value::as_str)
            .is_some_and(|failure| !failure.is_empty())
    );
    Ok(())
}
