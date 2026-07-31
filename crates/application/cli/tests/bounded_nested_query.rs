mod support;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

/// Create a fixture with src/lib.ts (1 export with 101 evidence + 101 relations)
/// and 101 *.test.ts files that each re-export a named export from lib.ts.
/// This produces exactly 102 findings (1 from lib.ts + 101 from test files),
/// 102 evidence rows on the lib.ts finding (1 definition + 101 test-only-reexport),
/// and 101 relations on the lib.ts finding (one per test file re-export).
fn create_fixture(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(root.join("src"))?;

    // src/lib.ts exports "dead" with zero production fan-in.
    fs::write(root.join("src/lib.ts"), "export const dead = 1;\n")?;

    // 101 test files that re-export "dead" from ../src/lib.ts.
    // Each test file's own re-export has zero production fan-in → a finding.
    // Each re-export also creates a test-only-reexport evidence row + relation on lib.ts finding.
    for i in 0..101 {
        let filename = format!("src/{i:03}.test.ts");
        let content = format!("export {{ dead as dead{i:03} }} from './lib.js';\n");
        fs::write(root.join(&filename), content)?;
    }
    Ok(())
}

fn json(stdout: &str) -> Result<Value, Box<dyn std::error::Error>> {
    serde_json::from_str(stdout).map_err(Into::into)
}

fn next_cursor(response: &Value) -> Option<&str> {
    response.get("nextCursor").and_then(Value::as_str)
}

fn required_cursor(response: &Value) -> Result<&str, Box<dyn std::error::Error>> {
    response
        .get("nextCursor")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("required nextCursor missing").into())
}

fn scope_total(response: &Value) -> Result<u64, Box<dyn std::error::Error>> {
    response
        .get("scopeTotal")
        .and_then(Value::as_u64)
        .ok_or_else(|| std::io::Error::other("missing scopeTotal").into())
}

fn returned(response: &Value) -> Result<u64, Box<dyn std::error::Error>> {
    response
        .get("returned")
        .and_then(Value::as_u64)
        .ok_or_else(|| std::io::Error::other("missing returned").into())
}

fn items(response: &Value) -> Result<&Vec<Value>, Box<dyn std::error::Error>> {
    response
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("missing items array").into())
}

fn collect_all_findings(
    root: &Path,
    run_id: &str,
) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let page1 = support::run(root, &["findings", "--run", run_id, "--area", "dead-code"])?;
    support::assert_status(&page1, 0);
    let p1 = json(&page1.stdout)?;
    let mut all = items(&p1)?.clone();
    if let Some(c) = next_cursor(&p1) {
        let page2 = support::run(
            root,
            &[
                "findings",
                "--run",
                run_id,
                "--area",
                "dead-code",
                "--cursor",
                c,
            ],
        )?;
        support::assert_status(&page2, 0);
        all.extend(items(&json(&page2.stdout)?)?.iter().cloned());
    }
    Ok(all)
}

fn find_lib_finding(findings: &[Value]) -> Result<&Value, Box<dyn std::error::Error>> {
    findings
        .iter()
        .find(|item| {
            item.pointer("/path/display")
                .and_then(Value::as_str)
                .is_some_and(|d| d.contains("lib.ts"))
        })
        .ok_or_else(|| std::io::Error::other("lib.ts finding not found").into())
}

fn finding_id(finding: &Value) -> Result<&str, Box<dyn std::error::Error>> {
    finding
        .get("findingId")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("missing findingId").into())
}

// ─── Run findings pagination ─────────────────────────────────────────────

