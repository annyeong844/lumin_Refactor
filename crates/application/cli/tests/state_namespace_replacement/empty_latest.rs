use super::*;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const FINAL_STAGES: [&str; 2] = [
    "before-empty-latest-return",
    "before-empty-latest-return:attempt",
];
const READ_ERROR_STAGES: [&str; 2] = [
    "after-latest-derivation-open-read-error",
    "after-latest-derivation-open-read-error:attempt",
];

fn empty_fixture() -> TestResult<tempfile::TempDir> {
    let root = fixture()?;
    assert_empty_overview(root.path())?;
    assert_empty_observation(&complete_observation(root.path())?)?;
    Ok(root)
}

fn assert_empty_overview(root: &Path) -> TestResult {
    let output = run(root, &["overview"])?;
    assert_status(&output, 2);
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr,
        "lumin: no completed run exists for this repository\n"
    );
    Ok(())
}

fn complete_observation(root: &Path) -> TestResult<Vec<u8>> {
    lumin_engine::complete_logical_store_observation_for_test(root).map_err(Into::into)
}

fn durable_namespace_snapshot(root: &Path) -> TestResult<DurableNamespaceSnapshot> {
    Ok((
        physical_identity(&root.join(".lumin/lifecycle.store"))?,
        super::attempt_session::session_filesystem_snapshot(root)?,
    ))
}

fn assert_empty_observation(bytes: &[u8]) -> TestResult {
    let snapshot: Value = serde_json::from_slice(bytes)?;
    for table in ["attempt_leases", "run_catalog", "pointers", "operations"] {
        assert_eq!(snapshot["records"][table], serde_json::json!({}), "{table}");
    }
    assert!(snapshot["records"]["sequences"].get("attempt").is_none());
    Ok(())
}

fn assert_empty_retry(
    root: &Path,
    logical: &[u8],
    physical: &DurableNamespaceSnapshot,
) -> TestResult {
    for _ in 0..2 {
        assert_empty_overview(root)?;
        assert_eq!(complete_observation(root)?, logical);
        assert_eq!(&durable_namespace_snapshot(root)?, physical);
    }
    Ok(())
}

#[test]
fn empty_latest_read_preserves_foreign_store() -> TestResult {
    for stage in [
        "after-latest-derivation-open",
        "after-latest-derivation-open:attempt",
        FINAL_STAGES[0],
        FINAL_STAGES[1],
        READ_ERROR_STAGES[0],
        READ_ERROR_STAGES[1],
    ] {
        let root = empty_fixture()?;
        let canonical = root.path().join(".lumin/lifecycle.store");
        let mut replacement = FileReplacement::prepare(
            &canonical,
            root.path().join("w8-prepared.store"),
            root.path().join("w8-authentic.store"),
        )?;
        // The substitute is independently admissible before the fault: failure
        // at the exact turn must authenticate the original physical identity.
        assert!(replacement.activate()?);
        assert_empty_overview(root.path())?;
        fs::rename(&canonical, &replacement.prepared)?;
        fs::rename(&replacement.authentic, &canonical)?;
        replacement.active = false;
        // Admission can update redb's private recovery bytes. Recopy only this
        // owned fixture file after that proof so the fault is byte-identical.
        fs::copy(&canonical, &replacement.prepared)?;
        assert!(fs::read(&canonical)? == fs::read(&replacement.prepared)?);
        let foreign = (
            physical_identity(&replacement.prepared)?,
            fs::read(&replacement.prepared)?,
        );
        assert_ne!(physical_identity(&canonical)?, foreign.0);
        let permissions = fs::metadata(&canonical)?.permissions();
        let barrier = NamespaceBarrier::new(stage)?;
        let mut child = barrier.spawn(root.path(), &["audit", "--jobs", "1"])?;
        let mut permit = barrier.accept(&mut child)?;
        let paused = permit.read_observation()?;
        assert_empty_observation(&paused)?;
        let physical = durable_namespace_snapshot(root.path())?;
        let replaced = replacement.activate()?;
        eprintln!("W8 {stage}: store substitution installed={replaced}");
        assert!(
            replaced || cfg!(windows),
            "Linux must substitute the held store at {stage}"
        );
        permit.release()?;
        let output = child.finish()?;
        let read_error = READ_ERROR_STAGES.contains(&stage);
        if replaced {
            assert_integrity_failure(&output);
            assert!(
                output
                    .stderr
                    .contains("lifecycle.store physical identity changed"),
                "{stage}: {}",
                output.stderr
            );
        } else if read_error {
            assert_integrity_failure(&output);
            assert!(output.stderr.contains("injected latest index read failure"));
        } else {
            assert_status(&output, 0);
            assert_eq!(field(&output.stdout, "runId")?, "run_0000000000000001");
        }
        let foreign_path = if replaced {
            &canonical
        } else {
            &replacement.prepared
        };
        assert_eq!(physical_identity(foreign_path)?, foreign.0);
        assert!(
            fs::read(foreign_path)? == foreign.1,
            "foreign backend bytes changed at {stage}"
        );
        if !replaced {
            assert_eq!(fs::metadata(&canonical)?.permissions(), permissions);
            assert!(
                replacement.activate()?,
                "the identical rename must work after handle release"
            );
        }
        replacement.restore()?;
        if replaced || read_error {
            assert_eq!(complete_observation(root.path())?, paused);
            assert_eq!(durable_namespace_snapshot(root.path())?, physical);
            assert_empty_retry(root.path(), &paused, &physical)?;
        } else {
            assert_latest_run(root.path(), "run_0000000000000001")?;
            let state = complete_observation(root.path())?;
            let files = durable_namespace_snapshot(root.path())?;
            assert_latest_run(root.path(), "run_0000000000000001")?;
            assert_eq!(complete_observation(root.path())?, state);
            assert_eq!(durable_namespace_snapshot(root.path())?, files);
        }
    }
    Ok(())
}

