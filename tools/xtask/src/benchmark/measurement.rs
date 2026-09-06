use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

pub(super) struct ProcessMeasurement {
    pub(super) elapsed_nanoseconds: u64,
    pub(super) peak_rss_bytes: u64,
    pub(super) response: Value,
    pub(super) raw: Value,
}

pub(super) struct HostEnvironment {
    pub(super) classification: String,
    pub(super) blocking: bool,
    pub(super) details: Value,
}

pub(super) fn require_python() -> Result<PathBuf, String> {
    let configured = std::env::var_os("PINNED_PYTHON")
        .map(PathBuf::from)
        .ok_or_else(|| "PINNED_PYTHON is required for benchmark measurement".to_owned())?;
    let python = configured.canonicalize().map_err(|error| {
        format!(
            "cannot resolve PINNED_PYTHON {}: {error}",
            configured.display()
        )
    })?;
    if !python.is_file() {
        return Err(format!("PINNED_PYTHON is not a file: {}", python.display()));
    }
    Ok(python)
}

pub(super) fn inspect_host(
    python: &Path,
    script: &Path,
    scratch: &Path,
) -> Result<HostEnvironment, String> {
    let output_path = scratch.join("host.json");
    let output = Command::new(python)
        .arg("-I")
        .arg("-S")
        .arg(script)
        .arg("host")
        .arg("--root")
        .arg(scratch)
        .arg("--output")
        .arg(&output_path)
        .output()
        .map_err(|error| format!("cannot inspect benchmark host: {error}"))?;
    require_helper_success(&output, "benchmark host inspection")?;
    let details = read_json(&output_path, "benchmark host inspection")?;
    fs::remove_file(&output_path)
        .map_err(|error| format!("cannot remove benchmark host capture: {error}"))?;
    let inferred = classify_host(&details, scratch)?;
    if let Some(configured) = std::env::var_os("LUMIN_BENCHMARK_ENVIRONMENT") {
        let configured = configured
            .into_string()
            .map_err(|_| "LUMIN_BENCHMARK_ENVIRONMENT must be UTF-8".to_owned())?;
        if configured != inferred {
            return Err(format!(
                "configured benchmark environment {configured:?} disagrees with observed {inferred:?}"
            ));
        }
    }
    Ok(HostEnvironment {
        blocking: inferred != "wsl2-mnt-report-only",
        classification: inferred,
        details,
    })
}

fn classify_host(details: &Value, scratch: &Path) -> Result<String, String> {
    let operating_system = required_string(details, "/operatingSystem")?;
    let filesystem = required_string(details, "/filesystemClass")?;
    let wsl = details
        .pointer("/wsl")
        .and_then(Value::as_bool)
        .ok_or_else(|| "host inspection omitted wsl".to_owned())?;
    match (operating_system, wsl) {
        ("windows", false) if filesystem.eq_ignore_ascii_case("ntfs") => {
            Ok("windows-ntfs".to_owned())
        }
        ("linux", true) if scratch.starts_with("/mnt") => Ok("wsl2-mnt-report-only".to_owned()),
        ("linux", true) if filesystem == "ext4" => Ok("wsl2-ext4".to_owned()),
        ("linux", false) => Ok("linux-release".to_owned()),
        _ => Err(format!(
            "unsupported blocking benchmark host: os={operating_system} wsl={wsl} filesystem={filesystem} scratch={}",
            scratch.display()
        )),
    }
}

pub(super) fn measure_product(
    python: &Path,
    script: &Path,
    binary: &Path,
    root: &Path,
    arguments: &[OsString],
    capture: &Path,
) -> Result<ProcessMeasurement, String> {
    measure_product_mode(python, script, binary, root, arguments, capture, false)
}

pub(super) fn measure_diagnostic(
    python: &Path,
    script: &Path,
    binary: &Path,
    root: &Path,
    arguments: &[OsString],
    capture: &Path,
) -> Result<ProcessMeasurement, String> {
    measure_product_mode(python, script, binary, root, arguments, capture, true)
}