#[test]
fn run_findings_pages_102_as_100_plus_2() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    create_fixture(root.path())?;

    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);
    let run_id = support::field(&audit.stdout, "runId")?;

    // Page 1: 100 findings
    let page1 = support::run(
        root.path(),
        &["findings", "--run", &run_id, "--area", "dead-code"],
    )?;
    support::assert_status(&page1, 0);
    let p1 = json(&page1.stdout)?;
    assert_eq!(scope_total(&p1)?, 102);
    assert_eq!(returned(&p1)?, 100);
    assert_eq!(items(&p1)?.len(), 100);
    let cursor = required_cursor(&p1)?;

    // Page 2: 2 findings
    let page2 = support::run(
        root.path(),
        &[
            "findings",
            "--run",
            &run_id,
            "--area",
            "dead-code",
            "--cursor",
            cursor,
        ],
    )?;
    support::assert_status(&page2, 0);
    let p2 = json(&page2.stdout)?;
    assert_eq!(scope_total(&p2)?, 102);
    assert_eq!(returned(&p2)?, 2);
    assert_eq!(items(&p2)?.len(), 2);
    assert!(next_cursor(&p2).is_none());

    // Collect all finding IDs and assert exact totals and exactly-once
    let all_ids: Vec<String> = items(&p1)?
        .iter()
        .chain(items(&p2)?.iter())
        .map(|item| {
            item.get("findingId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| std::io::Error::other("finding item missing findingId"))?;
    assert_eq!(all_ids.len(), 102);
    let unique: BTreeSet<&String> = all_ids.iter().collect();
    assert_eq!(unique.len(), 102, "finding IDs must be exactly-once");
    Ok(())
}

// ─── Run explain: evidence pagination ─────────────────────────────────────

#[test]
fn run_explain_evidence_pages_102_as_100_plus_2() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    create_fixture(root.path())?;

    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);
    let run_id = support::field(&audit.stdout, "runId")?;

    // Collect all 102 findings
    let all_findings = collect_all_findings(root.path(), &run_id)?;
    assert_eq!(all_findings.len(), 102);
    let all_finding_ids: BTreeSet<String> = all_findings
        .iter()
        .filter_map(|f| {
            f.get("findingId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    assert_eq!(all_finding_ids.len(), 102);

    let lib_finding = find_lib_finding(&all_findings)?;
    let fid = finding_id(lib_finding)?;

    // Explain page 1
    let explain1 = support::run(root.path(), &["explain", "--run", &run_id, fid])?;
    support::assert_status(&explain1, 0);
    let e1 = json(&explain1.stdout)?;
    let evidence1 = e1
        .get("evidence")
        .ok_or_else(|| std::io::Error::other("missing evidence"))?;
    assert_eq!(scope_total(evidence1)?, 102);
    assert_eq!(returned(evidence1)?, 100);
    let ev_cursor = required_cursor(evidence1)?;

    // Explain page 2
    let explain2 = support::run(
        root.path(),
        &[
            "explain",
            "--run",
            &run_id,
            fid,
            "--evidence-cursor",
            ev_cursor,
        ],
    )?;
    support::assert_status(&explain2, 0);
    let e2 = json(&explain2.stdout)?;
    let evidence2 = e2
        .get("evidence")
        .ok_or_else(|| std::io::Error::other("missing evidence"))?;
    assert_eq!(scope_total(evidence2)?, 102);
    assert_eq!(returned(evidence2)?, 2);
    assert!(next_cursor(evidence2).is_none());

    // Collect all evidence IDs across pages and verify exactly-once
    let ev_ids_1: Vec<String> = items(evidence1)?
        .iter()
        .filter_map(|e| {
            e.get("evidenceId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    let ev_ids_2: Vec<String> = items(evidence2)?
        .iter()
        .filter_map(|e| {
            e.get("evidenceId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    let mut all_ev_ids = ev_ids_1;
    all_ev_ids.extend(ev_ids_2);
    assert_eq!(all_ev_ids.len(), 102);
    let unique_ev: BTreeSet<&String> = all_ev_ids.iter().collect();
    assert_eq!(unique_ev.len(), 102, "evidence IDs must be exactly-once");

    Ok(())
}

// ─── Run explain: relations pagination ─────────────────────────────────────

#[test]
fn run_explain_relations_pages_101_as_100_plus_1() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    create_fixture(root.path())?;

    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);
    let run_id = support::field(&audit.stdout, "runId")?;

    // Collect all 102 findings and their IDs for relation target validation
    let all_findings = collect_all_findings(root.path(), &run_id)?;
    assert_eq!(all_findings.len(), 102);
    let all_finding_ids: BTreeSet<String> = all_findings
        .iter()
        .filter_map(|f| {
            f.get("findingId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    assert_eq!(all_finding_ids.len(), 102);

    let lib_finding = find_lib_finding(&all_findings)?;
    let fid = finding_id(lib_finding)?;

    // Get full evidence set for grounding check
    let explain_full = support::run(root.path(), &["explain", "--run", &run_id, fid])?;
    support::assert_status(&explain_full, 0);
    let ef = json(&explain_full.stdout)?;
    let evidence_section = ef
        .get("evidence")
        .ok_or_else(|| std::io::Error::other("missing evidence"))?;
    let ev_cursor_str = required_cursor(evidence_section)?;
    // Collect page-1 evidence IDs
    let mut complete_evidence_ids: BTreeSet<String> = items(evidence_section)?
        .iter()
        .filter_map(|e| {
            e.get("evidenceId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    // Collect page-2 evidence IDs
    let explain_ev2 = support::run(
        root.path(),
        &[
            "explain",
            "--run",
            &run_id,
            fid,
            "--evidence-cursor",
            ev_cursor_str,
        ],
    )?;
    support::assert_status(&explain_ev2, 0);
    let ef2 = json(&explain_ev2.stdout)?;
    let evidence_section_2 = ef2
        .get("evidence")
        .ok_or_else(|| std::io::Error::other("missing evidence page 2"))?;
    complete_evidence_ids.extend(items(evidence_section_2)?.iter().filter_map(|e| {
        e.get("evidenceId")
            .and_then(Value::as_str)
            .map(str::to_owned)
    }));
    assert_eq!(complete_evidence_ids.len(), 102);

    // Relations page 1
    let explain1 = support::run(root.path(), &["explain", "--run", &run_id, fid])?;
    support::assert_status(&explain1, 0);
    let e1 = json(&explain1.stdout)?;
    let relations1 = e1
        .get("relations")
        .ok_or_else(|| std::io::Error::other("missing relations"))?;
    assert_eq!(scope_total(relations1)?, 101);
    assert_eq!(returned(relations1)?, 100);
    let rel_cursor = required_cursor(relations1)?;

    // Relations page 2
    let explain2 = support::run(
        root.path(),
        &[
            "explain",
            "--run",
            &run_id,
            fid,
            "--relations-cursor",
            rel_cursor,
        ],
    )?;
    support::assert_status(&explain2, 0);
    let e2 = json(&explain2.stdout)?;
    let relations2 = e2
        .get("relations")
        .ok_or_else(|| std::io::Error::other("missing relations"))?;
    assert_eq!(scope_total(relations2)?, 101);
    assert_eq!(returned(relations2)?, 1);
    assert!(next_cursor(relations2).is_none());

    // Collect all relation IDs across pages and verify exactly-once
    let all_relations: Vec<&Value> = items(relations1)?
        .iter()
        .chain(items(relations2)?.iter())
        .collect();
    assert_eq!(all_relations.len(), 101);

    let relation_ids: Vec<String> = all_relations
        .iter()
        .filter_map(|r| {
            r.get("relationId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    assert_eq!(relation_ids.len(), 101);
    let unique_rel: BTreeSet<&String> = relation_ids.iter().collect();
    assert_eq!(unique_rel.len(), 101, "relation IDs must be exactly-once");

    // Assert relation grounding evidence IDs are present in the finding's complete evidence set
    for rel in &all_relations {
        let grounding_id = rel
            .get("groundingEvidenceId")
            .and_then(Value::as_str)
            .ok_or_else(|| std::io::Error::other("relation missing groundingEvidenceId"))?;
        assert!(
            complete_evidence_ids.contains(grounding_id),
            "grounding evidence ID {grounding_id} not in finding's evidence set"
        );
    }

    // Assert relation target finding IDs are present in the complete 102-finding set
    for rel in &all_relations {
        let target_id = rel
            .get("targetFindingId")
            .and_then(Value::as_str)
            .ok_or_else(|| std::io::Error::other("relation missing targetFindingId"))?;
        assert!(
            all_finding_ids.contains(target_id),
            "relation target {target_id} not in the 102-finding set"
        );
    }

    Ok(())
}

// ─── Cursor immutability across repository mutation ────────────────────────

#[test]
fn run_cursor_survives_second_audit_mutation() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    create_fixture(root.path())?;

    // First audit
    let audit1 = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit1, 0);
    let run_id_1 = support::field(&audit1.stdout, "runId")?;

    // Get cursor from first run
    let page1 = support::run(
        root.path(),
        &["findings", "--run", &run_id_1, "--area", "dead-code"],
    )?;
    support::assert_status(&page1, 0);
    let p1 = json(&page1.stdout)?;
    let cursor = required_cursor(&p1)?;

    // Mutate the repository: add a new file
    fs::write(
        root.path().join("src/extra.ts"),
        "export const extra = 42;\n",
    )?;

    // Second audit (new run)
    let audit2 = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit2, 0);
    let run_id_2 = support::field(&audit2.stdout, "runId")?;
    assert_ne!(run_id_1, run_id_2);

    // Original cursor still works against original run
    let page2 = support::run(
        root.path(),
        &[
            "findings",
            "--run",
            &run_id_1,
            "--area",
            "dead-code",
            "--cursor",
            cursor,
        ],
    )?;
    support::assert_status(&page2, 0);
    let p2 = json(&page2.stdout)?;
    assert_eq!(returned(&p2)?, 2);

    Ok(())
}

// ─── Cross-run cursor rejection ──────────────────────────────────────────

#[test]
fn cross_run_cursor_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    create_fixture(root.path())?;

    let audit1 = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit1, 0);
    let run_id_1 = support::field(&audit1.stdout, "runId")?;

    // Get cursor from run 1
    let page1 = support::run(
        root.path(),
        &["findings", "--run", &run_id_1, "--area", "dead-code"],
    )?;
    support::assert_status(&page1, 0);
    let p1 = json(&page1.stdout)?;
    let cursor = required_cursor(&p1)?;

    // Create a different run
    fs::write(
        root.path().join("src/extra.ts"),
        "export const extra = 42;\n",
    )?;
    let audit2 = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit2, 0);
    let run_id_2 = support::field(&audit2.stdout, "runId")?;

    // Use run-1 cursor against run-2 → must fail with exit 2 and cursor-scope diagnostic
    let result = support::run(
        root.path(),
        &[
            "findings",
            "--run",
            &run_id_2,
            "--area",
            "dead-code",
            "--cursor",
            cursor,
        ],
    )?;
    support::assert_status(&result, 2);
    assert!(
        result.stderr.contains("cursor scope"),
        "stderr must contain cursor-scope diagnostic: {}",
        result.stderr
    );
    Ok(())
}

// ─── Cross-gate cursor rejection ─────────────────────────────────────────

#[test]
fn cross_gate_and_run_vs_gate_cursor_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    create_fixture(root.path())?;

    // Audit to create a run
    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);
    let run_id = support::field(&audit.stdout, "runId")?;

    // Get a run cursor (required)
    let page1 = support::run(
        root.path(),
        &["findings", "--run", &run_id, "--area", "dead-code"],
    )?;
    support::assert_status(&page1, 0);
    let p1 = json(&page1.stdout)?;
    let run_cursor = required_cursor(&p1)?;

    // Open a gate and close it to get a revision with evidence
    let pre = support::run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-1",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    support::assert_status(&pre, 0);
    let pre_json = json(&pre.stdout)?;
    let gate_id = pre_json
        .get("gateId")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("missing gateId"))?;

    // Close the gate to seal revision 1
    let post = support::run(
        root.path(),
        &["post-write", gate_id, "--operation-id", "op-close-1"],
    )?;
    support::assert_status(&post, 0);

    // Get gate findings at revision 1 (produces a cursor because fixture has 102 findings)
    let gate_findings = support::run(
        root.path(),
        &["gate", "findings", gate_id, "--revision", "1"],
    )?;
    support::assert_status(&gate_findings, 0);
    let gf = json(&gate_findings.stdout)?;
    let gate_cursor = required_cursor(&gf)?;

    // Use run cursor against gate → must fail with exit 2 and cursor-scope diagnostic
    let result = support::run(
        root.path(),
        &[
            "gate",
            "findings",
            gate_id,
            "--revision",
            "1",
            "--cursor",
            run_cursor,
        ],
    )?;
    support::assert_status(&result, 2);
    assert!(
        result.stderr.contains("cursor scope") || result.stderr.contains("cursor payload"),
        "run cursor on gate must produce cursor diagnostic: {}",
        result.stderr
    );

    // Use gate cursor on run → must fail with exit 2
    let result2 = support::run(
        root.path(),
        &[
            "findings",
            "--run",
            &run_id,
            "--area",
            "dead-code",
            "--cursor",
            gate_cursor,
        ],
    )?;
    support::assert_status(&result2, 2);

    Ok(())
}

// ─── Cross-revision gate cursor rejection ────────────────────────────────

#[test]
fn cross_gate_cursor_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    create_fixture(root.path())?;

    // Open gate A
    let pre_a = support::run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-a",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    support::assert_status(&pre_a, 0);
    let pre_a_json = json(&pre_a.stdout)?;
    let gate_a = pre_a_json
        .get("gateId")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("missing gateId A"))?;

    // Close gate A
    let post_a = support::run(
        root.path(),
        &["post-write", gate_a, "--operation-id", "op-close-a"],
    )?;
    support::assert_status(&post_a, 0);

    // Open gate B on a different path
    let pre_b = support::run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-b",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    support::assert_status(&pre_b, 0);
    let pre_b_json = json(&pre_b.stdout)?;
    let gate_b = pre_b_json
        .get("gateId")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("missing gateId B"))?;

    // Close gate B
    let post_b = support::run(
        root.path(),
        &["post-write", gate_b, "--operation-id", "op-close-b"],
    )?;
    support::assert_status(&post_b, 0);

    // Get cursor from gate A at revision 1 (required)
    let gf_a = support::run(
        root.path(),
        &["gate", "findings", gate_a, "--revision", "1"],
    )?;
    support::assert_status(&gf_a, 0);
    let gf_a_json = json(&gf_a.stdout)?;
    let cursor_a = required_cursor(&gf_a_json)?;

    // Use gate A cursor on gate B → must fail with exit 2 and cursor-scope diagnostic
    let result = support::run(
        root.path(),
        &[
            "gate",
            "findings",
            gate_b,
            "--revision",
            "1",
            "--cursor",
            cursor_a,
        ],
    )?;
    support::assert_status(&result, 2);
    assert!(
        result.stderr.contains("cursor scope"),
        "cross-gate cursor must produce cursor-scope diagnostic: {}",
        result.stderr
    );
    Ok(())
}

// ─── Tampered cursor rejection ───────────────────────────────────────────

#[test]
fn tampered_cursor_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    create_fixture(root.path())?;

    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);
    let run_id = support::field(&audit.stdout, "runId")?;

    // Obtain a real issued cursor from a paginated findings query
    let page1 = support::run(
        root.path(),
        &["findings", "--run", &run_id, "--area", "dead-code"],
    )?;
    support::assert_status(&page1, 0);
    let p1 = json(&page1.stdout)?;
    let real_cursor = required_cursor(&p1)?;

    // Deterministically mutate the cursor bytes while keeping valid Base64URL alphabet.
    // Flip each ASCII byte to its complement within the URL-safe Base64 alphabet
    // (A-Z, a-z, 0-9, -, _). This preserves structural plausibility but corrupts the payload.
    let tampered: String = real_cursor
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' => b'A' + (b'Z' - byte),
            b'a'..=b'z' => b'a' + (b'z' - byte),
            b'0'..=b'9' => b'0' + (b'9' - byte),
            b'-' => b'_',
            b'_' => b'-',
            other => other,
        })
        .map(|byte| byte as char)
        .collect();

    // The tampered string is valid Base64URL alphabet but decodes to garbage payload
    let result = support::run(
        root.path(),
        &[
            "findings",
            "--run",
            &run_id,
            "--area",
            "dead-code",
            "--cursor",
            &tampered,
        ],
    )?;
    support::assert_status(&result, 2);
    assert!(
        result.stderr.contains("cursor")
            && (result.stderr.contains("Base64")
                || result.stderr.contains("payload")
                || result.stderr.contains("scope")),
        "tampered cursor must produce cursor encoding/payload/scope diagnostic: {}",
        result.stderr
    );
    Ok(())
}

