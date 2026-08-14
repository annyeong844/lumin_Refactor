use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

mod support;

use support::{assert_status, field, run};

const BOUNDED_MAIN: &str = concat!(
    "const segment = process.argv[2] ?? 'one';\n",
    "void import(`./features/${segment}.js`);\n",
    "void import('./shared/prefix-' + segment);\n",
    "void import(`./missing/${segment}.js`);\n",
);

#[test]
fn nonliteral_dynamic_imports_preserve_bounded_and_workspace_opacity()
-> Result<(), Box<dyn std::error::Error>> {
    verify_bounded_prefixes()?;
    verify_unbounded_expression()?;
    Ok(())
}

fn verify_bounded_prefixes() -> Result<(), Box<dyn std::error::Error>> {
    let root = bounded_fixture()?;
    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    let audit_json: Value = serde_json::from_str(&audit.stdout)?;
    assert_eq!(
        audit_json.get("status").and_then(Value::as_str),
        Some("incomplete")
    );
    assert_eq!(
        audit_json.get("limitationCount").and_then(Value::as_u64),
        Some(3)
    );
    let run_id = field(&audit.stdout, "runId")?;

    assert_eq!(
        finding_paths(root.path(), &run_id)?,
        BTreeSet::from([
            "src/features-old.ts".to_owned(),
            "src/shared/other.ts".to_owned(),
            "src/unrelated.ts".to_owned(),
        ]),
        "bounded opacity suppressed findings outside its static prefixes",
    );

    let overview = overview(root.path(), &run_id)?;
    let limitations = overview
        .get("limitations")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("overview limitations are missing"))?;
    assert_eq!(limitations.len(), 3);

    assert_bounded_limitation(
        limitations,
        "./features/",
        BTreeSet::from([
            source_id(root.path(), &run_id, "src/features/one.ts")?,
            source_id(root.path(), &run_id, "src/features/nested/two.ts")?,
        ]),
    )?;
    assert_bounded_limitation(
        limitations,
        "./shared/prefix-",
        BTreeSet::from([source_id(root.path(), &run_id, "src/shared/prefix-one.ts")?]),
    )?;
    assert_bounded_limitation(limitations, "./missing/", BTreeSet::new())?;

    let opened = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-bounded-nonliteral",
            "--path",
            "src/main.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);
    assert_eq!(field(&opened.stdout, "decision")?, "allow-with-warnings");
    let gate_id = field(&opened.stdout, "gateId")?;
    let shown = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&shown, 0);
    let shown: Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(
        shown
            .pointer("/baseline/limitationCount")
            .and_then(Value::as_u64),
        Some(3)
    );
    assert!(
        shown
            .get("protectedSemanticInputCount")
            .and_then(Value::as_u64)
            .is_some_and(|count| count >= 4),
        "bounded candidate inputs were not protected: {shown:#?}",
    );

    let candidate_conflict = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-bounded-candidate-conflict",
            "--path",
            "src/features/one.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&candidate_conflict, 4);
    let conflict: Value = serde_json::from_str(&candidate_conflict.stdout)?;
    assert!(
        conflict
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some("write-conflict")
                    && signal
                        .get("gateIds")
                        .and_then(Value::as_array)
                        .is_some_and(|gate_ids| {
                            gate_ids
                                .iter()
                                .any(|candidate| candidate.as_str() == Some(gate_id.as_str()))
                        })
            })),
        "candidate write did not conflict with the active reader: {conflict:#?}",
    );

    let unrelated = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-bounded-unrelated",
            "--path",
            "src/unrelated.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&unrelated, 0);
    let unrelated_gate = field(&unrelated.stdout, "gateId")?;
    let unrelated_abandoned = run(
        root.path(),
        &[
            "gate",
            "abandon",
            &unrelated_gate,
            "--operation-id",
            "op-abandon-bounded-unrelated",
            "--reason",
            "fixture control complete",
        ],
    )?;
    assert_status(&unrelated_abandoned, 0);
    let closed = run(
        root.path(),
        &[
            "post-write",
            &gate_id,
            "--operation-id",
            "op-close-bounded-nonliteral",
        ],
    )?;
    assert_status(&closed, 0);
    assert_eq!(field(&closed.stdout, "decision")?, "allow-with-warnings");
    assert_eq!(field(&closed.stdout, "lifecycle")?, "closed");
    let closed: Value = serde_json::from_str(&closed.stdout)?;
    let opacity_deltas = closed
        .get("deltas")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("post-write deltas are missing"))?
        .iter()
        .filter(|delta| delta.pointer("/key/family").and_then(Value::as_str) == Some("opacity"))
        .collect::<Vec<_>>();
    assert_eq!(opacity_deltas.len(), 3);
    assert!(opacity_deltas.iter().all(|delta| {
        delta
            .pointer("/classification/kind")
            .and_then(Value::as_str)
            == Some("unchanged")
    }));
    assert!(
        !closed
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some("required-evidence-incomplete")
            })),
        "bounded opacity became required-evidence incompleteness at close: {closed:#?}",
    );
    Ok(())
}

