use super::*;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const ATTEMPT: &str = "attempt_0000000000000002";
const RUN: &str = "run_0000000000000002";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Prepare,
    Finalize,
    Release,
}

#[test]
fn generation_bound_attempt_session_read_preserves_foreign_store_after_open() -> TestResult {
    for (phase, stage) in [
        (Phase::Prepare, "after-attempt-session-open"),
        (Phase::Finalize, "after-attempt-session-open:finalize"),
        (Phase::Release, "after-attempt-session-open:release"),
    ] {
        substitute_session_store(stage, phase, false)?;
    }
    Ok(())
}

#[test]
fn generation_bound_attempt_session_read_preserves_foreign_store_after_final_validation()
-> TestResult {
    for (phase, stage) in [
        (Phase::Prepare, "before-attempt-session-return"),
        (Phase::Finalize, "before-attempt-session-return:finalize"),
        (Phase::Release, "before-attempt-session-return:release"),
    ] {
        substitute_session_store(stage, phase, false)?;
    }
    Ok(())
}

#[test]
fn generation_bound_attempt_session_read_preserves_foreign_store_with_read_error() -> TestResult {
    for (phase, stage) in [
        (Phase::Prepare, "after-attempt-session-open-read-error"),
        (
            Phase::Finalize,
            "after-attempt-session-open-read-error:finalize",
        ),
        (
            Phase::Release,
            "after-attempt-session-open-read-error:release",
        ),
    ] {
        substitute_session_store(stage, phase, true)?;
    }
    Ok(())
}

fn substitute_session_store(
    stage: &'static str,
    phase: Phase,
    forced_read_error: bool,
) -> TestResult {
    let root = fixture()?;
    let baseline_run = initialize(root.path())?;
    assert_eq!(baseline_run, "run_0000000000000001");
    let canonical = root.path().join(".lumin/lifecycle.store");
    let mut replacement = FileReplacement::prepare(
        &canonical,
        root.path().join("session-foreign.store"),
        root.path().join("session-authentic.store"),
    )?;
    // Independently admit the closed substitute before the fault. Its namespace,
    // generation and receipts must not be the reason that the later swap fails.
    assert!(replacement.activate()?);
    assert_latest_run(root.path(), &baseline_run)?;
    fs::rename(&canonical, &replacement.prepared)?;
    fs::rename(&replacement.authentic, &canonical)?;
    replacement.active = false;
    let foreign_before = (
        physical_identity(&replacement.prepared)?,
        fs::read(&replacement.prepared)?,
    );
    assert_ne!(physical_identity(&canonical)?, foreign_before.0);
    let permissions = fs::metadata(&canonical)?.permissions();
    let barrier = NamespaceBarrier::new(stage)?;
    let mut audit = barrier.spawn(root.path(), &["audit", "--jobs", "1"])?;
    let mut permit = barrier.accept(&mut audit)?;
    // Use the child's held backend: a second guard would deadlock at the
    // exclusive finalize/release phases and could reopen the backend writable.
    let paused_logical = permit.read_observation()?;
    let paused_files = session_filesystem_snapshot(root.path())?;
    let paused_attempt = assert_phase(root.path(), phase, &paused_logical, &baseline_run)?;
    let rename_error = match fs::rename(&canonical, &replacement.authentic) {
        Ok(()) => {
            fs::rename(&replacement.prepared, &canonical)?;
            replacement.active = true;
            None
        }
        Err(error) => {
            assert!(
                cfg!(windows) && matches!(error.raw_os_error(), Some(5 | 32 | 33)),
                "unexpected store replacement failure at {stage}: {error}"
            );
            assert_eq!(fs::metadata(&canonical)?.permissions(), permissions);
            Some(error)
        }
    };
    let substituted = replacement.active;
    permit.release()?;
    let output = audit.finish()?;
    if substituted {
        assert_status(&output, 1);
        assert!(output.stdout.is_empty());
        assert!(
            output
                .stderr
                .contains("lifecycle.store physical identity changed"),
            "otherwise admissible substitute was not rejected by original identity: {}",
            output.stderr
        );
        assert!(
            output
                .stderr
                .contains("attempt session rejected lifecycle backend access")
        );
    } else if forced_read_error {
        assert_status(&output, 1);
        assert!(output.stdout.is_empty());
        assert!(
            output
                .stderr
                .contains("injected attempt-session lease read failure")
        );
        assert!(
            !output
                .stderr
                .contains("attempt session rejected lifecycle backend access")
        );
    } else {
        assert_status(&output, 0);
        assert_eq!(field(&output.stdout, "runId")?, RUN);
    }
    let foreign_path = if substituted {
        &canonical
    } else {
        &replacement.prepared
    };
    assert_eq!(physical_identity(foreign_path)?, foreign_before.0);
    let foreign_after = fs::read(foreign_path)?;
    assert!(
        foreign_after == foreign_before.1,
        "foreign lifecycle.store bytes changed at {stage}: before={} after={}; {}",
        lumin_model::digest_hex(&foreign_before.1),
        lumin_model::digest_hex(&foreign_after),
        output.stderr
    );
    if let Some(error) = rename_error {
        eprintln!("{stage}: Windows denied live-handle rename: {error}");
        assert_eq!(fs::metadata(&canonical)?.permissions(), permissions);
        // The same prepared rename must work after child handles close.
        assert!(replacement.activate()?);
    }
    replacement.restore()?;
    if substituted {
        assert_eq!(session_filesystem_snapshot(root.path())?, paused_files);
        assert_eq!(complete_observation(root.path())?, paused_logical);
    } else if forced_read_error && phase != Phase::Prepare {
        // Failure continuation cannot rewrite an already Completed envelope.
        assert_eq!(read_attempt(root.path(), ATTEMPT)?, paused_attempt);
        assert_eq!(session_filesystem_snapshot(root.path())?, paused_files);
        assert_eq!(complete_observation(root.path())?, paused_logical);
    }

    let completed = phase != Phase::Prepare || (!substituted && !forced_read_error);
    let expected_state = if completed {
        "completed"
    } else if substituted {
        "interrupted"
    } else {
        "failed"
    };
    let recovered = run_success(root.path(), &["overview"])?;
    assert_recovered(
        root.path(),
        &paused_logical,
        &paused_attempt,
        &baseline_run,
        expected_state,
    )?;
    assert_recovered_files(
        root.path(),
        &paused_files,
        &paused_logical,
        phase,
        completed,
    )?;
    assert_eq!(
        json(&recovered.stdout)?["scope"]["id"],
        if completed { RUN } else { &baseline_run }
    );
    assert_stable_retry(root.path(), &recovered.stdout)?;
    eprintln!("{stage}: exact foreign, paused, recovery and retry snapshots passed");
    Ok(())
}