// ─── Cross-finding cursor rejection ─────────────────────────────────────

#[test]
fn cross_finding_evidence_cursor_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    create_fixture(root.path())?;

    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);
    let run_id = support::field(&audit.stdout, "runId")?;

    // Get all findings
    let all_findings = collect_all_findings(root.path(), &run_id)?;

    // Find the lib.ts finding (has 102 evidence)
    let lib_finding = find_lib_finding(&all_findings)?;
    let lib_finding_id = finding_id(lib_finding)?;

    // Get an evidence cursor for the lib.ts finding (required)
    let explain1 = support::run(root.path(), &["explain", "--run", &run_id, lib_finding_id])?;
    support::assert_status(&explain1, 0);
    let e1 = json(&explain1.stdout)?;
    let evidence_section = e1
        .get("evidence")
        .ok_or_else(|| std::io::Error::other("missing evidence"))?;
    let evidence_cursor = required_cursor(evidence_section)?;

    // Use evidence cursor of lib.ts finding on a different finding → must fail with exit 2
    let other_finding = all_findings
        .iter()
        .find(|item| {
            item.pointer("/path/display")
                .and_then(Value::as_str)
                .is_some_and(|d| !d.contains("lib.ts"))
        })
        .ok_or_else(|| std::io::Error::other("other finding not found"))?;
    let other_finding_id = finding_id(other_finding)?;

    let result = support::run(
        root.path(),
        &[
            "explain",
            "--run",
            &run_id,
            other_finding_id,
            "--evidence-cursor",
            evidence_cursor,
        ],
    )?;
    support::assert_status(&result, 2);
    assert!(
        result.stderr.contains("cursor scope"),
        "cross-finding cursor must produce cursor-scope diagnostic: {}",
        result.stderr
    );
    Ok(())
}

