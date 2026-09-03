use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use lumin_model::{PhysicalFileIdentity, RepoPath};
use serde_json::Value;

mod support;

use support::{ProcessResult, assert_status, field, run};

#[derive(Debug, Eq, PartialEq)]
struct NamespaceSnapshot {
    lifecycle_store_identity: PhysicalFileIdentity,
    lifecycle_store_size: u64,
    lifecycle_store_logical_bytes: Vec<u8>,
    other_entries: BTreeMap<String, Option<Vec<u8>>>,
}

#[test]
fn corrupt_canonical_evidence_hard_stops_without_fallback_or_empty_evidence()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(
        root.path().join("src/lib.ts"),
        "export const used = 1; export const firstDead = 2;\n",
    )?;
    fs::write(
        root.path().join("src/main.ts"),
        "import { used } from './lib.js'; console.log(used);\n",
    )?;

    let first = audit(root.path())?;
    assert_run_truth(root.path(), &first, 1)?;

    fs::write(
        root.path().join("src/lib.ts"),
        concat!(
            "export const used = 1;\n",
            "export const firstDead = 2;\n",
            "export const secondDead = 3;\n",
        ),
    )?;
    let latest = audit(root.path())?;
    assert_ne!(first, latest);
    assert_run_truth(root.path(), &latest, 2)?;

    let latest_overview = run(root.path(), &["overview"])?;
    assert_status(&latest_overview, 0);
    let latest_json: Value = serde_json::from_str(&latest_overview.stdout)?;
    assert_eq!(
        latest_json.pointer("/scope/id").and_then(Value::as_str),
        Some(latest.as_str())
    );
    assert_eq!(
        latest_json.get("findingCount").and_then(Value::as_u64),
        Some(2)
    );

    let evidence_path = root
        .path()
        .join(".lumin/runs")
        .join(&latest)
        .join("evidence.store");
    let mut corrupt_bytes = fs::read(&evidence_path)?;
    let original_length = corrupt_bytes.len();
    let changed_index = original_length
        .checked_div(2)
        .filter(|_| original_length > 0)
        .ok_or_else(|| std::io::Error::other("published evidence store is empty"))?;
    corrupt_bytes[changed_index] ^= 1;
    fs::write(&evidence_path, &corrupt_bytes)?;
    assert_eq!(
        fs::metadata(&evidence_path)?.len(),
        u64::try_from(original_length)?
    );

    let corrupt_snapshot = namespace_snapshot(root.path())?;
    let diagnostic = format!(
        "lumin: state namespace integrity failure: evidence store identity mismatch for {latest}\n"
    );
    for arguments in [
        vec!["overview"],
        vec!["overview", "--run", latest.as_str()],
        vec!["findings", "--run", latest.as_str(), "--area", "dead-code"],
    ] {
        let rejected = run(root.path(), &arguments)?;
        assert_integrity_hard_stop(&rejected, &diagnostic);
        assert_namespace_unchanged(
            root.path(),
            &corrupt_snapshot,
            &format!("rejected `{}`", arguments.join(" ")),
        )?;
    }

    // The older immutable run remains readable only when explicitly selected;
    // the unqualified overview above must not reinterpret it as latest.
    assert_run_truth(root.path(), &first, 1)?;
    assert_namespace_unchanged(root.path(), &corrupt_snapshot, "older-run queries")?;
    Ok(())
}

fn audit(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let result = run(root, &["audit", "--jobs", "1"])?;
    assert_status(&result, 0);
    let body: Value = serde_json::from_str(&result.stdout)?;
    assert_eq!(body.get("status").and_then(Value::as_str), Some("complete"));
    field(&result.stdout, "runId")
}

