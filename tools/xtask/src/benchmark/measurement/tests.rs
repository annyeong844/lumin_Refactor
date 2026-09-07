use super::*;

fn observation() -> Value {
    serde_json::json!({
        "schemaVersion":"lumin.windows-process-observation.v1", "method":"private-inherited-job.v1",
        "helperProcessId":10, "processId":20,
        "helperCreationTime100ns":100, "processCreationTime100ns":200, "processExitTime100ns":300,
        "helperInJob":true, "processInJob":true, "limitFlagsBefore":0, "limitFlagsAfter":0,
        "before":{"totalProcesses":1,"activeProcesses":1,"totalTerminatedProcesses":0},
        "after":{"totalProcesses":2,"activeProcesses":1,"totalTerminatedProcesses":0}
    })
}

fn measurement() -> Value {
    serde_json::json!({"processId":20,"rssSource":"GetProcessMemoryInfo.PeakWorkingSetSize"})
}

fn canonical(value: &Value) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[test]
fn windows_companion_requires_exact_accounting_lifetimes_and_launcher() -> Result<(), String> {
    let valid = observation();
    let measured = measurement();
    for diagnostic in [false, true] {
        windows_observation::validate(&canonical(&valid)?, 10, &measured, diagnostic)?;
    }
    let mut retained_reference = valid.clone();
    retained_reference["after"]["activeProcesses"] = 2.into();
    windows_observation::validate(&canonical(&retained_reference)?, 10, &measured, true)?;
    assert!(windows_observation::validate(&canonical(&valid)?, 11, &measured, true).is_err());
    let mut wrong_product = measured.clone();
    wrong_product["processId"] = 21.into();
    assert!(windows_observation::validate(&canonical(&valid)?, 10, &wrong_product, true).is_err());
    for (pointer, replacement) in [
        ("/schemaVersion", serde_json::json!("old")),
        ("/method", serde_json::json!("toolhelp")),
        ("/helperProcessId", serde_json::json!(0)),
        ("/processId", serde_json::json!(0)),
        ("/processId", serde_json::json!(10)),
        ("/processId", serde_json::json!(4_294_967_296_u64)),
        ("/processId", serde_json::json!(20.0)),
        ("/helperInJob", serde_json::json!(false)),
        ("/processInJob", serde_json::json!(1)),
        ("/processInJob", serde_json::json!(false)),
        ("/helperCreationTime100ns", serde_json::json!(0)),
        ("/helperCreationTime100ns", serde_json::json!(201)),
        ("/processCreationTime100ns", serde_json::json!(0)),
        ("/processExitTime100ns", serde_json::json!(199)),
        ("/limitFlagsBefore", serde_json::json!(1)),
        ("/limitFlagsAfter", serde_json::json!(-1)),
        ("/before/totalProcesses", serde_json::json!(2)),
        ("/before/activeProcesses", serde_json::json!(0)),
        ("/before/totalTerminatedProcesses", serde_json::json!(1)),
        ("/after/totalProcesses", serde_json::json!(1)),
        ("/after/totalProcesses", serde_json::json!(3)),
        ("/after/activeProcesses", serde_json::json!(0)),
        ("/after/activeProcesses", serde_json::json!(3)),
        ("/after/totalTerminatedProcesses", serde_json::json!(1)),
    ] {
        let mut altered = valid.clone();
        *altered.pointer_mut(pointer).ok_or("bad fixture pointer")? = replacement;
        assert!(
            windows_observation::validate(&canonical(&altered)?, 10, &measured, true).is_err(),
            "{pointer}"
        );
    }
    Ok(())
}

#[test]
fn windows_companion_rejects_missing_opaque_duplicate_and_noncanonical_bytes() -> Result<(), String>
{
    let valid = observation();
    for key in valid.as_object().ok_or("fixture not object")?.keys() {
        let mut changed = valid.clone();
        changed
            .as_object_mut()
            .ok_or("fixture not object")?
            .remove(key);
        assert!(
            windows_observation::validate(&canonical(&changed)?, 10, &measurement(), true).is_err()
        );
    }
    let text = String::from_utf8(canonical(&valid)?).map_err(|error| error.to_string())?;
    for changed in [
        text.replace("\"processId\":20", "\"processId\":20,\"processId\":20"),
        text.replace("\"processId\":20", "\"processId\":20,\"opaque\":1"),
        text.replace(
            "\"activeProcesses\":1",
            "\"activeProcesses\":1,\"opaque\":1",
        ),
        text.replace(
            "\"totalProcesses\":1",
            "\"totalProcesses\":1,\"totalProcesses\":1",
        ),
        text.replace(
            "\"processExitTime100ns\":300",
            "\"processExitTime100ns\":18446744073709551616",
        ),
        text.trim_end().to_owned(),
        format!("{text}\n"),
        format!(" {text}"),
        "{}\n".to_owned(),
    ] {
        assert!(
            windows_observation::validate(changed.as_bytes(), 10, &measurement(), true).is_err()
        );
    }
    let capture = tempfile::tempdir().map_err(|error| error.to_string())?;
    assert!(
        windows_observation::validate_capture(capture.path(), 10, &measurement(), true, true)
            .is_err()
    );
    let linux = serde_json::json!({"rssSource":"wait4-rusage-ru_maxrss-kib"});
    windows_observation::validate_capture(capture.path(), 10, &linux, false, false)?;
    fs::write(
        capture.path().join(windows_observation::COMPANION),
        canonical(&valid)?,
    )
    .map_err(|error| error.to_string())?;
    assert!(
        windows_observation::validate_capture(capture.path(), 10, &linux, false, false).is_err()
    );
    Ok(())
}

#[cfg(windows)]
#[test]
fn real_windows_helper_rejects_terminated_child_and_preserves_job_receipt() -> Result<(), String> {
    let python = require_python()?;
    let workspace = crate::metadata::find_workspace_root().map_err(|error| error.to_string())?;
    let helper = workspace.join("tools/xtask/benchmark/measure-process.py");
    let root = tempfile::tempdir().map_err(|error| error.to_string())?;
    for diagnostic in [false, true] {
        let capture = root
            .path()
            .join(if diagnostic { "diagnostic" } else { "ordinary" });
        let result = measure_product_mode(
            &python,
            &helper,
            &python,
            root.path(),
            &arguments(&[
                "-I",
                "-S",
                "-c",
                "import subprocess,sys; subprocess.run([sys.executable,'-I','-S','-c','pass'],check=True); print('{}')",
            ]),
            &capture,
            diagnostic,
        );
        let Err(error) = result else {
            return Err("accepted a real child process".into());
        };
        assert!(error.contains("job total 3"), "{error}");
        assert_eq!(
            fs::read(capture.join("stdout")).map_err(|error| error.to_string())?,
            b"{}\r\n"
        );
        assert!(!capture.join("measurement.json").exists());
        let receipt = read_json(
            &capture.join(windows_observation::COMPANION),
            "raw job receipt",
        )?;
        assert_eq!(receipt["after"]["totalProcesses"], 3);
        assert_eq!(receipt["before"]["totalProcesses"], 1);
        assert!(
            fs::read(capture.join("helper.stderr"))
                .map_err(|error| error.to_string())?
                .windows(17)
                .any(|part| part == b"cleanup requested")
        );
    }
    Ok(())
}
