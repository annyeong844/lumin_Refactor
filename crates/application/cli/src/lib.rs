mod retention;

use std::ffi::OsString;
use std::path::Path;

use lumin_engine::{
    AbandonGateRequest, AuditRequest, EngineError, GateDecision, GateOperationResult,
    PostWriteRequest, PreWriteRequest,
};
use lumin_model::{
    GateId, OperationId, RepoPath, ResolutionProfile, RoleOverride, RunId, ScanRole,
};
use lumin_protocol::ProtocolError;
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Error)]
enum CliError {
    #[error("missing command")]
    MissingCommand,
    #[error("unknown command or argument: {0}")]
    UnknownArgument(String),
    #[error("missing value for {0}")]
    MissingValue(String),
    #[error("argument is not valid UTF-8: {0}")]
    NonUtf8(String),
    #[error("invalid worker count: {0}")]
    InvalidJobs(String),
    #[error("invalid Unix millisecond timestamp: {0}")]
    InvalidTimestamp(String),
    #[error("unsupported output format: {0}")]
    UnsupportedFormat(String),
    #[error("unknown source role: {0}")]
    UnknownRole(String),
    #[error("unknown resolution profile: {0}")]
    UnknownResolutionProfile(String),
    #[error("--run is required")]
    RunRequired,
    #[error("only --area dead-code is available in this slice")]
    InvalidArea,
    #[error("no completed run exists for this repository")]
    NoCompletedRun,
    #[error("identifier must not be empty: {0}")]
    EmptyIdentifier(String),
    #[error("abandon reason must not be empty")]
    EmptyReason,
    #[error("invalid repository path: {0}")]
    InvalidRepoPath(String),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
}

pub fn execute(root: &Path, arguments: Vec<OsString>) -> CommandOutput {
    match execute_inner(root, arguments) {
        Ok(output) => output,
        Err(error) => CommandOutput {
            exit_code: error_exit_code(&error),
            stdout: String::new(),
            stderr: format!("lumin: {error}\n"),
        },
    }
}

struct CommandSuccess {
    exit_code: i32,
    stdout: String,
}

impl From<CommandSuccess> for CommandOutput {
    fn from(success: CommandSuccess) -> Self {
        Self {
            exit_code: success.exit_code,
            stdout: success.stdout,
            stderr: String::new(),
        }
    }
}

fn execute_inner(root: &Path, arguments: Vec<OsString>) -> Result<CommandOutput, CliError> {
    let mut arguments = Arguments::new(arguments);
    let command = arguments
        .next_utf8("command")?
        .ok_or(CliError::MissingCommand)?;
    match command.as_str() {
        "audit" => audit(root, &mut arguments).map(success),
        "overview" => overview(root, &mut arguments).map(success),
        "findings" => findings(root, &mut arguments).map(success),
        "pre-write" => pre_write(root, &mut arguments),
        "post-write" => post_write(root, &mut arguments),
        "gate" => gate(root, &mut arguments),
        "operation" => operation(root, &mut arguments),
        "runs" => retention::runs(root, &mut arguments),
        _ => Err(CliError::UnknownArgument(command)),
    }
}

fn success(stdout: String) -> CommandOutput {
    CommandSuccess {
        exit_code: 0,
        stdout,
    }
    .into()
}

