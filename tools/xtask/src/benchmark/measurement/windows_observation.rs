//! Closed validation of the helper-owned Windows job receipt, not product evidence.

use std::fs;
use std::path::Path;

use serde_json::Value;

pub(super) const COMPANION: &str = "windows-process-observation.json";

struct Accounting {
    total_processes: u32,
    active_processes: u32,
    total_terminated_processes: u32,
}

struct Observation {
    schema_version: String,
    method: String,
    helper_process_id: u32,
    process_id: u32,
    helper_creation_time100ns: u64,
    process_creation_time100ns: u64,
    process_exit_time100ns: u64,
    helper_in_job: bool,
    process_in_job: bool,
    limit_flags_before: u32,
    limit_flags_after: u32,
    before: Accounting,
    after: Accounting,
}

fn closed(value: &Value, fields: &[&str]) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or("Windows observation must be an object")?;
    if object.len() != fields.len() || fields.iter().any(|field| !object.contains_key(*field)) {
        return Err("Windows observation has missing or opaque fields".into());
    }
    Ok(())
}

fn integer(value: &Value, field: &str) -> Result<u64, String> {
    value[field]
        .as_u64()
        .ok_or_else(|| format!("Windows observation omitted unsigned integer {field}"))
}

fn dword(value: &Value, field: &str) -> Result<u32, String> {
    u32::try_from(integer(value, field)?)
        .map_err(|error| format!("invalid Windows DWORD {field}: {error}"))
}

fn boolean(value: &Value, field: &str) -> Result<bool, String> {
    value[field]
        .as_bool()
        .ok_or_else(|| format!("Windows observation omitted boolean {field}"))
}

fn string(value: &Value, field: &str) -> Result<String, String> {
    value[field]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("Windows observation omitted string {field}"))
}

impl Accounting {
    fn decode(value: &Value) -> Result<Self, String> {
        closed(
            value,
            &[
                "totalProcesses",
                "activeProcesses",
                "totalTerminatedProcesses",
            ],
        )?;
        Ok(Self {
            total_processes: dword(value, "totalProcesses")?,
            active_processes: dword(value, "activeProcesses")?,
            total_terminated_processes: dword(value, "totalTerminatedProcesses")?,
        })
    }
}

impl Observation {
    fn decode(value: &Value) -> Result<Self, String> {
        closed(
            value,
            &[
                "schemaVersion",
                "method",
                "helperProcessId",
                "processId",
                "helperCreationTime100ns",
                "processCreationTime100ns",
                "processExitTime100ns",
                "helperInJob",
                "processInJob",
                "limitFlagsBefore",
                "limitFlagsAfter",
                "before",
                "after",
            ],
        )?;
        Ok(Self {
            schema_version: string(value, "schemaVersion")?,
            method: string(value, "method")?,
            helper_process_id: dword(value, "helperProcessId")?,
            process_id: dword(value, "processId")?,
            helper_creation_time100ns: integer(value, "helperCreationTime100ns")?,
            process_creation_time100ns: integer(value, "processCreationTime100ns")?,
            process_exit_time100ns: integer(value, "processExitTime100ns")?,
            helper_in_job: boolean(value, "helperInJob")?,
            process_in_job: boolean(value, "processInJob")?,
            limit_flags_before: dword(value, "limitFlagsBefore")?,
            limit_flags_after: dword(value, "limitFlagsAfter")?,
            before: Accounting::decode(&value["before"])?,
            after: Accounting::decode(&value["after"])?,
        })
    }
}

pub(super) fn validate_capture(
    capture: &Path,
    helper_pid: u32,
    measurement: &Value,
    diagnostic: bool,
    windows: bool,
) -> Result<(), String> {
    let path = capture.join(COMPANION);
    if !windows {
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot inspect Windows process companion: {error}")),
            Ok(_) => return Err("unexpected Windows process companion on non-Windows".into()),
        }
        if measurement["rssSource"] != "wait4-rusage-ru_maxrss-kib" {
            return Err("unexpected non-Windows RSS source".into());
        }
        return Ok(());
    }
    let bytes = fs::read(path)
        .map_err(|error| format!("cannot read Windows process companion: {error}"))?;
    validate(&bytes, helper_pid, measurement, diagnostic)
}

pub(super) fn validate(
    bytes: &[u8],
    helper_pid: u32,
    measurement: &Value,
    diagnostic: bool,
) -> Result<(), String> {
    let raw: Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("invalid Windows process companion: {error}"))?;
    let mut canonical = serde_json::to_vec(&raw).map_err(|error| error.to_string())?;
    canonical.push(b'\n');
    if bytes != canonical {
        return Err("noncanonical or duplicate-key Windows process companion".into());
    }
    let observed = Observation::decode(&raw)?;
    if observed.schema_version != "lumin.windows-process-observation.v1"
        || observed.method != "private-inherited-job.v1"
        || helper_pid == 0
        || observed.helper_process_id != helper_pid
        || observed.process_id == 0
        || observed.process_id == helper_pid
        || !observed.helper_in_job
        || !observed.process_in_job
        || observed.helper_creation_time100ns == 0
        || observed.helper_creation_time100ns > observed.process_creation_time100ns
        || observed.process_creation_time100ns > observed.process_exit_time100ns
        || observed.limit_flags_before != 0
        || observed.limit_flags_after != 0
        || observed.before.total_processes != 1
        || observed.before.active_processes != 1
        || observed.before.total_terminated_processes != 0
        || observed.after.total_processes != 2
        || !(1..=2).contains(&observed.after.active_processes)
        || observed.after.total_terminated_processes != 0
        || measurement["rssSource"] != "GetProcessMemoryInfo.PeakWorkingSetSize"
        || (diagnostic && measurement["processId"].as_u64() != Some(u64::from(observed.process_id)))
    {
        return Err("contradictory Windows process companion or launcher identity".into());
    }
    Ok(())
}