#[test]
fn empty_latest_read_error_precedes_new_attempt_allocation() -> TestResult {
    for stage in READ_ERROR_STAGES {
        let root = empty_fixture()?;
        let barrier = NamespaceBarrier::new(stage)?;
        let mut child = barrier.spawn(root.path(), &["audit", "--jobs", "1"])?;
        let mut permit = barrier.accept(&mut child)?;
        let paused = permit.read_observation()?;
        assert_empty_observation(&paused)?;
        let physical = durable_namespace_snapshot(root.path())?;
        permit.release()?;
        let output = child.finish()?;
        assert_integrity_failure(&output);
        assert!(output.stderr.contains("injected latest index read failure"));
        assert!(!output.stderr.contains("backend access was rejected"));
        assert_eq!(complete_observation(root.path())?, paused);
        assert_eq!(durable_namespace_snapshot(root.path())?, physical);
        assert_empty_retry(root.path(), &paused, &physical)?;
        assert_eq!(initialize(root.path())?, "run_0000000000000001");
    }
    Ok(())
}

#[test]
fn empty_latest_read_preserves_late_pointer_arrivals() -> TestResult {
    for stage in FINAL_STAGES {
        #[cfg(not(windows))]
        let names = ["latest.json", "latest.json.pending"];
        #[cfg(windows)]
        let names = [
            "latest.json",
            "latest.json.pending",
            "LATEST.JSON",
            "LATEST.JSON.PENDING",
        ];
        for name in names {
            let root = empty_fixture()?;
            let barrier = NamespaceBarrier::new(stage)?;
            let mut child = barrier.spawn(root.path(), &["audit", "--jobs", "1"])?;
            let mut permit = barrier.accept(&mut child)?;
            let paused = permit.read_observation()?;
            assert_empty_observation(&paused)?;
            let physical = durable_namespace_snapshot(root.path())?;
            let foreign = root.path().join(".lumin").join(name);
            fs::write(&foreign, b"{\"schemaVersion\":\"lumin-latest.v1\",\"latestAttempt\":null,\"latestCompleted\":null}")?;
            let foreign_before = (physical_identity(&foreign)?, fs::read(&foreign)?);
            permit.release()?;
            let output = child.finish()?;
            assert_integrity_failure(&output);
            assert!(
                output
                    .stderr
                    .contains("latest pointer appeared during empty index derivation")
            );
            assert_eq!(
                (physical_identity(&foreign)?, fs::read(&foreign)?),
                foreign_before
            );
            // Restore only this fixture's injected child before ordinary admission.
            fs::remove_file(&foreign)?;
            assert_eq!(complete_observation(root.path())?, paused);
            assert_eq!(durable_namespace_snapshot(root.path())?, physical);
            assert_empty_retry(root.path(), &paused, &physical)?;
        }
    }
    Ok(())
}

