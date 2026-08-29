use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

#[allow(dead_code)]
mod support;

use support::{assert_status, run, run_with_env};

const CRASH_POINT_ENV: &str = "LUMIN_TEST_NAMESPACE_BOOTSTRAP_CRASH_POINT";
const FORCE_NAMED_UNPUBLISHED_ENV: &str = "LUMIN_TEST_NAMESPACE_FORCE_NAMED_UNPUBLISHED";
const CRASH_EXIT_CODE: i32 = 97;
const INVALID_SELECTOR_EXIT_CODE: i32 = 98;
const NAMED_FALLBACK_RECOVERY_POINTS: &[&str] = &[
    "after-marker-candidate-created",
    "after-marker-candidate-flushed",
];

const RECOVERABLE_CRASH_POINTS: &[&str] = &[
    "before-state-directory",
    "after-lifecycle-lock-header-flushed",
    "after-attempts-anchor-flushed",
    "after-attempts-parent-flushed",
    "after-runs-anchor-flushed",
    "after-runs-parent-flushed",
    "after-trash-anchor-flushed",
    "after-trash-parent-flushed",
    "after-cache-anchor-flushed",
    "after-cache-parent-flushed",
    "after-cache-evictions-anchor-flushed",
    "after-cache-evictions-parent-flushed",
    "after-trash-parent-flushed-for-cache-evictions",
    "after-all-parents-flushed",
    "before-marker-candidate",
    "after-marker-candidate-created",
    "after-marker-candidate-flushed",
    "after-marker-published",
    "after-marker-parent-flushed",
    "before-store-creation",
    "after-store-created",
    "after-store-initialized",
    "after-store-parent-flushed",
    "after-complete-validation",
];

const FOREIGN_CRASH_POINTS: &[&str] = &[
    "after-state-directory-created",
    "after-state-directory-flushed",
    "after-lifecycle-lock-created",
    "after-lifecycle-lock-acquired",
    "after-global-binding-allocated",
    "after-attempts-directory-created",
    "after-attempts-anchor-created",
    "after-attempts-binding-allocated",
    "after-runs-directory-created",
    "after-runs-anchor-created",
    "after-runs-binding-allocated",
    "after-trash-directory-created",
    "after-trash-anchor-created",
    "after-trash-binding-allocated",
    "after-cache-directory-created",
    "after-cache-anchor-created",
    "after-cache-binding-allocated",
    "after-cache-evictions-directory-created",
    "after-cache-evictions-anchor-created",
    "after-cache-evictions-binding-allocated",
];