fn measure_product_mode(
    python: &Path,
    script: &Path,
    binary: &Path,
    root: &Path,
    arguments: &[OsString],
    capture: &Path,
    diagnostic: bool,
) -> Result<ProcessMeasurement, String> {
    fs::create_dir(capture).map_err(|error| {
        format!(
            "cannot create process capture directory {}: {error}",
            capture.display()
        )
    })?;
    let metrics_path = capture.join("measurement.json");
    super::archive::write_json(
        &capture.join("invocation.json"),
        &serde_json::json!({
            "binary":binary, "root":root,
            "arguments":arguments.iter().map(|arg| arg.to_string_lossy()).collect::<Vec<_>>(),
            "observerMode":if diagnostic { "measure-audit-diagnostic" } else { "measure" },
        }),
    )?;
    let stdout_path = capture.join("stdout");
    let stderr_path = capture.join("stderr");
    let mut command = Command::new(python);
    command
        .arg("-I")
        .arg("-S")
        .arg(script)
        .arg(if diagnostic {
            "measure-audit-diagnostic"
        } else {
            "measure"
        })
        .arg("--cwd")
        .arg(root)
        .arg("--output")
        .arg(&metrics_path)
        .arg("--stdout")
        .arg(&stdout_path)
        .arg("--stderr")
        .arg(&stderr_path)
        .arg("--")
        .arg(binary)
        .args(arguments);
    let helper = command
        .output()
        .map_err(|error| format!("cannot launch benchmark process helper: {error}"))?;
    super::archive::write_bytes(&capture.join("helper.stdout"), &helper.stdout)?;
    super::archive::write_bytes(&capture.join("helper.stderr"), &helper.stderr)?;
    require_helper_success(&helper, "benchmark process measurement")?;

    let raw = read_measurement(&metrics_path, diagnostic)?;
    let schema = if diagnostic {
        "lumin.phase1-process-measurement.v2"
    } else {
        "lumin.phase1-process-measurement.v1"
    };
    if raw["schemaVersion"] != schema || (diagnostic && required_u64(&raw, "/processId")? == 0) {
        return Err("unexpected process measurement schema or PID".to_owned());
    }
    let exit_code = raw
        .pointer("/exitCode")
        .and_then(Value::as_i64)
        .ok_or_else(|| "process measurement omitted exitCode".to_owned())?;
    let product_stdout = fs::read(&stdout_path)
        .map_err(|error| format!("cannot read measured product stdout: {error}"))?;
    let product_stderr = fs::read(&stderr_path)
        .map_err(|error| format!("cannot read measured product stderr: {error}"))?;
    if exit_code != 0 || (!diagnostic && !product_stderr.is_empty()) {
        return Err(format!(
            "measured product exited {exit_code}; stdout={} stderr={}",
            String::from_utf8_lossy(&product_stdout),
            String::from_utf8_lossy(&product_stderr)
        ));
    }
    let children = raw
        .pointer("/analysisChildPids")
        .and_then(Value::as_array)
        .ok_or_else(|| "process measurement omitted analysisChildPids".to_owned())?;
    if !children.is_empty() {
        return Err(format!(
            "measured product launched analysis child processes: {children:?}"
        ));
    }
    let response = parse_canonical_product_json(&product_stdout, "measured product")?;
    Ok(ProcessMeasurement {
        elapsed_nanoseconds: required_u64(&raw, "/elapsedNanoseconds")?,
        peak_rss_bytes: required_u64(&raw, "/peakRssBytes")?,
        response,
        raw,
    })
}

pub(super) fn run_query(
    binary: &Path,
    root: &Path,
    arguments: &[OsString],
    capture: &Path,
) -> Result<Value, String> {
    fs::create_dir(capture).map_err(|error| format!("cannot create query capture: {error}"))?;
    super::archive::write_json(
        &capture.join("invocation.json"),
        &serde_json::json!({
            "binary":binary, "root":root,
            "arguments":arguments.iter().map(|arg| arg.to_string_lossy()).collect::<Vec<_>>(),
        }),
    )?;
    let mut command = Command::new(binary);
    command.env_clear().current_dir(root).args(arguments);
    #[cfg(windows)]
    command.env(
        "SystemRoot",
        std::env::var_os("SystemRoot")
            .ok_or_else(|| "SystemRoot is required to launch lumin on Windows".to_owned())?,
    );
    let stdout_path = capture.join("stdout");
    let stderr_path = capture.join("stderr");
    let stdout = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stdout_path)
        .map_err(|error| error.to_string())?;
    let stderr = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stderr_path)
        .map_err(|error| error.to_string())?;
    let status = command
        .stdout(stdout)
        .stderr(stderr)
        .stdin(std::process::Stdio::null())
        .status()
        .map_err(|error| format!("cannot run packaged benchmark query: {error}"))?;
    let product_stdout = fs::read(stdout_path).map_err(|error| error.to_string())?;
    let product_stderr = fs::read(stderr_path).map_err(|error| error.to_string())?;
    super::archive::write_json(
        &capture.join("status.json"),
        &serde_json::json!({"exitCode": status.code()}),
    )?;
    if !status.success() || !product_stderr.is_empty() {
        return Err(format!(
            "packaged benchmark query exited {:?}; stdout={} stderr={}",
            status.code(),
            String::from_utf8_lossy(&product_stdout),
            String::from_utf8_lossy(&product_stderr)
        ));
    }
    parse_canonical_product_json(&product_stdout, "packaged benchmark query")
}

