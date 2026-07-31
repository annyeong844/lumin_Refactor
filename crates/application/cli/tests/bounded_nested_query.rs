mod support;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::Value;

const CURSOR_BINDING_DOMAIN: &[u8] = b"lumin-cursor-binding.v1\0";
const CURSOR_BINDING_HEADER_LEN: usize = CURSOR_BINDING_DOMAIN.len() + 1 + 32;

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

#[path = "bounded_nested_query/cursor_integrity.rs"]
mod cursor_integrity;
