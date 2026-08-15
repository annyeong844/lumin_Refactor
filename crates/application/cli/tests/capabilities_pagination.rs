mod support;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;

use lumin_model::BuildIdentity;
use serde_json::Value;

use support::{assert_status, field, run};

fn expected_capabilities() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("dead-code.v1".to_owned(), "complete".to_owned()),
        (
            "inventory/dependency-ownership.v1".to_owned(),
            "complete".to_owned(),
        ),
        ("sfc/astro.v1".to_owned(), "unavailable".to_owned()),
        ("sfc/svelte.v1".to_owned(), "unavailable".to_owned()),
        ("sfc/vue.v1".to_owned(), "complete".to_owned()),
    ])
}

fn assert_collection_envelope(page: &Value, returned: u64, truncated: bool) {
    assert_eq!(
        page.get("schemaVersion").and_then(Value::as_str),
        Some("lumin.collection.v1")
    );
    assert_eq!(
        page.get("ordering").and_then(Value::as_str),
        Some("capabilities.v1")
    );
    assert_eq!(
        page.get("filters")
            .and_then(Value::as_object)
            .map(serde_json::Map::is_empty),
        Some(true)
    );
    assert_eq!(page.get("scopeTotal").and_then(Value::as_u64), Some(5));
    assert_eq!(page.get("total").and_then(Value::as_u64), Some(5));
    assert_eq!(page.get("returned").and_then(Value::as_u64), Some(returned));
    assert_eq!(
        page.get("truncated").and_then(Value::as_bool),
        Some(truncated)
    );
    if truncated {
        assert!(page.get("nextCursor").and_then(Value::as_str).is_some());
    } else {
        assert_eq!(page.get("nextCursor"), Some(&Value::Null));
    }
}

fn capability_rows<'a>(
    pages: impl IntoIterator<Item = &'a Value>,
) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let mut rows = BTreeMap::new();
    for page in pages {
        let items = page
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| std::io::Error::other("capability items missing"))?;
        for item in items {
            let object = item
                .as_object()
                .ok_or_else(|| std::io::Error::other("capability item must be an object"))?;
            if object.len() != 2 {
                return Err(std::io::Error::other(
                    "public capability item must contain only ID and state",
                )
                .into());
            }
            let id = object
                .get("capabilityId")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("capabilityId missing"))?
                .to_owned();
            let state = object
                .get("state")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("capability state missing"))?
                .to_owned();
            if rows.insert(id.clone(), state).is_some() {
                return Err(std::io::Error::other(format!(
                    "duplicate capabilityId across pages: {id}"
                ))
                .into());
            }
        }
    }
    Ok(rows)
}

/// Binary capabilities 3+2 pagination before .lumin and no state creation.
#[test]
fn binary_capabilities_pagination_without_state_directory() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;

    // Query capabilities in binary scope; must not create .lumin
    let first = run(root.path(), &["capabilities", "--format", "json"])?;
    assert_status(&first, 0);
    assert!(
        !root.path().join(".lumin").exists(),
        "binary capabilities must not create .lumin"
    );

    let first_json: Value = serde_json::from_str(&first.stdout)?;
    assert_collection_envelope(&first_json, 3, true);

    let scope = first_json.get("scope").ok_or("missing scope")?;
    assert_eq!(scope.get("kind").and_then(Value::as_str), Some("binary"));
    assert!(scope.get("buildId").and_then(Value::as_str).is_some());

    let items = first_json
        .get("items")
        .and_then(Value::as_array)
        .ok_or("missing items")?;
    assert_eq!(items.len(), 3);
    // Verify items have capabilityId and state
    for item in items {
        assert!(item.get("capabilityId").and_then(Value::as_str).is_some());
        assert!(item.get("state").and_then(Value::as_str).is_some());
    }

    // Verify sorted by capabilityId
    let ids: Vec<&str> = items
        .iter()
        .filter_map(|item| item.get("capabilityId").and_then(Value::as_str))
        .collect();
    let mut sorted_ids = ids.clone();
    sorted_ids.sort();
    assert_eq!(ids, sorted_ids);

    // Page 2
    let next_cursor = first_json
        .get("nextCursor")
        .and_then(Value::as_str)
        .ok_or("missing nextCursor")?;
    let second = run(
        root.path(),
        &["capabilities", "--cursor", next_cursor, "--format", "json"],
    )?;
    assert_status(&second, 0);

    let second_json: Value = serde_json::from_str(&second.stdout)?;
    assert_collection_envelope(&second_json, 2, false);

    assert_eq!(
        capability_rows([&first_json, &second_json])?,
        expected_capabilities(),
        "binary pages must return every grounded capability exactly once",
    );
    assert_eq!(
        second_json.pointer("/scope/buildId"),
        first_json.pointer("/scope/buildId"),
    );

    // Still no .lumin
    assert!(
        !root.path().join(".lumin").exists(),
        "binary capabilities must never create .lumin"
    );
    Ok(())
}