fn assert_run_truth(
    root: &Path,
    run_id: &str,
    expected_findings: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let overview = run(root, &["overview", "--run", run_id])?;
    assert_status(&overview, 0);
    let overview: Value = serde_json::from_str(&overview.stdout)?;
    assert_eq!(
        overview.pointer("/scope/id").and_then(Value::as_str),
        Some(run_id)
    );
    assert_eq!(
        overview.get("findingCount").and_then(Value::as_u64),
        Some(expected_findings)
    );

    let findings = run(root, &["findings", "--run", run_id, "--area", "dead-code"])?;
    assert_status(&findings, 0);
    let findings: Value = serde_json::from_str(&findings.stdout)?;
    assert_eq!(
        findings.get("total").and_then(Value::as_u64),
        Some(expected_findings)
    );
    assert_eq!(
        findings
            .get("items")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(expected_findings as usize)
    );
    Ok(())
}

fn assert_integrity_hard_stop(result: &ProcessResult, expected_diagnostic: &str) {
    assert_status(result, 1);
    assert!(result.stdout.is_empty());
    assert_eq!(result.stderr, expected_diagnostic);
}

fn namespace_snapshot(root: &Path) -> Result<NamespaceSnapshot, Box<dyn std::error::Error>> {
    let state = root.join(".lumin");
    let lifecycle_store = state.join("lifecycle.store");
    let lifecycle_store_path = RepoPath::from_portable(".lumin/lifecycle.store")?;
    let mut other_entries = BTreeMap::new();
    snapshot_directory(&state, &state, &mut other_entries)?;
    Ok(NamespaceSnapshot {
        lifecycle_store_identity: lumin_engine::path_physical_identity_for_test(
            root,
            &lifecycle_store_path,
        )?,
        lifecycle_store_size: fs::metadata(lifecycle_store)?.len(),
        lifecycle_store_logical_bytes: lumin_engine::current_logical_store_snapshot_for_test(root)?,
        other_entries,
    })
}

fn assert_namespace_unchanged(
    root: &Path,
    expected: &NamespaceSnapshot,
    context: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let observed = namespace_snapshot(root)?;
    assert_eq!(
        observed.lifecycle_store_identity, expected.lifecycle_store_identity,
        "lifecycle.store physical identity changed"
    );
    assert_eq!(
        observed.lifecycle_store_size, expected.lifecycle_store_size,
        "lifecycle.store size changed after {context}"
    );
    if observed.lifecycle_store_logical_bytes != expected.lifecycle_store_logical_bytes {
        return Err(std::io::Error::other(format!(
            "lifecycle.store logical bytes changed after {context}: expected {} bytes, observed {} bytes",
            expected.lifecycle_store_logical_bytes.len(),
            observed.lifecycle_store_logical_bytes.len()
        ))
        .into());
    }
    if observed.other_entries != expected.other_entries {
        let names = expected
            .other_entries
            .keys()
            .chain(observed.other_entries.keys())
            .collect::<BTreeSet<_>>();
        let differences = names
            .into_iter()
            .filter(|name| expected.other_entries.get(*name) != observed.other_entries.get(*name))
            .map(|name| {
                format!(
                    "{name}: expected {:?}, observed {:?}",
                    expected
                        .other_entries
                        .get(name)
                        .map(|bytes| bytes.as_ref().map(Vec::len)),
                    observed
                        .other_entries
                        .get(name)
                        .map(|bytes| bytes.as_ref().map(Vec::len))
                )
            })
            .collect::<Vec<_>>();
        return Err(std::io::Error::other(format!(
            "state namespace changed after {context}:\n{}",
            differences.join("\n")
        ))
        .into());
    }
    Ok(())
}

fn snapshot_directory(
    state: &Path,
    directory: &Path,
    snapshot: &mut BTreeMap<String, Option<Vec<u8>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let relative = path.strip_prefix(state)?;
        let name = relative
            .components()
            .map(|component| {
                component
                    .as_os_str()
                    .to_str()
                    .ok_or_else(|| std::io::Error::other("state entry name is not UTF-8"))
            })
            .collect::<Result<Vec<_>, _>>()?
            .join("/");
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            snapshot.insert(format!("{name}/"), None);
            snapshot_directory(state, &path, snapshot)?;
        } else if file_type.is_file() {
            if name != "lifecycle.store" {
                snapshot.insert(name, Some(fs::read(path)?));
            }
        } else {
            return Err(std::io::Error::other(format!(
                "state snapshot encountered unsupported entry {name}"
            ))
            .into());
        }
    }
    Ok(())
}
