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
const VUE_IMPORT: &str = "void import(`./features/${segment}.js`);";
const VUE_BASE: &str = concat!(
    "<template><div /></template>\n",
    "<script setup lang=\"ts\">\n",
    "const segment = 'one';\n",
    "void import(`./features/${segment}.js`);\n",
    "</script>\n",
);
const VUE_SHIFTED: &str = concat!(
    "<template><div /></template>\n",
    "<script setup lang=\"ts\">\n",
    "const segment = 'one';\n",
    "// unrelated leading edit\n",
    "void import(`./features/${segment}.js`);\n",
    "</script>\n",
);
const VUE_DUPLICATE: &str = concat!(
    "<template><div /></template>\n",
    "<script setup lang=\"ts\">\n",
    "const segment = 'one';\n",
    "// unrelated leading edit\n",
    "void import(`./features/${segment}.js`);\n",
    "void import(`./features/${segment}.js`);\n",
    "</script>\n",
);

#[test]
fn nonliteral_dynamic_imports_preserve_bounded_and_workspace_opacity()
-> Result<(), Box<dyn std::error::Error>> {
    verify_bounded_prefixes()?;
    verify_test_like_bounded_fan_in()?;
    verify_embedded_span_and_stable_delta()?;
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

    let findings = finding_items(root.path(), &run_id)?;
    assert_eq!(findings.len(), 6);
    assert!(
        findings
            .iter()
            .all(|finding| { finding.get("namespace").and_then(Value::as_str) == Some("type") })
    );
    assert_eq!(
        finding_paths_from_items(&findings)?,
        BTreeSet::from([
            "src/features-old.ts".to_owned(),
            "src/features/nested/two.ts".to_owned(),
            "src/features/one.ts".to_owned(),
            "src/shared/other.ts".to_owned(),
            "src/shared/prefix-one.ts".to_owned(),
            "src/unrelated.ts".to_owned(),
        ]),
        "runtime opacity protected a type-only export or missed a reachable value export",
    );

    let overview = overview(root.path(), &run_id)?;
    let limitations = overview
        .get("limitations")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("overview limitations are missing"))?;
    assert_eq!(limitations.len(), 3);

    let all_candidates = [
        "src/main.ts",
        "src/features/one.ts",
        "src/features/nested/two.ts",
        "src/shared/prefix-one.ts",
        "src/features-old.ts",
        "src/shared/other.ts",
        "src/unrelated.ts",
    ]
    .into_iter()
    .map(|path| source_id(root.path(), &run_id, path))
    .collect::<Result<BTreeSet<_>, _>>()?;
    assert_bounded_limitation(limitations, "./features/", all_candidates.clone())?;
    assert_bounded_limitation(limitations, "./shared/prefix-", all_candidates.clone())?;
    assert_bounded_limitation(limitations, "./missing/", all_candidates)?;

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
            .is_some_and(|count| count >= 7),
        "bounded candidate inputs were not protected: {shown:#?}",
    );

    assert_candidate_conflict(
        root.path(),
        &gate_id,
        "op-bounded-candidate-conflict",
        "src/features/one.ts",
    )?;
    assert_candidate_conflict(
        root.path(),
        &gate_id,
        "op-bounded-escape-conflict",
        "src/unrelated.ts",
    )?;
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

fn verify_test_like_bounded_fan_in() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"test-bounded-dynamic","private":true,"type":"module"}"#,
    )?;
    write(
        root.path(),
        "tests/loader.test.ts",
        "const segment = 'target'; void import('../src/targets/' + segment);\n",
    )?;
    write(
        root.path(),
        "src/target.ts",
        "export const targetValue = 1; export type TargetType = string;\n",
    )?;
    write(
        root.path(),
        "src/other.ts",
        "export const otherValue = 1; export type OtherType = string;\n",
    )?;

    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    let run_id = field(&audit.stdout, "runId")?;
    let findings = finding_items(root.path(), &run_id)?;
    assert_eq!(findings.len(), 4);
    for finding in findings {
        let namespace = finding
            .get("namespace")
            .and_then(Value::as_str)
            .ok_or_else(|| std::io::Error::other("finding namespace is missing"))?;
        let claim = finding
            .get("claim")
            .and_then(Value::as_str)
            .ok_or_else(|| std::io::Error::other("finding claim is missing"))?;
        if namespace == "value" {
            assert!(
                claim.contains("consumed only by test-like sources"),
                "test broad fan-in was not preserved: {finding:#?}",
            );
        } else {
            assert_eq!(namespace, "type");
            assert!(
                !claim.contains("consumed only by test-like sources"),
                "runtime opacity leaked into the type namespace: {finding:#?}",
            );
        }
    }
    Ok(())
}