/// Cross-directory binary cursor success: cursor from one directory works in another.
#[test]
fn binary_cursor_works_across_directories() -> Result<(), Box<dyn std::error::Error>> {
    let dir_a = tempfile::tempdir()?;
    let dir_b = tempfile::tempdir()?;

    let first = run(dir_a.path(), &["capabilities", "--format", "json"])?;
    assert_status(&first, 0);
    let first_json: Value = serde_json::from_str(&first.stdout)?;
    let next_cursor = first_json
        .get("nextCursor")
        .and_then(Value::as_str)
        .ok_or("missing nextCursor")?;

    // Use cursor from dir_a in dir_b -- must succeed because binary scope is repository-independent
    let second = run(
        dir_b.path(),
        &["capabilities", "--cursor", next_cursor, "--format", "json"],
    )?;
    assert_status(&second, 0);
    let second_json: Value = serde_json::from_str(&second.stdout)?;
    assert_eq!(second_json.get("returned").and_then(Value::as_u64), Some(2));
    Ok(())
}

/// Run capabilities 3+2 with exact sorted IDs/states and cursor survives unrelated audit.
#[test]
fn run_capabilities_pagination_and_cursor_survives_audit() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("lib.ts"), "export const dead = 1;")?;

    let audit = run(root.path(), &["audit", "--jobs", "1", "--format", "json"])?;
    assert_status(&audit, 0);
    let run_id = field(&audit.stdout, "runId")?;

    // First page of run capabilities
    let first = run(
        root.path(),
        &["capabilities", "--run", &run_id, "--format", "json"],
    )?;
    assert_status(&first, 0);

    let first_json: Value = serde_json::from_str(&first.stdout)?;
    assert_collection_envelope(&first_json, 3, true);

    let scope = first_json.get("scope").ok_or("missing scope")?;
    assert_eq!(scope.get("kind").and_then(Value::as_str), Some("run"));

    let items = first_json
        .get("items")
        .and_then(Value::as_array)
        .ok_or("missing items")?;
    // Verify exact sorted IDs and states
    let ids: Vec<&str> = items
        .iter()
        .filter_map(|item| item.get("capabilityId").and_then(Value::as_str))
        .collect();
    let mut sorted_ids = ids.clone();
    sorted_ids.sort();
    assert_eq!(
        ids, sorted_ids,
        "capabilities must be sorted by capabilityId"
    );

    // Run an unrelated audit to prove cursor survives
    fs::write(root.path().join("other.ts"), "export const x = 1;")?;
    let audit2 = run(root.path(), &["audit", "--jobs", "1", "--format", "json"])?;
    assert_status(&audit2, 0);

    // Use the original cursor
    let next_cursor = first_json
        .get("nextCursor")
        .and_then(Value::as_str)
        .ok_or("missing nextCursor")?;
    let second = run(
        root.path(),
        &[
            "capabilities",
            "--run",
            &run_id,
            "--cursor",
            next_cursor,
            "--format",
            "json",
        ],
    )?;
    assert_status(&second, 0);
    let second_json: Value = serde_json::from_str(&second.stdout)?;
    assert_collection_envelope(&second_json, 2, false);
    assert_eq!(
        capability_rows([&first_json, &second_json])?,
        expected_capabilities(),
        "run pages must return exact recorded states exactly once",
    );
    Ok(())
}

