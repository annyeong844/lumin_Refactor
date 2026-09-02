mod support;

use std::fs;
use std::path::Path;

#[cfg(feature = "collection-ordering-test-perturb")]
use base64::Engine;
#[cfg(feature = "collection-ordering-test-perturb")]
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;
#[cfg(feature = "collection-ordering-test-perturb")]
use std::collections::BTreeSet;
#[cfg(feature = "collection-ordering-test-perturb")]
use std::fmt::Debug;

fn json(stdout: &str) -> Result<Value, Box<dyn std::error::Error>> {
    serde_json::from_str(stdout).map_err(Into::into)
}

// --- `lumin related` ---

#[test]
fn related_returns_relation_collection_for_run_finding() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    create_related_fixture(root.path())?;
    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);
    let run_id = support::field(&audit.stdout, "runId")?;

    // Get first finding that has nested collections
    let findings = support::run(
        root.path(),
        &["findings", "--run", &run_id, "--area", "dead-code"],
    )?;
    support::assert_status(&findings, 0);
    let findings_json = json(&findings.stdout)?;
    let items = findings_json
        .get("items")
        .and_then(Value::as_array)
        .ok_or("missing items")?;
    let finding_id = items[0]
        .get("findingId")
        .and_then(Value::as_str)
        .ok_or("missing findingId")?;

    let related = support::run(
        root.path(),
        &["related", "--run", &run_id, finding_id, "--format", "json"],
    )?;
    support::assert_status(&related, 0);
    let response = json(&related.stdout)?;
    assert_eq!(
        response.get("schemaVersion").and_then(Value::as_str),
        Some("lumin.collection.v1")
    );
    assert_eq!(
        response.get("ordering").and_then(Value::as_str),
        Some("relations.v1")
    );
    assert!(response.get("scopeTotal").and_then(Value::as_u64).is_some());
    assert!(response.get("total").and_then(Value::as_u64).is_some());
    assert!(response.get("items").and_then(Value::as_array).is_some());
    Ok(())
}

#[test]
fn related_missing_run_exits_2() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    create_related_fixture(root.path())?;
    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);

    let result = support::run(
        root.path(),
        &["related", "--run", "nonexistent-run", "finding-id"],
    )?;
    support::assert_status(&result, 2);
    Ok(())
}

// --- `lumin files` ---

#[test]
fn files_returns_file_findings_collection() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(
        root.path().join("src/a.ts"),
        "export const first = 1;\nexport const second = 2;\n",
    )?;
    fs::write(root.path().join("src/b.ts"), "export const third = 3;\n")?;
    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);
    let run_id = support::field(&audit.stdout, "runId")?;

    let files_result = support::run(
        root.path(),
        &["files", "--run", &run_id, "src/a.ts", "--format", "json"],
    )?;
    support::assert_status(&files_result, 0);
    let response = json(&files_result.stdout)?;
    assert_eq!(
        response.get("schemaVersion").and_then(Value::as_str),
        Some("lumin.collection.v1")
    );
    assert_eq!(
        response.get("ordering").and_then(Value::as_str),
        Some("file-findings.v1")
    );
    // scopeTotal = all run findings (3), total = matches for src/a.ts (2).
    assert_eq!(response.get("scopeTotal").and_then(Value::as_u64), Some(3));
    assert_eq!(response.get("total").and_then(Value::as_u64), Some(2));
    assert_eq!(response.get("returned").and_then(Value::as_u64), Some(2));
    assert_eq!(
        response.get("truncated").and_then(Value::as_bool),
        Some(false)
    );
    let items = response
        .get("items")
        .and_then(Value::as_array)
        .ok_or("missing items")?;
    assert_eq!(items.len(), 2);
    let exported_names = items
        .iter()
        .map(|item| item.get("exportedName").and_then(Value::as_str))
        .collect::<Option<Vec<_>>>()
        .ok_or("file finding omitted exportedName")?;
    assert_eq!(exported_names, ["first", "second"]);
    let spans = items
        .iter()
        .map(|item| {
            Some((
                item.pointer("/span/start")?.as_u64()?,
                item.pointer("/span/end")?.as_u64()?,
            ))
        })
        .collect::<Option<Vec<_>>>()
        .ok_or("file finding omitted source span")?;
    assert!(spans[0] < spans[1], "file-findings.v1 order regressed");
    let finding_ids = items
        .iter()
        .map(|item| item.get("findingId").and_then(Value::as_str))
        .collect::<Option<Vec<_>>>()
        .ok_or("file finding omitted findingId")?;
    assert_ne!(
        finding_ids[0], finding_ids[1],
        "file findings must traverse exactly once"
    );
    Ok(())
}