#[test]
fn empty_latest_read_validation_failures_poison_before_new_allocation() -> TestResult {
    for (stage, diagnostic) in [
        (
            "before-empty-latest-return-generation-error",
            "lifecycle store generation changed before mutation: expected 1, observed 2",
        ),
        (
            "before-empty-latest-return-generation-error:attempt",
            "lifecycle store generation changed before mutation: expected 1, observed 2",
        ),
        (
            "before-empty-latest-return-receipt-error",
            "injected latest validation receipt failure",
        ),
        (
            "before-empty-latest-return-receipt-error:attempt",
            "injected latest validation receipt failure",
        ),
    ] {
        let root = empty_fixture()?;
        let barrier = NamespaceBarrier::new(stage)?;
        let mut child = barrier.spawn(root.path(), &["audit", "--jobs", "1"])?;
        let mut permit = barrier.accept(&mut child)?;
        let paused = permit.read_observation()?;
        assert_empty_observation(&paused)?;
        let physical = durable_namespace_snapshot(root.path())?;
        permit.release()?;
        let output = child.finish()?;
        assert_status(&output, 1);
        assert!(output.stdout.is_empty());
        assert!(
            output.stderr.contains(diagnostic),
            "{stage}: {}",
            output.stderr
        );
        // These public errors are injected at the second owner validation, not
        // by the barrier or by rewriting/repairing the durable header. The owner
        // test separately corrupts the real header to verify both validators.
        assert_empty_retry(root.path(), &paused, &physical)?;
        assert_eq!(initialize(root.path())?, "run_0000000000000001");
    }
    Ok(())
}

#[test]
fn empty_latest_read_preserves_replaced_namespace_bindings() -> TestResult {
    for stage in FINAL_STAGES {
        for relative in [
            ".lumin",
            ".lumin/attempts",
            ".lumin/runs",
            ".lumin/cache",
            ".lumin/trash",
            ".lumin/trash/cache-evictions",
        ] {
            let root = empty_fixture()?;
            let mut replacement = DirectoryReplacement::prepare(
                root.path(),
                &root.path().join(relative),
                "empty-latest",
            )?;
            let foreign = tree_snapshot(&replacement.prepared)?;
            let barrier = NamespaceBarrier::new(stage)?;
            let mut child = barrier.spawn(root.path(), &["audit", "--jobs", "1"])?;
            let mut permit = barrier.accept(&mut child)?;
            let paused = permit.read_observation()?;
            assert_empty_observation(&paused)?;
            let physical = durable_namespace_snapshot(root.path())?;
            let permissions = fs::metadata(&replacement.canonical)?.permissions();
            let replaced = replacement.activate()?;
            assert!(replaced || cfg!(windows));
            permit.release()?;
            let output = child.finish()?;
            if replaced {
                assert_integrity_failure(&output);
            } else {
                assert_status(&output, 0);
            }
            let foreign_path = if replaced {
                &replacement.canonical
            } else {
                &replacement.prepared
            };
            assert_eq!(tree_snapshot(foreign_path)?, foreign);
            if !replaced {
                assert_eq!(
                    fs::metadata(&replacement.canonical)?.permissions(),
                    permissions
                );
                assert!(replacement.activate()?);
            }
            replacement.restore()?;
            if replaced {
                assert_eq!(complete_observation(root.path())?, paused);
                assert_eq!(durable_namespace_snapshot(root.path())?, physical);
                assert_empty_retry(root.path(), &paused, &physical)?;
            } else {
                assert_latest_run(root.path(), "run_0000000000000001")?;
            }
        }
        let root = empty_fixture()?;
        let canonical = root.path().join(".lumin/lifecycle.lock");
        let mut replacement = FileReplacement::prepare(
            &canonical,
            root.path().join("w8-prepared.lock"),
            root.path().join("w8-authentic.lock"),
        )?;
        let foreign = (
            physical_identity(&replacement.prepared)?,
            fs::read(&replacement.prepared)?,
        );
        let barrier = NamespaceBarrier::new(stage)?;
        let mut child = barrier.spawn(root.path(), &["audit", "--jobs", "1"])?;
        let mut permit = barrier.accept(&mut child)?;
        let paused = permit.read_observation()?;
        let physical = durable_namespace_snapshot(root.path())?;
        let replaced = replacement.activate()?;
        assert!(replaced || cfg!(windows));
        permit.release()?;
        let output = child.finish()?;
        if replaced {
            assert_integrity_failure(&output);
        } else {
            assert_status(&output, 0);
        }
        let foreign_path = if replaced {
            &canonical
        } else {
            &replacement.prepared
        };
        assert_eq!(
            (physical_identity(foreign_path)?, fs::read(foreign_path)?),
            foreign
        );
        if !replaced {
            assert!(replacement.activate()?);
        }
        replacement.restore()?;
        if replaced {
            assert_empty_retry(root.path(), &paused, &physical)?;
        }
    }
    Ok(())
}