#[test]
fn dependency_ownership_run_capability_reports_owner_gaps() -> Result<(), Box<dyn std::error::Error>>
{
    let root = tempfile::tempdir()?;
    fs::write(
        root.path().join("package.json"),
        r#"{"name":"app","private":true,"dependencies":[]}"#,
    )?;
    fs::write(root.path().join("lib.ts"), "export const unused = 1;\n")?;

    let audit = run(root.path(), &["audit", "--jobs", "1", "--format", "json"])?;
    assert_status(&audit, 0);
    let audit_json: Value = serde_json::from_str(&audit.stdout)?;
    assert_eq!(
        audit_json.get("findingCount").and_then(Value::as_u64),
        Some(1),
        "dependency-owner uncertainty must not suppress complete dead-code evidence",
    );
    let run_id = field(&audit.stdout, "runId")?;
    let first = run(
        root.path(),
        &["capabilities", "--run", &run_id, "--format", "json"],
    )?;
    assert_status(&first, 0);
    let first: Value = serde_json::from_str(&first.stdout)?;
    let cursor = first
        .get("nextCursor")
        .and_then(Value::as_str)
        .ok_or("missing capability continuation")?;
    let second = run(
        root.path(),
        &[
            "capabilities",
            "--run",
            &run_id,
            "--cursor",
            cursor,
            "--format",
            "json",
        ],
    )?;
    assert_status(&second, 0);
    let second: Value = serde_json::from_str(&second.stdout)?;

    let capabilities = capability_rows([&first, &second])?;
    assert_eq!(
        capabilities.get("dead-code.v1").map(String::as_str),
        Some("complete"),
    );
    assert_eq!(
        capabilities
            .get("inventory/dependency-ownership.v1")
            .map(String::as_str),
        Some("incomplete"),
    );
    Ok(())
}

#[test]
fn binary_cursor_rejects_different_build_identity() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let first_identity = BuildIdentity::derive("lumin-cli", "0.1.0", Some("first"), "registry");
    let second_identity = BuildIdentity::derive("lumin-cli", "0.1.0", Some("second"), "registry");
    let first = lumin_cli::execute_with_build_identity(
        root.path(),
        vec![OsString::from("capabilities")],
        &first_identity,
    );
    assert_eq!(first.exit_code, 0, "stderr={}", first.stderr);
    let first_json: Value = serde_json::from_str(&first.stdout)?;
    let cursor = first_json
        .get("nextCursor")
        .and_then(Value::as_str)
        .ok_or("missing binary cursor")?;
    let rejected = lumin_cli::execute_with_build_identity(
        root.path(),
        vec![
            OsString::from("capabilities"),
            OsString::from("--cursor"),
            OsString::from(cursor),
        ],
        &second_identity,
    );
    assert_eq!(rejected.exit_code, 2, "stderr={}", rejected.stderr);
    assert!(rejected.stdout.is_empty());
    Ok(())
}

#[test]
fn binary_and_run_cursors_cannot_cross_scope() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("lib.ts"), "export const dead = 1;")?;
    let binary = run(root.path(), &["capabilities"])?;
    assert_status(&binary, 0);
    let binary_json: Value = serde_json::from_str(&binary.stdout)?;
    let binary_cursor = binary_json
        .get("nextCursor")
        .and_then(Value::as_str)
        .ok_or("missing binary cursor")?;

    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    let run_id = field(&audit.stdout, "runId")?;
    let run_page = run(root.path(), &["capabilities", "--run", &run_id])?;
    assert_status(&run_page, 0);
    let run_json: Value = serde_json::from_str(&run_page.stdout)?;
    let run_cursor = run_json
        .get("nextCursor")
        .and_then(Value::as_str)
        .ok_or("missing run cursor")?;

    let binary_into_run = run(
        root.path(),
        &["capabilities", "--run", &run_id, "--cursor", binary_cursor],
    )?;
    assert_status(&binary_into_run, 2);
    assert!(binary_into_run.stdout.is_empty());

    let run_into_binary = run(root.path(), &["capabilities", "--cursor", run_cursor])?;
    assert_status(&run_into_binary, 2);
    assert!(run_into_binary.stdout.is_empty());
    Ok(())
}

/// Malformed binary cursor rejection: empty stdout, exit 2.
#[test]
fn malformed_binary_cursor_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;

    let result = run(
        root.path(),
        &[
            "capabilities",
            "--cursor",
            "not-valid-base64!!!!",
            "--format",
            "json",
        ],
    )?;
    assert_status(&result, 2);
    assert!(result.stdout.is_empty());
    Ok(())
}