fn audit(root: &Path, arguments: &mut Arguments) -> Result<String, CliError> {
    let mut includes = Vec::new();
    let mut excludes = Vec::new();
    let mut role_overrides = Vec::new();
    let mut jobs = std::thread::available_parallelism().map_or(1, usize::from);
    let mut resolution_profile = None;
    let mut format = "json".to_owned();

    while let Some(argument) = arguments.next_utf8("audit argument")? {
        match argument.as_str() {
            "--include" => includes.push(arguments.required_utf8("--include")?),
            "--exclude" => excludes.push(arguments.required_utf8("--exclude")?),
            "--role-at" => {
                let pattern = arguments.required_utf8("--role-at pattern")?;
                let role = parse_role(&arguments.required_utf8("--role-at role")?)?;
                role_overrides.push(RoleOverride { pattern, role });
            }
            "--jobs" => {
                let value = arguments.required_utf8("--jobs")?;
                jobs = value
                    .parse::<usize>()
                    .ok()
                    .filter(|value| *value > 0)
                    .ok_or_else(|| CliError::InvalidJobs(value.clone()))?;
            }
            "--format" => format = arguments.required_utf8("--format")?,
            "--resolution-profile" => {
                resolution_profile = Some(parse_resolution_profile(
                    &arguments.required_utf8("--resolution-profile")?,
                )?);
            }
            _ => return Err(CliError::UnknownArgument(argument)),
        }
    }
    require_json(&format)?;

    let result = lumin_engine::audit(&AuditRequest {
        root: root.to_path_buf(),
        includes,
        excludes,
        role_overrides,
        jobs,
        resolution_profile,
    })?;
    let response = lumin_protocol::audit_response(
        result.published.attempt_id,
        result.published.run_id,
        result.published.sequence,
        &result.evidence,
    );
    lumin_protocol::to_json(&response).map_err(Into::into)
}

fn overview(root: &Path, arguments: &mut Arguments) -> Result<String, CliError> {
    let mut run_id = None;
    let mut format = "json".to_owned();
    while let Some(argument) = arguments.next_utf8("overview argument")? {
        match argument.as_str() {
            "--run" => {
                run_id = Some(RunId::from_string(arguments.required_utf8("--run")?));
            }
            "--format" => format = arguments.required_utf8("--format")?,
            _ => return Err(CliError::UnknownArgument(argument)),
        }
    }
    require_json(&format)?;

    match run_id {
        Some(run_id) => match lumin_engine::lookup_run(root, &run_id)? {
            lumin_engine::RecordLookup::Live((record, evidence)) => {
                lumin_protocol::to_json(&lumin_protocol::overview_response(
                    record.attempt_id,
                    record.run_id,
                    record.sequence,
                    &evidence,
                ))
                .map_err(Into::into)
            }
            lumin_engine::RecordLookup::Pruning(tombstone) => {
                lumin_protocol::to_json(&lumin_protocol::LookupTombstoneResponseDto::Pruning {
                    tombstone,
                })
                .map_err(Into::into)
            }
            lumin_engine::RecordLookup::Pruned(tombstone) => {
                lumin_protocol::to_json(&lumin_protocol::LookupTombstoneResponseDto::Pruned {
                    tombstone,
                })
                .map_err(Into::into)
            }
        },
        None => {
            let (record, evidence) =
                lumin_engine::load_latest_run(root)?.ok_or(CliError::NoCompletedRun)?;
            lumin_protocol::to_json(&lumin_protocol::overview_response(
                record.attempt_id,
                record.run_id,
                record.sequence,
                &evidence,
            ))
            .map_err(Into::into)
        }
    }
}

fn findings(root: &Path, arguments: &mut Arguments) -> Result<String, CliError> {
    let mut run_id = None;
    let mut cursor = None;
    let mut area = None;
    let mut format = "json".to_owned();
    while let Some(argument) = arguments.next_utf8("findings argument")? {
        match argument.as_str() {
            "--run" => {
                run_id = Some(RunId::from_string(arguments.required_utf8("--run")?));
            }
            "--cursor" => cursor = Some(arguments.required_utf8("--cursor")?),
            "--area" => area = Some(arguments.required_utf8("--area")?),
            "--format" => format = arguments.required_utf8("--format")?,
            _ => return Err(CliError::UnknownArgument(argument)),
        }
    }
    require_json(&format)?;
    if area.as_deref() != Some("dead-code") {
        return Err(CliError::InvalidArea);
    }
    let run_id = run_id.ok_or(CliError::RunRequired)?;
    let (_, evidence) = lumin_engine::load_run(root, &run_id)?;
    let response = lumin_protocol::findings_response(run_id, &evidence, cursor.as_deref())?;
    lumin_protocol::to_json(&response).map_err(Into::into)
}

