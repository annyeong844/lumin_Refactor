use std::path::Path;

use lumin_engine::{
    ConfirmRetentionPlanRequest, PinRunRequest, PrepareRetentionPlanRequest, UnpinRunRequest,
};
use lumin_model::{OperationId, PinId, RetentionPlanId, RunId};

use crate::{Arguments, CliError, CommandOutput, CommandSuccess, require_json};

pub(super) fn runs(root: &Path, arguments: &mut Arguments) -> Result<CommandOutput, CliError> {
    let subcommand = arguments
        .next_utf8("runs subcommand")?
        .ok_or(CliError::MissingCommand)?;
    match subcommand.as_str() {
        "list" => list(root, arguments),
        "pin" => pin(root, arguments),
        "unpin" => unpin(root, arguments),
        "prune" => run_prune(root, arguments),
        _ => Err(CliError::UnknownArgument(subcommand)),
    }
}

fn list(root: &Path, arguments: &mut Arguments) -> Result<CommandOutput, CliError> {
    let mut cursor = None;
    let mut format = "json".to_owned();
    while let Some(argument) = arguments.next_utf8("runs list argument")? {
        match argument.as_str() {
            "--cursor" => cursor = Some(arguments.required_utf8("--cursor")?),
            "--format" => format = arguments.required_utf8("--format")?,
            _ => return Err(CliError::UnknownArgument(argument)),
        }
    }
    require_json(&format)?;
    let catalog = lumin_engine::list_runs(root)?;
    let runs = catalog
        .runs
        .into_iter()
        .map(|run| lumin_protocol::run_catalog_item(run.attempt_id, run.run_id, run.sequence))
        .collect::<Vec<_>>();
    let response =
        lumin_protocol::run_catalog_response(catalog.revision, &runs, cursor.as_deref())?;
    json_success(lumin_protocol::to_json(&response))
}

pub(super) fn gate_prune(
    root: &Path,
    arguments: &mut Arguments,
) -> Result<CommandOutput, CliError> {
    prune(root, arguments, PruneDomain::Gates)
}

fn pin(root: &Path, arguments: &mut Arguments) -> Result<CommandOutput, CliError> {
    let run_id = RunId::from_string(nonempty(arguments.required_utf8("run-id")?, "run-id")?);
    let mut operation_id = None;
    let mut reason = None;
    let mut format = "json".to_owned();
    while let Some(argument) = arguments.next_utf8("runs pin argument")? {
        match argument.as_str() {
            "--operation-id" => {
                operation_id = Some(operation_id_argument(arguments)?);
            }
            "--reason" => reason = Some(arguments.required_utf8("--reason")?),
            "--format" => format = arguments.required_utf8("--format")?,
            _ => return Err(CliError::UnknownArgument(argument)),
        }
    }
    require_json(&format)?;
    let reason = reason.ok_or_else(|| CliError::MissingValue("--reason".to_owned()))?;
    if reason.trim().is_empty() {
        return Err(CliError::EmptyReason);
    }
    let pin = lumin_engine::pin_run(&PinRunRequest {
        root: root.to_path_buf(),
        run_id,
        operation_id: operation_id
            .ok_or_else(|| CliError::MissingValue("--operation-id".to_owned()))?,
        reason,
    })?;
    json_success(lumin_protocol::to_json(&lumin_protocol::run_pin_response(
        &pin,
    )))
}

fn unpin(root: &Path, arguments: &mut Arguments) -> Result<CommandOutput, CliError> {
    let pin_id = PinId::from_string(nonempty(arguments.required_utf8("pin-id")?, "pin-id")?);
    let mut operation_id = None;
    let mut format = "json".to_owned();
    while let Some(argument) = arguments.next_utf8("runs unpin argument")? {
        match argument.as_str() {
            "--operation-id" => operation_id = Some(operation_id_argument(arguments)?),
            "--format" => format = arguments.required_utf8("--format")?,
            _ => return Err(CliError::UnknownArgument(argument)),
        }
    }
    require_json(&format)?;
    let pin = lumin_engine::unpin_run(&UnpinRunRequest {
        root: root.to_path_buf(),
        pin_id,
        operation_id: operation_id
            .ok_or_else(|| CliError::MissingValue("--operation-id".to_owned()))?,
    })?;
    json_success(lumin_protocol::to_json(&lumin_protocol::run_pin_response(
        &pin,
    )))
}

fn run_prune(root: &Path, arguments: &mut Arguments) -> Result<CommandOutput, CliError> {
    prune(root, arguments, PruneDomain::Runs)
}

fn prune(
    root: &Path,
    arguments: &mut Arguments,
    domain: PruneDomain,
) -> Result<CommandOutput, CliError> {
    let subcommand = arguments
        .next_utf8("prune subcommand")?
        .ok_or(CliError::MissingCommand)?;
    match subcommand.as_str() {
        "plan" => plan(root, arguments, domain),
        "confirm" => confirm(root, arguments),
        _ => Err(CliError::UnknownArgument(subcommand)),
    }
}