#[test]
fn files_zero_match_exits_0_empty() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("lib.ts"), "export const dead = 1;\n")?;
    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);
    let run_id = support::field(&audit.stdout, "runId")?;

    let files_result = support::run(root.path(), &["files", "--run", &run_id, "nonexistent.ts"])?;
    support::assert_status(&files_result, 0);
    let response = json(&files_result.stdout)?;
    assert_eq!(response.get("total").and_then(Value::as_u64), Some(0));
    assert_eq!(response.get("returned").and_then(Value::as_u64), Some(0));
    Ok(())
}

#[test]
fn files_invalid_path_exits_2() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("lib.ts"), "export const dead = 1;\n")?;
    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);
    let run_id = support::field(&audit.stdout, "runId")?;

    // Absolute path is invalid
    let result = support::run(
        root.path(),
        &["files", "--run", &run_id, "/absolute/path.ts"],
    )?;
    support::assert_status(&result, 2);
    Ok(())
}

// --- `lumin gate list` ---

#[test]
fn gate_list_requires_active_flag() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("lib.ts"), "export const dead = 1;\n")?;
    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);

    let result = support::run(root.path(), &["gate", "list"])?;
    support::assert_status(&result, 2);
    assert!(result.stdout.is_empty());
    Ok(())
}

#[test]
fn gate_list_active_returns_empty_collection() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("lib.ts"), "export const dead = 1;\n")?;
    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);

    let result = support::run(
        root.path(),
        &["gate", "list", "--active", "--format", "json"],
    )?;
    support::assert_status(&result, 0);
    let response = json(&result.stdout)?;
    assert_eq!(
        response.get("schemaVersion").and_then(Value::as_str),
        Some("lumin.active-gates.v1")
    );
    assert_eq!(
        response.get("ordering").and_then(Value::as_str),
        Some("active-gates.v1")
    );
    assert_eq!(response.get("scopeTotal").and_then(Value::as_u64), Some(0));
    assert_eq!(response.get("total").and_then(Value::as_u64), Some(0));
    assert_eq!(response.get("returned").and_then(Value::as_u64), Some(0));
    assert_eq!(
        response.get("truncated").and_then(Value::as_bool),
        Some(false)
    );
    assert!(
        response.get("nextCursor").is_none() || response.get("nextCursor") == Some(&Value::Null)
    );
    Ok(())
}

#[test]
fn gate_list_active_orders_open_gates() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(root.path().join("src/a.ts"), "console.log('a');\n")?;
    fs::write(root.path().join("src/b.ts"), "console.log('b');\n")?;
    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);

    let gate_a = open_active_gate(root.path(), "op-gate-list-a", "src/a.ts")?;
    let gate_b = open_active_gate(root.path(), "op-gate-list-b", "src/b.ts")?;

    let result = support::run(root.path(), &["gate", "list", "--active"])?;
    support::assert_status(&result, 0);
    let response = json(&result.stdout)?;
    assert_eq!(
        response.get("schemaVersion").and_then(Value::as_str),
        Some("lumin.active-gates.v1")
    );
    assert_eq!(
        response.get("ordering").and_then(Value::as_str),
        Some("active-gates.v1")
    );
    assert_eq!(response.get("scopeTotal").and_then(Value::as_u64), Some(2));
    assert_eq!(response.get("total").and_then(Value::as_u64), Some(2));
    assert_eq!(response.get("returned").and_then(Value::as_u64), Some(2));
    let items = response
        .get("items")
        .and_then(Value::as_array)
        .ok_or("missing items")?;
    assert_eq!(items.len(), 2);
    let observed_gate_ids = items
        .iter()
        .map(|item| {
            item.get("gateId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect::<Option<Vec<_>>>()
        .ok_or("active gate omitted gateId")?;
    assert_eq!(observed_gate_ids, [gate_a, gate_b]);
    let order_keys = items
        .iter()
        .map(|item| {
            Some((
                item.get("openingTransitionSequence")?.as_u64()?,
                item.get("gateId")?.as_str()?.to_owned(),
            ))
        })
        .collect::<Option<Vec<_>>>()
        .ok_or("active gate omitted ordering fields")?;
    assert!(
        order_keys[0] < order_keys[1],
        "active-gates.v1 order regressed"
    );
    Ok(())
}

#[test]
fn gate_list_active_malformed_cursor_exits_2() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("lib.ts"), "export const dead = 1;\n")?;
    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);

    let result = support::run(
        root.path(),
        &["gate", "list", "--active", "--cursor", "not-valid-base64!"],
    )?;
    support::assert_status(&result, 2);
    assert!(result.stdout.is_empty());
    Ok(())
}