#[test]
fn public_namespace_initialization_recovers_or_rejects_every_named_crash_boundary()
-> Result<(), Box<dyn std::error::Error>> {
    let all_points = RECOVERABLE_CRASH_POINTS
        .iter()
        .chain(FOREIGN_CRASH_POINTS)
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(RECOVERABLE_CRASH_POINTS.len(), 24);
    assert_eq!(FOREIGN_CRASH_POINTS.len(), 20);
    assert_eq!(all_points.len(), 44);

    for point in RECOVERABLE_CRASH_POINTS {
        let root = fixture()?;
        let mut environment = vec![(CRASH_POINT_ENV, *point)];
        if NAMED_FALLBACK_RECOVERY_POINTS.contains(point) {
            environment.push((FORCE_NAMED_UNPUBLISHED_ENV, "1"));
        }
        let crashed = run_with_env(root.path(), &["audit", "--jobs", "1"], &environment)?;
        assert_status(&crashed, CRASH_EXIT_CODE);
        assert!(crashed.stdout.is_empty(), "stdout at {point}");
        assert!(crashed.stderr.is_empty(), "stderr at {point}");

        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        if NAMED_FALLBACK_RECOVERY_POINTS.contains(point) {
            assert_named_marker_candidate(root.path(), point)?;
        }

        let recovered = run(root.path(), &["audit", "--jobs", "1"])?;
        assert_status(&recovered, 0);
        assert!(recovered.stderr.is_empty(), "recovery stderr at {point}");
        assert_complete_namespace(root.path())?;
        assert_status(&run(root.path(), &["overview"])?, 0);
    }

    for point in FOREIGN_CRASH_POINTS {
        let root = fixture()?;
        let crashed = run_with_env(
            root.path(),
            &["audit", "--jobs", "1"],
            &[(CRASH_POINT_ENV, point)],
        )?;
        assert_status(&crashed, CRASH_EXIT_CODE);
        assert!(crashed.stdout.is_empty(), "stdout at {point}");
        assert!(crashed.stderr.is_empty(), "stderr at {point}");

        let state = root.path().join(".lumin");
        let before = tree_snapshot(&state)?;
        let rejected = run(root.path(), &["audit", "--jobs", "1"])?;
        assert_status(&rejected, 1);
        assert!(rejected.stdout.is_empty(), "foreign stdout at {point}");
        assert!(
            rejected
                .stderr
                .contains("state namespace integrity failure"),
            "unexpected foreign diagnostic at {point}: {}",
            rejected.stderr,
        );
        assert_eq!(
            tree_snapshot(&state)?,
            before,
            "foreign state changed at {point}"
        );
    }
    Ok(())
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn assert_named_marker_candidate(
    root: &Path,
    crash_point: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let state = root.join(".lumin");
    let lock = read_json(&state.join("lifecycle.lock"))?;
    let nonce = lock
        .pointer("/global/namespaceNonce")
        .and_then(Value::as_str)
        .ok_or("bootstrap lock omitted its namespace nonce")?;
    let candidate = state.join(format!(".lumin-unpublished-repository-{nonce}"));
    let bytes = fs::read(candidate)?;
    if crash_point == "after-marker-candidate-created" {
        assert!(bytes.is_empty());
    } else {
        let marker: Value = serde_json::from_slice(&bytes)?;
        assert_eq!(
            marker.get("schemaVersion").and_then(Value::as_str),
            Some("lumin-repository.v4")
        );
        assert_eq!(marker.pointer("/binding/global"), lock.get("global"));
    }
    Ok(())
}

#[test]
fn unknown_namespace_crash_selector_fails_before_state_initialization()
-> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let rejected = run_with_env(
        root.path(),
        &["audit", "--jobs", "1"],
        &[(CRASH_POINT_ENV, "not-a-bootstrap-turn")],
    )?;
    assert_status(&rejected, INVALID_SELECTOR_EXIT_CODE);
    assert!(rejected.stdout.is_empty());
    assert_eq!(
        rejected.stderr,
        "unknown namespace bootstrap test crash point: not-a-bootstrap-turn\n"
    );
    assert!(!root.path().join(".lumin").exists());
    Ok(())
}

#[test]
fn missing_nested_marker_or_store_binding_is_incompatible_without_adoption()
-> Result<(), Box<dyn std::error::Error>> {
    let named_root = fixture()?;
    let named = run_with_env(
        named_root.path(),
        &["audit", "--jobs", "1"],
        &[(FORCE_NAMED_UNPUBLISHED_ENV, "1")],
    )?;
    assert_status(&named, 0);
    assert!(named.stderr.is_empty());
    assert_complete_namespace(named_root.path())?;

    let marker_root = initialized_fixture()?;
    let marker_path = marker_root.path().join(".lumin/repository.json");
    let original_marker = fs::read(&marker_path)?;
    let without_nested =
        remove_last_object_member(std::str::from_utf8(&original_marker)?, "cacheEvictions")?;
    fs::write(&marker_path, without_nested)?;
    let marker_state = tree_snapshot(&marker_root.path().join(".lumin"))?;

    let rejected = run(marker_root.path(), &["overview"])?;
    assert_status(&rejected, 1);
    assert!(rejected.stdout.is_empty());
    assert_eq!(
        rejected.stderr,
        "lumin: incompatible state schema: repository marker omitted the cache-eviction parent binding\n"
    );
    assert_eq!(
        tree_snapshot(&marker_root.path().join(".lumin"))?,
        marker_state,
        "marker rejection mutated or adopted nested state",
    );
    fs::write(&marker_path, original_marker)?;
    assert_status(&run(marker_root.path(), &["overview"])?, 0);

    let store_root = initialized_fixture()?;
    let fixture_command = run(
        store_root.path(),
        &["store", "test-remove-store-cache-eviction-binding"],
    )?;
    assert_status(&fixture_command, 0);
    assert!(fixture_command.stdout.is_empty());
    assert!(fixture_command.stderr.is_empty());
    let store_state = tree_snapshot(&store_root.path().join(".lumin"))?;

    let rejected = run(store_root.path(), &["overview"])?;
    assert_status(&rejected, 1);
    assert!(rejected.stdout.is_empty());
    assert_eq!(
        rejected.stderr,
        "lumin: incompatible state schema: lifecycle.store omitted the cache-eviction parent binding\n"
    );
    assert_eq!(
        tree_snapshot(&store_root.path().join(".lumin"))?,
        store_state,
        "store rejection mutated or adopted nested state",
    );
    Ok(())
}