pub(super) fn python_identity(python: &Path, script: &Path, host: &Value) -> Result<Value, String> {
    let python_bytes = fs::read(python)
        .map_err(|error| format!("cannot hash pinned Python {}: {error}", python.display()))?;
    let script_bytes = fs::read(script)
        .map_err(|error| format!("cannot hash process helper {}: {error}", script.display()))?;
    Ok(serde_json::json!({
        "pythonExecutable": python.to_string_lossy(),
        "pythonVersion": required_string(host, "/pythonVersion")?,
        "pythonSha256": super::sha256_hex(&python_bytes),
        "measurementScript": "tools/xtask/benchmark/measure-process.py",
        "measurementScriptSha256": super::sha256_hex(&script_bytes),
    }))
}

pub(super) fn arguments(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

fn parse_canonical_product_json(bytes: &[u8], label: &str) -> Result<Value, String> {
    let Some(body) = bytes.strip_suffix(b"\n") else {
        return Err(format!("{label} did not end with one transport newline"));
    };
    if body.is_empty() || body.contains(&b'\n') || body.contains(&b'\r') {
        return Err(format!(
            "{label} did not return one compact JSON value and one newline"
        ));
    }
    let value = serde_json::from_slice::<Value>(body)
        .map_err(|error| format!("{label} returned invalid JSON: {error}"))?;
    Ok(value)
}

fn read_json(path: &Path, label: &str) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|error| format!("cannot read {label}: {error}"))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("{label} is invalid JSON: {error}"))
}

fn read_measurement(path: &Path, diagnostic: bool) -> Result<Value, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("cannot read process measurement: {error}"))?;
    let raw: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid process measurement: {error}"))?;
    let mut canonical = serde_json::to_vec(&raw).map_err(|error| error.to_string())?;
    canonical.push(b'\n');
    if bytes != canonical {
        return Err("noncanonical or duplicate-key process measurement".to_owned());
    }
    let mut fields = vec![
        "schemaVersion",
        "analysisChildPids",
        "elapsedNanoseconds",
        "exitCode",
        "observerResolutionNanoseconds",
        "peakRssBytes",
        "rssSource",
    ];
    if diagnostic {
        fields.push("processId");
    }
    let object = raw
        .as_object()
        .ok_or("process measurement is not an object")?;
    if object.len() != fields.len() || fields.iter().any(|key| !object.contains_key(*key)) {
        return Err("process measurement has missing or opaque fields".to_owned());
    }
    if required_u64(&raw, "/elapsedNanoseconds")? == 0
        || required_u64(&raw, "/observerResolutionNanoseconds")? != 1_000_000
        || required_u64(&raw, "/peakRssBytes")? == 0
        || !matches!(
            required_string(&raw, "/rssSource")?,
            "GetProcessMemoryInfo.PeakWorkingSetSize" | "wait4-rusage-ru_maxrss-kib"
        )
    {
        return Err("unexpected process measurement observation".to_owned());
    }
    Ok(raw)
}

fn require_helper_success(output: &std::process::Output, label: &str) -> Result<(), String> {
    if !output.status.success() || !output.stderr.is_empty() || !output.stdout.is_empty() {
        return Err(format!(
            "{label} exited {:?}; stdout={} stderr={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

fn required_string<'a>(value: &'a Value, pointer: &str) -> Result<&'a str, String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("JSON document omitted string {pointer}"))
}

fn required_u64(value: &Value, pointer: &str) -> Result<u64, String> {
    value
        .pointer(pointer)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("JSON document omitted integer {pointer}"))
}