#[test]
fn generation_bound_attempt_session_read_failure_persists_one_failed_attempt() -> TestResult {
    let root = fixture()?;
    let baseline_run = initialize(root.path())?;
    let barrier = NamespaceBarrier::new("after-attempt-session-open-read-error")?;
    let mut audit = barrier.spawn(root.path(), &["audit", "--jobs", "1"])?;
    let mut permit = barrier.accept(&mut audit)?;
    let paused = permit.read_observation()?;
    let paused_files = session_filesystem_snapshot(root.path())?;
    let attempted = assert_phase(root.path(), Phase::Prepare, &paused, &baseline_run)?;
    permit.release()?;
    let failed = audit.finish()?;
    assert_status(&failed, 1);
    assert!(failed.stdout.is_empty());
    assert!(
        failed
            .stderr
            .contains("injected attempt-session lease read failure")
    );
    assert!(
        !failed
            .stderr
            .contains("attempt session rejected lifecycle backend access")
    );
    // Assert failure persistence before any ordinary recovery command.
    assert_recovered(root.path(), &paused, &attempted, &baseline_run, "failed")?;
    assert_recovered_files(root.path(), &paused_files, &paused, Phase::Prepare, false)?;
    let overview = run_success(root.path(), &["overview"])?;
    assert_stable_retry(root.path(), &overview.stdout)?;
    let healthy = run_success(root.path(), &["audit", "--jobs", "1"])?;
    assert_eq!(field(&healthy.stdout, "runId")?, "run_0000000000000003");
    assert_eq!(read_attempt(root.path(), ATTEMPT)?["state"], "failed");
    assert_eq!(
        read_attempt(root.path(), "attempt_0000000000000003")?["state"],
        "completed"
    );
    let runs = run_success(root.path(), &["runs", "list"])?;
    assert_eq!(
        json(&runs.stdout)?["runs"]
            .as_array()
            .ok_or("runs array missing")?
            .iter()
            .map(|run| run["runId"].as_str().ok_or("run ID missing"))
            .collect::<Result<Vec<_>, _>>()?,
        ["run_0000000000000003", baseline_run.as_str()]
    );
    let overview = run_success(root.path(), &["overview"])?;
    assert_stable_retry(root.path(), &overview.stdout)
}

