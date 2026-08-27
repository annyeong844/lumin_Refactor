use std::path::Path;

use crate::{
    Arguments, CliError, CommandOutput, CommandResultDelivery, CommandSuccess,
    MutationDeliveryRecord, require_json,
};

pub(super) fn execute(root: &Path, arguments: &mut Arguments) -> Result<CommandOutput, CliError> {
    let subcommand = arguments
        .next_utf8("store subcommand")?
        .ok_or(CliError::MissingCommand)?;
    #[cfg(feature = "lifecycle-test-fault")]
    if subcommand == "test-downgrade-v12" {
        let mut cleanup_operation = None;
        let mut legacy_delivery = None;
        while let Some(argument) = arguments.next_utf8("store test fixture argument")? {
            match argument.as_str() {
                "--cleanup-operation" if cleanup_operation.is_none() => {
                    cleanup_operation = Some(crate::parse_operation_id(
                        arguments.required_utf8("--cleanup-operation")?,
                    )?);
                }
                "--legacy-delivery" if legacy_delivery.is_none() => {
                    legacy_delivery = Some(
                        match arguments.required_utf8("--legacy-delivery")?.as_str() {
                            "not-attempted" => {
                                lumin_engine::PriorCacheCleanupDeliveryStatusForTest::NotAttempted
                            }
                            "succeeded" => {
                                lumin_engine::PriorCacheCleanupDeliveryStatusForTest::Succeeded
                            }
                            "failed" => {
                                lumin_engine::PriorCacheCleanupDeliveryStatusForTest::Failed
                            }
                            value => return Err(CliError::UnknownArgument(value.to_owned())),
                        },
                    );
                }
                _ => return Err(CliError::UnknownArgument(argument)),
            }
        }
        match (cleanup_operation.as_ref(), legacy_delivery) {
            (Some(operation_id), Some(status)) => {
                lumin_engine::rewrite_lifecycle_store_with_cleanup_as_prior_for_test(
                    root,
                    operation_id,
                    status,
                )?;
            }
            (None, None) => lumin_engine::rewrite_lifecycle_store_as_prior_for_test(root)?,
            (Some(_), None) => {
                return Err(CliError::MissingValue("--legacy-delivery".to_owned()));
            }
            (None, Some(_)) => {
                return Err(CliError::MissingValue("--cleanup-operation".to_owned()));
            }
        }
        return Ok(CommandSuccess {
            exit_code: 0,
            stdout: String::new(),
            result_delivery: CommandResultDelivery::ReadOnly,
            mutation_delivery: None,
        }
        .into());
    }
    #[cfg(feature = "lifecycle-test-fault")]
    if subcommand == "test-corrupt-v12-cleanup" {
        let operation_id = crate::parse_operation_id(
            arguments
                .next_utf8("store test corruption operation ID")?
                .ok_or_else(|| CliError::MissingValue("operation-id".to_owned()))?,
        )?;
        if let Some(argument) = arguments.next_utf8("store test corruption argument")? {
            return Err(CliError::UnknownArgument(argument));
        }
        lumin_engine::corrupt_migrating_cleanup_operation_for_test(root, &operation_id)?;
        return Ok(CommandSuccess {
            exit_code: 0,
            stdout: String::new(),
            result_delivery: CommandResultDelivery::ReadOnly,
            mutation_delivery: None,
        }
        .into());
    }
    #[cfg(feature = "lifecycle-test-fault")]
    if subcommand == "test-corrupt-v13-anchor" {
        if let Some(argument) = arguments.next_utf8("store test anchor corruption argument")? {
            return Err(CliError::UnknownArgument(argument));
        }
        lumin_engine::corrupt_migration_anchor_for_test(root)?;
        return Ok(CommandSuccess {
            exit_code: 0,
            stdout: String::new(),
            result_delivery: CommandResultDelivery::ReadOnly,
            mutation_delivery: None,
        }
        .into());
    }
    #[cfg(feature = "lifecycle-test-fault")]
    if subcommand == "test-remove-v12-root-authorization" {
        if let Some(argument) = arguments.next_utf8("store test authorization argument")? {
            return Err(CliError::UnknownArgument(argument));
        }
        lumin_engine::remove_bound_root_authorization_for_test(root)?;
        return Ok(CommandSuccess {
            exit_code: 0,
            stdout: String::new(),
            result_delivery: CommandResultDelivery::ReadOnly,
            mutation_delivery: None,
        }
        .into());
    }
    if subcommand != "migrate" {
        return Err(CliError::UnknownArgument(subcommand));
    }

    let mut format = None;
    while let Some(argument) = arguments.next_utf8("store migrate argument")? {
        match argument.as_str() {
            "--format" if format.is_none() => {
                format = Some(arguments.required_utf8("--format")?);
            }
            _ => return Err(CliError::UnknownArgument(argument)),
        }
    }
    require_json(format.as_deref().unwrap_or("json"))?;
    lumin_engine::migrate_lifecycle_store(root)?;
    let stdout = lumin_protocol::to_json(&lumin_protocol::lifecycle_store_migration_response())?;
    Ok(CommandSuccess {
        exit_code: 0,
        stdout,
        result_delivery: CommandResultDelivery::RecoverableMutation,
        mutation_delivery: Some(MutationDeliveryRecord::LifecycleStoreMigration),
    }
    .into())
}