fn verify_embedded_span_and_stable_delta() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(
        root.path(),
        "package.json",
        r#"{"name":"embedded-dynamic","private":true,"type":"module"}"#,
    )?;
    write(root.path(), "src/App.vue", VUE_BASE)?;
    write(
        root.path(),
        "src/features/one.ts",
        "export const selected = 1;\n",
    )?;

    let audit = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audit, 0);
    let run_id = field(&audit.stdout, "runId")?;
    let overview = overview(root.path(), &run_id)?;
    let limitation = overview
        .get("limitations")
        .and_then(Value::as_array)
        .and_then(|limitations| {
            limitations.iter().find(|limitation| {
                limitation.get("reason").and_then(Value::as_str)
                    == Some("dynamic-import-non-literal")
            })
        })
        .ok_or_else(|| std::io::Error::other("embedded dynamic limitation is missing"))?;
    assert_eq!(
        limitation.pointer("/span/start").and_then(Value::as_u64),
        Some(
            VUE_BASE
                .find(VUE_IMPORT)
                .ok_or_else(|| std::io::Error::other("Vue import text is missing"))?
                .checked_add(
                    VUE_IMPORT
                        .find("import(")
                        .ok_or_else(|| std::io::Error::other("import expression is missing"))?,
                )
                .ok_or_else(|| std::io::Error::other("Vue import offset overflowed"))?
                as u64
        ),
        "embedded limitation span was not translated to parent coordinates",
    );

    let opened = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-embedded-stable-open",
            "--path",
            "src/App.vue",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);
    let gate_id = field(&opened.stdout, "gateId")?;
    write(root.path(), "src/App.vue", VUE_SHIFTED)?;
    let closed = run(
        root.path(),
        &[
            "post-write",
            &gate_id,
            "--operation-id",
            "op-embedded-stable-close",
        ],
    )?;
    assert_status(&closed, 0);
    assert_eq!(field(&closed.stdout, "lifecycle")?, "closed");
    let response: Value = serde_json::from_str(&closed.stdout)?;
    let opacity = response
        .get("deltas")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("embedded post-write deltas are missing"))?
        .iter()
        .filter(|delta| delta.pointer("/key/family").and_then(Value::as_str) == Some("opacity"))
        .collect::<Vec<_>>();
    assert_eq!(opacity.len(), 1);
    assert_eq!(
        opacity[0]
            .pointer("/classification/kind")
            .and_then(Value::as_str),
        Some("unchanged"),
        "an unrelated leading edit changed the opacity identity: {response:#?}",
    );

    let duplicate_opened = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-embedded-duplicate-open",
            "--path",
            "src/App.vue",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&duplicate_opened, 0);
    let duplicate_gate_id = field(&duplicate_opened.stdout, "gateId")?;
    write(root.path(), "src/App.vue", VUE_DUPLICATE)?;
    let duplicate_closed = run(
        root.path(),
        &[
            "post-write",
            &duplicate_gate_id,
            "--operation-id",
            "op-embedded-duplicate-close",
        ],
    )?;
    assert_status(&duplicate_closed, 4);
    assert_eq!(field(&duplicate_closed.stdout, "decision")?, "incomplete");
    assert_eq!(field(&duplicate_closed.stdout, "lifecycle")?, "active");
    let duplicate_response: Value = serde_json::from_str(&duplicate_closed.stdout)?;
    let duplicate_opacity = duplicate_response
        .get("deltas")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("duplicate opacity deltas are missing"))?
        .iter()
        .filter(|delta| delta.pointer("/key/family").and_then(Value::as_str) == Some("opacity"))
        .collect::<Vec<_>>();
    assert_eq!(duplicate_opacity.len(), 2);
    assert_eq!(
        duplicate_opacity
            .iter()
            .filter(|delta| {
                delta
                    .pointer("/classification/kind")
                    .and_then(Value::as_str)
                    == Some("unchanged")
            })
            .count(),
        1,
    );
    assert_eq!(
        duplicate_opacity
            .iter()
            .filter(|delta| {
                delta
                    .pointer("/classification/kind")
                    .and_then(Value::as_str)
                    == Some("introduced")
            })
            .count(),
        1,
    );
    assert!(
        duplicate_response
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some("opacity-introduced")
            })),
        "adding a same-prefix construct did not produce opacity-introduced: {duplicate_response:#?}",
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

fn assert_candidate_conflict(
    root: &Path,
    gate_id: &str,
    operation_id: &str,
    path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let candidate = run(
        root,
        &[
            "pre-write",
            "--operation-id",
            operation_id,
            "--path",
            path,
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&candidate, 4);
    let conflict: Value = serde_json::from_str(&candidate.stdout)?;
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
                                .any(|candidate| candidate.as_str() == Some(gate_id))
                        })
            })),
        "candidate write did not conflict with the active reader: {conflict:#?}",
    );
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
        write(
            root.path(),
            path,
            &format!("export const {name} = 1; export type {name}Type = string;\n"),
        )?;
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
    finding_paths_from_items(&finding_items(root, run_id)?).map_err(Into::into)
}

fn finding_items(root: &Path, run_id: &str) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
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
    Ok(items.clone())
}

fn finding_paths_from_items(items: &[Value]) -> Result<BTreeSet<String>, std::io::Error> {
    items
        .iter()
        .map(|item| {
            item.pointer("/path/display")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| std::io::Error::other("finding path is missing"))
        })
        .collect()
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
