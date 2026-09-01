use super::*;

#[test]
fn disjoint_gates_reconcile_a_terminal_transition_on_retry()
-> Result<(), Box<dyn std::error::Error>> {
    let root = disjoint_fixture()?;
    let gate_a = open_gate(root.path(), "op-a-open", "src/a.ts")?;
    let gate_b = open_gate(root.path(), "op-b-open", "src/b.ts")?;

    fs::write(root.path().join("src/b.ts"), "console.log('b2');\n")?;
    let pending_a = run(
        root.path(),
        &["post-write", &gate_a, "--operation-id", "op-a-pending"],
    )?;
    assert_status(&pending_a, 4);
    assert_eq!(field(&pending_a.stdout, "decision")?, "incomplete");
    assert!(
        serde_json::from_str::<Value>(&pending_a.stdout)?
            .get("signals")
            .and_then(Value::as_array)
            .is_some_and(|signals| signals.iter().any(|signal| {
                signal.get("kind").and_then(Value::as_str) == Some("active-transition-pending")
            }))
    );

    let close_b = run(
        root.path(),
        &["post-write", &gate_b, "--operation-id", "op-b-close"],
    )?;
    assert_status(&close_b, 0);
    assert_eq!(field(&close_b.stdout, "decision")?, "allow");

    let active_a = run(root.path(), &["gate", "show", &gate_a])?;
    assert_status(&active_a, 0);
    let active_a: Value = serde_json::from_str(&active_a.stdout)?;
    let transition_refs = active_a
        .get("transitionRefs")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("active gate omitted transitionRefs"))?;
    assert_eq!(transition_refs.len(), 1);
    let terminal_transition_sequence = transition_refs[0]
        .as_u64()
        .ok_or_else(|| std::io::Error::other("transition reference was not a sequence"))?;

    let protected_plan = prepare_and_show_gate_plan(root.path(), "op-transition-plan-protected")?;
    assert_eq!(protected_plan.get("total").and_then(Value::as_u64), Some(2));
    assert_eq!(
        protected_plan.get("returned").and_then(Value::as_u64),
        Some(2)
    );
    assert_eq!(
        protected_plan
            .get("items")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );
    let exclusions = protected_plan
        .get("exclusions")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("protected gate plan omitted exclusions"))?;
    let mut active_a_exclusions = exclusions.iter().filter(|exclusion| {
        exclusion.get("kind").and_then(Value::as_str) == Some("gate")
            && exclusion.get("recordId").and_then(Value::as_str) == Some(gate_a.as_str())
            && exclusion.pointer("/reason/reason").and_then(Value::as_str) == Some("active-gate")
    });
    assert!(active_a_exclusions.next().is_some());
    assert!(active_a_exclusions.next().is_none());
    let mut referenced_b_exclusions = exclusions.iter().filter(|exclusion| {
        exclusion.get("kind").and_then(Value::as_str) == Some("gate")
            && exclusion.get("recordId").and_then(Value::as_str) == Some(gate_b.as_str())
            && exclusion.pointer("/reason/reason").and_then(Value::as_str)
                == Some("active-transition-reference")
    });
    let referenced_b = referenced_b_exclusions
        .next()
        .ok_or_else(|| std::io::Error::other("terminal gate omitted transition exclusion"))?;
    assert!(referenced_b_exclusions.next().is_none());
    let protecting_gate_ids = referenced_b
        .pointer("/reason/gateIds")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("transition exclusion omitted gateIds"))?;
    assert_eq!(protecting_gate_ids.len(), 1);
    assert_eq!(protecting_gate_ids[0].as_str(), Some(gate_a.as_str()));

    fs::write(root.path().join("src/a.ts"), "console.log('a2');\n")?;
    let close_a = run(
        root.path(),
        &["post-write", &gate_a, "--operation-id", "op-a-close"],
    )?;
    assert_status(&close_a, 0);
    assert_eq!(field(&close_a.stdout, "decision")?, "allow");

    let shown = run(root.path(), &["gate", "show", &gate_a])?;
    assert_status(&shown, 0);
    let shown_json: Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(
        shown_json
            .pointer("/revisions/2/reconciledTransitionSequences/0")
            .and_then(Value::as_u64),
        Some(terminal_transition_sequence)
    );
    assert_eq!(
        shown_json
            .get("transitionRefs")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );

    let released_plan = prepare_and_show_gate_plan(root.path(), "op-transition-plan-released")?;
    assert_eq!(
        released_plan
            .get("exclusions")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );
    let items = released_plan
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("released gate plan omitted items"))?;
    let contains_item = |kind: &str, record_id: &str| {
        items.iter().any(|item| {
            item.get("kind").and_then(Value::as_str) == Some(kind)
                && item.get("recordId").and_then(Value::as_str) == Some(record_id)
        })
    };
    assert!(contains_item("gate", &gate_b));
    assert!(contains_item(
        "gate-revision",
        &format!("gate:{gate_b}/revision:0")
    ));
    assert!(contains_item(
        "gate-revision",
        &format!("gate:{gate_b}/revision:1")
    ));
    assert!(contains_item(
        "evidence",
        &format!("gate:{gate_b}/baseline/evidence")
    ));
    assert!(contains_item(
        "evidence",
        &format!("gate:{gate_b}/revision:1/evidence")
    ));
    assert!(contains_item("operation", "op-b-open"));
    assert!(contains_item("operation", "op-b-close"));
    assert!(contains_item(
        "transition",
        &format!("transition_{terminal_transition_sequence:016x}")
    ));
    Ok(())
}