// ─── Cross-collection cursor rejection ───────────────────────────────────

#[test]
fn cross_collection_cursor_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    create_fixture(root.path())?;

    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);
    let run_id = support::field(&audit.stdout, "runId")?;

    // Get the lib.ts finding
    let all_findings = collect_all_findings(root.path(), &run_id)?;
    let lib_finding = find_lib_finding(&all_findings)?;
    let fid = finding_id(lib_finding)?;

    // Get evidence cursor (required)
    let explain = support::run(root.path(), &["explain", "--run", &run_id, fid])?;
    support::assert_status(&explain, 0);
    let e = json(&explain.stdout)?;
    let evidence_section = e
        .get("evidence")
        .ok_or_else(|| std::io::Error::other("missing evidence"))?;
    let evidence_cursor = required_cursor(evidence_section)?;

    // Use evidence cursor as relations cursor → must fail with exit 2
    let result = support::run(
        root.path(),
        &[
            "explain",
            "--run",
            &run_id,
            fid,
            "--relations-cursor",
            evidence_cursor,
        ],
    )?;
    support::assert_status(&result, 2);
    assert!(
        result.stderr.contains("cursor scope"),
        "evidence cursor as relations must produce cursor-scope diagnostic: {}",
        result.stderr
    );

    Ok(())
}