#[test]
fn empty_latest_read_rejects_extra_store_links() -> TestResult {
    for stage in FINAL_STAGES {
        let root = empty_fixture()?;
        let canonical = root.path().join(".lumin/lifecycle.store");
        let alias = root.path().join("w8-store-link");
        let barrier = NamespaceBarrier::new(stage)?;
        let mut child = barrier.spawn(root.path(), &["audit", "--jobs", "1"])?;
        let mut permit = barrier.accept(&mut child)?;
        let paused = permit.read_observation()?;
        let physical = durable_namespace_snapshot(root.path())?;
        fs::hard_link(&canonical, &alias)?;
        let identity = physical_identity(&alias)?;
        assert_eq!(identity, physical_identity(&canonical)?);
        permit.release()?;
        let output = child.finish()?;
        assert_integrity_failure(&output);
        assert!(output.stderr.contains("exactly one physical link"));
        assert_eq!(physical_identity(&alias)?, identity);
        assert_eq!(physical_identity(&canonical)?, identity);
        assert!(fs::read(&alias)? == fs::read(&canonical)?);
        fs::remove_file(alias)?;
        assert_empty_retry(root.path(), &paused, &physical)?;
    }
    Ok(())
}

#[test]
fn empty_latest_read_preserves_preexisting_allocation_recovery() -> TestResult {
    for (point, expected_state) in [
        ("after-attempt-lock-creation", "allocating"),
        ("after-catalog-allocation", "active"),
    ] {
        let root = empty_fixture()?;
        let crashed = support::run_with_env(
            root.path(),
            &["audit", "--jobs", "1"],
            &[("LUMIN_TEST_PUBLICATION_CRASH_POINT", point)],
        )?;
        assert_status(&crashed, 95);
        assert!(crashed.stdout.is_empty());
        assert!(crashed.stderr.is_empty());
        let mut expected: Value = serde_json::from_slice(&complete_observation(root.path())?)?;
        assert_eq!(expected["records"]["pointers"], serde_json::json!({}));
        assert_eq!(expected["records"]["run_catalog"], serde_json::json!({}));
        assert_eq!(expected["records"]["sequences"]["attempt"], 1);
        let leases = expected["records"]["attempt_leases"]
            .as_object_mut()
            .ok_or("missing lease table")?;
        assert_eq!(leases.len(), 1);
        let bytes: Vec<u8> = serde_json::from_value(
            leases
                .remove("attempt_0000000000000001")
                .ok_or("missing reserved attempt")?,
        )?;
        let lease: Value = serde_json::from_slice(&bytes)?;
        assert_eq!(lease["state"], expected_state);
        assert!(!root.path().join(".lumin/latest.json").exists());
        assert!(
            !root
                .path()
                .join(".lumin/attempts/attempt_0000000000000001")
                .exists()
        );
        let physical = durable_namespace_snapshot(root.path())?;
        let retained = physical
            .1
            .into_iter()
            .filter(|entry| !entry.relative.starts_with("attempt-liveness-"))
            .collect::<Vec<_>>();
        assert_empty_overview(root.path())?;
        assert_eq!(
            serde_json::from_slice::<Value>(&complete_observation(root.path())?)?,
            expected
        );
        assert_eq!(
            durable_namespace_snapshot(root.path())?,
            (physical.0, retained)
        );
        let recovered = complete_observation(root.path())?;
        let recovered_files = durable_namespace_snapshot(root.path())?;
        assert_empty_retry(root.path(), &recovered, &recovered_files)?;
        assert_eq!(initialize(root.path())?, "run_0000000000000002");
        let final_state = complete_observation(root.path())?;
        let final_files = durable_namespace_snapshot(root.path())?;
        let records: Value = serde_json::from_slice(&final_state)?;
        assert_eq!(records["records"]["attempt_leases"], serde_json::json!({}));
        assert_eq!(records["records"]["sequences"]["attempt"], 2);
        assert_eq!(records["records"]["sequences"]["run-catalog"], 1);
        assert_eq!(
            records["records"]["run_catalog"]
                .as_object()
                .ok_or("missing run catalog")?
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["run_0000000000000002"]
        );
        for _ in 0..2 {
            assert_latest_run(root.path(), "run_0000000000000002")?;
            assert_eq!(complete_observation(root.path())?, final_state);
            assert_eq!(durable_namespace_snapshot(root.path())?, final_files);
        }
    }
    Ok(())
}

