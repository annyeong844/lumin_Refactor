use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CorpusClass {
    LegacyExactGit,
    SyntheticStack,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CorpusEntry {
    pub path: String,
    pub class: CorpusClass,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CorpusManifest {
    pub schema: String,
    pub corpus_id: String,
    pub source_repository: String,
    pub source_commit: String,
    pub generator_sha256: String,
    pub legacy_file_count: usize,
    pub legacy_bytes: u64,
    pub synthetic_file_count: usize,
    pub synthetic_bytes: u64,
    pub entries: Vec<CorpusEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceFileHash {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExecutableIdentity {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ParseOutcome {
    Parsed,
    InvalidUtf8,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileResult {
    pub path: String,
    pub class: CorpusClass,
    pub bytes: u64,
    pub source_sha256: String,
    pub outcome: ParseOutcome,
    pub top_level_statements: usize,
    pub directives: usize,
    pub parse_diagnostics: usize,
    pub diagnostic_digest: String,
    pub parser_panicked: bool,
    pub program_end: u32,
    pub allocator_used_bytes: usize,
    pub allocator_capacity_bytes: usize,
    pub read_micros: u128,
    pub parse_micros: u128,
    pub lower_micros: u128,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryObservation {
    pub current_rss_bytes: Option<u64>,
    pub peak_rss_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WaveResult {
    pub wave: usize,
    pub elapsed_micros: u128,
    pub semantic_digest: String,
    pub parsed_files: usize,
    pub invalid_utf8_files: usize,
    pub files_with_diagnostics: usize,
    pub total_parse_diagnostics: usize,
    pub parser_panicked_files: usize,
    pub allocator_used_bytes_total: u64,
    pub allocator_capacity_bytes_total: u64,
    pub allocator_used_bytes_max_file: u64,
    pub allocator_capacity_bytes_max_file: u64,
    pub read_micros_total: u128,
    pub parse_micros_total: u128,
    pub lower_micros_total: u128,
    pub memory_after_drop: MemoryObservation,
}

#[derive(Debug, Serialize)]
pub struct IdentityReport {
    pub probe_id: &'static str,
    pub architecture_commit: &'static str,
    pub architecture_manifest_sha256: &'static str,
    pub host_os: &'static str,
    pub host_arch: &'static str,
    pub available_parallelism: usize,
    pub executable: ExecutableIdentity,
    pub source_files: Vec<SourceFileHash>,
    pub source_manifest_sha256: String,
}

#[derive(Debug, Serialize)]
pub struct RunReport {
    pub probe_id: &'static str,
    pub status: &'static str,
    pub architecture_commit: &'static str,
    pub architecture_manifest_sha256: &'static str,
    pub host_os: &'static str,
    pub host_arch: &'static str,
    pub platform_label: String,
    pub filesystem_class: String,
    pub available_parallelism: usize,
    pub requested_workers: usize,
    pub actual_workers: usize,
    pub worker_stack_bytes: usize,
    pub waves: usize,
    pub corpus_id: String,
    pub corpus_manifest_sha256: String,
    pub corpus_file_count: usize,
    pub corpus_bytes: u64,
    pub baseline_memory: MemoryObservation,
    pub wave_results: Vec<WaveResult>,
    pub semantic_digest: String,
    pub file_results: Vec<FileResult>,
    pub executable: ExecutableIdentity,
    pub source_files: Vec<SourceFileHash>,
    pub source_manifest_sha256: String,
}