fn plan(
    root: &Path,
    arguments: &mut Arguments,
    domain: PruneDomain,
) -> Result<CommandOutput, CliError> {
    let first = arguments.next_utf8("prune plan argument")?;
    if first.as_deref() == Some("show") {
        return show_plan(root, arguments);
    }
    let mut cutoff = None;
    let mut operation_id = None;
    let mut format = "json".to_owned();
    let mut current = first;
    while let Some(argument) = current.take() {
        match argument.as_str() {
            "--before" if domain == PruneDomain::Runs => {
                cutoff = Some(timestamp_argument(arguments, "--before")?);
            }
            "--terminal-before" if domain == PruneDomain::Gates => {
                cutoff = Some(timestamp_argument(arguments, "--terminal-before")?);
            }
            "--operation-id" => operation_id = Some(operation_id_argument(arguments)?),
            "--format" => format = arguments.required_utf8("--format")?,
            _ => return Err(CliError::UnknownArgument(argument)),
        }
        current = arguments.next_utf8("prune plan argument")?;
    }
    require_json(&format)?;
    let cutoff = cutoff.ok_or_else(|| {
        CliError::MissingValue(match domain {
            PruneDomain::Runs => "--before".to_owned(),
            PruneDomain::Gates => "--terminal-before".to_owned(),
        })
    })?;
    let scope = match domain {
        PruneDomain::Runs => lumin_engine::RetentionPlanScope::Runs {
            before_unix_millis: cutoff,
        },
        PruneDomain::Gates => lumin_engine::RetentionPlanScope::Gates {
            terminal_before_unix_millis: cutoff,
        },
    };
    let result = lumin_engine::prepare_retention_plan(&PrepareRetentionPlanRequest {
        root: root.to_path_buf(),
        scope,
        operation_id: operation_id
            .ok_or_else(|| CliError::MissingValue("--operation-id".to_owned()))?,
    })?;
    mutation_output(&result)
}

fn show_plan(root: &Path, arguments: &mut Arguments) -> Result<CommandOutput, CliError> {
    let plan_id =
        RetentionPlanId::from_string(nonempty(arguments.required_utf8("plan-id")?, "plan-id")?);
    let mut cursor = None;
    let mut format = "json".to_owned();
    while let Some(argument) = arguments.next_utf8("prune plan show argument")? {
        match argument.as_str() {
            "--cursor" => cursor = Some(arguments.required_utf8("--cursor")?),
            "--format" => format = arguments.required_utf8("--format")?,
            _ => return Err(CliError::UnknownArgument(argument)),
        }
    }
    require_json(&format)?;
    let plan = lumin_engine::load_retention_plan(root, &plan_id)?;
    let response = lumin_protocol::retention_plan_response(&plan, cursor.as_deref())?;
    json_success(lumin_protocol::to_json(&response))
}

fn confirm(root: &Path, arguments: &mut Arguments) -> Result<CommandOutput, CliError> {
    let plan_id =
        RetentionPlanId::from_string(nonempty(arguments.required_utf8("plan-id")?, "plan-id")?);
    let mut operation_id = None;
    let mut format = "json".to_owned();
    while let Some(argument) = arguments.next_utf8("prune confirm argument")? {
        match argument.as_str() {
            "--operation-id" => operation_id = Some(operation_id_argument(arguments)?),
            "--format" => format = arguments.required_utf8("--format")?,
            _ => return Err(CliError::UnknownArgument(argument)),
        }
    }
    require_json(&format)?;
    let result = lumin_engine::confirm_retention_plan(&ConfirmRetentionPlanRequest {
        root: root.to_path_buf(),
        plan_id,
        operation_id: operation_id
            .ok_or_else(|| CliError::MissingValue("--operation-id".to_owned()))?,
    })?;
    mutation_output(&result)
}

fn mutation_output(
    result: &lumin_engine::RetentionMutationResult,
) -> Result<CommandOutput, CliError> {
    let stdout = lumin_protocol::to_json(&lumin_protocol::retention_mutation_response(result))?;
    let exit_code = if matches!(result, lumin_engine::RetentionMutationResult::Stale { .. }) {
        5
    } else {
        0
    };
    Ok(CommandSuccess { exit_code, stdout }.into())
}

fn json_success(
    output: Result<String, lumin_protocol::ProtocolError>,
) -> Result<CommandOutput, CliError> {
    output
        .map(|stdout| {
            CommandSuccess {
                exit_code: 0,
                stdout,
            }
            .into()
        })
        .map_err(Into::into)
}

fn operation_id_argument(arguments: &mut Arguments) -> Result<OperationId, CliError> {
    Ok(OperationId::from_string(nonempty(
        arguments.required_utf8("--operation-id")?,
        "operation-id",
    )?))
}

fn timestamp_argument(arguments: &mut Arguments, name: &str) -> Result<u64, CliError> {
    let value = arguments.required_utf8(name)?;
    value
        .parse::<u64>()
        .map_err(|_| CliError::InvalidTimestamp(value))
}

fn nonempty(value: String, label: &str) -> Result<String, CliError> {
    if value.is_empty() {
        Err(CliError::EmptyIdentifier(label.to_owned()))
    } else {
        Ok(value)
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum PruneDomain {
    Runs,
    Gates,
}

#[cfg(test)]
mod tests;