// ─── Cross-repository cursor rejection ───────────────────────────────────

#[test]
fn cross_repository_cursor_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let root1 = tempfile::tempdir()?;
    create_fixture(root1.path())?;

    // Create a different fixture in root2 (different content → different run_id)
    let root2 = tempfile::tempdir()?;
    fs::create_dir_all(root2.path().join("src"))?;
    fs::write(root2.path().join("src/lib.ts"), "export const other = 1;\n")?;
    for i in 0..101 {
        let filename = format!("src/{i:03}.test.ts");
        let content = format!("export {{ other as other{i:03} }} from './lib.js';\n");
        fs::write(root2.path().join(&filename), content)?;
    }

    // Audit repo 1
    let audit1 = support::run(root1.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit1, 0);
    let run_id_1 = support::field(&audit1.stdout, "runId")?;

    // Audit repo 2
    let audit2 = support::run(root2.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit2, 0);

    // Get cursor from repo 1 (required)
    let page1 = support::run(
        root1.path(),
        &["findings", "--run", &run_id_1, "--area", "dead-code"],
    )?;
    support::assert_status(&page1, 0);
    let p1 = json(&page1.stdout)?;
    let cursor = required_cursor(&p1)?;

    // Use repo-1 cursor in repo-2 with run_id_1 → must fail
    // (the run doesn't exist in repo 2's store)
    let result = support::run(
        root2.path(),
        &[
            "findings",
            "--run",
            &run_id_1,
            "--area",
            "dead-code",
            "--cursor",
            cursor,
        ],
    )?;
    assert_ne!(
        result.status, 0,
        "cursor from repo 1 must not work in repo 2"
    );

    Ok(())
}