fn pre_write(root: &Path, arguments: &mut Arguments) -> Result<CommandOutput, CliError> {
    let mut operation_id = None;
    let mut paths = Vec::new();
    let mut jobs = std::thread::available_parallelism().map_or(1, usize::from);
    let mut resolution_profile = None;
    let mut format = "json".to_owned();
    while let Some(argument) = arguments.next_utf8("pre-write argument")? {
        match argument.as_str() {
            "--operation-id" => {
                operation_id = Some(parse_operation_id(
                    arguments.required_utf8("--operation-id")?,
                )?);
            }
            "--path" => {
                let value = arguments.required_utf8("--path")?;
                paths.push(
                    RepoPath::from_portable(&value)
                        .map_err(|error| CliError::InvalidRepoPath(error.to_string()))?,
                );
            }
            "--jobs" => {
                let value = arguments.required_utf8("--jobs")?;
                jobs = value
                    .parse::<usize>()
                    .ok()
                    .filter(|jobs| *jobs > 0)
                    .ok_or_else(|| CliError::InvalidJobs(value.clone()))?;
            }
            "--resolution-profile" => {
                resolution_profile = Some(parse_resolution_profile(
                    &arguments.required_utf8("--resolution-profile")?,
                )?);
            }
            "--format" => format = arguments.required_utf8("--format")?,
            _ => return Err(CliError::UnknownArgument(argument)),
        }
    }
    require_json(&format)?;
    let operation_id =
        operation_id.ok_or_else(|| CliError::MissingValue("--operation-id".into()))?;
    let result = lumin_engine::open_write_gate(&PreWriteRequest {
        root: root.to_path_buf(),
        operation_id,
        paths,
        jobs,
        resolution_profile,
    })?;
    gate_command_output(&result)
}

fn post_write(root: &Path, arguments: &mut Arguments) -> Result<CommandOutput, CliError> {
    let gate_id = parse_gate_id(arguments.required_utf8("gate-id")?)?;
    let mut operation_id = None;
    let mut format = "json".to_owned();
    while let Some(argument) = arguments.next_utf8("post-write argument")? {
        match argument.as_str() {
            "--operation-id" => {
                operation_id = Some(parse_operation_id(
                    arguments.required_utf8("--operation-id")?,
                )?);
            }
            "--format" => format = arguments.required_utf8("--format")?,
            _ => return Err(CliError::UnknownArgument(argument)),
        }
    }
    require_json(&format)?;
    let operation_id =
        operation_id.ok_or_else(|| CliError::MissingValue("--operation-id".into()))?;
    let result = lumin_engine::close_write_gate(&PostWriteRequest {
        root: root.to_path_buf(),
        gate_id,
        operation_id,
    })?;
    gate_command_output(&result)
}

fn gate(root: &Path, arguments: &mut Arguments) -> Result<CommandOutput, CliError> {
    let subcommand = arguments
        .next_utf8("gate subcommand")?
        .ok_or(CliError::MissingCommand)?;
    match subcommand.as_str() {
        "show" => gate_show(root, arguments),
        "abandon" => gate_abandon(root, arguments),
        "prune" => retention::gate_prune(root, arguments),
        _ => Err(CliError::UnknownArgument(subcommand)),
    }
}

fn gate_show(root: &Path, arguments: &mut Arguments) -> Result<CommandOutput, CliError> {
    let gate_id = parse_gate_id(arguments.required_utf8("gate-id")?)?;
    let format = parse_read_format(arguments, "gate show argument")?;
    require_json(&format)?;
    let gate = lumin_engine::lookup_gate(root, &gate_id)?;
    let response = lumin_protocol::gate_lookup_response(gate);
    lumin_protocol::to_json(&response)
        .map(success)
        .map_err(Into::into)
}

