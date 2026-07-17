use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::{Context, Result, ensure};
use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_span::SourceType;
use rayon::prelude::*;
use sha2::{Digest, Sha256};

use crate::{
    ARCHITECTURE_COMMIT, ARCHITECTURE_MANIFEST,
    corpus::{ValidatedCorpus, entry_path},
    memory,
    model::{CorpusEntry, FileResult, ParseOutcome, RunReport, WaveResult},
    util::{executable_identity, sha256, source_hashes, source_manifest_sha256},
};

pub struct RunOptions {
    pub corpus_root: PathBuf,
    pub corpus: ValidatedCorpus,
    pub workers: usize,
    pub stack_bytes: usize,
    pub waves: usize,
    pub platform_label: String,
    pub filesystem_class: String,
}

pub fn run(options: RunOptions) -> Result<RunReport> {
    ensure!(options.workers > 0, "worker count must be positive");
    ensure!(options.stack_bytes > 0, "worker stack must be positive");
    ensure!(options.waves > 0, "wave count must be positive");

    let source_files = source_hashes();
    let source_manifest = source_manifest_sha256(&source_files);
    let available_parallelism = std::thread::available_parallelism()?.get();
    let baseline_memory = memory::observe()?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(options.workers)
        .stack_size(options.stack_bytes)
        .thread_name(|index| format!("lumin-phase0-oxc-{index}"))
        .build()
        .context("build local Rayon pool")?;
    ensure!(
        pool.current_num_threads() == options.workers,
        "Rayon worker count mismatch"
    );

    let mut wave_results = Vec::with_capacity(options.waves);
    let mut first_results = None;
    let mut expected_digest = None;
    for wave in 0..options.waves {
        let started = Instant::now();
        let mut results = pool.install(|| {
            options
                .corpus
                .entries
                .par_iter()
                .map(|entry| parse_file(&options.corpus_root, entry))
                .collect::<Result<Vec<_>>>()
        })?;
        results.sort_unstable_by(|left, right| left.path.cmp(&right.path));
        let semantic_digest = semantic_digest(&results);
        if let Some(expected) = expected_digest.as_ref() {
            ensure!(
                expected == &semantic_digest,
                "semantic digest changed between waves"
            );
        } else {
            expected_digest = Some(semantic_digest.clone());
        }
        let memory_after_drop = memory::observe()?;
        wave_results.push(summarize_wave(
            wave,
            started.elapsed().as_micros(),
            semantic_digest,
            &results,
            memory_after_drop,
        )?);
        if first_results.is_none() {
            first_results = Some(results);
        }
    }

    Ok(RunReport {
        probe_id: "lumin-phase0-oxc-run-v1",
        status: "PASS",
        architecture_commit: ARCHITECTURE_COMMIT,
        architecture_manifest_sha256: ARCHITECTURE_MANIFEST,
        host_os: std::env::consts::OS,
        host_arch: std::env::consts::ARCH,
        platform_label: options.platform_label,
        filesystem_class: options.filesystem_class,
        available_parallelism,
        requested_workers: options.workers,
        actual_workers: pool.current_num_threads(),
        worker_stack_bytes: options.stack_bytes,
        waves: options.waves,
        corpus_id: options.corpus.manifest.corpus_id,
        corpus_manifest_sha256: options.corpus.manifest_sha256,
        corpus_file_count: options.corpus.entries.len(),
        corpus_bytes: options.corpus.total_bytes,
        baseline_memory,
        wave_results,
        semantic_digest: expected_digest.context("missing semantic digest")?,
        file_results: first_results.context("missing first-wave results")?,
        executable: executable_identity()?,
        source_files,
        source_manifest_sha256: source_manifest,
    })
}

