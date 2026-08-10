mod query;
mod retention;

use std::ffi::OsString;
use std::io::Read;
use std::num::NonZeroUsize;
use std::path::Path;

use lumin_engine::{
    AbandonGateRequest, AuditRequest, EngineError, GateDecision, GateOperationResult,
    PostWriteRequest, PreWriteRequest,
};
use lumin_model::{
    BuildIdentity, FindingId, GateId, OperationId, RepoPath, ResolutionProfile, RoleOverride,
    RunId, ScanRole,
};
use lumin_protocol::ProtocolError;
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub result_delivery: CommandResultDelivery,
}

impl CommandOutput {
    pub const fn delivery_failure_exit_code(&self) -> i32 {
        match self.result_delivery {
            CommandResultDelivery::ReadOnly => self.exit_code,
            CommandResultDelivery::RecoverableMutation => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandResultDelivery {
    ReadOnly,
    RecoverableMutation,
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
    #[error("--revision is required")]
    RevisionRequired,
    #[error("invalid gate revision: {0}")]
    InvalidRevision(String),
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
    #[error("--paths0-from may be provided only once")]
    DuplicatePaths0From,
    #[error("--paths0-from supports only stdin ('-')")]
    InvalidPaths0Source,
    #[error("cannot read --paths0-from stdin: {0}")]
    Paths0Read(String),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
}

pub fn execute(root: &Path, arguments: Vec<OsString>) -> CommandOutput {
    let build_identity = match default_build_identity() {
        Ok(identity) => identity,
        Err(error) => {
            return CommandOutput {
                exit_code: error_exit_code(&error),
                stdout: String::new(),
                stderr: format!("lumin: {error}\n"),
                result_delivery: CommandResultDelivery::ReadOnly,
            };
        }
    };
    let mut stdin = std::io::stdin().lock();
    execute_with_input(root, arguments, &build_identity, &mut stdin)
}

/// Execute with an explicit BuildIdentity, for testing cross-build cursor rejection.
pub fn execute_with_build_identity(
    root: &Path,
    arguments: Vec<OsString>,
    build_identity: &BuildIdentity,
) -> CommandOutput {
    let mut input = std::io::empty();
    execute_with_input(root, arguments, build_identity, &mut input)
}

fn execute_with_input(
    root: &Path,
    arguments: Vec<OsString>,
    build_identity: &BuildIdentity,
    input: &mut dyn Read,
) -> CommandOutput {
    match execute_inner(root, arguments, build_identity, input) {
        Ok(output) => output,
        Err(error) => CommandOutput {
            exit_code: error_exit_code(&error),
            stdout: String::new(),
            stderr: format!("lumin: {error}\n"),
            result_delivery: CommandResultDelivery::ReadOnly,
        },
    }
}

fn default_build_identity() -> Result<BuildIdentity, CliError> {
    let registry = lumin_engine::compiled_capability_registry()?;
    let revision = option_env!("LUMIN_BUILD_REVISION")
        .filter(|value| !value.is_empty())
        .or_else(|| option_env!("GITHUB_SHA").filter(|value| !value.is_empty()));
    Ok(BuildIdentity::derive(
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        revision,
        registry.contract_digest(),
    ))
}

struct CommandSuccess {
    exit_code: i32,
    stdout: String,
    result_delivery: CommandResultDelivery,
}

impl From<CommandSuccess> for CommandOutput {
    fn from(success: CommandSuccess) -> Self {
        Self {
            exit_code: success.exit_code,
            stdout: success.stdout,
            stderr: String::new(),
            result_delivery: success.result_delivery,
        }
    }
}

fn execute_inner(
    root: &Path,
    arguments: Vec<OsString>,
    build_identity: &BuildIdentity,
    input: &mut dyn Read,
) -> Result<CommandOutput, CliError> {
    let mut arguments = Arguments::new(arguments);
    let command = arguments
        .next_utf8("command")?
        .ok_or(CliError::MissingCommand)?;
    match command.as_str() {
        "audit" => audit(root, &mut arguments).map(success),
        "overview" => overview(root, &mut arguments).map(success),
        "findings" => findings(root, &mut arguments).map(success),
        "explain" => explain(root, &mut arguments).map(success),
        "related" => query::related(root, &mut arguments).map(success),
        "files" => query::files(root, &mut arguments).map(success),
        "capabilities" => capabilities(root, &mut arguments, build_identity).map(success),
        "pre-write" => pre_write(root, &mut arguments, input),
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
        result_delivery: CommandResultDelivery::ReadOnly,
    }
    .into()
}

fn compute_default_jobs(available: Option<NonZeroUsize>) -> usize {
    available.map_or(1, |value| value.get().min(8))
}

/// Falls back to one worker when the quota-aware runtime observation is unavailable.
fn default_jobs() -> usize {
    compute_default_jobs(std::thread::available_parallelism().ok())
}

fn audit(root: &Path, arguments: &mut Arguments) -> Result<String, CliError> {
    let mut includes = Vec::new();
    let mut excludes = Vec::new();
    let mut role_overrides = Vec::new();
    let mut entries = Vec::new();
    let mut jobs = default_jobs();
    let mut resolution_profile = None;
    let mut format = "json".to_owned();

    while let Some(argument) = arguments.next_utf8("audit argument")? {
        match argument.as_str() {
            "--include" => includes.push(arguments.required_utf8("--include")?),
            "--exclude" => excludes.push(arguments.required_utf8("--exclude")?),
            "--entry" => {
                let value = arguments.required_os("--entry")?;
                entries.push(
                    lumin_engine::lower_native_repo_path(&value)
                        .map_err(|error| CliError::InvalidRepoPath(error.to_string()))?,
                );
            }
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
        entries,
        jobs,
        resolution_profile,
    })?;
    let response = lumin_protocol::audit_response(
        &result.repository_root,
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
            (_, lumin_engine::RecordLookup::Live((record, evidence))) => {
                lumin_protocol::to_json(&lumin_protocol::overview_response(
                    record.attempt_id,
                    record.run_id,
                    record.sequence,
                    None,
                    &evidence,
                ))
                .map_err(Into::into)
            }
            (_, lumin_engine::RecordLookup::Pruning(tombstone)) => {
                lumin_protocol::to_json(&lumin_protocol::LookupTombstoneResponseDto::Pruning {
                    tombstone,
                })
                .map_err(Into::into)
            }
            (_, lumin_engine::RecordLookup::Pruned(tombstone)) => {
                lumin_protocol::to_json(&lumin_protocol::LookupTombstoneResponseDto::Pruned {
                    tombstone,
                })
                .map_err(Into::into)
            }
        },
        None => {
            let latest = lumin_engine::load_latest_overview(root)?;
            let latest_attempt =
                latest
                    .latest_attempt
                    .map(|attempt| lumin_protocol::AttemptSummaryDto {
                        attempt_id: attempt.attempt_id,
                        sequence: attempt.sequence,
                        status: attempt.status,
                        failure: attempt.failure,
                    });
            match latest.completed {
                Some((record, evidence)) => {
                    lumin_protocol::to_json(&lumin_protocol::overview_response(
                        record.attempt_id,
                        record.run_id,
                        record.sequence,
                        latest_attempt,
                        &evidence,
                    ))
                    .map_err(Into::into)
                }
                None => lumin_protocol::to_json(&lumin_protocol::attempt_overview_response(
                    latest_attempt.ok_or(CliError::NoCompletedRun)?,
                ))
                .map_err(Into::into),
            }
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
    match lumin_engine::lookup_run(root, &run_id)? {
        (repository_id, lumin_engine::RecordLookup::Live((_, evidence))) => {
            let decoded_cursor = lumin_protocol::decode_run_query_cursor(cursor.as_deref())?;
            let page = lumin_engine::query_run_findings(
                &repository_id,
                &run_id,
                &evidence,
                decoded_cursor,
            )?;
            let response = lumin_protocol::run_findings_response(&page)?;
            lumin_protocol::to_json(&response).map_err(Into::into)
        }
        (_, lumin_engine::RecordLookup::Pruning(tombstone)) => {
            lumin_protocol::to_json(&lumin_protocol::LookupTombstoneResponseDto::Pruning {
                tombstone,
            })
            .map_err(Into::into)
        }
        (_, lumin_engine::RecordLookup::Pruned(tombstone)) => {
            lumin_protocol::to_json(&lumin_protocol::LookupTombstoneResponseDto::Pruned {
                tombstone,
            })
            .map_err(Into::into)
        }
    }
}

fn explain(root: &Path, arguments: &mut Arguments) -> Result<String, CliError> {
    let mut run_id = None;
    let mut finding_id = None;
    let mut evidence_cursor = None;
    let mut relations_cursor = None;
    let mut format = "json".to_owned();
    while let Some(argument) = arguments.next_utf8("explain argument")? {
        match argument.as_str() {
            "--run" => {
                run_id = Some(RunId::from_string(arguments.required_utf8("--run")?));
            }
            "--evidence-cursor" => {
                evidence_cursor = Some(arguments.required_utf8("--evidence-cursor")?);
            }
            "--relations-cursor" => {
                relations_cursor = Some(arguments.required_utf8("--relations-cursor")?);
            }
            "--format" => format = arguments.required_utf8("--format")?,
            _ if argument.starts_with("--") || finding_id.is_some() => {
                return Err(CliError::UnknownArgument(argument));
            }
            _ => finding_id = Some(parse_finding_id(argument)?),
        }
    }
    require_json(&format)?;
    let run_id = run_id.ok_or(CliError::RunRequired)?;
    let finding_id = finding_id.ok_or_else(|| CliError::MissingValue("finding-id".to_owned()))?;
    match lumin_engine::lookup_run(root, &run_id)? {
        (repository_id, lumin_engine::RecordLookup::Live((_, evidence))) => {
            let evidence_cursor =
                lumin_protocol::decode_run_query_cursor(evidence_cursor.as_deref())?;
            let relations_cursor =
                lumin_protocol::decode_run_query_cursor(relations_cursor.as_deref())?;
            let explanation = lumin_engine::query_run_explain(
                &repository_id,
                &run_id,
                &evidence,
                &finding_id,
                evidence_cursor,
                relations_cursor,
            )?;
            let response = lumin_protocol::run_explain_response(&explanation)?;
            lumin_protocol::to_json(&response).map_err(Into::into)
        }
        (_, lumin_engine::RecordLookup::Pruning(tombstone)) => {
            lumin_protocol::to_json(&lumin_protocol::LookupTombstoneResponseDto::Pruning {
                tombstone,
            })
            .map_err(Into::into)
        }
        (_, lumin_engine::RecordLookup::Pruned(tombstone)) => {
            lumin_protocol::to_json(&lumin_protocol::LookupTombstoneResponseDto::Pruned {
                tombstone,
            })
            .map_err(Into::into)
        }
    }
}

fn capabilities(
    root: &Path,
    arguments: &mut Arguments,
    build_identity: &BuildIdentity,
) -> Result<String, CliError> {
    let mut run_id = None;
    let mut cursor = None;
    let mut format = "json".to_owned();
    while let Some(argument) = arguments.next_utf8("capabilities argument")? {
        match argument.as_str() {
            "--run" => {
                run_id = Some(RunId::from_string(arguments.required_utf8("--run")?));
            }
            "--cursor" => cursor = Some(arguments.required_utf8("--cursor")?),
            "--format" => format = arguments.required_utf8("--format")?,
            _ => return Err(CliError::UnknownArgument(argument)),
        }
    }
    require_json(&format)?;

    match run_id {
        Some(run_id) => {
            // Run query: requires .lumin
            match lumin_engine::lookup_run(root, &run_id)? {
                (repository_id, lumin_engine::RecordLookup::Live((_, evidence))) => {
                    let decoded_cursor =
                        lumin_protocol::decode_run_query_cursor(cursor.as_deref())?;
                    let page = lumin_engine::query_run_capabilities(
                        &repository_id,
                        &run_id,
                        &evidence,
                        decoded_cursor,
                    )?;
                    let response = lumin_protocol::capabilities_response(&page)?;
                    lumin_protocol::to_json(&response).map_err(Into::into)
                }
                (_, lumin_engine::RecordLookup::Pruning(tombstone)) => {
                    lumin_protocol::to_json(&lumin_protocol::LookupTombstoneResponseDto::Pruning {
                        tombstone,
                    })
                    .map_err(Into::into)
                }
                (_, lumin_engine::RecordLookup::Pruned(tombstone)) => {
                    lumin_protocol::to_json(&lumin_protocol::LookupTombstoneResponseDto::Pruned {
                        tombstone,
                    })
                    .map_err(Into::into)
                }
            }
        }
        None => {
            // Binary query: never opens/creates .lumin, repository-independent
            let registry = lumin_engine::compiled_capability_registry()?;
            let decoded_cursor = lumin_protocol::decode_binary_query_cursor(cursor.as_deref())?;
            let page =
                lumin_engine::query_binary_capabilities(build_identity, &registry, decoded_cursor)?;
            let response = lumin_protocol::capabilities_response(&page)?;
            lumin_protocol::to_json(&response).map_err(Into::into)
        }
    }
}

fn pre_write(
    root: &Path,
    arguments: &mut Arguments,
    input: &mut dyn Read,
) -> Result<CommandOutput, CliError> {
    let mut operation_id = None;
    let mut paths = Vec::new();
    let mut includes = Vec::new();
    let mut excludes = Vec::new();
    let mut role_overrides = Vec::new();
    let mut entries = Vec::new();
    let mut jobs = default_jobs();
    let mut resolution_profile = None;
    let mut format = "json".to_owned();
    let mut paths0_seen = false;
    while let Some(argument) = arguments.next_utf8("pre-write argument")? {
        match argument.as_str() {
            "--operation-id" => {
                operation_id = Some(parse_operation_id(
                    arguments.required_utf8("--operation-id")?,
                )?);
            }
            "--path" => {
                let value = arguments.required_os("--path")?;
                paths.push(
                    lumin_engine::lower_native_repo_path(&value)
                        .map_err(|error| CliError::InvalidRepoPath(error.to_string()))?,
                );
            }
            "--paths0-from" => {
                if paths0_seen {
                    return Err(CliError::DuplicatePaths0From);
                }
                paths0_seen = true;
                if arguments.required_utf8("--paths0-from")? != "-" {
                    return Err(CliError::InvalidPaths0Source);
                }
                let mut bytes = Vec::new();
                input
                    .read_to_end(&mut bytes)
                    .map_err(|error| CliError::Paths0Read(error.to_string()))?;
                paths.extend(
                    lumin_engine::decode_native_repo_path_stream(&bytes)
                        .map_err(|error| CliError::InvalidRepoPath(error.to_string()))?,
                );
            }
            "--entry" => {
                let value = arguments.required_os("--entry")?;
                entries.push(
                    lumin_engine::lower_native_repo_path(&value)
                        .map_err(|error| CliError::InvalidRepoPath(error.to_string()))?,
                );
            }
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
        includes,
        excludes,
        role_overrides,
        entries,
        jobs,
        resolution_profile,
    })?;
    gate_command_output(&result)
}

fn post_write(root: &Path, arguments: &mut Arguments) -> Result<CommandOutput, CliError> {
    let gate_id = parse_gate_id(arguments.required_positional_utf8("gate-id")?)?;
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
        "findings" => gate_findings(root, arguments),
        "explain" => gate_explain(root, arguments),
        "abandon" => gate_abandon(root, arguments),
        "list" => query::gate_list(root, arguments),
        "prune" => retention::gate_prune(root, arguments),
        _ => Err(CliError::UnknownArgument(subcommand)),
    }
}

fn gate_show(root: &Path, arguments: &mut Arguments) -> Result<CommandOutput, CliError> {
    let gate_id = parse_gate_id(arguments.required_positional_utf8("gate-id")?)?;
    let mut revision = None;
    let mut format = "json".to_owned();
    while let Some(argument) = arguments.next_utf8("gate show argument")? {
        match argument.as_str() {
            "--revision" => {
                revision = Some(parse_revision(arguments.required_utf8("--revision")?)?);
            }
            "--format" => format = arguments.required_utf8("--format")?,
            _ => return Err(CliError::UnknownArgument(argument)),
        }
    }
    require_json(&format)?;
    let response = match lumin_engine::lookup_gate(root, &gate_id)? {
        (_, lumin_engine::RecordLookup::Live(gate)) => {
            let response = match revision {
                Some(revision) => lumin_protocol::gate_show_response_at(&gate, revision)?,
                None => lumin_protocol::gate_show_response(&gate),
            };
            lumin_protocol::GateLookupResponseDto::Live(response)
        }
        (_, lumin_engine::RecordLookup::Pruning(tombstone)) => {
            lumin_protocol::GateLookupResponseDto::Tombstone(
                lumin_protocol::LookupTombstoneResponseDto::Pruning { tombstone },
            )
        }
        (_, lumin_engine::RecordLookup::Pruned(tombstone)) => {
            lumin_protocol::GateLookupResponseDto::Tombstone(
                lumin_protocol::LookupTombstoneResponseDto::Pruned { tombstone },
            )
        }
    };
    lumin_protocol::to_json(&response)
        .map(success)
        .map_err(Into::into)
}

fn gate_findings(root: &Path, arguments: &mut Arguments) -> Result<CommandOutput, CliError> {
    let gate_id = parse_gate_id(arguments.required_positional_utf8("gate-id")?)?;
    let mut revision = None;
    let mut cursor = None;
    let mut format = "json".to_owned();
    while let Some(argument) = arguments.next_utf8("gate findings argument")? {
        match argument.as_str() {
            "--revision" => {
                revision = Some(parse_revision(arguments.required_utf8("--revision")?)?);
            }
            "--cursor" => cursor = Some(arguments.required_utf8("--cursor")?),
            "--format" => format = arguments.required_utf8("--format")?,
            _ => return Err(CliError::UnknownArgument(argument)),
        }
    }
    require_json(&format)?;
    let revision = revision.ok_or(CliError::RevisionRequired)?;
    let response = match lumin_engine::lookup_gate(root, &gate_id)? {
        (repository_id, lumin_engine::RecordLookup::Live(gate)) => {
            let cursor = lumin_protocol::decode_gate_query_cursor(cursor.as_deref())?;
            let page = lumin_engine::query_gate_findings(&repository_id, &gate, revision, cursor)?;
            lumin_protocol::to_json(&lumin_protocol::gate_findings_response(&page)?)?
        }
        (_, lumin_engine::RecordLookup::Pruning(tombstone)) => {
            lumin_protocol::to_json(&lumin_protocol::LookupTombstoneResponseDto::Pruning {
                tombstone,
            })?
        }
        (_, lumin_engine::RecordLookup::Pruned(tombstone)) => {
            lumin_protocol::to_json(&lumin_protocol::LookupTombstoneResponseDto::Pruned {
                tombstone,
            })?
        }
    };
    Ok(success(response))
}

fn gate_explain(root: &Path, arguments: &mut Arguments) -> Result<CommandOutput, CliError> {
    let gate_id = parse_gate_id(arguments.required_positional_utf8("gate-id")?)?;
    let mut revision = None;
    let mut finding_id = None;
    let mut evidence_cursor = None;
    let mut relations_cursor = None;
    let mut format = "json".to_owned();
    while let Some(argument) = arguments.next_utf8("gate explain argument")? {
        match argument.as_str() {
            "--revision" => {
                revision = Some(parse_revision(arguments.required_utf8("--revision")?)?);
            }
            "--evidence-cursor" => {
                evidence_cursor = Some(arguments.required_utf8("--evidence-cursor")?);
            }
            "--relations-cursor" => {
                relations_cursor = Some(arguments.required_utf8("--relations-cursor")?);
            }
            "--format" => format = arguments.required_utf8("--format")?,
            _ if argument.starts_with("--") || finding_id.is_some() => {
                return Err(CliError::UnknownArgument(argument));
            }
            _ => finding_id = Some(parse_finding_id(argument)?),
        }
    }
    require_json(&format)?;
    let revision = revision.ok_or(CliError::RevisionRequired)?;
    let finding_id = finding_id.ok_or_else(|| CliError::MissingValue("finding-id".to_owned()))?;
    let response = match lumin_engine::lookup_gate(root, &gate_id)? {
        (repository_id, lumin_engine::RecordLookup::Live(gate)) => {
            let evidence_cursor =
                lumin_protocol::decode_gate_query_cursor(evidence_cursor.as_deref())?;
            let relations_cursor =
                lumin_protocol::decode_gate_query_cursor(relations_cursor.as_deref())?;
            let explanation = lumin_engine::query_gate_explain(
                &repository_id,
                &gate,
                revision,
                &finding_id,
                evidence_cursor,
                relations_cursor,
            )?;
            lumin_protocol::to_json(&lumin_protocol::gate_explain_response(&explanation)?)?
        }
        (_, lumin_engine::RecordLookup::Pruning(tombstone)) => {
            lumin_protocol::to_json(&lumin_protocol::LookupTombstoneResponseDto::Pruning {
                tombstone,
            })?
        }
        (_, lumin_engine::RecordLookup::Pruned(tombstone)) => {
            lumin_protocol::to_json(&lumin_protocol::LookupTombstoneResponseDto::Pruned {
                tombstone,
            })?
        }
    };
    Ok(success(response))
}

fn gate_abandon(root: &Path, arguments: &mut Arguments) -> Result<CommandOutput, CliError> {
    let gate_id = parse_gate_id(arguments.required_positional_utf8("gate-id")?)?;
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
    let operation_id = parse_operation_id(arguments.required_positional_utf8("operation-id")?)?;
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
        result_delivery: CommandResultDelivery::RecoverableMutation,
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

fn parse_finding_id(value: String) -> Result<FindingId, CliError> {
    if value.is_empty() {
        Err(CliError::EmptyIdentifier("finding-id".to_owned()))
    } else {
        Ok(FindingId::from_string(value))
    }
}

fn parse_revision(value: String) -> Result<u64, CliError> {
    value.parse().map_err(|_| CliError::InvalidRevision(value))
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
        | CliError::RevisionRequired
        | CliError::InvalidRevision(_)
        | CliError::InvalidArea
        | CliError::NoCompletedRun
        | CliError::EmptyIdentifier(_)
        | CliError::EmptyReason
        | CliError::InvalidRepoPath(_)
        | CliError::DuplicatePaths0From
        | CliError::InvalidPaths0Source
        | CliError::Paths0Read(_) => 2,
        CliError::Protocol(error) => match error {
            ProtocolError::ResponseCursorAnchorMissing(_)
            | ProtocolError::ResponseOrderingMismatch { .. }
            | ProtocolError::Serialization(_) => 1,
            ProtocolError::CursorStale => 5,
            ProtocolError::CursorEncoding
            | ProtocolError::CursorPayload(_)
            | ProtocolError::CursorScopeMismatch
            | ProtocolError::CursorAnchorMissing
            | ProtocolError::GateRevisionMissing(_)
            | ProtocolError::GateRevisionEvidenceUnavailable(_)
            | ProtocolError::FindingNotFound(_)
            | ProtocolError::InvalidRepoPathDto(_)
            | ProtocolError::InvalidRepositoryRootDto(_) => 2,
        },
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

    fn required_positional_utf8(&mut self, name: &str) -> Result<String, CliError> {
        let value = self.required_utf8(name)?;
        if value.starts_with("--") {
            Err(CliError::UnknownArgument(value))
        } else {
            Ok(value)
        }
    }

    fn required_os(&mut self, name: &str) -> Result<OsString, CliError> {
        self.values
            .next()
            .ok_or_else(|| CliError::MissingValue(name.to_owned()))
    }
}

#[cfg(test)]
mod tests;