/// Cross-run cursor rejected with empty stdout.
#[test]
fn cross_run_capabilities_cursor_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("lib.ts"), "export const dead = 1;")?;

    let audit = run(root.path(), &["audit", "--jobs", "1", "--format", "json"])?;
    assert_status(&audit, 0);
    let run_id = field(&audit.stdout, "runId")?;

    // Get a cursor for this run
    let first = run(
        root.path(),
        &["capabilities", "--run", &run_id, "--format", "json"],
    )?;
    assert_status(&first, 0);
    let first_json: Value = serde_json::from_str(&first.stdout)?;
    let next_cursor = first_json
        .get("nextCursor")
        .and_then(Value::as_str)
        .ok_or("missing nextCursor")?;

    // Produce a second audit to get a different run_id
    fs::write(root.path().join("other.ts"), "export const other = 1;")?;
    let audit2 = run(root.path(), &["audit", "--jobs", "1", "--format", "json"])?;
    assert_status(&audit2, 0);
    let run_id_2 = field(&audit2.stdout, "runId")?;
    assert_ne!(run_id, run_id_2);

    // Use cursor from run 1 with run 2 -- must reject
    let cross = run(
        root.path(),
        &[
            "capabilities",
            "--run",
            &run_id_2,
            "--cursor",
            next_cursor,
            "--format",
            "json",
        ],
    )?;
    assert_status(&cross, 2);
    assert!(cross.stdout.is_empty());
    Ok(())
}

/// Cross-repository run cursor rejected with empty stdout.
#[test]
fn cross_repository_run_cursor_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let root_a = tempfile::tempdir()?;
    let root_b = tempfile::tempdir()?;
    fs::write(root_a.path().join("lib.ts"), "export const dead = 1;")?;
    fs::write(root_b.path().join("lib.ts"), "export const dead = 1;")?;

    let audit_a = run(root_a.path(), &["audit", "--jobs", "1", "--format", "json"])?;
    assert_status(&audit_a, 0);
    let run_id_a = field(&audit_a.stdout, "runId")?;

    let audit_b = run(root_b.path(), &["audit", "--jobs", "1", "--format", "json"])?;
    assert_status(&audit_b, 0);
    let run_id_b = field(&audit_b.stdout, "runId")?;
    assert_eq!(
        run_id_a, run_id_b,
        "fresh repositories must allocate the same local run ID for this scope test",
    );

    // Get cursor from repository A
    let first = run(
        root_a.path(),
        &["capabilities", "--run", &run_id_a, "--format", "json"],
    )?;
    assert_status(&first, 0);
    let first_json: Value = serde_json::from_str(&first.stdout)?;
    let next_cursor = first_json
        .get("nextCursor")
        .and_then(Value::as_str)
        .ok_or("missing nextCursor")?;

    // Use in repository B -- must reject (different repository_id in scope)
    let cross = run(
        root_b.path(),
        &[
            "capabilities",
            "--run",
            &run_id_b,
            "--cursor",
            next_cursor,
            "--format",
            "json",
        ],
    )?;
    assert_status(&cross, 2);
    assert!(cross.stdout.is_empty());
    Ok(())
}

/// A continuation is a repeatable read-only seek, not a single-use token.
#[test]
fn binary_cursor_is_repeatable_read_only_continuation() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;

    let first = run(root.path(), &["capabilities", "--format", "json"])?;
    assert_status(&first, 0);
    let first_json: Value = serde_json::from_str(&first.stdout)?;
    let next_cursor = first_json
        .get("nextCursor")
        .and_then(Value::as_str)
        .ok_or("missing nextCursor")?;

    // Repeating the same immutable continuation must return the same page.
    let second = run(
        root.path(),
        &["capabilities", "--cursor", next_cursor, "--format", "json"],
    )?;
    assert_status(&second, 0);

    let third = run(
        root.path(),
        &["capabilities", "--cursor", next_cursor, "--format", "json"],
    )?;
    assert_status(&third, 0);

    // Both uses produce the same result (deterministic, not single-use token)
    assert_eq!(second.stdout, third.stdout);
    Ok(())
}
