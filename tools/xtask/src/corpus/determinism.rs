use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

use super::{
    CorpusInvocation, CorpusMode, marker_path, run_parallel_ordered, target_dir, validate_marker,
};

const CAPTURE_ENV: &str = "LUMIN_CORPUS_DETERMINISM_CAPTURE";
const JOBS_POLICY_ENV: &str = "LUMIN_CORPUS_JOBS_POLICY";

static CAPTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(super) struct DeterminismOutcome {
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub semantic_captures: usize,
    pub marker_validated: bool,
}

#[derive(Clone, Copy)]
enum Variant {
    DefaultFirst,
    DefaultSecond,
    One,
}

const VARIANTS: [Variant; 3] = [Variant::DefaultFirst, Variant::DefaultSecond, Variant::One];

impl Variant {
    fn key(self) -> &'static str {
        match self {
            Self::DefaultFirst => "default-a",
            Self::DefaultSecond => "default-b",
            Self::One => "one",
        }
    }

    fn jobs_policy(self) -> &'static str {
        match self {
            Self::DefaultFirst | Self::DefaultSecond => "default",
            Self::One => "one",
        }
    }
}

struct VariantOutcome {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    captures: Vec<Vec<u8>>,
    marker_validated: bool,
}

pub(super) fn run(
    workspace: &Path,
    invocation: &CorpusInvocation,
    row_id: &str,
) -> DeterminismOutcome {
    let variants = match run_parallel_ordered(VARIANTS.len(), VARIANTS.len(), |index| {
        let variant = VARIANTS[index];
        Ok((variant, run_variant(workspace, invocation, row_id, variant)))
    }) {
        Ok(variants) => variants,
        Err(error) => {
            return DeterminismOutcome {
                success: false,
                stdout: Vec::new(),
                stderr: format!("[DETERMINISM WORKER FAILED] {error}\n").into_bytes(),
                semantic_captures: 0,
                marker_validated: false,
            };
        }
    };

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut success = true;
    for (variant, outcome) in &variants {
        if !outcome.success {
            success = false;
            stderr.extend_from_slice(
                format!("[DETERMINISM {} PROCESS FAILED]\n", variant.key()).as_bytes(),
            );
            stderr.extend_from_slice(&outcome.stderr);
            stdout.extend_from_slice(&outcome.stdout);
        }
    }

    let first = &variants[0].1.captures;
    if success {
        for (variant, outcome) in &variants[1..] {
            if first != &outcome.captures {
                success = false;
                stderr.extend_from_slice(
                    format!(
                        "[DETERMINISM MISMATCH] {} {} != {} {}\n",
                        Variant::DefaultFirst.key(),
                        capture_summary(first),
                        variant.key(),
                        capture_summary(&outcome.captures),
                    )
                    .as_bytes(),
                );
            }
        }
    }

    DeterminismOutcome {
        success,
        stdout,
        stderr,
        semantic_captures: first.len(),
        marker_validated: variants.iter().all(|(_, outcome)| outcome.marker_validated),
    }
}

fn run_variant(
    workspace: &Path,
    invocation: &CorpusInvocation,
    row_id: &str,
    variant: Variant,
) -> VariantOutcome {
    let capture = capture_path(row_id, invocation, variant);
    let marker = marker_path(row_id);
    let _ = fs::remove_file(&capture);
    let _ = fs::remove_file(&marker);
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let mut command = Command::new(cargo);
    command
        .current_dir(workspace)
        .arg("test")
        .arg("--locked")
        .arg("-p")
        .arg("lumin-cli")
        .arg("--test")
        .arg(invocation.target);
    let features = invocation.features.cargo_features();
    if !features.is_empty() {
        command.arg("--features").arg(features.join(","));
    }
    command
        .arg(invocation.filter)
        .arg("--")
        .arg("--exact")
        .arg("--nocapture")
        .env(
            "CARGO_TARGET_DIR",
            target_dir(workspace, CorpusMode::Determinism, invocation.features),
        )
        .env("LUMIN_CORPUS_ROW", row_id)
        .env("LUMIN_CORPUS_CHILD_MARKER", &marker)
        .env(CAPTURE_ENV, &capture)
        .env(JOBS_POLICY_ENV, variant.jobs_policy())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = match command.output() {
        Ok(output) => output,
        Err(error) => {
            let _ = fs::remove_file(&capture);
            let _ = fs::remove_file(&marker);
            return VariantOutcome {
                success: false,
                stdout: Vec::new(),
                stderr: format!("spawn: {error}").into_bytes(),
                captures: Vec::new(),
                marker_validated: false,
            };
        }
    };
    let captures = read_capture(&capture);
    let marker_result = validate_marker(&marker, row_id, 1);
    let _ = fs::remove_file(&capture);
    let _ = fs::remove_file(&marker);

    let mut success = output.status.success();
    let mut stderr = output.stderr;
    let captures = match captures {
        Ok(captures) => captures,
        Err(error) => {
            success = false;
            stderr.extend_from_slice(format!("\n[CAPTURE] {error}\n").as_bytes());
            Vec::new()
        }
    };
    let marker_validated = marker_result.is_ok();
    if let Err(error) = marker_result {
        success = false;
        stderr.extend_from_slice(format!("\n[MARKER] {error}\n").as_bytes());
    }
    VariantOutcome {
        success,
        stdout: output.stdout,
        stderr,
        captures,
        marker_validated,
    }
}

fn capture_path(row_id: &str, invocation: &CorpusInvocation, variant: Variant) -> PathBuf {
    let sequence = CAPTURE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let safe = format!("{row_id}-{}-{}", invocation.target, variant.key())
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    env::temp_dir().join(format!(
        "lumin_corpus_determinism_{}_{}_{}.jsonl",
        std::process::id(),
        sequence,
        safe
    ))
}

fn read_capture(path: &Path) -> Result<Vec<Vec<u8>>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let mut records = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.is_empty() {
            return Err(format!(
                "{} contains an empty record at line {}",
                path.display(),
                index + 1
            ));
        }
        serde_json::from_str::<serde_json::Value>(line).map_err(|error| {
            format!(
                "{} contains malformed JSON at line {}: {error}",
                path.display(),
                index + 1
            )
        })?;
        records.push(line.as_bytes().to_vec());
    }
    records.sort();
    Ok(records)
}

fn capture_summary(records: &[Vec<u8>]) -> String {
    let mut hash = Sha256::new();
    for record in records {
        hash.update((record.len() as u64).to_be_bytes());
        hash.update(record);
    }
    let digest = hash.finalize();
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("records={} sha256={hex}", records.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_reader_sorts_records_and_rejects_malformed_json() -> Result<(), String> {
        let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
        let capture = directory.path().join("capture.jsonl");
        fs::write(&capture, "{\"z\":1}\n{\"a\":2}\n").map_err(|error| error.to_string())?;
        assert_eq!(
            read_capture(&capture)?,
            [b"{\"a\":2}".to_vec(), b"{\"z\":1}".to_vec()]
        );

        fs::write(&capture, "not-json\n").map_err(|error| error.to_string())?;
        assert!(read_capture(&capture).is_err());
        Ok(())
    }
}