fn gate_abandon(root: &Path, arguments: &mut Arguments) -> Result<CommandOutput, CliError> {
    let gate_id = parse_gate_id(arguments.required_utf8("gate-id")?)?;
    let mut operation_id = None;
    let mut reason = None;
    let mut format = "json".to_owned();
    while let Some(argument) = arguments.next_utf8("gate abandon argument")? {
        match argument.as_str() {
            "--operation-id" => {
                operation_id = Some(parse_operation_id(
                    arguments.required_utf8("--operation-id")?,
                )?);
            }
            "--reason" => reason = Some(arguments.required_utf8("--reason")?),
            "--format" => format = arguments.required_utf8("--format")?,
            _ => return Err(CliError::UnknownArgument(argument)),
        }
    }
    require_json(&format)?;
    let operation_id =
        operation_id.ok_or_else(|| CliError::MissingValue("--operation-id".into()))?;
    let reason = reason.ok_or_else(|| CliError::MissingValue("--reason".into()))?;
    if reason.is_empty() {
        return Err(CliError::EmptyReason);
    }
    let result = lumin_engine::abandon_gate(&AbandonGateRequest {
        root: root.to_path_buf(),
        gate_id,
        operation_id,
        reason,
    })?;
    gate_command_output(&result)
}

fn operation(root: &Path, arguments: &mut Arguments) -> Result<CommandOutput, CliError> {
    let subcommand = arguments
        .next_utf8("operation subcommand")?
        .ok_or(CliError::MissingCommand)?;
    if subcommand != "show" {
        return Err(CliError::UnknownArgument(subcommand));
    }
    let operation_id = parse_operation_id(arguments.required_utf8("operation-id")?)?;
    let format = parse_read_format(arguments, "operation show argument")?;
    require_json(&format)?;
    let operation = lumin_engine::load_lifecycle_operation(root, &operation_id)?;
    let response = lumin_protocol::lifecycle_operation_response(&operation);
    lumin_protocol::to_json(&response)
        .map(success)
        .map_err(Into::into)
}

fn gate_command_output(result: &GateOperationResult) -> Result<CommandOutput, CliError> {
    let response = lumin_protocol::gate_mutation_response(result);
    let stdout = lumin_protocol::to_json(&response)?;
    Ok(CommandSuccess {
        exit_code: decision_exit_code(result.decision),
        stdout,
    }
    .into())
}

fn parse_read_format(arguments: &mut Arguments, name: &str) -> Result<String, CliError> {
    let mut format = "json".to_owned();
    while let Some(argument) = arguments.next_utf8(name)? {
        match argument.as_str() {
            "--format" => format = arguments.required_utf8("--format")?,
            _ => return Err(CliError::UnknownArgument(argument)),
        }
    }
    Ok(format)
}

fn parse_operation_id(value: String) -> Result<OperationId, CliError> {
    if value.is_empty() {
        Err(CliError::EmptyIdentifier("operation-id".to_owned()))
    } else {
        Ok(OperationId::from_string(value))
    }
}

fn parse_gate_id(value: String) -> Result<GateId, CliError> {
    if value.is_empty() {
        Err(CliError::EmptyIdentifier("gate-id".to_owned()))
    } else {
        Ok(GateId::from_string(value))
    }
}

fn decision_exit_code(decision: GateDecision) -> i32 {
    match decision {
        GateDecision::Allow | GateDecision::AllowWithWarnings => 0,
        GateDecision::Deny => 3,
        GateDecision::Incomplete => 4,
        GateDecision::Stale => 5,
    }
}

fn parse_role(value: &str) -> Result<ScanRole, CliError> {
    match value {
        "test" => Ok(ScanRole::Test),
        "production" => Ok(ScanRole::Production),
        "generated" => Ok(ScanRole::Generated),
        "vendor" => Ok(ScanRole::Vendor),
        "authored" => Ok(ScanRole::Authored),
        _ => Err(CliError::UnknownRole(value.to_owned())),
    }
}

