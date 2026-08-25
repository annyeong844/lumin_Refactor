use std::path::Path;

use crate::{
    Arguments, CliError, CommandOutput, CommandResultDelivery, CommandSuccess,
    MutationDeliveryRecord, parse_operation_id, require_json,
};

pub(super) fn execute(root: &Path, arguments: &mut Arguments) -> Result<CommandOutput, CliError> {
    let subcommand = arguments
        .next_utf8("cache subcommand")?
        .ok_or(CliError::MissingCommand)?;
    #[cfg(feature = "lifecycle-test-fault")]
    if subcommand == "test-write" {
        let name = arguments.required_utf8("cache test-write name")?;
        let payload = arguments.required_utf8("cache test-write payload")?;
        if let Some(argument) = arguments.next_utf8("cache test-write argument")? {
            return Err(CliError::UnknownArgument(argument));
        }
        lumin_engine::write_active_cache_payload_for_test(root, &name, payload.as_bytes())?;
        return Ok(CommandSuccess {
            exit_code: 0,
            stdout: String::new(),
            result_delivery: CommandResultDelivery::ReadOnly,
            mutation_delivery: None,
        }
        .into());
    }
    if subcommand != "clean" {
        return Err(CliError::UnknownArgument(subcommand));
    }

    let mut operation_id = None;
    let mut format = None;
    while let Some(argument) = arguments.next_utf8("cache clean argument")? {
        match argument.as_str() {
            "--operation-id" if operation_id.is_none() => {
                operation_id = Some(parse_operation_id(
                    arguments.required_utf8("--operation-id")?,
                )?);
            }
            "--format" if format.is_none() => {
                format = Some(arguments.required_utf8("--format")?);
            }
            _ => return Err(CliError::UnknownArgument(argument)),
        }
    }
    let operation_id =
        operation_id.ok_or_else(|| CliError::MissingValue("--operation-id".to_owned()))?;
    let format = format.unwrap_or_else(|| "json".to_owned());
    require_json(&format)?;
    let result = lumin_engine::clean_cache(&lumin_engine::CleanCacheRequest {
        root: root.to_path_buf(),
        operation_id: operation_id.clone(),
    })?;
    let stdout = lumin_protocol::to_json(&lumin_protocol::cache_cleanup_response(&result))?;
    Ok(CommandSuccess {
        exit_code: 0,
        stdout,
        result_delivery: CommandResultDelivery::RecoverableMutation,
        mutation_delivery: Some(MutationDeliveryRecord::CacheCleanup {
            operation_id,
            request_digest: result.request_digest,
        }),
    }
    .into())
}
