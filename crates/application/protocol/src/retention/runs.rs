use lumin_model::{AttemptId, RunId};
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
    revision: u64,
    ordering: String,
    last_run: RunCatalogItemDto,
}

pub fn run_catalog_item(attempt_id: AttemptId, run_id: RunId, sequence: u64) -> RunCatalogItemDto {
    RunCatalogItemDto {
        attempt_id,
        run_id,
        sequence,
    }
}

pub fn run_catalog_response(
    revision: u64,
    runs: &[RunCatalogItemDto],
    cursor: Option<&str>,
) -> Result<RunCatalogCollectionDto, ProtocolError> {
    let start = match cursor {
        Some(value) => resume_offset(revision, runs, value)?,
        None => 0,
    };
    let end = start.saturating_add(RUNS_PAGE_SIZE).min(runs.len());
    let page = runs[start..end].to_vec();
    let truncated = end < runs.len();
    let next_cursor = if truncated {
        page.last()
            .map(|run| encode_cursor(revision, run))
            .transpose()?
    } else {
        None
    };
    Ok(RunCatalogCollectionDto {
        schema_version: "lumin.runs.v1",
        revision,
        ordering: RUNS_ORDERING,
        total: runs.len(),
        returned: page.len(),
        truncated,
        next_cursor,
        runs: page,
    })
}

fn resume_offset(
    revision: u64,
    runs: &[RunCatalogItemDto],
    value: &str,
) -> Result<usize, ProtocolError> {
    let cursor = decode_cursor(value)?;
    if cursor.revision != revision {
        return Err(ProtocolError::CursorStale);
    }
    if cursor.ordering != RUNS_ORDERING {
        return Err(ProtocolError::CursorScopeMismatch);
    }
    runs.iter()
        .position(|run| run == &cursor.last_run)
        .map(|index| index + 1)
        .ok_or(ProtocolError::CursorAnchorMissing)
}

fn encode_cursor(revision: u64, last_run: &RunCatalogItemDto) -> Result<String, ProtocolError> {
    let cursor = RunCatalogCursorDto {
        schema_version: "lumin-runs-cursor.v1".to_owned(),
        revision,
        ordering: RUNS_ORDERING.to_owned(),
        last_run: last_run.clone(),
    };
    encode_cursor_payload(&cursor)
}

fn decode_cursor(value: &str) -> Result<RunCatalogCursorDto, ProtocolError> {
    let cursor: RunCatalogCursorDto = decode_cursor_payload(value)?;
    if cursor.schema_version != "lumin-runs-cursor.v1" {
        return Err(ProtocolError::CursorScopeMismatch);
    }
    Ok(cursor)
}