fn assert_phase(root: &Path, phase: Phase, bytes: &[u8], baseline_run: &str) -> TestResult<Value> {
    let logical: Value = serde_json::from_slice(bytes)?;
    let records = &logical["records"];
    assert_eq!(records["sequences"]["attempt"], 2);
    assert_eq!(
        records["attempt_leases"]
            .as_object()
            .ok_or("leases missing")?
            .len(),
        1
    );
    let lease = decode_row(&records["attempt_leases"][ATTEMPT])?;
    assert_eq!(lease["attemptId"], ATTEMPT);
    assert_eq!(lease["sequence"], 2);
    assert_eq!(lease["state"], "active");
    let indexed = phase == Phase::Release;
    assert_eq!(
        records["sequences"]["run-catalog"],
        if indexed { 2 } else { 1 }
    );
    let catalog = records["run_catalog"]
        .as_object()
        .ok_or("catalog missing")?;
    assert_eq!(
        catalog.keys().map(String::as_str).collect::<Vec<_>>(),
        if indexed {
            vec![baseline_run, RUN]
        } else {
            vec![baseline_run]
        }
    );
    assert_eq!(
        records["pointers"]["latest-attempt"],
        serde_json::json!(ATTEMPT.as_bytes())
    );
    assert_eq!(
        records["pointers"]["latest-completed"],
        serde_json::json!(if indexed { RUN } else { baseline_run }.as_bytes())
    );
    let attempt = read_attempt(root, ATTEMPT)?;
    assert_eq!(attempt["attemptId"], ATTEMPT);
    assert_eq!(attempt["sequence"], 2);
    assert_eq!(
        attempt["state"],
        if phase == Phase::Prepare {
            "running"
        } else {
            "completed"
        }
    );
    assert_eq!(
        attempt["runId"],
        if phase == Phase::Prepare {
            Value::Null
        } else {
            Value::from(RUN)
        }
    );
    assert!(attempt["failure"].is_null());
    assert_eq!(
        attempt["finishedUnixMillis"].is_null(),
        phase == Phase::Prepare
    );
    let mut run_names = fs::read_dir(root.join(".lumin/runs"))?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<Vec<_>, _>>()?;
    run_names.sort();
    let expected = if phase == Phase::Prepare {
        vec!["namespace.anchor", baseline_run]
    } else {
        vec!["namespace.anchor", baseline_run, RUN]
    };
    assert_eq!(
        run_names,
        expected
            .iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>()
    );
    let latest: Value = serde_json::from_slice(&fs::read(root.join(".lumin/latest.json"))?)?;
    assert_eq!(latest["latestAttempt"]["attemptId"], ATTEMPT);
    assert_eq!(
        latest["latestAttempt"]["status"],
        if indexed { "completed" } else { "running" }
    );
    assert_eq!(
        latest["latestCompleted"]["runId"],
        if indexed { RUN } else { baseline_run }
    );
    Ok(attempt)
}

fn assert_recovered(
    root: &Path,
    paused: &[u8],
    attempted: &Value,
    baseline_run: &str,
    state: &str,
) -> TestResult {
    let completed = state == "completed";
    let attempt = read_attempt(root, ATTEMPT)?;
    assert_eq!(attempt["attemptId"], ATTEMPT);
    assert_eq!(attempt["sequence"], 2);
    assert_eq!(attempt["startedUnixMillis"], attempted["startedUnixMillis"]);
    assert_eq!(attempt["state"], state);
    assert!(attempt["finishedUnixMillis"].as_u64().is_some());
    assert_eq!(
        attempt["runId"],
        if completed {
            Value::from(RUN)
        } else {
            Value::Null
        }
    );
    if completed {
        assert!(attempt["failure"].is_null());
        if attempted["state"] == "completed" {
            assert_eq!(&attempt, attempted);
        }
    } else if state == "interrupted" {
        assert_eq!(
            attempt["failure"],
            "attempt owner process exited before terminal publication"
        );
    } else {
        assert!(attempt["failure"].as_str().is_some_and(|failure| {
            failure.contains("injected attempt-session lease read failure")
        }));
    }

    let mut expected: Value = serde_json::from_slice(paused)?;
    expected["records"]["attempt_leases"] = serde_json::json!({});
    if completed {
        expected["records"]["sequences"]["run-catalog"] = Value::from(2);
        expected["records"]["pointers"]["latest-completed"] = serde_json::json!(RUN.as_bytes());
        let envelope: Value = serde_json::from_slice(&fs::read(
            root.join(".lumin/runs").join(RUN).join("run.json"),
        )?)?;
        assert_eq!(envelope["attemptId"], ATTEMPT);
        assert_eq!(envelope["runId"], RUN);
        assert_eq!(envelope["sequence"], 2);
        // Independent canonical field order; compare every other header/row too.
        let bytes = format!(
            "{{\"attemptId\":{},\"runId\":{},\"sequence\":{},\"evidenceStoreSha256\":{},\"evidenceStoreSize\":{}}}",
            envelope["attemptId"], envelope["runId"], envelope["sequence"],
            envelope["evidenceStoreSha256"], envelope["evidenceStoreSize"]
        ).into_bytes();
        expected["records"]["run_catalog"][RUN] = serde_json::json!(bytes);
    }
    assert_eq!(
        serde_json::from_slice::<Value>(&complete_observation(root)?)?,
        expected
    );
    let latest: Value = serde_json::from_slice(&fs::read(root.join(".lumin/latest.json"))?)?;
    assert_eq!(
        latest,
        serde_json::json!({
            "schemaVersion": "lumin-latest.v1",
            "latestAttempt": {"attemptId": ATTEMPT, "sequence": 2, "status": state},
            "latestCompleted": {"runId": if completed { RUN } else { baseline_run }, "sequence": if completed { 2 } else { 1 }}
        })
    );
    Ok(())
}

