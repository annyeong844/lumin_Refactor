use super::*;

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
fn structured_tampered_cursor_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    create_fixture(root.path())?;

    let audit = support::run(root.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit, 0);
    let run_id = support::field(&audit.stdout, "runId")?;

    let page1 = support::run(
        root.path(),
        &["findings", "--run", &run_id, "--area", "dead-code"],
    )?;
    support::assert_status(&page1, 0);
    let p1 = json(&page1.stdout)?;
    let real_cursor = required_cursor(&p1)?;
    let substituted_id = p1
        .pointer("/items/0/findingId")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("missing substitute finding ID"))?;

    // Keep a valid content-binding envelope and valid inner JSON, but replace lastId
    // with another real row while retaining the binding issued for the original payload.
    let mut envelope = URL_SAFE_NO_PAD.decode(real_cursor)?;
    assert!(
        envelope.starts_with(CURSOR_BINDING_DOMAIN) && envelope.len() >= CURSOR_BINDING_HEADER_LEN,
        "issued cursor must use the bound envelope"
    );
    let mut payload: Value = serde_json::from_slice(&envelope[CURSOR_BINDING_HEADER_LEN..])?;
    payload["lastId"] = Value::String(substituted_id.to_owned());
    let changed_payload = serde_json::to_vec(&payload)?;
    let _: Value = serde_json::from_slice(&changed_payload)?;
    envelope.truncate(CURSOR_BINDING_HEADER_LEN);
    envelope.extend_from_slice(&changed_payload);
    let tampered = URL_SAFE_NO_PAD.encode(envelope);

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
        result.stdout.is_empty(),
        "malformed cursor must emit no stdout"
    );
    assert!(
        result.stderr.contains("cursor") && result.stderr.contains("content binding"),
        "structured payload mutation must produce content-binding diagnostic: {}",
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
    // Two IDENTICAL fixture repositories have the same finding anchors and fresh
    // repository-local sequences, but distinct physical roots and RepositoryIds.
    let root1 = tempfile::tempdir()?;
    create_fixture(root1.path())?;

    let root2 = tempfile::tempdir()?;
    create_fixture(root2.path())?;

    // Audit both repos with identical content
    let audit1 = support::run(root1.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit1, 0);
    let run_id_1 = support::field(&audit1.stdout, "runId")?;

    let audit2 = support::run(root2.path(), &["audit", "--jobs", "1"])?;
    support::assert_status(&audit2, 0);
    let run_id_2 = support::field(&audit2.stdout, "runId")?;

    // Fresh stores allocate the same first repository-local run ID.
    assert_eq!(
        run_id_1, run_id_2,
        "fresh stores must allocate the same first run ID"
    );

    // Get a run cursor from repo1
    let page1 = support::run(
        root1.path(),
        &["findings", "--run", &run_id_1, "--area", "dead-code"],
    )?;
    support::assert_status(&page1, 0);
    let p1 = json(&page1.stdout)?;
    let cursor = required_cursor(&p1)?;

    // Use repo1 run cursor against repo2 with the same run ID → must fail with exit 2 cursor-scope
    let result = support::run(
        root2.path(),
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
        "run cursor from repo1 must be rejected on repo2 with cursor-scope diagnostic: {}",
        result.stderr
    );

    // Gate cross-repository test: open identical gates in both repos
    let pre1 = support::run(
        root1.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-cross-repo",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    support::assert_status(&pre1, 0);
    let pre1_json = json(&pre1.stdout)?;
    let gate_id_1 = pre1_json
        .get("gateId")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("missing gateId repo1"))?
        .to_owned();

    let pre2 = support::run(
        root2.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-cross-repo",
            "--path",
            "src/lib.ts",
            "--jobs",
            "1",
        ],
    )?;
    support::assert_status(&pre2, 0);
    let pre2_json = json(&pre2.stdout)?;
    let gate_id_2 = pre2_json
        .get("gateId")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("missing gateId repo2"))?
        .to_owned();

    // Fresh stores allocate the same first repository-local gate ID.
    assert_eq!(
        gate_id_1, gate_id_2,
        "fresh stores must allocate the same first gate ID"
    );

    // Close both gates
    let post1 = support::run(
        root1.path(),
        &["post-write", &gate_id_1, "--operation-id", "op-close-cross"],
    )?;
    support::assert_status(&post1, 0);

    let post2 = support::run(
        root2.path(),
        &["post-write", &gate_id_2, "--operation-id", "op-close-cross"],
    )?;
    support::assert_status(&post2, 0);

    // Get gate cursor from repo1 revision 0
    let gf1 = support::run(
        root1.path(),
        &["gate", "findings", &gate_id_1, "--revision", "0"],
    )?;
    support::assert_status(&gf1, 0);
    let gf1_json = json(&gf1.stdout)?;
    let gate_cursor = required_cursor(&gf1_json)?;

    // Use repo1 gate cursor against repo2 identical gate → must fail with exit 2 cursor-scope
    let gate_result = support::run(
        root2.path(),
        &[
            "gate",
            "findings",
            &gate_id_2,
            "--revision",
            "0",
            "--cursor",
            gate_cursor,
        ],
    )?;
    support::assert_status(&gate_result, 2);
    assert!(
        gate_result.stderr.contains("cursor scope"),
        "gate cursor from repo1 must be rejected on repo2 with cursor-scope diagnostic: {}",
        gate_result.stderr
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