#[test]
fn disjoint_gates_reconcile_transitions_across_different_scan_scopes()
-> Result<(), Box<dyn std::error::Error>> {
    let root = disjoint_config_fixture()?;
    let gate_a = open_scoped_gate(
        root.path(),
        "op-scope-a-open",
        "packages/a/src/a.ts",
        "packages/a/**",
    )?;
    fs::write(
        root.path().join("packages/b/src/b.ts"),
        "console.log('b scoped');\n",
    )?;
    let opened_b = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-scope-b-open",
            "--path",
            "packages/b",
            "--include",
            "packages/b/**",
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened_b, 0);
    let gate_b = field(&opened_b.stdout, "gateId")?;

    fs::write(
        root.path().join("packages/b/tsconfig.json"),
        r#"{"compilerOptions":{"moduleResolution":"bundler","module":"esnext"}}"#,
    )?;
    let close_b = run(
        root.path(),
        &["post-write", &gate_b, "--operation-id", "op-scope-b-close"],
    )?;
    assert_status(&close_b, 0);
    assert_eq!(field(&close_b.stdout, "decision")?, "allow");
    let close_b: Value = serde_json::from_str(&close_b.stdout)?;
    assert_eq!(
        display_paths(&close_b, "/actualWriteSet/paths")?,
        ["packages/b/tsconfig.json"]
    );
    let close_b_operation = run(root.path(), &["operation", "show", "op-scope-b-close"])?;
    assert_status(&close_b_operation, 0);
    let close_b_operation: Value = serde_json::from_str(&close_b_operation.stdout)?;
    assert_eq!(
        close_b_operation.get("gateId").and_then(Value::as_str),
        Some(gate_b.as_str())
    );
    assert_eq!(
        close_b_operation
            .pointer("/result/gateId")
            .and_then(Value::as_str),
        Some(gate_b.as_str())
    );
    assert_eq!(close_b_operation.get("result"), Some(&close_b));
    let expected_terminal_transition_sequence = close_b_operation
        .get("transitionSequence")
        .and_then(Value::as_u64)
        .ok_or_else(|| std::io::Error::other("B close omitted its transition ceiling"))?
        .checked_add(1)
        .ok_or_else(|| std::io::Error::other("B transition sequence overflowed"))?;

    let pending_a = run(root.path(), &["gate", "show", &gate_a])?;
    assert_status(&pending_a, 0);
    let pending_a: Value = serde_json::from_str(&pending_a.stdout)?;
    let transition_refs = pending_a
        .get("transitionRefs")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("active gate omitted transitionRefs"))?;
    assert_eq!(transition_refs.len(), 1);
    let terminal_transition_sequence = transition_refs[0]
        .as_u64()
        .ok_or_else(|| std::io::Error::other("transition reference was not a sequence"))?;
    assert_eq!(
        terminal_transition_sequence,
        expected_terminal_transition_sequence
    );

    let protected_plan =
        prepare_and_show_gate_plan(root.path(), "op-scope-transition-plan-protected")?;
    let exclusions = protected_plan
        .get("exclusions")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("protected plan omitted exclusions"))?;
    let referenced_b = exclusions
        .iter()
        .filter(|exclusion| {
            exclusion.get("kind").and_then(Value::as_str) == Some("gate")
                && exclusion.get("recordId").and_then(Value::as_str) == Some(gate_b.as_str())
                && exclusion.pointer("/reason/reason").and_then(Value::as_str)
                    == Some("active-transition-reference")
        })
        .collect::<Vec<_>>();
    assert_eq!(referenced_b.len(), 1);
    let protecting_gate_ids = referenced_b[0]
        .pointer("/reason/gateIds")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("B exclusion omitted protecting gate IDs"))?;
    assert_eq!(protecting_gate_ids.len(), 1);
    assert_eq!(protecting_gate_ids[0].as_str(), Some(gate_a.as_str()));

    fs::write(
        root.path().join("packages/a/src/a.ts"),
        "console.log('a scoped');\n",
    )?;
    let close_a = run(
        root.path(),
        &["post-write", &gate_a, "--operation-id", "op-scope-a-close"],
    )?;
    assert_status(&close_a, 0);
    assert_eq!(field(&close_a.stdout, "decision")?, "allow");
    let close_a: Value = serde_json::from_str(&close_a.stdout)?;
    assert_eq!(
        display_paths(&close_a, "/actualWriteSet/paths")?,
        ["packages/a/src/a.ts"]
    );
    assert_eq!(
        close_a
            .get("signals")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );

    let shown = run(root.path(), &["gate", "show", &gate_a])?;
    assert_status(&shown, 0);
    let shown: Value = serde_json::from_str(&shown.stdout)?;
    let reconciled_transitions = shown
        .pointer("/revisions/1/reconciledTransitionSequences")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("close omitted reconciled transitions"))?;
    assert_eq!(
        reconciled_transitions.as_slice(),
        [Value::from(terminal_transition_sequence)]
    );
    assert_eq!(
        shown
            .get("transitionRefs")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );
    assert_eq!(
        display_paths(&shown, "/revisions/1/changedPaths")?,
        ["packages/a/src/a.ts"]
    );
    assert_eq!(
        display_paths(&shown, "/revisions/1/actualWriteSet/paths")?,
        ["packages/a/src/a.ts"]
    );
    assert!(
        shown
            .pointer("/revisions/1/analysisInputId")
            .and_then(Value::as_str)
            .is_some()
    );
    assert_eq!(
        shown
            .pointer("/revisions/1/observationBinding/state")
            .and_then(Value::as_str),
        Some("sealed")
    );
    assert_eq!(
        shown
            .pointer("/revisions/1/observationBinding/observation/kind")
            .and_then(Value::as_str),
        Some("close")
    );
    Ok(())
}