fn assert_recovered_files(
    root: &Path,
    paused_files: &[TreeEntry],
    paused_logical: &[u8],
    phase: Phase,
    completed: bool,
) -> TestResult {
    let logical: Value = serde_json::from_slice(paused_logical)?;
    let lease = decode_row(&logical["records"]["attempt_leases"][ATTEMPT])?;
    let lock_name = lease["lockName"]
        .as_str()
        .ok_or("lease lock name missing")?;
    let after = session_filesystem_snapshot(root)?;
    assert!(!after.iter().any(|entry| entry.relative == lock_name));
    let attempt_file = format!("attempts/{ATTEMPT}/attempt.json");
    let new_run = format!("runs/{RUN}");
    let retain = |entry: &&TreeEntry| {
        entry.relative != lock_name
            && !(phase == Phase::Prepare && entry.relative == attempt_file)
            && !(phase != Phase::Release && entry.relative == "latest.json")
            && !(phase == Phase::Prepare
                && completed
                && (entry.relative == new_run
                    || entry.relative.starts_with(&format!("{new_run}/"))))
    };
    // Only the selected lease may disappear. Prepare may terminalize its one
    // envelope; earlier phases may advance latest. Every already published run,
    // directory identity, anchor, cache object and unrelated envelope must stay.
    assert_eq!(
        after.iter().filter(retain).collect::<Vec<_>>(),
        paused_files.iter().filter(retain).collect::<Vec<_>>()
    );
    if phase == Phase::Prepare && completed {
        assert_eq!(
            rebased_subtree(&after, &new_run)
                .iter()
                .map(|entry| entry.relative.as_str())
                .collect::<Vec<_>>(),
            [".", "evidence.store", "run.json"]
        );
    }
    Ok(())
}

fn assert_stable_retry(root: &Path, overview: &str) -> TestResult {
    let files = session_filesystem_snapshot(root)?;
    let logical = complete_observation(root)?;
    let retry = run_success(root, &["overview"])?;
    assert_eq!(retry.stdout, overview);
    assert_eq!(session_filesystem_snapshot(root)?, files);
    assert_eq!(complete_observation(root)?, logical);
    Ok(())
}

fn complete_observation(root: &Path) -> TestResult<Vec<u8>> {
    lumin_engine::complete_logical_store_observation_for_test(root).map_err(Into::into)
}

fn decode_row(value: &Value) -> TestResult<Value> {
    let bytes: Vec<u8> = serde_json::from_value(value.clone())?;
    serde_json::from_slice(&bytes).map_err(Into::into)
}

fn read_attempt(root: &Path, id: &str) -> TestResult<Value> {
    serde_json::from_slice(&fs::read(
        root.join(".lumin/attempts").join(id).join("attempt.json"),
    )?)
    .map_err(Into::into)
}

fn session_filesystem_snapshot(root: &Path) -> TestResult<Vec<TreeEntry>> {
    let state = root.join(".lumin");
    let mut entries = vec![TreeEntry {
        relative: ".".to_owned(),
        kind: "directory",
        physical_identity: physical_identity(&state)?,
        bytes: Vec::new(),
    }];
    for entry in fs::read_dir(&state)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "non-UTF-8 test state entry")?;
        if entry.file_type()?.is_dir() {
            for mut child in tree_snapshot(&entry.path())? {
                child.relative = if child.relative == "." {
                    name.clone()
                } else {
                    format!("{name}/{}", child.relative)
                };
                entries.push(child);
            }
        } else {
            assert!(entry.file_type()?.is_file());
            // Backend recovery metadata is not byte-immutable. Windows live
            // byte-range locks forbid unrelated handle reads while paused.
            let bytes = if name == "lifecycle.store" || name.ends_with(".lock") {
                Vec::new()
            } else {
                fs::read(entry.path())?
            };
            entries.push(TreeEntry {
                relative: name,
                kind: "file",
                physical_identity: physical_identity(&entry.path())?,
                bytes,
            });
        }
    }
    entries.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok(entries)
}
