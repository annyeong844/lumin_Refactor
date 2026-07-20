use std::fs;

mod support;

use support::{assert_status, field, run};

#[test]
fn public_process_rejects_state_directory_replacement() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let initial = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&initial, 0);
    let run_id = field(&initial.stdout, "runId")?;

    let state = root.path().join(".lumin");
    let displaced = root.path().join(".lumin.displaced");
    fs::rename(&state, &displaced)?;
    fs::create_dir(&state)?;

    let rejected = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&rejected, 1);
    assert!(
        rejected
            .stderr
            .contains("state namespace integrity failure")
    );

    fs::remove_dir(&state)?;
    fs::rename(displaced, &state)?;
    let recovered = run(root.path(), &["overview", "--run", &run_id])?;
    assert_status(&recovered, 0);
    Ok(())
}

#[test]
fn public_process_rejects_lifecycle_lock_replacement() -> Result<(), Box<dyn std::error::Error>> {
    let root = fixture()?;
    let initial = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&initial, 0);
    let run_id = field(&initial.stdout, "runId")?;

    let state = root.path().join(".lumin");
    let lock = state.join("lifecycle.lock");
    let displaced = state.join("lifecycle.lock.displaced");
    let bytes = fs::read(&lock)?;
    fs::rename(&lock, &displaced)?;
    fs::write(&lock, bytes)?;

    let rejected = run(root.path(), &["audit", "--jobs", "1"])?;
    assert_status(&rejected, 1);
    assert!(
        rejected
            .stderr
            .contains("state namespace integrity failure")
    );

    fs::remove_file(lock)?;
    fs::rename(displaced, state.join("lifecycle.lock"))?;
    let recovered = run(root.path(), &["overview", "--run", &run_id])?;
    assert_status(&recovered, 0);
    Ok(())
}

fn fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(
        root.path().join("src/lib.ts"),
        "export const visible = 1;\n",
    )?;
    Ok(root)
}