#[cfg(feature = "collection-ordering-test-perturb")]
#[test]
fn perturbed_public_collections_traverse_once_in_canonical_order()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let trace = tempfile::tempdir()?;
    create_perturbed_fixture(root.path())?;

    let mut authored_runs = Vec::new();
    for _ in 0..4 {
        let audit = run_perturbed(root.path(), trace.path(), &["audit", "--jobs", "1"])?;
        support::assert_status(&audit, 0);
        authored_runs.push(support::field(&audit.stdout, "runId")?);
    }
    assert_eq!(
        authored_runs.iter().collect::<BTreeSet<_>>().len(),
        authored_runs.len(),
        "fixture reused a run ID"
    );
    let run_id = authored_runs
        .last()
        .ok_or_else(|| std::io::Error::other("fixture produced no run"))?;

    let finding_items = collect_single_perturbed_collection(
        root.path(),
        trace.path(),
        &["findings", "--run", run_id, "--area", "dead-code"],
        "",
        "/items",
        "--cursor",
        "findings.v1",
    )?;
    assert_eq!(finding_items.len(), 7);
    let expected_findings = BTreeSet::from([
        ("src/10.test.ts".to_owned(), "test10".to_owned()),
        ("src/20.test.ts".to_owned(), "test20".to_owned()),
        ("src/30.test.ts".to_owned(), "test30".to_owned()),
        ("src/40.test.ts".to_owned(), "test40".to_owned()),
        ("src/lib.ts".to_owned(), "alpha".to_owned()),
        ("src/lib.ts".to_owned(), "omega".to_owned()),
        ("src/lib.ts".to_owned(), "zeta".to_owned()),
    ]);
    let observed_findings = finding_items
        .iter()
        .map(|finding| {
            Ok((
                required_str(finding, "/path/display")?.to_owned(),
                required_str(finding, "/exportedName")?.to_owned(),
            ))
        })
        .collect::<Result<BTreeSet<_>, Box<dyn std::error::Error>>>()?;
    assert_eq!(observed_findings, expected_findings);
    let finding_keys = finding_items
        .iter()
        .map(finding_order_key)
        .collect::<Result<Vec<_>, _>>()?;
    assert_strictly_increasing(&finding_keys, "findings.v1");
    assert_unique_ids(&finding_items, "/findingId", "findings.v1")?;

    let zeta_finding = finding_items
        .iter()
        .find(|finding| {
            finding.get("exportedName").and_then(Value::as_str) == Some("zeta")
                && finding
                    .pointer("/path/display")
                    .and_then(Value::as_str)
                    .is_some_and(|path| path == "src/lib.ts")
        })
        .ok_or_else(|| std::io::Error::other("fixture omitted the zeta finding"))?;
    let finding_id = required_str(zeta_finding, "/findingId")?;
    let evidence_items = collect_single_perturbed_collection(
        root.path(),
        trace.path(),
        &["explain", "--run", run_id, finding_id],
        "/evidence",
        "/items",
        "--evidence-cursor",
        "evidence.v1",
    )?;
    assert_eq!(evidence_items.len(), 5);
    let expected_evidence = BTreeSet::from([
        ("definition".to_owned(), "src/lib.ts".to_owned()),
        ("test-only-reexport".to_owned(), "src/10.test.ts".to_owned()),
        ("test-only-reexport".to_owned(), "src/20.test.ts".to_owned()),
        ("test-only-reexport".to_owned(), "src/30.test.ts".to_owned()),
        ("test-only-reexport".to_owned(), "src/40.test.ts".to_owned()),
    ]);
    let observed_evidence = evidence_items
        .iter()
        .map(|record| {
            Ok((
                required_str(record, "/kind")?.to_owned(),
                required_str(record, "/path/display")?.to_owned(),
            ))
        })
        .collect::<Result<BTreeSet<_>, Box<dyn std::error::Error>>>()?;
    assert_eq!(observed_evidence, expected_evidence);
    let evidence_keys = evidence_items
        .iter()
        .map(evidence_order_key)
        .collect::<Result<Vec<_>, _>>()?;
    assert_strictly_increasing(&evidence_keys, "evidence.v1");
    assert_unique_ids(&evidence_items, "/evidenceId", "evidence.v1")?;

    let relation_items = collect_single_perturbed_collection(
        root.path(),
        trace.path(),
        &["explain", "--run", run_id, finding_id],
        "/relations",
        "/items",
        "--relations-cursor",
        "relations.v1",
    )?;
    assert_eq!(relation_items.len(), 4);
    let mut expected_relation_grounding = BTreeSet::new();
    for (path, exported_name) in [
        ("src/10.test.ts", "test10"),
        ("src/20.test.ts", "test20"),
        ("src/30.test.ts", "test30"),
        ("src/40.test.ts", "test40"),
    ] {
        let target = finding_items
            .iter()
            .find(|finding| {
                finding.pointer("/path/display").and_then(Value::as_str) == Some(path)
                    && finding.get("exportedName").and_then(Value::as_str) == Some(exported_name)
            })
            .ok_or_else(|| std::io::Error::other(format!("missing alias finding {path}")))?;
        let grounding = evidence_items
            .iter()
            .find(|record| {
                record.pointer("/path/display").and_then(Value::as_str) == Some(path)
                    && record.get("kind").and_then(Value::as_str) == Some("test-only-reexport")
            })
            .ok_or_else(|| std::io::Error::other(format!("missing re-export evidence {path}")))?;
        expected_relation_grounding.insert((
            required_str(target, "/findingId")?.to_owned(),
            required_str(grounding, "/evidenceId")?.to_owned(),
        ));
    }
    let observed_relation_grounding = relation_items
        .iter()
        .map(|relation| {
            assert_eq!(required_str(relation, "/kind")?, "test-only-reexport");
            Ok((
                required_str(relation, "/targetFindingId")?.to_owned(),
                required_str(relation, "/groundingEvidenceId")?.to_owned(),
            ))
        })
        .collect::<Result<BTreeSet<_>, Box<dyn std::error::Error>>>()?;
    assert_eq!(observed_relation_grounding, expected_relation_grounding);
    let relation_keys = relation_items
        .iter()
        .map(relation_order_key)
        .collect::<Result<Vec<_>, _>>()?;
    assert_strictly_increasing(&relation_keys, "relations.v1");
    assert_unique_ids(&relation_items, "/relationId", "relations.v1")?;

    let related_items = collect_single_perturbed_collection(
        root.path(),
        trace.path(),
        &["related", "--run", run_id, finding_id],
        "",
        "/items",
        "--cursor",
        "relations.v1",
    )?;
    assert_eq!(
        related_items, relation_items,
        "related did not reuse the canonical relation collection"
    );

    let file_items = collect_single_perturbed_collection(
        root.path(),
        trace.path(),
        &["files", "--run", run_id, "src/lib.ts"],
        "",
        "/items",
        "--cursor",
        "file-findings.v1",
    )?;
    assert_eq!(
        file_items
            .iter()
            .map(|finding| required_str(finding, "/exportedName"))
            .collect::<Result<Vec<_>, _>>()?,
        ["zeta", "alpha", "omega"]
    );
    let file_keys = file_items
        .iter()
        .map(file_finding_order_key)
        .collect::<Result<Vec<_>, _>>()?;
    assert_strictly_increasing(&file_keys, "file-findings.v1");
    assert_unique_ids(&file_items, "/findingId", "file-findings.v1")?;

    let run_items = collect_single_perturbed_collection(
        root.path(),
        trace.path(),
        &["runs", "list"],
        "",
        "/runs",
        "--cursor",
        "runs.v1",
    )?;
    assert_eq!(run_items.len(), authored_runs.len());
    let observed_run_ids = run_items
        .iter()
        .map(|run| required_str(run, "/runId"))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(
        observed_run_ids,
        authored_runs
            .iter()
            .rev()
            .map(String::as_str)
            .collect::<Vec<_>>()
    );
    assert_unique_ids(&run_items, "/runId", "runs.v1")?;
    let run_keys = run_items
        .iter()
        .map(|run| {
            Ok((
                std::cmp::Reverse(required_u64(run, "/sequence")?),
                required_str(run, "/runId")?.to_owned(),
            ))
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    assert_strictly_increasing(&run_keys, "runs.v1");

    let gate_paths = [
        "src/gate-c.ts",
        "src/gate-a.ts",
        "src/gate-d.ts",
        "src/gate-b.ts",
    ];
    let mut authored_gates = Vec::new();
    for (index, path) in gate_paths.iter().enumerate() {
        authored_gates.push(open_perturbed_gate(
            root.path(),
            trace.path(),
            &format!("op-collection-ordering-{index}"),
            path,
        )?);
    }
    let gate_items = collect_single_perturbed_collection(
        root.path(),
        trace.path(),
        &["gate", "list", "--active"],
        "",
        "/items",
        "--cursor",
        "active-gates.v1",
    )?;
    assert_eq!(gate_items.len(), authored_gates.len());
    assert_eq!(
        gate_items
            .iter()
            .map(|gate| required_str(gate, "/gateId"))
            .collect::<Result<Vec<_>, _>>()?,
        authored_gates
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    );
    let gate_keys = gate_items
        .iter()
        .map(|gate| {
            Ok((
                required_u64(gate, "/openingTransitionSequence")?,
                required_str(gate, "/gateId")?.to_owned(),
            ))
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    assert_strictly_increasing(&gate_keys, "active-gates.v1");
    assert_unique_ids(&gate_items, "/gateId", "active-gates.v1")?;

    let plan = run_perturbed(
        root.path(),
        trace.path(),
        &[
            "runs",
            "prune",
            "plan",
            "--before",
            "9000000000000",
            "--operation-id",
            "op-collection-ordering-plan",
        ],
    )?;
    support::assert_status(&plan, 0);
    let plan = json(&plan.stdout)?;
    let plan_id = required_str(&plan, "/result/planId")?;
    let plan_groups = collect_perturbed_pages(
        root.path(),
        trace.path(),
        &["runs", "prune", "plan", "show", plan_id],
        "",
        &["/items", "/exclusions"],
        "--cursor",
        "retention-plan-items.v1",
    )?;
    let mut plan_groups = plan_groups.into_iter();
    let plan_items = plan_groups
        .next()
        .ok_or_else(|| std::io::Error::other("retention pages omitted items"))?;
    let exclusions = plan_groups
        .next()
        .ok_or_else(|| std::io::Error::other("retention pages omitted exclusions"))?;
    assert!(plan_groups.next().is_none());
    let plan_keys = plan_items
        .iter()
        .map(retention_item_order_key)
        .collect::<Result<Vec<_>, _>>()?;
    assert_strictly_increasing(&plan_keys, "retention-plan-items.v1");
    let observed_plan_items = plan_items
        .iter()
        .map(|item| {
            Ok((
                required_str(item, "/kind")?.to_owned(),
                required_str(item, "/recordId")?.to_owned(),
            ))
        })
        .collect::<Result<BTreeSet<_>, Box<dyn std::error::Error>>>()?;
    assert_eq!(observed_plan_items.len(), plan_items.len());
    let latest_run = authored_runs
        .last()
        .ok_or_else(|| std::io::Error::other("fixture produced no latest run"))?;
    let latest_attempt = required_str(
        run_items
            .first()
            .ok_or_else(|| std::io::Error::other("run collection is empty"))?,
        "/attemptId",
    )?;
    let mut expected_plan_items = BTreeSet::new();
    for run in &run_items {
        let run_id = required_str(run, "/runId")?;
        if run_id == latest_run {
            continue;
        }
        let attempt_id = required_str(run, "/attemptId")?;
        expected_plan_items.extend([
            ("attempt".to_owned(), attempt_id.to_owned()),
            ("run".to_owned(), run_id.to_owned()),
            ("evidence".to_owned(), format!("run:{run_id}/evidence")),
        ]);
    }
    assert_eq!(observed_plan_items, expected_plan_items);
    let observed_exclusions = exclusions
        .iter()
        .map(|exclusion| {
            Ok((
                required_str(exclusion, "/kind")?.to_owned(),
                required_str(exclusion, "/recordId")?.to_owned(),
                required_str(exclusion, "/reason/reason")?.to_owned(),
            ))
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    assert_eq!(
        observed_exclusions,
        [
            (
                "attempt".to_owned(),
                latest_attempt.to_owned(),
                "latest-attempt".to_owned(),
            ),
            (
                "attempt".to_owned(),
                latest_attempt.to_owned(),
                "latest-completed".to_owned(),
            ),
            (
                "run".to_owned(),
                latest_run.to_owned(),
                "latest-completed".to_owned(),
            ),
        ]
    );

    let traces = fs::read_dir(trace.path())?
        .map(|entry| {
            entry?
                .file_name()
                .into_string()
                .map_err(|_| std::io::Error::other("trace name is not UTF-8"))
        })
        .collect::<Result<BTreeSet<_>, std::io::Error>>()?;
    assert_eq!(
        traces,
        BTreeSet::from([
            "active-gates".to_owned(),
            "evidence".to_owned(),
            "findings".to_owned(),
            "relations".to_owned(),
            "retention-plan-items".to_owned(),
            "runs".to_owned(),
        ]),
        "the fixture did not perturb every owner/backend collection"
    );
    Ok(())
}

// --- Helpers ---

#[cfg(feature = "collection-ordering-test-perturb")]
fn run_perturbed(
    root: &Path,
    trace: &Path,
    arguments: &[&str],
) -> Result<support::ProcessResult, Box<dyn std::error::Error>> {
    let trace = trace
        .to_str()
        .ok_or_else(|| std::io::Error::other("trace path is not UTF-8"))?;
    support::run_with_env(
        root,
        arguments,
        &[
            ("LUMIN_TEST_COLLECTION_ORDERING_PERTURB", "reverse"),
            ("LUMIN_TEST_COLLECTION_ORDERING_PAGE_SIZE", "2"),
            ("LUMIN_TEST_COLLECTION_ORDERING_TRACE", trace),
        ],
    )
}

#[cfg(feature = "collection-ordering-test-perturb")]
fn create_perturbed_fixture(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(root.join("src"))?;
    for (path, contents) in [
        (
            "src/40.test.ts",
            "export { zeta as test40 } from './lib.js';\n",
        ),
        ("src/gate-c.ts", "console.log('c');\n"),
        (
            "src/10.test.ts",
            "export { zeta as test10 } from './lib.js';\n",
        ),
        (
            "src/lib.ts",
            "export const zeta = 1;\nexport const alpha = 2;\nexport const omega = 3;\n",
        ),
        ("src/gate-a.ts", "console.log('a');\n"),
        (
            "src/30.test.ts",
            "export { zeta as test30 } from './lib.js';\n",
        ),
        ("src/gate-d.ts", "console.log('d');\n"),
        (
            "src/20.test.ts",
            "export { zeta as test20 } from './lib.js';\n",
        ),
        ("src/gate-b.ts", "console.log('b');\n"),
    ] {
        fs::write(root.join(path), contents)?;
    }
    Ok(())
}

#[cfg(feature = "collection-ordering-test-perturb")]
fn open_perturbed_gate(
    root: &Path,
    trace: &Path,
    operation_id: &str,
    path: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let opened = run_perturbed(
        root,
        trace,
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
    support::assert_status(&opened, 0);
    let opened = json(&opened.stdout)?;
    assert_eq!(
        opened.get("lifecycle").and_then(Value::as_str),
        Some("active")
    );
    Ok(required_str(&opened, "/gateId")?.to_owned())
}

#[cfg(feature = "collection-ordering-test-perturb")]
fn collect_single_perturbed_collection(
    root: &Path,
    trace: &Path,
    base_arguments: &[&str],
    collection_pointer: &str,
    items_pointer: &str,
    cursor_flag: &str,
    ordering: &str,
) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let mut groups = collect_perturbed_pages(
        root,
        trace,
        base_arguments,
        collection_pointer,
        &[items_pointer],
        cursor_flag,
        ordering,
    )?;
    groups
        .pop()
        .ok_or_else(|| std::io::Error::other(format!("{ordering} omitted its item group")).into())
}

#[cfg(feature = "collection-ordering-test-perturb")]
fn collect_perturbed_pages(
    root: &Path,
    trace: &Path,
    base_arguments: &[&str],
    collection_pointer: &str,
    item_pointers: &[&str],
    cursor_flag: &str,
    ordering: &str,
) -> Result<Vec<Vec<Value>>, Box<dyn std::error::Error>> {
    let mut groups = vec![Vec::new(); item_pointers.len()];
    let mut expected_total = None;
    let mut cursor: Option<String> = None;
    let mut issued_cursors = BTreeSet::new();
    let mut page_count = 0_usize;
    let mut highest_nonempty_group = None;

    loop {
        let mut arguments = base_arguments
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect::<Vec<_>>();
        if let Some(value) = cursor.as_ref() {
            arguments.push(cursor_flag.to_owned());
            arguments.push(value.clone());
        }
        let argument_refs = arguments.iter().map(String::as_str).collect::<Vec<_>>();
        let result = run_perturbed(root, trace, &argument_refs)?;
        support::assert_status(&result, 0);
        let response = json(&result.stdout)?;
        let collection = if collection_pointer.is_empty() {
            &response
        } else {
            response.pointer(collection_pointer).ok_or_else(|| {
                std::io::Error::other(format!("missing collection at {collection_pointer}"))
            })?
        };
        assert_eq!(required_str(collection, "/ordering")?, ordering);

        let total = usize::try_from(required_u64(collection, "/total")?)?;
        if let Some(expected) = expected_total {
            assert_eq!(total, expected, "{ordering} total changed between pages");
        } else {
            expected_total = Some(total);
        }

        let mut page_len = 0_usize;
        for (index, pointer) in item_pointers.iter().enumerate() {
            let items = required_array(collection, pointer)?;
            if !items.is_empty() {
                if highest_nonempty_group.is_some_and(|previous| index < previous) {
                    return Err(std::io::Error::other(format!(
                        "{ordering} returned an earlier item group after a later group"
                    ))
                    .into());
                }
                highest_nonempty_group = Some(index);
            }
            page_len += items.len();
            groups[index].extend(items.iter().cloned());
        }
        assert_eq!(
            usize::try_from(required_u64(collection, "/returned")?)?,
            page_len,
            "{ordering} returned count disagrees with its page"
        );
        assert!(page_len <= 2, "{ordering} ignored the test page size");
        page_count += 1;

        let truncated = collection
            .get("truncated")
            .and_then(Value::as_bool)
            .ok_or_else(|| std::io::Error::other(format!("{ordering} omitted truncated")))?;
        let next_cursor = match collection.get("nextCursor") {
            None | Some(Value::Null) => None,
            Some(Value::String(value)) => Some(value.clone()),
            Some(_) => {
                return Err(std::io::Error::other(format!(
                    "{ordering} emitted a non-string continuation cursor"
                ))
                .into());
            }
        };
        assert_eq!(
            truncated,
            next_cursor.is_some(),
            "{ordering} cursor presence disagrees with truncation"
        );

        let collected = groups.iter().map(Vec::len).sum::<usize>();
        if !truncated {
            assert_eq!(
                collected,
                expected_total.unwrap_or_default(),
                "{ordering} traversal skipped or repeated an item"
            );
            break;
        }
        if page_len == 0 || collected >= total {
            return Err(std::io::Error::other(format!(
                "{ordering} advertised a continuation without remaining items"
            ))
            .into());
        }
        let next_cursor = next_cursor.ok_or_else(|| {
            std::io::Error::other(format!("{ordering} omitted its continuation cursor"))
        })?;
        if !issued_cursors.insert(next_cursor.clone()) {
            return Err(std::io::Error::other(format!(
                "{ordering} repeated a continuation cursor"
            ))
            .into());
        }
        cursor = Some(next_cursor);
    }

    assert!(
        page_count > 1,
        "{ordering} fixture did not cross a page boundary"
    );
    Ok(groups)
}

#[cfg(feature = "collection-ordering-test-perturb")]
fn required_array<'a>(
    value: &'a Value,
    pointer: &str,
) -> Result<&'a Vec<Value>, Box<dyn std::error::Error>> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other(format!("missing array at {pointer}")).into())
}

#[cfg(feature = "collection-ordering-test-perturb")]
fn required_str<'a>(
    value: &'a Value,
    pointer: &str,
) -> Result<&'a str, Box<dyn std::error::Error>> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other(format!("missing string at {pointer}")).into())
}

#[cfg(feature = "collection-ordering-test-perturb")]
fn required_u64(value: &Value, pointer: &str) -> Result<u64, Box<dyn std::error::Error>> {
    value
        .pointer(pointer)
        .and_then(Value::as_u64)
        .ok_or_else(|| std::io::Error::other(format!("missing integer at {pointer}")).into())
}

#[cfg(feature = "collection-ordering-test-perturb")]
fn assert_unique_ids(
    items: &[Value],
    pointer: &str,
    ordering: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let ids = items
        .iter()
        .map(|item| required_str(item, pointer))
        .collect::<Result<BTreeSet<_>, _>>()?;
    assert_eq!(ids.len(), items.len(), "{ordering} repeated an item");
    Ok(())
}

#[cfg(feature = "collection-ordering-test-perturb")]
fn assert_strictly_increasing<T: Debug + Ord>(keys: &[T], ordering: &str) {
    for pair in keys.windows(2) {
        assert!(
            pair[0] < pair[1],
            "{ordering} is not strictly increasing: {:?} then {:?}",
            pair[0],
            pair[1]
        );
    }
}

#[cfg(feature = "collection-ordering-test-perturb")]
type FindingOrderKey = (String, Vec<u8>, u64, u64, String);

#[cfg(feature = "collection-ordering-test-perturb")]
fn finding_order_key(value: &Value) -> Result<FindingOrderKey, Box<dyn std::error::Error>> {
    assert_eq!(required_str(value, "/severity")?, "warning");
    assert_eq!(required_str(value, "/confidence")?, "grounded");
    Ok((
        required_str(value, "/ruleId")?.to_owned(),
        STANDARD.decode(required_str(value, "/path/canonicalBase64")?)?,
        required_u64(value, "/span/start")?,
        required_u64(value, "/span/end")?,
        required_str(value, "/findingId")?.to_owned(),
    ))
}

#[cfg(feature = "collection-ordering-test-perturb")]
type EvidenceOrderKey = (String, String, u64, u64, String);

#[cfg(feature = "collection-ordering-test-perturb")]
fn evidence_order_key(value: &Value) -> Result<EvidenceOrderKey, Box<dyn std::error::Error>> {
    Ok((
        required_str(value, "/kind")?.to_owned(),
        required_str(value, "/sourceId")?.to_owned(),
        required_u64(value, "/span/start")?,
        required_u64(value, "/span/end")?,
        required_str(value, "/evidenceId")?.to_owned(),
    ))
}

#[cfg(feature = "collection-ordering-test-perturb")]
type RelationOrderKey = (String, String, String);

#[cfg(feature = "collection-ordering-test-perturb")]
fn relation_order_key(value: &Value) -> Result<RelationOrderKey, Box<dyn std::error::Error>> {
    Ok((
        required_str(value, "/kind")?.to_owned(),
        required_str(value, "/targetFindingId")?.to_owned(),
        required_str(value, "/relationId")?.to_owned(),
    ))
}

#[cfg(feature = "collection-ordering-test-perturb")]
type FileFindingOrderKey = (Vec<u8>, u64, u64, String);

#[cfg(feature = "collection-ordering-test-perturb")]
fn file_finding_order_key(
    value: &Value,
) -> Result<FileFindingOrderKey, Box<dyn std::error::Error>> {
    Ok((
        STANDARD.decode(required_str(value, "/path/canonicalBase64")?)?,
        required_u64(value, "/span/start")?,
        required_u64(value, "/span/end")?,
        required_str(value, "/findingId")?.to_owned(),
    ))
}

#[cfg(feature = "collection-ordering-test-perturb")]
fn retention_item_order_key(
    value: &Value,
) -> Result<(u8, u64, String), Box<dyn std::error::Error>> {
    let kind = required_str(value, "/kind")?;
    let rank = match kind {
        "attempt" => 0,
        "run" => 1,
        "gate" => 2,
        "gate-revision" => 3,
        "finding" => 4,
        "evidence" => 5,
        "operation" => 6,
        "transition" => 7,
        "pin-or-reference" => 8,
        "orphan-payload" => 9,
        "tombstone" => 10,
        _ => return Err(std::io::Error::other(format!("unknown retention kind {kind}")).into()),
    };
    Ok((
        rank,
        required_u64(value, "/owningSequence")?,
        required_str(value, "/recordId")?.to_owned(),
    ))
}

fn open_active_gate(
    root: &Path,
    operation_id: &str,
    path: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let pre_write = support::run(
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
    support::assert_status(&pre_write, 0);
    let response = json(&pre_write.stdout)?;
    assert_eq!(
        response.get("lifecycle").and_then(Value::as_str),
        Some("active")
    );
    response
        .get("gateId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| std::io::Error::other("pre-write omitted gateId").into())
}

fn create_related_fixture(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(root.join("src"))?;
    // src/lib.ts exports "dead" with zero production fan-in.
    fs::write(root.join("src/lib.ts"), "export const dead = 1;\n")?;
    // One test file that re-exports "dead" from lib.ts.
    fs::write(
        root.join("src/test.test.ts"),
        "export { dead as testDead } from './lib.js';\n",
    )?;
    Ok(())
}
