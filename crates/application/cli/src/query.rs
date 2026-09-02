use super::*;

pub(super) fn gate_list(root: &Path, arguments: &mut Arguments) -> Result<CommandOutput, CliError> {
    let mut active = false;
    let mut cursor = None;
    let mut format = "json".to_owned();
    while let Some(argument) = arguments.next_utf8("gate list argument")? {
        match argument.as_str() {
            "--active" => active = true,
            "--cursor" => cursor = Some(arguments.required_utf8("--cursor")?),
            "--format" => format = arguments.required_utf8("--format")?,
            _ => return Err(CliError::UnknownArgument(argument)),
        }
    }
    require_json(&format)?;
    if !active {
        return Err(CliError::MissingValue("--active".to_owned()));
    }
    let decoded_cursor = cursor
        .as_deref()
        .map(lumin_protocol::decode_active_gates_cursor)
        .transpose()?;
    let store_cursor = decoded_cursor.map(|c| lumin_engine::ActiveGateCatalogCursor {
        repository_id: c.repository_id,
        revision: c.revision,
        page_size: c.page_size,
        opening_sequence: c.opening_sequence,
        gate_id: c.gate_id,
    });
    let snapshot = lumin_engine::list_active_gates(
        root,
        store_cursor.as_ref(),
        lumin_protocol::active_gates_page_size(),
    )?;
    let items: Vec<lumin_protocol::ActiveGateItemDto> = snapshot
        .items
        .iter()
        .map(|item| lumin_protocol::ActiveGateItemDto {
            gate_id: item.gate_id.clone(),
            current_revision: item.current_revision,
            opening_transition_sequence: item.opening_transition_sequence,
        })
        .collect();
    let response = lumin_protocol::active_gates_response(
        snapshot.repository_id,
        snapshot.revision,
        snapshot.scope_total,
        snapshot.total,
        items,
        snapshot.truncated,
    )?;
    lumin_protocol::to_json(&response)
        .map(success)
        .map_err(Into::into)
}

pub(super) fn related(root: &Path, arguments: &mut Arguments) -> Result<String, CliError> {
    let mut run_id = None;
    let mut finding_id = None;
    let mut cursor = None;
    let mut format = "json".to_owned();
    while let Some(argument) = arguments.next_utf8("related argument")? {
        match argument.as_str() {
            "--run" => {
                run_id = Some(RunId::from_string(arguments.required_utf8("--run")?));
            }
            "--cursor" => cursor = Some(arguments.required_utf8("--cursor")?),
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
            let decoded_cursor = lumin_protocol::decode_run_query_cursor(cursor.as_deref())?;
            let page = lumin_engine::query_run_relations(
                &repository_id,
                &run_id,
                &evidence,
                &finding_id,
                decoded_cursor,
            )?;
            let response = lumin_protocol::run_relations_response(&page)?;
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

#[cfg(unix)]
fn native_argument_is_option_shaped(argument: &std::ffi::OsStr) -> bool {
    use std::os::unix::ffi::OsStrExt;

    argument.as_bytes().starts_with(b"--")
}

#[cfg(windows)]
fn native_argument_is_option_shaped(argument: &std::ffi::OsStr) -> bool {
    use std::os::windows::ffi::OsStrExt;

    argument
        .encode_wide()
        .take(2)
        .eq([b'-' as u16, b'-' as u16])
}

#[cfg(not(any(unix, windows)))]
fn native_argument_is_option_shaped(argument: &std::ffi::OsStr) -> bool {
    argument
        .to_str()
        .is_none_or(|value| value.starts_with("--"))
}

pub(super) fn files(root: &Path, arguments: &mut Arguments) -> Result<String, CliError> {
    let mut run_id = None;
    let mut repo_path = None;
    let mut cursor = None;
    let mut format = "json".to_owned();
    while let Some(argument) = arguments.next_os() {
        match argument.to_str() {
            Some("--") if repo_path.is_none() => {
                let value = arguments.required_os("repository path")?;
                repo_path = Some(
                    lumin_engine::lower_native_repo_path(value.as_os_str())
                        .map_err(|error| CliError::InvalidRepoPath(error.to_string()))?,
                );
                if let Some(extra) = arguments.next_os() {
                    return Err(CliError::UnknownArgument(
                        extra.to_string_lossy().into_owned(),
                    ));
                }
            }
            Some("--run") => {
                run_id = Some(RunId::from_string(arguments.required_utf8("--run")?));
            }
            Some("--cursor") => cursor = Some(arguments.required_utf8("--cursor")?),
            Some("--format") => format = arguments.required_utf8("--format")?,
            _ if native_argument_is_option_shaped(argument.as_os_str()) || repo_path.is_some() => {
                return Err(CliError::UnknownArgument(
                    argument.to_string_lossy().into_owned(),
                ));
            }
            _ => {
                repo_path = Some(
                    lumin_engine::lower_native_repo_path(argument.as_os_str())
                        .map_err(|error| CliError::InvalidRepoPath(error.to_string()))?,
                );
            }
        }
    }
    require_json(&format)?;
    let run_id = run_id.ok_or(CliError::RunRequired)?;
    let repo_path =
        repo_path.ok_or_else(|| CliError::MissingValue("repository path".to_owned()))?;
    match lumin_engine::lookup_run(root, &run_id)? {
        (repository_id, lumin_engine::RecordLookup::Live((_, evidence))) => {
            let decoded_cursor = lumin_protocol::decode_run_query_cursor(cursor.as_deref())?;
            let page = lumin_engine::query_run_file_findings(
                &repository_id,
                &run_id,
                &evidence,
                &repo_path,
                decoded_cursor,
            )?;
            let source = lumin_engine::query_run_source_envelope(&evidence, &repo_path)?;
            let response = lumin_protocol::run_file_findings_response(
                &page,
                source.classification,
                source.context,
                source.observation,
                source.resolution_profile,
                &source.resolutions,
            )?;
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