fn disjoint_config_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("packages/a/src"))?;
    fs::create_dir_all(root.path().join("packages/b/src"))?;
    fs::write(
        root.path().join("packages/a/src/a.ts"),
        "console.log('a');\n",
    )?;
    Ok(root)
}

fn open_scoped_gate(
    root: &Path,
    operation_id: &str,
    path: &str,
    include: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let opened = run(
        root,
        &[
            "pre-write",
            "--operation-id",
            operation_id,
            "--path",
            path,
            "--include",
            include,
            "--jobs",
            "1",
        ],
    )?;
    assert_status(&opened, 0);
    field(&opened.stdout, "gateId")
}

fn prepare_and_show_gate_plan(
    root: &Path,
    operation_id: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let prepared = run(
        root,
        &[
            "gate",
            "prune",
            "plan",
            "--terminal-before",
            "9000000000000",
            "--operation-id",
            operation_id,
        ],
    )?;
    assert_status(&prepared, 0);
    let prepared: Value = serde_json::from_str(&prepared.stdout)?;
    assert_eq!(
        prepared.get("schemaVersion").and_then(Value::as_str),
        Some("lumin.retention-mutation.v1")
    );
    let plan_id = prepared
        .pointer("/result/planId")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("gate plan response omitted planId"))?;
    let content_identity = prepared
        .pointer("/result/contentIdentity")
        .and_then(Value::as_str)
        .ok_or_else(|| std::io::Error::other("gate plan response omitted contentIdentity"))?;
    let shown = run(root, &["gate", "prune", "plan", "show", plan_id])?;
    assert_status(&shown, 0);
    let shown: Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(
        shown.get("schemaVersion").and_then(Value::as_str),
        Some("lumin.retention-plan.v1")
    );
    assert_eq!(shown.get("planId").and_then(Value::as_str), Some(plan_id));
    assert_eq!(
        shown.get("contentIdentity").and_then(Value::as_str),
        Some(content_identity)
    );
    assert_eq!(
        shown.pointer("/scope/kind").and_then(Value::as_str),
        Some("gates")
    );
    assert_eq!(
        shown
            .pointer("/scope/terminalBeforeUnixMillis")
            .and_then(Value::as_u64),
        Some(9_000_000_000_000)
    );
    assert_eq!(shown.get("state").and_then(Value::as_str), Some("prepared"));
    assert_eq!(
        shown.get("ordering").and_then(Value::as_str),
        Some("retention-plan-items.v1")
    );
    assert_eq!(shown.get("truncated").and_then(Value::as_bool), Some(false));
    assert_eq!(
        shown.get("returned").and_then(Value::as_u64),
        shown.get("total").and_then(Value::as_u64)
    );
    Ok(shown)
}
