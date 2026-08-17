use std::path::Path;

use crate::{
    Arguments, CliError, CommandOutput, CommandResultDelivery, CommandSuccess, parse_read_format,
    require_json,
};

pub(super) fn execute(root: &Path, arguments: &mut Arguments) -> Result<CommandOutput, CliError> {
    let subcommand = arguments
        .next_utf8("cache subcommand")?
        .ok_or(CliError::MissingCommand)?;
    if subcommand != "clean" {
        return Err(CliError::UnknownArgument(subcommand));
    }

    let format = parse_read_format(arguments, "cache clean argument")?;
    require_json(&format)?;
    lumin_engine::clean_cache(root)?;
    let stdout = lumin_protocol::to_json(&lumin_protocol::cache_cleanup_response())?;
    Ok(CommandSuccess {
        exit_code: 0,
        stdout,
        result_delivery: CommandResultDelivery::RecoverableMutation,
    }
    .into())
}
