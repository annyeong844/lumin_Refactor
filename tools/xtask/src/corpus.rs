//! Corpus foundation orchestration and public-row registry.
//!
//! Exit codes: 0 = all selected rows pass, 1 = behavior failures/unmapped, 2 = tool error.

use std::collections::BTreeSet;
use std::env;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

mod determinism;
mod registry;
mod required_checks;

use required_checks::{CheckOutcome, RequiredCheck};

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatureSet {
    None,
    LifecycleFault,
    PublicationCrash,
    RetentionCrash,
    PublicationAndRetentionCrash,
}
impl FeatureSet {
    pub fn cargo_features(self) -> &'static [&'static str] {
        match self {
            Self::None => &[],
            Self::LifecycleFault => &["lifecycle-test-fault"],
            Self::PublicationCrash => &["publication-test-crash"],
            Self::RetentionCrash => &["retention-test-crash"],
            Self::PublicationAndRetentionCrash => {
                &["publication-test-crash", "retention-test-crash"]
            }
        }
    }
    pub fn dir_key(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::LifecycleFault => "lf",
            Self::PublicationCrash => "pc",
            Self::RetentionCrash => "rc",
            Self::PublicationAndRetentionCrash => "pcrc",
        }
    }
    pub fn is_crash(self) -> bool {
        matches!(
            self,
            Self::PublicationCrash | Self::RetentionCrash | Self::PublicationAndRetentionCrash
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CorpusMode {
    Standard,
    Determinism,
    StoreCrash,
}
impl fmt::Display for CorpusMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Standard => "standard",
            Self::Determinism => "determinism",
            Self::StoreCrash => "store-crash",
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputFormat {
    Human,
    Json,
}

#[derive(Clone, Debug)]
pub struct CorpusInvocation {
    pub target: &'static str,
    pub filter: &'static str,
    pub features: FeatureSet,
}

/// Per-mode applicability for a registry row.
/// - `None` = mode not applicable to this row (row skipped in that mode).
/// - `Some(&[])` = applicable but unmapped (required, causes exit 1).
/// - `Some(&[..])` = mapped with concrete invocations.
#[derive(Clone, Debug)]
pub struct RegistryRow {
    pub id: &'static str,
    pub standard: Option<&'static [CorpusInvocation]>,
    pub determinism: Option<&'static [CorpusInvocation]>,
    pub store_crash: Option<&'static [CorpusInvocation]>,
    pub required_checks: &'static [RequiredCheck],
}
impl RegistryRow {
    pub fn mode_invocations(&self, mode: CorpusMode) -> Option<&'static [CorpusInvocation]> {
        match mode {
            CorpusMode::Standard => self.standard,
            CorpusMode::Determinism => self.determinism,
            CorpusMode::StoreCrash => self.store_crash,
        }
    }
    pub fn is_applicable(&self, mode: CorpusMode) -> bool {
        self.mode_invocations(mode).is_some()
    }
    pub fn is_mapped(&self, mode: CorpusMode) -> bool {
        matches!(self.mode_invocations(mode), Some(s) if !s.is_empty())
    }
}

use registry::REGISTRY;

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

pub struct CorpusArgs {
    pub mode: CorpusMode,
    pub format: OutputFormat,
    pub row: Option<String>,
}

pub fn parse_args(args: &[String]) -> Result<CorpusArgs, String> {
    let (mut mode, mut format, mut row) = (CorpusMode::Standard, OutputFormat::Human, None);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--determinism" => {
                if mode != CorpusMode::Standard {
                    return Err("conflicting mode flags".into());
                }
                mode = CorpusMode::Determinism;
            }
            "--store-crash" => {
                if mode != CorpusMode::Standard {
                    return Err("conflicting mode flags".into());
                }
                mode = CorpusMode::StoreCrash;
            }
            "--format" => {
                i += 1;
                match args.get(i).map(|s| s.as_str()) {
                    Some("human") => format = OutputFormat::Human,
                    Some("json") => format = OutputFormat::Json,
                    Some(o) => return Err(format!("unknown format: {o}")),
                    None => return Err("--format requires a value".into()),
                }
            }
            "--row" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| "--row requires a value".to_owned())?;
                if value.is_empty() {
                    return Err("--row requires a nonempty value".to_owned());
                }
                if row.replace(value.clone()).is_some() {
                    return Err("--row may be provided only once".to_owned());
                }
            }
            o => return Err(format!("unknown argument: {o}")),
        }
        i += 1;
    }
    Ok(CorpusArgs { mode, format, row })
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

