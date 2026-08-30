use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use lumin_model::{CapabilityIntentKind, GateId, Limitation, LogicalSourceId, RepoPath};
use serde_json::Value;

mod support;

use support::{assert_status, field, run};

#[test]
fn capability_unavailability_has_one_owner() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(root.path(), "src/main.ts", "console.log('supported');\n")?;
    write(root.path(), "src/lib.rs", "pub fn unavailable() {}\n")?;

    let arguments = [
        "pre-write",
        "--operation-id",
        "op-capability-unavailable",
        "--path",
        "src/main.ts",
        "--path",
        "src/lib.rs",
        "--capability-at",
        "src/main.ts",
        "shape",
        "--capability-at",
        "src/main.ts",
        "clone",
        "--capability-at",
        "src/main.ts",
        "type-escape",
        "--jobs",
        "1",
    ];
    let opened = run(root.path(), &arguments)?;
    assert_status(&opened, 4);
    assert!(opened.stderr.is_empty());
    let opened_json: Value = serde_json::from_str(&opened.stdout)?;
    assert_eq!(
        opened_json.get("decision").and_then(Value::as_str),
        Some("incomplete")
    );
    assert_eq!(
        opened_json.get("lifecycle").and_then(Value::as_str),
        Some("rejected")
    );
    let signals = opened_json
        .get("signals")
        .and_then(Value::as_array)
        .ok_or_else(|| std::io::Error::other("capability gate omitted signals"))?;
    assert_eq!(signals.len(), 1, "unexpected signals: {signals:?}");
    assert_eq!(
        signals[0].get("kind").and_then(Value::as_str),
        Some("required-owner-unavailable")
    );
    assert_eq!(signals[0].get("count").and_then(Value::as_u64), Some(4));

    let replay = run(root.path(), &arguments)?;
    assert_status(&replay, 4);
    assert_eq!(replay.stdout, opened.stdout);

    let gate_id = GateId::from_string(field(&opened.stdout, "gateId")?);
    let shown = run(root.path(), &["gate", "show", gate_id.as_str()])?;
    assert_status(&shown, 0);
    let shown_json: Value = serde_json::from_str(&shown.stdout)?;
    assert_eq!(
        shown_json
            .pointer("/baseline/limitationCount")
            .and_then(Value::as_u64),
        Some(4)
    );
    assert_eq!(
        shown_json
            .pointer("/revisions/0/signals/0/kind")
            .and_then(Value::as_str),
        Some("required-owner-unavailable")
    );
    let gate = lumin_engine::load_gate(root.path(), &gate_id)?;
    let baseline = gate
        .baseline
        .as_ref()
        .ok_or_else(|| std::io::Error::other("capability gate omitted its baseline"))?;
    assert_eq!(baseline.snapshot.evidence.capabilities.len(), 5);
    assert_eq!(
        baseline
            .snapshot
            .evidence
            .source_contexts
            .iter()
            .map(|context| context.path.display.as_str())
            .collect::<Vec<_>>(),
        vec!["src/main.ts"]
    );
    assert!(baseline.snapshot.evidence.findings.is_empty());
    assert_eq!(
        baseline.snapshot.scan_invocation.capability_intents.len(),
        4
    );
    assert_eq!(gate.revisions[0].signals.len(), 1);

    let mut availability = BTreeMap::new();
    for limitation in &baseline.snapshot.evidence.limitations {
        let Limitation::CapabilityUnavailable {
            capability,
            targets,
        } = limitation
        else {
            return Err(format!(
                "non-registry limitation entered the availability gate: {limitation:?}"
            )
            .into());
        };
        assert!(availability.insert(*capability, targets.clone()).is_none());
    }
    let main = LogicalSourceId::from_path(&RepoPath::from_portable("src/main.ts")?);
    let rust = LogicalSourceId::from_path(&RepoPath::from_portable("src/lib.rs")?);
    assert_eq!(
        availability,
        BTreeMap::from([
            (CapabilityIntentKind::Rust, vec![rust]),
            (CapabilityIntentKind::Shape, vec![main.clone()]),
            (CapabilityIntentKind::Clone, vec![main.clone()]),
            (CapabilityIntentKind::Discipline, vec![main]),
        ])
    );

    assert_eq!(
        public_capability_states(root.path())?,
        expected_public_capabilities()
    );
    Ok(())
}

#[test]
fn capability_intent_syntax_is_closed_before_state_initialization()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write(root.path(), "src/main.ts", "console.log('supported');\n")?;
    let rejected = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-invalid-capability",
            "--path",
            "src/main.ts",
            "--capability-at",
            "src/main.ts",
            "rust",
        ],
    )?;
    assert_status(&rejected, 2);
    assert!(rejected.stdout.is_empty());
    assert_eq!(rejected.stderr, "lumin: unknown capability intent: rust\n");
    assert!(!root.path().join(".lumin").exists());

    let outside = run(
        root.path(),
        &[
            "pre-write",
            "--operation-id",
            "op-outside-capability",
            "--path",
            "src/main.ts",
            "--capability-at",
            "src/other.ts",
            "shape",
        ],
    )?;
    assert_status(&outside, 2);
    assert!(outside.stdout.is_empty());
    assert_eq!(
        outside.stderr,
        "lumin: capability intent is outside the declared write set: src/other.ts\n"
    );
    assert!(!root.path().join(".lumin").exists());
    Ok(())
}

fn public_capability_states(
    root: &Path,
) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let mut states = BTreeMap::new();
    let mut cursor = None;
    loop {
        let mut arguments = vec!["capabilities"];
        if let Some(value) = cursor.as_deref() {
            arguments.extend(["--cursor", value]);
        }
        let page = run(root, &arguments)?;
        assert_status(&page, 0);
        let response: Value = serde_json::from_str(&page.stdout)?;
        for row in response
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| std::io::Error::other("capabilities page omitted items"))?
        {
            let capability_id = row
                .get("capabilityId")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("capability row omitted its ID"))?;
            let state = row
                .get("state")
                .and_then(Value::as_str)
                .ok_or_else(|| std::io::Error::other("capability row omitted its state"))?;
            assert!(
                states
                    .insert(capability_id.to_owned(), state.to_owned())
                    .is_none(),
                "duplicate public capability {capability_id}"
            );
        }
        cursor = response
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    Ok(states)
}

fn expected_public_capabilities() -> BTreeMap<String, String> {
    [
        ("dead-code.v1", "complete"),
        ("inventory/dependency-ownership.v1", "complete"),
        ("sfc/astro.v1", "unavailable"),
        ("sfc/svelte.v1", "unavailable"),
        ("sfc/vue.v1", "complete"),
    ]
    .into_iter()
    .map(|(capability, state)| (capability.to_owned(), state.to_owned()))
    .collect()
}

fn write(root: &Path, path: &str, contents: &str) -> std::io::Result<()> {
    let path = root.join(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}