#[test]
fn empty_latest_read_orders_waiting_writer_and_migration() -> TestResult {
    for stage in FINAL_STAGES {
        for migration in [false, true] {
            let root = empty_fixture()?;
            let barrier = NamespaceBarrier::new(stage)?;
            let mut reader = barrier.spawn(root.path(), &["audit", "--jobs", "1"])?;
            let mut reader_permit = barrier.accept(&mut reader)?;
            assert_empty_observation(&reader_permit.read_observation()?)?;
            let waiter_barrier = NamespaceBarrier::new("namespace-lock-contended")?;
            let arguments = if migration {
                vec!["store", "migrate", "--format", "json"]
            } else {
                vec!["cache", "test-write", "ordered.bin", "after-empty-read"]
            };
            let mut waiter = waiter_barrier.spawn(root.path(), &arguments)?;
            // This acknowledgement exists only after an actual failed native
            // exclusive-lock acquisition, not merely after starting a child.
            let waiter_permit = waiter_barrier.accept(&mut waiter)?;
            assert!(!waiter.has_exited()?);
            assert!(!root.path().join(".lumin/cache/ordered.bin").exists());
            reader_permit.release()?;
            let read = reader.finish()?;
            assert_status(&read, 0);
            assert_eq!(field(&read.stdout, "runId")?, "run_0000000000000001");
            let logical = complete_observation(root.path())?;
            let physical = durable_namespace_snapshot(root.path())?;
            waiter_permit.release()?;
            let waited = waiter.finish()?;
            assert_status(&waited, 0);
            assert!(waited.stderr.is_empty());
            let mut expected_logical: Value = serde_json::from_slice(&logical)?;
            if !migration {
                // The owner test writer creates its empty reservation table on
                // first admission; no operation, allocator or receipt changes.
                let tables = expected_logical["tableNames"]
                    .as_array_mut()
                    .ok_or("missing table inventory")?;
                assert!(!tables.contains(&Value::from("cache-cleanup-operations")));
                tables.push(Value::from("cache-cleanup-operations"));
                tables.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
            }
            assert_eq!(
                serde_json::from_slice::<Value>(&complete_observation(root.path())?)?,
                expected_logical
            );
            let mut expected = physical.clone();
            if migration {
                assert_eq!(
                    waited.stdout,
                    "{\"schemaVersion\":\"lumin.lifecycle-store-migration.v1\",\"storeSchema\":\"lumin-lifecycle-store-header.v13\",\"status\":\"ready\"}\n"
                );
            } else {
                let path = root.path().join(".lumin/cache/ordered.bin");
                assert_eq!(fs::read(&path)?, b"after-empty-read");
                expected.1.push(TreeEntry {
                    relative: "cache/ordered.bin".to_owned(),
                    kind: "file",
                    physical_identity: physical_identity(&path)?,
                    bytes: b"after-empty-read".to_vec(),
                });
                expected
                    .1
                    .sort_by(|left, right| left.relative.cmp(&right.relative));
            }
            assert_eq!(durable_namespace_snapshot(root.path())?, expected);
            if migration {
                let replay = run(root.path(), &arguments)?;
                assert_status(&replay, 0);
                assert_eq!(replay.stdout, waited.stdout);
            } else {
                assert_latest_run(root.path(), "run_0000000000000001")?;
            }
            assert_eq!(
                serde_json::from_slice::<Value>(&complete_observation(root.path())?)?,
                expected_logical
            );
            assert_eq!(durable_namespace_snapshot(root.path())?, expected);
        }
    }
    Ok(())
}
