#[allow(dead_code)]
mod support;

use support::{assert_status, run};

#[test]
fn help_agent_owns_the_recovery_workflow_without_creating_state()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let output = run(root.path(), &["help-agent"])?;
    assert_status(&output, 0);
    assert!(output.stderr.is_empty());
    assert!(output.stdout.starts_with("Lumin agent workflow\n"));
    assert!(
        output
            .stdout
            .contains("lumin operation show <operation-id> --format json")
    );
    assert!(output.stdout.contains("lumin store migrate --format json"));
    assert!(output.stdout.contains(
        "{\"schemaVersion\":\"lumin.lifecycle-store-migration.v1\",\"storeSchema\":\"lumin-lifecycle-store-header.v13\",\"status\":\"ready\"}",
    ));
    assert!(output.stdout.contains("Never read or modify .lumin."));
    assert!(!root.path().join(".lumin").exists());
    Ok(())
}

#[test]
fn help_agent_rejects_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let output = run(root.path(), &["help-agent", "extra"])?;
    assert_status(&output, 2);
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, "lumin: unknown command or argument: extra\n");
    assert!(!root.path().join(".lumin").exists());
    Ok(())
}