fn parse_resolution_profile(value: &str) -> Result<ResolutionProfile, CliError> {
    match value {
        "bundler" => Ok(ResolutionProfile::Bundler),
        "node" | "node10" => Ok(ResolutionProfile::Node),
        "node16" => Ok(ResolutionProfile::Node16),
        "nodenext" => Ok(ResolutionProfile::NodeNext),
        _ => Err(CliError::UnknownResolutionProfile(value.to_owned())),
    }
}

fn require_json(value: &str) -> Result<(), CliError> {
    if value == "json" {
        Ok(())
    } else {
        Err(CliError::UnsupportedFormat(value.to_owned()))
    }
}

fn error_exit_code(error: &CliError) -> i32 {
    match error {
        CliError::Protocol(ProtocolError::CursorStale) => 5,
        CliError::MissingCommand
        | CliError::UnknownArgument(_)
        | CliError::MissingValue(_)
        | CliError::NonUtf8(_)
        | CliError::InvalidJobs(_)
        | CliError::InvalidTimestamp(_)
        | CliError::UnsupportedFormat(_)
        | CliError::UnknownRole(_)
        | CliError::UnknownResolutionProfile(_)
        | CliError::RunRequired
        | CliError::InvalidArea
        | CliError::NoCompletedRun
        | CliError::EmptyIdentifier(_)
        | CliError::EmptyReason
        | CliError::InvalidRepoPath(_)
        | CliError::Protocol(_) => 2,
        CliError::Engine(error) => error.lifecycle_exit_code(),
    }
}

struct Arguments {
    values: std::vec::IntoIter<OsString>,
}

impl Arguments {
    fn new(values: Vec<OsString>) -> Self {
        Self {
            values: values.into_iter(),
        }
    }

    fn next_utf8(&mut self, name: &str) -> Result<Option<String>, CliError> {
        self.values
            .next()
            .map(|value| {
                value.into_string().map_err(|value| {
                    CliError::NonUtf8(format!("{name}: {}", value.to_string_lossy()))
                })
            })
            .transpose()
    }

    fn required_utf8(&mut self, name: &str) -> Result<String, CliError> {
        self.next_utf8(name)?
            .ok_or_else(|| CliError::MissingValue(name.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::Value;

    use super::*;

    #[test]
    fn audit_then_findings_reopens_the_persisted_run() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::write(root.path().join("lib.ts"), "export const dead = 1;")?;
        let audit = execute(
            root.path(),
            vec!["audit".into(), "--jobs".into(), "1".into()],
        );
        assert_eq!(audit.exit_code, 0, "{}", audit.stderr);
        let audit_json: Value = serde_json::from_str(&audit.stdout)?;
        let run_id = audit_json
            .get("runId")
            .and_then(Value::as_str)
            .ok_or("audit response omitted runId")?;

        let findings = execute(
            root.path(),
            vec![
                "findings".into(),
                "--run".into(),
                run_id.into(),
                "--area".into(),
                "dead-code".into(),
            ],
        );
        assert_eq!(findings.exit_code, 0, "{}", findings.stderr);
        let findings_json: Value = serde_json::from_str(&findings.stdout)?;
        assert_eq!(findings_json.get("filters"), Some(&serde_json::json!({})));
        assert_eq!(
            findings_json.get("scopeTotal").and_then(Value::as_u64),
            Some(1)
        );
        assert_eq!(findings_json.get("total").and_then(Value::as_u64), Some(1));
        Ok(())
    }

    #[test]
    fn unfiltered_query_keeps_review_only_findings() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("src"))?;
        fs::write(
            root.path().join("lumin.json"),
            r#"{"schemaVersion":"lumin-config.v1","scan":{"roles":[{"pattern":"src/vendor.ts","role":"vendor"}]}}"#,
        )?;
        fs::write(
            root.path().join("src/authored.ts"),
            "export const authored = 1;",
        )?;
        fs::write(
            root.path().join("src/generated.ts"),
            "// @generated\nexport const generated = 1;",
        )?;
        fs::write(
            root.path().join("src/vendor.ts"),
            "export const vendor = 1;",
        )?;
        let audit = execute(
            root.path(),
            vec!["audit".into(), "--jobs".into(), "2".into()],
        );
        assert_eq!(audit.exit_code, 0, "{}", audit.stderr);
        let audit_json: Value = serde_json::from_str(&audit.stdout)?;
        let run_id = audit_json
            .get("runId")
            .and_then(Value::as_str)
            .ok_or("audit response omitted runId")?;
        let findings = execute(
            root.path(),
            vec![
                "findings".into(),
                "--run".into(),
                run_id.into(),
                "--area".into(),
                "dead-code".into(),
            ],
        );
        assert_eq!(findings.exit_code, 0, "{}", findings.stderr);
        let response: Value = serde_json::from_str(&findings.stdout)?;
        assert_eq!(response.get("filters"), Some(&serde_json::json!({})));
        assert_eq!(response.get("scopeTotal").and_then(Value::as_u64), Some(3));
        assert_eq!(response.get("total").and_then(Value::as_u64), Some(3));
        let review_only = response
            .get("items")
            .and_then(Value::as_array)
            .ok_or("findings response omitted items")?
            .iter()
            .filter(|item| {
                item.pointer("/disposition/kind").and_then(Value::as_str) == Some("review-only")
            })
            .count();
        assert_eq!(review_only, 2);
        Ok(())
    }

