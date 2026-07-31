use std::fs;

use serde_json::Value;

use super::*;

#[test]
fn audit_then_findings_reopens_the_persisted_run() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("lib.ts"), "export const dead = 1;")?;
    let audit = execute(
        root.path(),
        vec!["audit".into(), "--jobs".into(), "1".into()],
    );
    assert_eq!(audit.exit_code, 0, "{}", audit.stderr);
    let audit_json: Value = serde_json::from_str(&audit.stdout)?;
    let run_id = audit_json
        .get("runId")
        .and_then(Value::as_str)
        .ok_or("audit response omitted runId")?;

    let findings = execute(
        root.path(),
        vec![
            "findings".into(),
            "--run".into(),
            run_id.into(),
            "--area".into(),
            "dead-code".into(),
        ],
    );
    assert_eq!(findings.exit_code, 0, "{}", findings.stderr);
    let findings_json: Value = serde_json::from_str(&findings.stdout)?;
    assert_eq!(findings_json.get("filters"), Some(&serde_json::json!({})));
    assert_eq!(
        findings_json.get("scopeTotal").and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(findings_json.get("total").and_then(Value::as_u64), Some(1));
    Ok(())
}

#[test]
fn unfiltered_query_keeps_review_only_findings() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(
        root.path().join("lumin.json"),
        r#"{"schemaVersion":"lumin-config.v1","scan":{"roles":[{"pattern":"src/vendor.ts","role":"vendor"}]}}"#,
    )?;
    fs::write(
        root.path().join("src/authored.ts"),
        "export const authored = 1;",
    )?;
    fs::write(
        root.path().join("src/generated.ts"),
        "// @generated\nexport const generated = 1;",
    )?;
    fs::write(
        root.path().join("src/vendor.ts"),
        "export const vendor = 1;",
    )?;
    let audit = execute(
        root.path(),
        vec!["audit".into(), "--jobs".into(), "2".into()],
    );
    assert_eq!(audit.exit_code, 0, "{}", audit.stderr);
    let audit_json: Value = serde_json::from_str(&audit.stdout)?;
    let run_id = audit_json
        .get("runId")
        .and_then(Value::as_str)
        .ok_or("audit response omitted runId")?;
    let findings = execute(
        root.path(),
        vec![
            "findings".into(),
            "--run".into(),
            run_id.into(),
            "--area".into(),
            "dead-code".into(),
        ],
    );
    assert_eq!(findings.exit_code, 0, "{}", findings.stderr);
    let response: Value = serde_json::from_str(&findings.stdout)?;
    assert_eq!(response.get("filters"), Some(&serde_json::json!({})));
    assert_eq!(response.get("scopeTotal").and_then(Value::as_u64), Some(3));
    assert_eq!(response.get("total").and_then(Value::as_u64), Some(3));
    let review_only = response
        .get("items")
        .and_then(Value::as_array)
        .ok_or("findings response omitted items")?
        .iter()
        .filter(|item| {
            item.pointer("/disposition/kind").and_then(Value::as_str) == Some("review-only")
        })
        .count();
    assert_eq!(review_only, 2);
    Ok(())
}

#[test]
fn parse_failure_is_persisted_as_incomplete() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("broken.ts"), "export const = ;")?;
    let audit = execute(root.path(), vec!["audit".into()]);
    assert_eq!(audit.exit_code, 0, "{}", audit.stderr);
    let response: Value = serde_json::from_str(&audit.stdout)?;
    assert_eq!(
        response.get("status").and_then(Value::as_str),
        Some("incomplete")
    );
    assert_eq!(
        response.get("findingCount").and_then(Value::as_u64),
        Some(0)
    );
    assert!(
        response
            .get("limitationCount")
            .and_then(Value::as_u64)
            .is_some_and(|count| count > 0)
    );
    Ok(())
}

#[test]
fn resolution_profile_override_is_validated_and_persisted() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(
        root.path().join("package.json"),
        r#"{"name":"app","type":"module"}"#,
    )?;
    fs::write(
        root.path().join("tsconfig.json"),
        r#"{"compilerOptions":{"moduleResolution":"node16"}}"#,
    )?;
    fs::write(root.path().join("src/lib.ts"), "export const used = 1;")?;
    fs::write(
        root.path().join("src/main.ts"),
        "import { used } from './lib'; console.log(used);",
    )?;

    let invalid = execute(
        root.path(),
        vec![
            "audit".into(),
            "--resolution-profile".into(),
            "browser".into(),
        ],
    );
    assert_eq!(invalid.exit_code, 2);
    assert!(
        invalid
            .stderr
            .contains("unknown resolution profile: browser")
    );

    let audit = execute(
        root.path(),
        vec![
            "audit".into(),
            "--jobs".into(),
            "1".into(),
            "--resolution-profile".into(),
            "node10".into(),
        ],
    );
    assert_eq!(audit.exit_code, 0, "{}", audit.stderr);
    let audit_json: Value = serde_json::from_str(&audit.stdout)?;
    let run_id = audit_json
        .get("runId")
        .and_then(Value::as_str)
        .ok_or("audit response omitted runId")?;
    let overview = execute(
        root.path(),
        vec!["overview".into(), "--run".into(), run_id.into()],
    );
    assert_eq!(overview.exit_code, 0, "{}", overview.stderr);
    let overview_json: Value = serde_json::from_str(&overview.stdout)?;
    let profiles = overview_json
        .get("resolutionProfiles")
        .and_then(Value::as_array)
        .ok_or("overview omitted resolutionProfiles")?;
    assert!(!profiles.is_empty());
    assert!(profiles.iter().all(|profile| {
        profile.get("profile").and_then(Value::as_str) == Some("node")
            && profile.pointer("/source/kind").and_then(Value::as_str) == Some("invocation")
    }));
    Ok(())
}