fn verify_unbounded_expression() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"unbounded-dynamic","private":true,"type":"module"}"#,
    )?;
    write(
        root.path(),
        "src/main.ts",
        "const request = process.argv[2]; void import(request);\n",
    )?;
    write(
        root.path(),
        "src/blocked.ts",
        "export const potentiallyConsumed = 1;\n",
    )?;

    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    let audit_json: Value = serde_json::from_str(&audit.stdout)?;
    assert_eq!(
        audit_json.get("status").and_then(Value::as_str),
        Some("incomplete")
    );
    assert_eq!(
        audit_json.get("limitationCount").and_then(Value::as_u64),
        Some(1)
    );
    let run_id = field(&audit.stdout, "runId")?;
    assert!(finding_paths(root.path(), &run_id)?.is_empty());

    let overview = overview(root.path(), &run_id)?;
    let limitations = overview
        .get("limitations")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("overview limitations are missing"))?;
    assert_eq!(limitations.len(), 1);
    let limitation = &limitations[0];
    assert_eq!(
        limitation.get("reason").and_then(Value::as_str),
        Some("dynamic-import-non-literal")
    );
    assert!(limitation.get("staticPrefix").is_none());
    assert_eq!(
        limitation
            .get("candidates")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );
    assert_eq!(
        limitation
            .pointer("/targetScope/kind")
            .and_then(Value::as_str),
        Some("workspace")
    );

    let blocked = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-unbounded-nonliteral",
            "--path",
            "src/blocked.ts",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&blocked, 4);
    assert_eq!(field(&blocked.stdout, "decision")?, "incomplete");
    assert_eq!(field(&blocked.stdout, "lifecycle")?, "rejected");
    let blocked_json: Value = serde_json::from_str(&blocked.stdout)?;
    assert!(
        blocked_json
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some("required-evidence-incomplete")
            }))
    );
    let gate_id = field(&blocked.stdout, "gateId")?;
    let shown = run(root.path(), &["gate", "show", &gate_id])?;
    assert_status(&shown, 0);
    assert_eq!(field(&shown.stdout, "lifecycle")?, "rejected");
    let active = run(root.path(), &["gate", "list", "--active"])?;
    assert_status(&active, 0);
    let active: Value = serde_json::from_str(&active.stdout)?;
    assert!(
        !active
            .get("items")
            .and_then(Value::as_array)
            .is_some_and(|items| items.iter().any(|item| {
                item.get("gateId").and_then(Value::as_str) == Some(gate_id.as_str())
            }))
    );
    Ok(())
}

fn assert_bounded_limitation(
    limitations: &[Value],
    prefix: &str,
    expected_candidates: BTreeSet<String>,
) -> Result<(), std::io::Error> {
    let limitation = limitations
        .iter()
        .find(|limitation| limitation.get("staticPrefix").and_then(Value::as_str) == Some(prefix))
        .ok_or_else(|| std::io::Error::other(format!("limitation for {prefix} is missing")))?;
    assert_eq!(
        limitation.get("reason").and_then(Value::as_str),
        Some("dynamic-import-non-literal")
    );
    assert_eq!(
        limitation
            .pointer("/targetScope/kind")
            .and_then(Value::as_str),
        Some("explicit-targets")
    );
    let candidates = limitation
        .get("candidates")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("limitation candidates are missing"))?
        .iter()
        .map(|candidate| {
            candidate
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| std::io::Error::other("candidate source ID is not a string"))
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    assert_eq!(candidates, expected_candidates);
    Ok(())
}

fn bounded_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"bounded-dynamic","private":true,"type":"module"}"#,
    )?;
    write(root.path(), "src/main.ts", BOUNDED_MAIN)?;
    for (path, name) in [
        ("src/features/one.ts", "one"),
        ("src/features/nested/two.ts", "two"),
        ("src/shared/prefix-one.ts", "prefixOne"),
        ("src/features-old.ts", "featuresOld"),
        ("src/shared/other.ts", "other"),
        ("src/unrelated.ts", "unrelated"),
    ] {
        write(root.path(), path, &format!("export const {name} = 1;\n"))?;
    }
    Ok(root)
}

fn overview(root: &Path, run_id: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let output = run(root, &["overview", "--run", run_id])?;
    assert_status(&output, 0);
    Ok(serde_json::from_str(&output.stdout)?)
}

fn finding_paths(
    root: &Path,
    run_id: &str,
) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
    let output = run(root, &["findings", "--run", run_id, "--area", "dead-code"])?;
    assert_status(&output, 0);
    let response: Value = serde_json::from_str(&output.stdout)?;
    let items = response
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("finding items are missing"))?;
    assert_eq!(
        response.get("total").and_then(Value::as_u64),
        Some(items.len() as u64)
    );
    items
        .iter()
        .map(|item| {
            item.pointer("/path/display")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| std::io::Error::other("finding path is missing"))
        })
        .collect::<Result<_, _>>()
        .map_err(Into::into)
}

fn source_id(root: &Path, run_id: &str, path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let output = run(root, &["files", "--run", run_id, path])?;
    assert_status(&output, 0);
    let response: Value = serde_json::from_str(&output.stdout)?;
    Ok(response
        .pointer("/sourceContext/sourceId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other("source ID is missing"))?)
}

fn write(root: &Path, path: &str, contents: &str) -> Result<(), std::io::Error> {
    let path = root.join(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}
