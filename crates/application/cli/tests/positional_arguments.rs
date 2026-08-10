#[allow(dead_code)]
mod support;

use std::fs;

use serde_json::Value;
use support::{assert_status, run};

#[test]
fn leading_options_are_not_consumed_as_required_positional_ids()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let cases: &[(&[&str], &str)] = &[
        (
            &["post-write", "--operation-id", "op-close"],
            "--operation-id",
        ),
        (&["gate", "show", "--revision", "1"], "--revision"),
        (&["gate", "findings", "--revision", "1"], "--revision"),
        (
            &["gate", "explain", "--revision", "1", "finding"],
            "--revision",
        ),
        (
            &[
                "gate",
                "abandon",
                "--operation-id",
                "op-abandon",
                "--reason",
                "done",
            ],
            "--operation-id",
        ),
        (&["operation", "show", "--format", "json"], "--format"),
        (
            &[
                "runs",
                "pin",
                "--reason",
                "review",
                "--operation-id",
                "op-pin",
            ],
            "--reason",
        ),
        (
            &["runs", "unpin", "--operation-id", "op-unpin"],
            "--operation-id",
        ),
        (
            &["runs", "prune", "plan", "show", "--cursor", "cursor"],
            "--cursor",
        ),
        (
            &["runs", "prune", "confirm", "--operation-id", "op-confirm"],
            "--operation-id",
        ),
        (
            &["gate", "prune", "plan", "show", "--cursor", "cursor"],
            "--cursor",
        ),
        (
            &["gate", "prune", "confirm", "--operation-id", "op-confirm"],
            "--operation-id",
        ),
    ];

    for (arguments, leading_option) in cases {
        let result = run(root.path(), arguments)?;
        assert_status(&result, 2);
        assert!(result.stdout.is_empty(), "arguments={arguments:?}");
        assert_eq!(
            result.stderr,
            format!("lumin: unknown command or argument: {leading_option}\n"),
            "arguments={arguments:?}",
        );
    }

    Ok(())
}

#[test]
fn option_shaped_operation_ids_are_recoverable_through_explicit_positional_escape()
-> Result<(), Box<dyn std::error::Error>> {
    const OPERATION_ID: &str = "--retry-token";

    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(root.path().join("src/lib.ts"), "export const value = 1;\n")?;

    let opened = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            OPERATION_ID,
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);

    let ambiguous = run(root.path(), &["operation", "show", OPERATION_ID])?;
    assert_status(&ambiguous, 2);
    assert_eq!(
        ambiguous.stderr,
        "lumin: unknown command or argument: --retry-token\n"
    );

    let recovered = run(root.path(), &["operation", "show", "--", OPERATION_ID])?;
    assert_status(&recovered, 0);
    let recovered: Value = serde_json::from_str(&recovered.stdout)?;
    assert_eq!(
        recovered.get("kind").and_then(Value::as_str),
        Some("pre-write")
    );
    assert_eq!(
        recovered.get("status").and_then(Value::as_str),
        Some("committed")
    );
    assert_eq!(
        recovered
            .pointer("/result/operationId")
            .and_then(Value::as_str),
        Some(OPERATION_ID)
    );
    assert!(
        recovered
            .pointer("/result/gateId")
            .and_then(Value::as_str)
            .is_some()
    );
    Ok(())
}