fn parse_file(root: &Path, entry: &CorpusEntry) -> Result<FileResult> {
    let path = entry_path(root, entry);
    let read_started = Instant::now();
    let bytes = fs::read(&path).with_context(|| format!("read source {}", path.display()))?;
    ensure!(
        u64::try_from(bytes.len())? == entry.bytes,
        "source byte count drift: {}",
        entry.path
    );
    let source_sha256 = sha256(&bytes);
    ensure!(
        source_sha256 == entry.sha256,
        "source identity drift: {}",
        entry.path
    );
    let read_micros = read_started.elapsed().as_micros();
    let Ok(source) = std::str::from_utf8(&bytes) else {
        return Ok(FileResult {
            path: entry.path.clone(),
            class: entry.class.clone(),
            bytes: entry.bytes,
            source_sha256,
            outcome: ParseOutcome::InvalidUtf8,
            top_level_statements: 0,
            directives: 0,
            parse_diagnostics: 0,
            diagnostic_digest: sha256(b"invalid-utf8"),
            parser_panicked: false,
            program_end: 0,
            allocator_used_bytes: 0,
            allocator_capacity_bytes: 0,
            read_micros,
            parse_micros: 0,
            lower_micros: 0,
        });
    };
    let source_type = SourceType::from_path(&entry.path)
        .with_context(|| format!("derive OXC source type for {}", entry.path))?;
    let allocator = Allocator::default();
    let parse_started = Instant::now();
    let parser_return = Parser::new(&allocator, source, source_type).parse();
    let parse_micros = parse_started.elapsed().as_micros();
    let lower_started = Instant::now();
    let diagnostic_digest = {
        let mut hasher = Sha256::new();
        for diagnostic in &parser_return.errors {
            hasher.update(format!("{diagnostic:?}\n").as_bytes());
        }
        format!("{:x}", hasher.finalize())
    };
    let result = FileResult {
        path: entry.path.clone(),
        class: entry.class.clone(),
        bytes: entry.bytes,
        source_sha256,
        outcome: ParseOutcome::Parsed,
        top_level_statements: parser_return.program.body.len(),
        directives: parser_return.program.directives.len(),
        parse_diagnostics: parser_return.errors.len(),
        diagnostic_digest,
        parser_panicked: parser_return.panicked,
        program_end: parser_return.program.span.end,
        allocator_used_bytes: allocator.used_bytes(),
        allocator_capacity_bytes: allocator.capacity(),
        read_micros,
        parse_micros,
        lower_micros: lower_started.elapsed().as_micros(),
    };
    drop(parser_return);
    drop(allocator);
    Ok(result)
}

fn semantic_digest(results: &[FileResult]) -> String {
    let mut hasher = Sha256::new();
    for result in results {
        hasher.update(result.path.as_bytes());
        hasher.update(b"\0");
        hasher.update(result.source_sha256.as_bytes());
        hasher.update(b"\0");
        hasher.update(format!(
            "{:?}|{}|{}|{}|{}|{}|{}\n",
            result.outcome,
            result.top_level_statements,
            result.directives,
            result.parse_diagnostics,
            result.diagnostic_digest,
            result.parser_panicked,
            result.program_end
        ));
    }
    format!("{:x}", hasher.finalize())
}

fn summarize_wave(
    wave: usize,
    elapsed_micros: u128,
    semantic_digest: String,
    results: &[FileResult],
    memory_after_drop: crate::model::MemoryObservation,
) -> Result<WaveResult> {
    let parsed_files = results
        .iter()
        .filter(|result| matches!(result.outcome, ParseOutcome::Parsed))
        .count();
    let invalid_utf8_files = results.len() - parsed_files;
    let files_with_diagnostics = results
        .iter()
        .filter(|result| result.parse_diagnostics > 0)
        .count();
    let total_parse_diagnostics = results.iter().map(|result| result.parse_diagnostics).sum();
    let parser_panicked_files = results
        .iter()
        .filter(|result| result.parser_panicked)
        .count();
    let allocator_used_bytes_total = results.iter().try_fold(0_u64, |total, result| {
        total.checked_add(u64::try_from(result.allocator_used_bytes).ok()?)
    });
    let allocator_capacity_bytes_total = results.iter().try_fold(0_u64, |total, result| {
        total.checked_add(u64::try_from(result.allocator_capacity_bytes).ok()?)
    });
    Ok(WaveResult {
        wave,
        elapsed_micros,
        semantic_digest,
        parsed_files,
        invalid_utf8_files,
        files_with_diagnostics,
        total_parse_diagnostics,
        parser_panicked_files,
        allocator_used_bytes_total: allocator_used_bytes_total
            .context("allocator used total overflow")?,
        allocator_capacity_bytes_total: allocator_capacity_bytes_total
            .context("allocator capacity total overflow")?,
        allocator_used_bytes_max_file: results
            .iter()
            .map(|result| result.allocator_used_bytes as u64)
            .max()
            .unwrap_or(0),
        allocator_capacity_bytes_max_file: results
            .iter()
            .map(|result| result.allocator_capacity_bytes as u64)
            .max()
            .unwrap_or(0),
        read_micros_total: results.iter().map(|result| result.read_micros).sum(),
        parse_micros_total: results.iter().map(|result| result.parse_micros).sum(),
        lower_micros_total: results.iter().map(|result| result.lower_micros).sum(),
        memory_after_drop,
    })
}

#[cfg(test)]
mod tests {
    use oxc_allocator::Allocator;
    use oxc_parser::Parser;
    use oxc_span::SourceType;

    #[test]
    fn parses_typescript_with_worker_local_allocator() {
        let source = "export const answer: number = 42;";
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();
        assert!(parsed.errors.is_empty());
        assert_eq!(parsed.program.body.len(), 1);
        assert!(allocator.used_bytes() > 0);
        drop(parsed);
        drop(allocator);
    }
}
