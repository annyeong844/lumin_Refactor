#[allow(dead_code)]
mod support;

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