    #[test]
    fn parse_failure_is_persisted_as_incomplete() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::write(root.path().join("broken.ts"), "export const = ;")?;
        let audit = execute(root.path(), vec!["audit".into()]);
        assert_eq!(audit.exit_code, 0, "{}", audit.stderr);
        let response: Value = serde_json::from_str(&audit.stdout)?;
        assert_eq!(
            response.get("status").and_then(Value::as_str),
            Some("incomplete")
        );
        assert_eq!(
            response.get("findingCount").and_then(Value::as_u64),
            Some(0)
        );
        assert!(
            response
                .get("limitationCount")
                .and_then(Value::as_u64)
                .is_some_and(|count| count > 0)
        );
        Ok(())
    }

    #[test]
    fn resolution_profile_override_is_validated_and_persisted()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("src"))?;
        fs::write(
            root.path().join("package.json"),
            r#"{"name":"app","type":"module"}"#,
        )?;
        fs::write(
            root.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"moduleResolution":"node16"}}"#,
        )?;
        fs::write(root.path().join("src/lib.ts"), "export const used = 1;")?;
        fs::write(
            root.path().join("src/main.ts"),
            "import { used } from './lib'; console.log(used);",
        )?;

        let invalid = execute(
            root.path(),
            vec![
                "audit".into(),
                "--resolution-profile".into(),
                "browser".into(),
            ],
        );
        assert_eq!(invalid.exit_code, 2);
        assert!(
            invalid
                .stderr
                .contains("unknown resolution profile: browser")
        );

        let audit = execute(
            root.path(),
            vec![
                "audit".into(),
                "--jobs".into(),
                "1".into(),
                "--resolution-profile".into(),
                "node10".into(),
            ],
        );
        assert_eq!(audit.exit_code, 0, "{}", audit.stderr);
        let audit_json: Value = serde_json::from_str(&audit.stdout)?;
        let run_id = audit_json
            .get("runId")
            .and_then(Value::as_str)
            .ok_or("audit response omitted runId")?;
        let overview = execute(
            root.path(),
            vec!["overview".into(), "--run".into(), run_id.into()],
        );
        assert_eq!(overview.exit_code, 0, "{}", overview.stderr);
        let overview_json: Value = serde_json::from_str(&overview.stdout)?;
        let profiles = overview_json
            .get("resolutionProfiles")
            .and_then(Value::as_array)
            .ok_or("overview omitted resolutionProfiles")?;
        assert!(!profiles.is_empty());
        assert!(profiles.iter().all(|profile| {
            profile.get("profile").and_then(Value::as_str) == Some("node")
                && profile.pointer("/source/kind").and_then(Value::as_str) == Some("invocation")
        }));
        Ok(())
    }
}