pub fn validate_registry() -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for row in REGISTRY {
        if row.id.is_empty() {
            return Err("empty corpus ID".into());
        }
        if !seen.insert(row.id) {
            return Err(format!("duplicate ID: {}", row.id));
        }
        // Verify invocation uniqueness per mode and feature consistency.
        for mode in [
            CorpusMode::Standard,
            CorpusMode::Determinism,
            CorpusMode::StoreCrash,
        ] {
            if let Some(invs) = row.mode_invocations(mode) {
                let mut inv_set = std::collections::HashSet::new();
                for inv in invs {
                    if inv.target.is_empty() {
                        return Err(format!("empty target in {}", row.id));
                    }
                    if inv.filter.is_empty() {
                        return Err(format!("empty filter in {}", row.id));
                    }
                    let key = (inv.target, inv.filter);
                    if !inv_set.insert(key) {
                        return Err(format!(
                            "duplicate invocation {}/{} in {} mode {}",
                            inv.target, inv.filter, row.id, mode
                        ));
                    }
                    // Feature consistency: standard allows None and LifecycleFault only.
                    // StoreCrash requires crash features only.
                    match mode {
                        CorpusMode::Standard => {
                            if inv.features.is_crash() {
                                return Err(format!(
                                    "crash feature in standard mode row {}",
                                    row.id
                                ));
                            }
                        }
                        CorpusMode::StoreCrash => {
                            if !inv.features.is_crash() && inv.features != FeatureSet::None {
                                return Err(format!(
                                    "non-crash non-none feature in store-crash row {}",
                                    row.id
                                ));
                            }
                        }
                        CorpusMode::Determinism => {}
                    }
                }
            }
        }
        if row.is_mapped(CorpusMode::Standard) != row.is_mapped(CorpusMode::Determinism) {
            return Err(format!(
                "standard/determinism mapping parity drift in {}",
                row.id
            ));
        }
        if row.required_checks != required_checks::expected_for_row(row.id) {
            return Err(format!(
                "required-check contract drift in {}: expected {:?}, found {:?}",
                row.id,
                required_checks::expected_for_row(row.id),
                row.required_checks,
            ));
        }
    }
    if REGISTRY.len() != 90 {
        return Err(format!("registry has {} rows, expected 90", REGISTRY.len()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Spec extraction (test-only)
// ---------------------------------------------------------------------------

#[cfg(test)]
pub fn extract_spec_ids(spec_text: &str) -> Result<Vec<&str>, String> {
    let mut ids = Vec::new();
    let (mut in_s9, mut in_table, mut hdr) = (false, false, false);
    for line in spec_text.lines() {
        if line.starts_with("## 9.") || line.starts_with("## 9 ") {
            in_s9 = true;
            continue;
        }
        if in_s9 && line.starts_with("## ") && !line.starts_with("## 9") {
            break;
        }
        if !in_s9 {
            continue;
        }
        let t = line.trim();
        if t.starts_with("| Corpus case") {
            in_table = true;
            continue;
        }
        if in_table && t.starts_with("| ---") {
            hdr = true;
            continue;
        }
        if in_table && hdr && t.starts_with("| `") {
            let after = &t[3..];
            if let Some(end) = after.find('`') {
                ids.push(&after[..end]);
            }
        }
        if in_table && hdr && !t.starts_with('|') && !t.is_empty() {
            break;
        }
    }
    Ok(ids)
}

// ---------------------------------------------------------------------------
// Marker system
// ---------------------------------------------------------------------------

static MARKER_COUNTER: AtomicU64 = AtomicU64::new(0);

fn marker_path(row_id: &str) -> PathBuf {
    let pid = std::process::id();
    let seq = MARKER_COUNTER.fetch_add(1, Ordering::Relaxed);
    let safe: String = row_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    env::temp_dir().join(format!("lumin_corpus_{pid}_{seq}_{safe}.marker"))
}

/// Validate marker file: at least `expected` lines, every line must equal
/// `row_id`, and no empty or non-matching lines are permitted.
pub fn validate_marker(path: &Path, row_id: &str, expected: usize) -> Result<(), String> {
    let content = fs::read_to_string(path).map_err(|e| format!("marker read: {e}"))?;
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() < expected {
        return Err(format!(
            "marker has {} lines, need >= {expected}",
            lines.len()
        ));
    }
    for (i, l) in lines.iter().enumerate() {
        if l.is_empty() {
            return Err(format!("marker line {i} is empty"));
        }
        if *l != row_id {
            return Err(format!("marker line {i} is {:?}, expected {:?}", l, row_id));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

fn target_dir(ws: &Path, mode: CorpusMode, feat: FeatureSet) -> PathBuf {
    let m = match mode {
        CorpusMode::Standard => "s",
        CorpusMode::Determinism => "d",
        CorpusMode::StoreCrash => "c",
    };
    ws.join("target").join("xc").join(m).join(feat.dir_key())
}

struct InvResult {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    semantic_captures: usize,
}

fn run_inv(
    ws: &Path,
    inv: &CorpusInvocation,
    mode: CorpusMode,
    row_id: &str,
    marker: &Path,
) -> InvResult {
    if mode == CorpusMode::Determinism {
        let result = determinism::run(ws, inv, row_id, marker);
        return InvResult {
            success: result.success,
            stdout: result.stdout,
            stderr: result.stderr,
            semantic_captures: result.semantic_captures,
        };
    }
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let td = target_dir(ws, mode, inv.features);
    let mut cmd = Command::new(&cargo);
    cmd.current_dir(ws)
        .arg("test")
        .arg("--locked")
        .arg("-p")
        .arg("lumin-cli")
        .arg("--test")
        .arg(inv.target);
    let feats = inv.features.cargo_features();
    if !feats.is_empty() {
        cmd.arg("--features").arg(feats.join(","));
    }
    cmd.arg(inv.filter)
        .arg("--")
        .arg("--exact")
        .arg("--nocapture");
    cmd.env("CARGO_TARGET_DIR", td.to_string_lossy().as_ref());
    cmd.env("LUMIN_CORPUS_ROW", row_id);
    cmd.env(
        "LUMIN_CORPUS_CHILD_MARKER",
        marker.to_string_lossy().as_ref(),
    );
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    match cmd.output() {
        Ok(o) => InvResult {
            success: o.status.success(),
            stdout: o.stdout,
            stderr: o.stderr,
            semantic_captures: 0,
        },
        Err(e) => InvResult {
            success: false,
            stdout: Vec::new(),
            stderr: format!("spawn: {e}").into_bytes(),
            semantic_captures: 0,
        },
    }
}

// ---------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct RowResult {
    id: &'static str,
    mapped: bool,
    passed: bool,
    invocations: usize,
    marker_ok: bool,
    semantic_captures: usize,
    required_checks: &'static [RequiredCheck],
    required_checks_validated: bool,
}

fn print_human(res: &[RowResult], mode: CorpusMode) {
    let (mapped, passed) = (
        res.iter().filter(|r| r.mapped).count(),
        res.iter().filter(|r| r.passed).count(),
    );
    let (unmapped, failed) = (res.len() - mapped, mapped - passed);
    println!("\n=== corpus foundation: {mode} ===");
    println!(
        "total: {}  mapped: {mapped}  passed: {passed}  unmapped: {unmapped}  failed: {failed}\n",
        res.len()
    );
    if unmapped > 0 {
        println!("unmapped:");
        for r in res.iter().filter(|r| !r.mapped) {
            println!("  {}", r.id);
        }
        println!();
    }
    if failed > 0 {
        println!("failed:");
        for r in res.iter().filter(|r| r.mapped && !r.passed) {
            println!("  {}", r.id);
        }
        println!();
    }
}

fn print_json(res: &[RowResult], mode: CorpusMode) -> Result<(), String> {
    let rows: Vec<serde_json::Value> = res
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "mapped": r.mapped,
                "passed": r.passed,
                "invocations": r.invocations,
                "markerValidated": r.marker_ok,
                "semanticCaptures": r.semantic_captures,
                "requiredChecks": r.required_checks.iter().map(|check| check.name()).collect::<Vec<_>>(),
                "requiredChecksValidated": r.required_checks_validated,
            })
        })
        .collect();
    let (mapped, passed) = (
        res.iter().filter(|r| r.mapped).count(),
        res.iter().filter(|r| r.passed).count(),
    );
    let s = serde_json::json!({
        "mode": mode.to_string(),
        "totalRows": res.len(),
        "mapped": mapped,
        "passed": passed,
        "unmapped": res.len() - mapped,
        "failed": mapped - passed,
        "rows": rows,
    });
    let text = serde_json::to_string_pretty(&s).map_err(|e| format!("json serialization: {e}"))?;
    println!("{text}");
    Ok(())
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run(args: &[String]) -> ExitCode {
    if args.first().map(|s| s.as_str()) != Some("foundation") {
        eprintln!("[CORPUS ERROR] subcommand must be 'foundation'");
        return ExitCode::from(2);
    }
    let parsed = match parse_args(&args[1..]) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("[CORPUS ERROR] {e}");
            return ExitCode::from(2);
        }
    };
    if let Err(e) = validate_registry() {
        eprintln!("[REGISTRY ERROR] {e}");
        return ExitCode::from(2);
    }
    let ws = match crate::metadata::find_workspace_root() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[TOOL ERROR] {e}");
            return ExitCode::from(2);
        }
    };
    let selected: Vec<&RegistryRow> = REGISTRY
        .iter()
        .filter(|registry_row| {
            registry_row.is_applicable(parsed.mode)
                && parsed
                    .row
                    .as_deref()
                    .is_none_or(|selected| selected == registry_row.id)
        })
        .collect();

    if selected.is_empty() {
        eprintln!(
            "[CORPUS ERROR] mode {} with row {:?} selects zero rows",
            parsed.mode, parsed.row
        );
        return ExitCode::from(2);
    }

    let selected_checks: BTreeSet<RequiredCheck> = selected
        .iter()
        .filter(|row| row.is_mapped(parsed.mode))
        .flat_map(|row| row.required_checks.iter().copied())
        .collect();
    let check_outcomes = match required_checks::run_required_checks(&ws, &selected_checks) {
        Ok(outcomes) => outcomes,
        Err(error) => {
            eprintln!("[TOOL ERROR] {error}");
            return ExitCode::from(2);
        }
    };
    for (check, outcome) in &check_outcomes {
        eprintln!(
            "[CORPUS] required check {} {}",
            check.name(),
            if outcome.passed { "passed" } else { "failed" },
        );
        if !outcome.passed {
            print_required_check_failure(*check, outcome);
        }
    }

    let (mut results, mut has_fail, mut has_unmap) =
        (Vec::with_capacity(selected.len()), false, false);
    for row in &selected {
        eprintln!("[CORPUS] {} row {}", parsed.mode, row.id);
        if !row.is_mapped(parsed.mode) {
            results.push(RowResult {
                id: row.id,
                mapped: false,
                passed: false,
                invocations: 0,
                marker_ok: false,
                semantic_captures: 0,
                required_checks: row.required_checks,
                required_checks_validated: false,
            });
            has_unmap = true;
            continue;
        }
        let Some(invs) = row.mode_invocations(parsed.mode) else {
            eprintln!(
                "[REGISTRY ERROR] mapped row {} lacks mode {} invocations",
                row.id, parsed.mode
            );
            return ExitCode::from(2);
        };
        let mp = marker_path(row.id);
        let _ = fs::remove_file(&mp);
        let (mut ok, mut succ, mut semantic_captures) = (true, 0usize, 0usize);
        for inv in invs {
            let r = run_inv(&ws, inv, parsed.mode, row.id, &mp);
            semantic_captures += r.semantic_captures;
            if r.success {
                succ += 1;
            } else {
                ok = false;
                eprintln!("--- FAIL: {} / {} {} ---", row.id, inv.target, inv.filter);
                let _ = std::io::stderr().write_all(&r.stderr);
                let _ = std::io::stdout().write_all(&r.stdout);
            }
        }
        let required_checks_validated = row.required_checks.iter().all(|check| {
            check_outcomes
                .get(check)
                .is_some_and(|outcome| outcome.passed)
        });
        if !required_checks_validated {
            ok = false;
        }
        if parsed.mode == CorpusMode::Determinism && ok && semantic_captures == 0 {
            eprintln!(
                "[DETERMINISM] {} produced no canonical semantic evidence",
                row.id
            );
            ok = false;
        }
        let m_ok = if ok && succ > 0 {
            match validate_marker(&mp, row.id, succ) {
                Ok(()) => {
                    let _ = fs::remove_file(&mp);
                    true
                }
                Err(e) => {
                    eprintln!("[MARKER] {}: {e}", row.id);
                    ok = false;
                    false
                }
            }
        } else {
            false
        };
        if !ok {
            has_fail = true;
        }
        results.push(RowResult {
            id: row.id,
            mapped: true,
            passed: ok,
            invocations: invs.len(),
            marker_ok: m_ok,
            semantic_captures,
            required_checks: row.required_checks,
            required_checks_validated,
        });
        eprintln!(
            "[CORPUS] {} row {} {} (semantic captures: {})",
            parsed.mode,
            row.id,
            if ok { "passed" } else { "failed" },
            semantic_captures
        );
    }
    match parsed.format {
        OutputFormat::Human => print_human(&results, parsed.mode),
        OutputFormat::Json => {
            if let Err(e) = print_json(&results, parsed.mode) {
                eprintln!("[TOOL ERROR] {e}");
                return ExitCode::from(2);
            }
        }
    }
    if has_fail || has_unmap {
        ExitCode::from(1)
    } else {
        ExitCode::from(0)
    }
}

fn print_required_check_failure(check: RequiredCheck, outcome: &CheckOutcome) {
    eprintln!("--- FAIL: required check {} ---", check.name());
    if !outcome.stdout.is_empty() {
        eprintln!("{}", outcome.stdout);
    }
    if !outcome.stderr.is_empty() {
        eprintln!("{}", outcome.stderr);
    }
}

#[cfg(test)]
#[path = "corpus/tests.rs"]
mod tests;
