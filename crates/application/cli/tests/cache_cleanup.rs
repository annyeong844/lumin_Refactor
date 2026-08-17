use std::fs;

use serde_json::json;

mod support;

use support::{assert_status, field, run};

#[test]
fn public_cache_cleanup_removes_payloads_and_preserves_the_namespace_binding()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(
        root.path().join("src/lib.ts"),
        "export const visible = 1;\n",
    )?;

    let initialized = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&initialized, 0);
    let run_id = field(&initialized.stdout, "runId")?;
    let state = root.path().join(".lumin");
    let cache = state.join("cache");
    let anchor = cache.join("namespace.anchor");
    let anchor_bytes = fs::read(&anchor)?;
    fs::create_dir_all(cache.join("nested/deep"))?;
    fs::write(cache.join("nested/deep/payload.bin"), b"nested")?;
    fs::write(cache.join("direct.bin"), b"direct")?;

    let cleaned = run(root.path(), &["cache", "clean"])?;
    assert_status(&cleaned, 0);
    assert!(cleaned.stderr.is_empty());
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&cleaned.stdout)?,
        json!({"schemaVersion":"lumin.cache-cleanup.v1","status":"clean"})
    );
    assert!(cache.is_dir());
    assert_eq!(
        fs::read_dir(&cache)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<Result<Vec<_>, _>>()?,
        [std::ffi::OsString::from("namespace.anchor")]
    );
    assert_eq!(fs::read(&anchor)?, anchor_bytes);

    let retained = run(root.path(), &["overview", "--run", &run_id])?;
    assert_status(&retained, 0);
    let repeated = run(root.path(), &["cache", "clean", "--format", "json"])?;
    assert_status(&repeated, 0);
    assert_eq!(repeated.stdout, cleaned.stdout);

    let audited = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&audited, 0);
    for parent in ["attempts", "runs", "trash", "cache"] {
        assert!(
            state.join(parent).join("namespace.anchor").is_file(),
            "managed state anchor disappeared for {parent}",
        );
    }
    Ok(())
}