fn fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(
        root.path().join("src/lib.ts"),
        "export const initializationFixture = 1;\n",
    )?;
    Ok(root)
}

fn initialized_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = fixture()?;
    assert_status(&run(root.path(), &["audit", "--jobs", "1"])?, 0);
    Ok(root)
}

fn assert_complete_namespace(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let state = root.join(".lumin");
    let marker = read_json(&state.join("repository.json"))?;
    let binding = marker
        .get("binding")
        .ok_or("recovered repository binding is missing")?;
    let global = binding
        .get("global")
        .ok_or("recovered global binding is missing")?;
    let lock = read_json(&state.join("lifecycle.lock"))?;
    assert_eq!(lock.as_object().map(serde_json::Map::len), Some(2));
    assert_eq!(lock.get("global"), Some(global));
    assert!(lock.get("managedParents").is_none());
    assert!(lock.get("cacheEvictions").is_none());

    let managed = binding
        .get("managedParents")
        .and_then(Value::as_array)
        .ok_or("recovered managed-parent set is missing")?;
    assert_eq!(managed.len(), 4);
    for (index, name) in ["attempts", "runs", "trash", "cache"]
        .into_iter()
        .enumerate()
    {
        let anchor = read_json(&state.join(name).join("namespace.anchor"))?;
        assert_eq!(anchor.get("global"), Some(global));
        assert_eq!(anchor.get("binding"), managed.get(index));
    }
    let nested_binding = binding
        .get("cacheEvictions")
        .ok_or("recovered cache-eviction binding is missing")?;
    let nested = read_json(&state.join("trash/cache-evictions/namespace.anchor"))?;
    assert_eq!(nested.get("global"), Some(global));
    assert_eq!(nested.get("trashBinding"), managed.get(2));
    assert_eq!(nested.get("binding"), Some(nested_binding));
    assert!(state.join("lifecycle.store").is_file());
    assert!(fs::read_dir(&state)?.all(|entry| entry.is_ok_and(|entry| {
        let name = entry.file_name();
        !name.to_string_lossy().starts_with(".lumin-unpublished-")
    })));
    Ok(())
}

fn read_json(path: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn remove_last_object_member(
    document: &str,
    member: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let needle = format!("\"{member}\": {{");
    let key = document.find(&needle).ok_or("JSON member is missing")?;
    let value = key + needle.len() - 1;
    let mut depth = 0_u64;
    let mut string = false;
    let mut escaped = false;
    let mut end = None;
    for (offset, character) in document[value..].char_indices() {
        if string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                string = false;
            }
            continue;
        }
        match character {
            '"' => string = true,
            '{' => depth = depth.checked_add(1).ok_or("JSON depth overflow")?,
            '}' if depth == 1 => {
                end = Some(value + offset + character.len_utf8());
                break;
            }
            '}' => depth = depth.checked_sub(1).ok_or("invalid JSON object depth")?,
            _ => {}
        }
    }
    let end = end.ok_or("JSON member object is incomplete")?;
    let comma = document[..key]
        .rfind(',')
        .ok_or("last JSON member has no preceding comma")?;
    let mut result = String::with_capacity(document.len() - (end - comma));
    result.push_str(&document[..comma]);
    result.push_str(&document[end..]);
    let parsed = serde_json::from_str::<Value>(&result)?;
    if parsed.pointer("/binding/cacheEvictions").is_some() {
        return Err("cache-eviction binding survived fixture rewrite".into());
    }
    Ok(result.into_bytes())
}

fn tree_snapshot(root: &Path) -> Result<Vec<(PathBuf, String, Vec<u8>)>, std::io::Error> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut pending = vec![root.to_owned()];
    let mut snapshot = vec![(PathBuf::from("."), physical_identity(root)?, Vec::new())];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            let relative = path
                .strip_prefix(root)
                .map_err(std::io::Error::other)?
                .to_owned();
            let identity = physical_identity(&path)?;
            if metadata.file_type().is_dir() {
                snapshot.push((relative.clone(), identity, Vec::new()));
                pending.push(path);
            } else {
                snapshot.push((relative, identity, fs::read(path)?));
            }
        }
    }
    snapshot.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(snapshot)
}

fn physical_identity(path: &Path) -> Result<String, std::io::Error> {
    lumin_engine::state_entry_physical_identity_for_test(path)
        .map(|identity| format!("{identity:?}"))
        .map_err(std::io::Error::other)
}
