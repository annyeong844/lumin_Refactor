use lumin_model::{AttemptId, RepositoryId, RunId};
use serde::{Deserialize, Serialize};

use crate::ProtocolError;
use crate::cursor::{decode_cursor_payload, encode_cursor_payload};

pub const RUNS_ORDERING: &str = "runs.v1";
pub const RUNS_PAGE_SIZE: usize = 100;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunCatalogItemDto {
    pub attempt_id: AttemptId,
    pub run_id: RunId,
    pub sequence: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunCatalogCollectionDto {
    pub schema_version: &'static str,
    pub repository_id: RepositoryId,
    pub revision: u64,
    pub ordering: &'static str,
    pub total: usize,
    pub returned: usize,
    pub truncated: bool,
    pub next_cursor: Option<String>,
    pub runs: Vec<RunCatalogItemDto>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunCatalogCursorDto {
    schema_version: String,
    repository_id: RepositoryId,
    revision: u64,
    ordering: String,
    last_run: RunCatalogItemDto,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedRunCatalogCursor {
    pub repository_id: RepositoryId,
    pub revision: u64,
    pub last_run: RunCatalogItemDto,
}

pub fn run_catalog_item(attempt_id: AttemptId, run_id: RunId, sequence: u64) -> RunCatalogItemDto {
    RunCatalogItemDto {
        attempt_id,
        run_id,
        sequence,
    }
}

pub fn run_catalog_response(
    repository_id: RepositoryId,
    revision: u64,
    total: usize,
    runs: Vec<RunCatalogItemDto>,
    truncated: bool,
) -> Result<RunCatalogCollectionDto, ProtocolError> {
    let next_cursor = if truncated {
        Some(encode_cursor(
            repository_id.clone(),
            revision,
            runs.last().ok_or(ProtocolError::CursorAnchorMissing)?,
        )?)
    } else {
        None
    };
    Ok(RunCatalogCollectionDto {
        schema_version: "lumin.runs.v1",
        repository_id,
        revision,
        ordering: RUNS_ORDERING,
        total,
        returned: runs.len(),
        truncated,
        next_cursor,
        runs,
    })
}

pub fn decode_run_catalog_cursor(value: &str) -> Result<DecodedRunCatalogCursor, ProtocolError> {
    let cursor = decode_cursor(value)?;
    Ok(DecodedRunCatalogCursor {
        repository_id: cursor.repository_id,
        revision: cursor.revision,
        last_run: cursor.last_run,
    })
}

fn encode_cursor(
    repository_id: RepositoryId,
    revision: u64,
    last_run: &RunCatalogItemDto,
) -> Result<String, ProtocolError> {
    let cursor = RunCatalogCursorDto {
        schema_version: "lumin-runs-cursor.v2".to_owned(),
        repository_id,
        revision,
        ordering: RUNS_ORDERING.to_owned(),
        last_run: last_run.clone(),
    };
    encode_cursor_payload(&cursor)
}

fn decode_cursor(value: &str) -> Result<RunCatalogCursorDto, ProtocolError> {
    let cursor: RunCatalogCursorDto = decode_cursor_payload(value)?;
    if cursor.schema_version != "lumin-runs-cursor.v2" || cursor.ordering != RUNS_ORDERING {
        return Err(ProtocolError::CursorScopeMismatch);
    }
    Ok(cursor)
}
