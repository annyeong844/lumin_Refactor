use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    Redb,
    Sqlite,
}

impl BackendKind {
    pub const ALL: [Self; 2] = [Self::Redb, Self::Sqlite];

    pub const fn file_name(self) -> &'static str {
        match self {
            Self::Redb => "lifecycle.redb",
            Self::Sqlite => "lifecycle.sqlite",
        }
    }
}

impl fmt::Display for BackendKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Redb => formatter.write_str("redb"),
            Self::Sqlite => formatter.write_str("sqlite"),
        }
    }
}

impl FromStr for BackendKind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "redb" => Ok(Self::Redb),
            "sqlite" => Ok(Self::Sqlite),
            _ => anyhow::bail!("unknown backend {value:?}; expected redb or sqlite"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AdmissionOutcome {
    Admitted { gate_id: String },
    Conflict { holder_gate_id: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HoldPhase {
    Uncommitted,
    Committed,
}

impl FromStr for HoldPhase {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "uncommitted" => Ok(Self::Uncommitted),
            "committed" => Ok(Self::Committed),
            _ => anyhow::bail!("unknown hold phase {value:?}; expected uncommitted or committed"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ChildResult {
    pub backend: BackendKind,
    pub actor: String,
    pub key: String,
    pub elapsed_micros: u128,
    pub outcome: AdmissionOutcome,
}

#[derive(Clone, Debug, Serialize)]
pub struct ContentionRound {
    pub round: u32,
    pub conflicting: bool,
    pub child_results: Vec<ChildResult>,
    pub canonical_holders: Vec<(String, Option<String>)>,
    pub elapsed_micros: u128,
}

#[derive(Clone, Debug, Serialize)]
pub struct CrashCaseResult {
    pub phase: String,
    pub recovery_outcome: ChildResult,
    pub canonical_holder: Option<String>,
    pub elapsed_micros: u128,
}

#[derive(Clone, Debug, Serialize)]
pub struct BackendReport {
    pub backend: BackendKind,
    pub status: String,
    pub error: Option<String>,
    pub contention_rounds: Vec<ContentionRound>,
    pub disjoint_rounds: Vec<ContentionRound>,
    pub crash_cases: Vec<CrashCaseResult>,
    pub database_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SourceFileHash {
    pub path: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ExecutableIdentity {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct IdentityReport {
    pub probe_id: String,
    pub architecture_commit: String,
    pub architecture_manifest_sha256: String,
    pub executable: PathBuf,
    pub executable_bytes: u64,
    pub executable_sha256: String,
    pub source_files: Vec<SourceFileHash>,
    pub source_manifest_sha256: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProbeReport {
    pub probe_id: String,
    pub architecture_commit: String,
    pub architecture_manifest_sha256: String,
    pub started_unix_millis: u128,
    pub finished_unix_millis: u128,
    pub host_os: String,
    pub host_arch: String,
    pub executable: PathBuf,
    pub executable_bytes: u64,
    pub executable_sha256: String,
    pub command: Vec<String>,
    pub rounds: u32,
    pub watchdog_millis: u64,
    pub source_files: Vec<SourceFileHash>,
    pub source_manifest_sha256: String,
    pub backends: Vec<BackendReport>,
    pub overall_status: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct FaultCaseResult {
    pub domain: String,
    pub crash_point: String,
    pub status: String,
    pub error: Option<String>,
    pub elapsed_micros: u128,
    pub observation: serde_json::Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct FaultBackendReport {
    pub backend: BackendKind,
    pub status: String,
    pub cases: Vec<FaultCaseResult>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FaultMatrixReport {
    pub probe_id: String,
    pub architecture_commit: String,
    pub architecture_manifest_sha256: String,
    pub started_unix_millis: u128,
    pub finished_unix_millis: u128,
    pub host_os: String,
    pub host_arch: String,
    pub executable: PathBuf,
    pub executable_bytes: u64,
    pub executable_sha256: String,
    pub command: Vec<String>,
    pub watchdog_millis: u64,
    pub source_files: Vec<SourceFileHash>,
    pub source_manifest_sha256: String,
    pub backends: Vec<FaultBackendReport>,
    pub overall_status: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct LatencySummary {
    pub samples: usize,
    pub min_micros: u128,
    pub p50_micros: u128,
    pub p95_micros: u128,
    pub p99_micros: u128,
    pub max_micros: u128,
    pub mean_micros: u128,
}

#[derive(Clone, Debug, Serialize)]
pub struct BenchmarkReport {
    pub probe_id: String,
    pub architecture_commit: String,
    pub architecture_manifest_sha256: String,
    pub started_unix_millis: u128,
    pub finished_unix_millis: u128,
    pub host_os: String,
    pub host_arch: String,
    pub backend: BackendKind,
    pub executable: PathBuf,
    pub executable_bytes: u64,
    pub executable_sha256: String,
    pub command: Vec<String>,
    pub records: usize,
    pub record_bytes: usize,
    pub durable_transactions: usize,
    pub initialize_micros: u128,
    pub bulk_insert_micros: u128,
    pub first_reopen_query_micros: u128,
    pub warm_reopen_query: LatencySummary,
    pub durable_admission: LatencySummary,
    pub peak_working_set_bytes: Option<u64>,
    pub store_bytes: u64,
    pub source_files: Vec<SourceFileHash>,
    pub source_manifest_sha256: String,
    pub status: String,
}