// ─── Gate revision immutability and cross-revision boundary ──────────────

#[test]
fn gate_revision_boundary_immutability() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    create_fixture(root.path())?;

    // ── Open gate → immutable revision 0 baseline ──
    let pre = support::run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-rev-open",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    support::assert_status(&pre, 0);
    let pre_json = json(&pre.stdout)?;
    let gate_id = pre_json
        .get("gateId")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("missing gateId"))?
        .to_owned();
    let open_revision = pre_json
        .get("revision")
        .and_then(Value::as_u64)
        .ok_or_else(|| std::io::Error::other("missing revision on open"))?;
    assert_eq!(open_revision, 0, "open gate must produce revision 0");

    // ── Obtain revision-0 findings page 1 cursor (102 findings: 100 + 2) ──
    let gf1 = support::run(
        root.path(),
        &["gate", "findings", &gate_id, "--revision", "0"],
    )?;
    support::assert_status(&gf1, 0);
    let gf1_json = json(&gf1.stdout)?;
    assert_eq!(scope_total(&gf1_json)?, 102);
    assert_eq!(returned(&gf1_json)?, 100);
    let findings_cursor_r0 = required_cursor(&gf1_json)?.to_owned();
    let findings_page1_ids: BTreeSet<String> = items(&gf1_json)?
        .iter()
        .filter_map(|f| {
            f.get("findingId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    assert_eq!(findings_page1_ids.len(), 100);

    // ── Obtain revision-0 evidence page 1 cursor (lib.ts finding: 102 evidence → 100 + 2) ──
    let lib_gate_finding = items(&gf1_json)?
        .iter()
        .find(|item| {
            item.pointer("/path/display")
                .and_then(Value::as_str)
                .is_some_and(|d| d.contains("lib.ts"))
        })
        .ok_or_else(|| std::io::Error::other("lib.ts finding not in page 1"))?;
    let lib_fid = finding_id(lib_gate_finding)?.to_owned();

    let ge1 = support::run(
        root.path(),
        &["gate", "explain", &gate_id, "--revision", "0", &lib_fid],
    )?;
    support::assert_status(&ge1, 0);
    let ge1_json = json(&ge1.stdout)?;
    let ev_section = ge1_json
        .get("evidence")
        .ok_or_else(|| std::io::Error::other("missing evidence in gate explain rev 0"))?;
    assert_eq!(scope_total(ev_section)?, 102);
    assert_eq!(returned(ev_section)?, 100);
    let ev_cursor_r0 = required_cursor(ev_section)?.to_owned();
    let ev_page1_ids: BTreeSet<String> = items(ev_section)?
        .iter()
        .filter_map(|e| {
            e.get("evidenceId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    assert_eq!(ev_page1_ids.len(), 100);

    // ── Obtain revision-0 relations page 1 cursor (lib.ts finding: 101 relations → 100 + 1) ──
    let rel_section = ge1_json
        .get("relations")
        .ok_or_else(|| std::io::Error::other("missing relations in gate explain rev 0"))?;
    assert_eq!(scope_total(rel_section)?, 101);
    assert_eq!(returned(rel_section)?, 100);
    let rel_cursor_r0 = required_cursor(rel_section)?.to_owned();
    let rel_page1_ids: BTreeSet<String> = items(rel_section)?
        .iter()
        .filter_map(|r| {
            r.get("relationId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    assert_eq!(rel_page1_ids.len(), 100);

    // ── Close the same gate → revision 1 ──
    let post = support::run(
        root.path(),
        &["post-write", &gate_id, "--operation-id", "op-rev-close"],
    )?;
    support::assert_status(&post, 0);
    let post_json = json(&post.stdout)?;
    let closed_revision = post_json
        .get("revision")
        .and_then(Value::as_u64)
        .ok_or_else(|| std::io::Error::other("missing revision on close"))?;
    assert_eq!(closed_revision, 1, "close must produce revision 1");

    // ── After revision 1 exists, resume all three old cursors against revision 0 ──

    // Resume findings cursor at revision 0: expect 2 remaining, no next cursor
    let gf2 = support::run(
        root.path(),
        &[
            "gate",
            "findings",
            &gate_id,
            "--revision",
            "0",
            "--cursor",
            &findings_cursor_r0,
        ],
    )?;
    support::assert_status(&gf2, 0);
    let gf2_json = json(&gf2.stdout)?;
    assert_eq!(returned(&gf2_json)?, 2);
    assert!(
        next_cursor(&gf2_json).is_none(),
        "findings page 2 must have no next cursor"
    );
    let findings_page2_ids: BTreeSet<String> = items(&gf2_json)?
        .iter()
        .filter_map(|f| {
            f.get("findingId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    assert_eq!(findings_page2_ids.len(), 2);
    // Exactly-once union: no overlap with page 1
    assert!(
        findings_page1_ids.is_disjoint(&findings_page2_ids),
        "findings pages must not overlap"
    );
    let all_finding_ids: BTreeSet<String> = findings_page1_ids
        .union(&findings_page2_ids)
        .cloned()
        .collect();
    assert_eq!(all_finding_ids.len(), 102);

    // Resume evidence cursor at revision 0: expect 2 remaining, no next cursor
    let ge2 = support::run(
        root.path(),
        &[
            "gate",
            "explain",
            &gate_id,
            "--revision",
            "0",
            &lib_fid,
            "--evidence-cursor",
            &ev_cursor_r0,
        ],
    )?;
    support::assert_status(&ge2, 0);
    let ge2_json = json(&ge2.stdout)?;
    let ev_section2 = ge2_json
        .get("evidence")
        .ok_or_else(|| std::io::Error::other("missing evidence page 2"))?;
    assert_eq!(returned(ev_section2)?, 2);
    assert!(
        next_cursor(ev_section2).is_none(),
        "evidence page 2 must have no next cursor"
    );
    let ev_page2_ids: BTreeSet<String> = items(ev_section2)?
        .iter()
        .filter_map(|e| {
            e.get("evidenceId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    assert_eq!(ev_page2_ids.len(), 2);
    assert!(
        ev_page1_ids.is_disjoint(&ev_page2_ids),
        "evidence pages must not overlap"
    );
    let all_ev_ids: BTreeSet<String> = ev_page1_ids.union(&ev_page2_ids).cloned().collect();
    assert_eq!(all_ev_ids.len(), 102);

    // Resume relations cursor at revision 0: expect 1 remaining, no next cursor
    let ge3 = support::run(
        root.path(),
        &[
            "gate",
            "explain",
            &gate_id,
            "--revision",
            "0",
            &lib_fid,
            "--relations-cursor",
            &rel_cursor_r0,
        ],
    )?;
    support::assert_status(&ge3, 0);
    let ge3_json = json(&ge3.stdout)?;
    let rel_section2 = ge3_json
        .get("relations")
        .ok_or_else(|| std::io::Error::other("missing relations page 2"))?;
    assert_eq!(returned(rel_section2)?, 1);
    assert!(
        next_cursor(rel_section2).is_none(),
        "relations page 2 must have no next cursor"
    );
    let rel_page2_ids: BTreeSet<String> = items(rel_section2)?
        .iter()
        .filter_map(|r| {
            r.get("relationId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    assert_eq!(rel_page2_ids.len(), 1);
    assert!(
        rel_page1_ids.is_disjoint(&rel_page2_ids),
        "relations pages must not overlap"
    );
    let all_rel_ids: BTreeSet<String> = rel_page1_ids.union(&rel_page2_ids).cloned().collect();
    assert_eq!(all_rel_ids.len(), 101);

    // ── Verify relation grounding evidence and targets against complete revision-0 sets ──
    let all_relations: Vec<&Value> = items(rel_section)?
        .iter()
        .chain(items(rel_section2)?.iter())
        .collect();
    assert_eq!(all_relations.len(), 101);

    for rel in &all_relations {
        let grounding_id = rel
            .get("groundingEvidenceId")
            .and_then(Value::as_str)
            .ok_or_else(|| std::io::Error::other("relation missing groundingEvidenceId"))?;
        assert!(
            all_ev_ids.contains(grounding_id),
            "grounding evidence ID {grounding_id} not in revision-0 evidence set"
        );
    }

    for rel in &all_relations {
        let target_id = rel
            .get("targetFindingId")
            .and_then(Value::as_str)
            .ok_or_else(|| std::io::Error::other("relation missing targetFindingId"))?;
        assert!(
            all_finding_ids.contains(target_id),
            "relation target {target_id} not in revision-0 finding set"
        );
    }

    // ── Present each old revision-0 cursor to revision 1 → exit 2, cursor-scope diagnostic ──

    let cross_rev_findings = support::run(
        root.path(),
        &[
            "gate",
            "findings",
            &gate_id,
            "--revision",
            "1",
            "--cursor",
            &findings_cursor_r0,
        ],
    )?;
    support::assert_status(&cross_rev_findings, 2);
    assert!(
        cross_rev_findings.stderr.contains("cursor scope"),
        "findings cursor from rev 0 must be rejected on rev 1: {}",
        cross_rev_findings.stderr
    );

    let cross_rev_ev = support::run(
        root.path(),
        &[
            "gate",
            "explain",
            &gate_id,
            "--revision",
            "1",
            &lib_fid,
            "--evidence-cursor",
            &ev_cursor_r0,
        ],
    )?;
    support::assert_status(&cross_rev_ev, 2);
    assert!(
        cross_rev_ev.stderr.contains("cursor scope"),
        "evidence cursor from rev 0 must be rejected on rev 1: {}",
        cross_rev_ev.stderr
    );

    let cross_rev_rel = support::run(
        root.path(),
        &[
            "gate",
            "explain",
            &gate_id,
            "--revision",
            "1",
            &lib_fid,
            "--relations-cursor",
            &rel_cursor_r0,
        ],
    )?;
    support::assert_status(&cross_rev_rel, 2);
    assert!(
        cross_rev_rel.stderr.contains("cursor scope"),
        "relations cursor from rev 0 must be rejected on rev 1: {}",
        cross_rev_rel.stderr
    );

    Ok(())
}
