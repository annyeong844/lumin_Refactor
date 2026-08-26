//! Corpus foundation orchestration and public-row registry.
//!
//! Exit codes: 0 = all selected rows pass, 1 = behavior failures/unmapped, 2 = tool error.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;

mod determinism;
mod registry;
mod required_checks;

use required_checks::{CheckOutcome, RequiredCheck};

const MAX_ROW_SHARDS: usize = 16;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatureSet {
    None,
    LifecycleFault,
    LifecycleCrash,
    PublicationCrash,
    RetentionCrash,
    LifecycleAndPublicationCrash,
    PublicationAndRetentionCrash,
}
impl FeatureSet {
    pub fn cargo_features(self) -> &'static [&'static str] {
        match self {
            Self::None => &[],
            Self::LifecycleFault | Self::LifecycleCrash => &["lifecycle-test-fault"],
            Self::PublicationCrash => &["publication-test-crash"],
            Self::RetentionCrash => &["retention-test-crash"],
            Self::LifecycleAndPublicationCrash => {
                &["lifecycle-test-fault", "publication-test-crash"]
            }
            Self::PublicationAndRetentionCrash => {
                &["publication-test-crash", "retention-test-crash"]
            }
        }
    }
    pub fn dir_key(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::LifecycleFault => "lf",
            Self::LifecycleCrash => "lc",
            Self::PublicationCrash => "pc",
            Self::RetentionCrash => "rc",
            Self::LifecycleAndPublicationCrash => "lfpc",
            Self::PublicationAndRetentionCrash => "pcrc",
        }
    }
    pub fn is_crash(self) -> bool {
        matches!(
            self,
            Self::LifecycleCrash
                | Self::PublicationCrash
                | Self::RetentionCrash
                | Self::LifecycleAndPublicationCrash
                | Self::PublicationAndRetentionCrash
        )
    }

    fn requires_process_isolation(self) -> bool {
        self != Self::None
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CorpusSelection {
    AllApplicable,
    MappedOnly,
}
impl CorpusSelection {
    fn as_str(self) -> &'static str {
        match self {
            Self::AllApplicable => "all-applicable",
            Self::MappedOnly => "mapped-only",
        }
    }
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
    /// Relative scheduling cost for determinism CI. Zero uses invocation count.
    pub determinism_shard_weight: usize,
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
    pub selection: CorpusSelection,
    pub row_jobs: usize,
    pub row_shard_index: usize,
    pub row_shard_count: usize,
}

pub fn parse_args(args: &[String]) -> Result<CorpusArgs, String> {
    let (
        mut mode,
        mut format,
        mut row,
        mut selection,
        mut row_jobs,
        mut row_shard_index,
        mut row_shard_count,
    ) = (
        CorpusMode::Standard,
        OutputFormat::Human,
        None,
        CorpusSelection::AllApplicable,
        None,
        None,
        None,
    );
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
            "--mapped-only" => {
                if selection == CorpusSelection::MappedOnly {
                    return Err("--mapped-only may be provided only once".to_owned());
                }
                selection = CorpusSelection::MappedOnly;
            }
            "--row-jobs" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| "--row-jobs requires a value".to_owned())?;
                let value = value
                    .parse::<usize>()
                    .map_err(|_| "--row-jobs must be an integer from 1 through 8".to_owned())?;
                if !(1..=8).contains(&value) {
                    return Err("--row-jobs must be an integer from 1 through 8".to_owned());
                }
                if row_jobs.replace(value).is_some() {
                    return Err("--row-jobs may be provided only once".to_owned());
                }
            }
            "--row-shard-index" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| "--row-shard-index requires a value".to_owned())?
                    .parse::<usize>()
                    .map_err(|_| "--row-shard-index requires an integer".to_owned())?;
                if row_shard_index.replace(value).is_some() {
                    return Err("--row-shard-index may be provided only once".to_owned());
                }
            }
            "--row-shard-count" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| "--row-shard-count requires a value".to_owned())?
                    .parse::<usize>()
                    .map_err(|_| "--row-shard-count requires an integer".to_owned())?;
                if row_shard_count.replace(value).is_some() {
                    return Err("--row-shard-count may be provided only once".to_owned());
                }
            }
            o => return Err(format!("unknown argument: {o}")),
        }
        i += 1;
    }
    if selection == CorpusSelection::MappedOnly && row.is_some() {
        return Err("--mapped-only cannot be combined with --row".to_owned());
    }
    let (row_shard_index, row_shard_count) = match (row_shard_index, row_shard_count) {
        (None, None) => (0, 1),
        (Some(index), Some(count)) if (1..=MAX_ROW_SHARDS).contains(&count) && index < count => {
            (index, count)
        }
        (Some(_), Some(_)) => {
            return Err(format!(
                "row shard count must be from 1 through {MAX_ROW_SHARDS} and index must be less than count"
            ));
        }
        _ => {
            return Err(
                "--row-shard-index and --row-shard-count must be provided together".to_owned(),
            );
        }
    };
    if row.is_some() && row_shard_count != 1 {
        return Err("--row cannot be combined with a multi-row shard".to_owned());
    }
    Ok(CorpusArgs {
        mode,
        format,
        row,
        selection,
        row_jobs: row_jobs.unwrap_or(1),
        row_shard_index,
        row_shard_count,
    })
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
    if REGISTRY.len() != 91 {
        return Err(format!("registry has {} rows, expected 91", REGISTRY.len()));
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
    let root = env::var_os("CARGO_TARGET_DIR").map_or_else(
        || ws.join("target"),
        |configured| {
            let configured = PathBuf::from(configured);
            if configured.is_absolute() {
                configured
            } else {
                ws.join(configured)
            }
        },
    );
    root.join("xc").join(m).join(feat.dir_key())
}

struct InvResult {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    semantic_captures: usize,
    marker_validated_internally: bool,
}

fn run_inv(
    ws: &Path,
    inv: &CorpusInvocation,
    mode: CorpusMode,
    row_id: &str,
    marker: &Path,
) -> InvResult {
    if mode == CorpusMode::Determinism {
        let result = determinism::run(ws, inv, row_id);
        return InvResult {
            success: result.success,
            stdout: result.stdout,
            stderr: result.stderr,
            semantic_captures: result.semantic_captures,
            marker_validated_internally: result.marker_validated,
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
            marker_validated_internally: false,
        },
        Err(e) => InvResult {
            success: false,
            stdout: Vec::new(),
            stderr: format!("spawn: {e}").into_bytes(),
            semantic_captures: 0,
            marker_validated_internally: false,
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

fn print_human(
    res: &[RowResult],
    mode: CorpusMode,
    selection: CorpusSelection,
    applicable_rows: usize,
    row_jobs: usize,
    row_shard_index: usize,
    row_shard_count: usize,
) {
    let (mapped, passed) = (
        res.iter().filter(|r| r.mapped).count(),
        res.iter().filter(|r| r.passed).count(),
    );
    let (unmapped, failed) = (res.len() - mapped, mapped - passed);
    println!("\n=== corpus foundation: {mode} ===");
    if selection == CorpusSelection::MappedOnly {
        println!(
            "selection: mapped-only ({} of {applicable_rows} applicable rows)",
            res.len()
        );
    }
    println!("row jobs: {row_jobs}");
    println!("row shard: {row_shard_index}/{row_shard_count}");
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

fn print_json(
    res: &[RowResult],
    mode: CorpusMode,
    selection: CorpusSelection,
    applicable_rows: usize,
    row_jobs: usize,
    row_shard_index: usize,
    row_shard_count: usize,
) -> Result<(), String> {
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
        "selection": selection.as_str(),
        "rowJobs": row_jobs,
        "rowShardIndex": row_shard_index,
        "rowShardCount": row_shard_count,
        "applicableRows": applicable_rows,
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

fn selected_rows(args: &CorpusArgs) -> Vec<&'static RegistryRow> {
    let eligible = REGISTRY
        .iter()
        .enumerate()
        .filter(|(_, registry_row)| {
            registry_row.is_applicable(args.mode)
                && args
                    .row
                    .as_deref()
                    .is_none_or(|selected| selected == registry_row.id)
                && (args.selection == CorpusSelection::AllApplicable
                    || registry_row.is_mapped(args.mode))
        })
        .collect::<Vec<_>>();
    balanced_shard_rows(
        eligible,
        args.mode,
        args.row_shard_index,
        args.row_shard_count,
    )
}

fn balanced_shard_rows(
    eligible: Vec<(usize, &'static RegistryRow)>,
    mode: CorpusMode,
    shard_index: usize,
    shard_count: usize,
) -> Vec<&'static RegistryRow> {
    if shard_count == 0 || shard_index >= shard_count {
        return Vec::new();
    }
    if shard_count == 1 {
        return eligible.into_iter().map(|(_, row)| row).collect();
    }

    let mut scheduling_order = eligible.clone();
    scheduling_order.sort_by(|(left_index, left), (right_index, right)| {
        shard_weight(right, mode)
            .cmp(&shard_weight(left, mode))
            .then_with(|| left_index.cmp(right_index))
    });

    let mut loads = vec![0usize; shard_count];
    let mut buckets = (0..shard_count).map(|_| Vec::new()).collect::<Vec<_>>();
    for (registry_index, row) in scheduling_order {
        let shard = (1..shard_count).fold(0, |best, candidate| {
            if (loads[candidate], candidate) < (loads[best], best) {
                candidate
            } else {
                best
            }
        });
        loads[shard] += shard_weight(row, mode);
        buckets[shard].push((registry_index, row));
    }

    let mut selected = buckets.remove(shard_index);
    selected.sort_by_key(|(registry_index, _)| *registry_index);
    selected.into_iter().map(|(_, row)| row).collect()
}

fn shard_weight(row: &RegistryRow, mode: CorpusMode) -> usize {
    let invocation_weight = row
        .mode_invocations(mode)
        .map(|invocations| invocations.len().max(1))
        .unwrap_or(1);
    if mode == CorpusMode::Determinism {
        invocation_weight.max(row.determinism_shard_weight)
    } else {
        invocation_weight
    }
}

struct RowExecution {
    result: RowResult,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

struct InvocationTask {
    row_id: &'static str,
    invocation: &'static CorpusInvocation,
    marker: PathBuf,
}

struct InvocationExecution {
    result: InvResult,
    marker_error: Option<String>,
}

fn execute_invocation(
    workspace: &Path,
    task: &InvocationTask,
    mode: CorpusMode,
) -> InvocationExecution {
    let _ = fs::remove_file(&task.marker);
    let result = run_inv(workspace, task.invocation, mode, task.row_id, &task.marker);
    let marker_error = if result.success && !result.marker_validated_internally {
        validate_marker(&task.marker, task.row_id, 1).err()
    } else {
        None
    };
    let _ = fs::remove_file(&task.marker);
    InvocationExecution {
        result,
        marker_error,
    }
}

pub(crate) fn run_parallel_ordered<T, F>(
    item_count: usize,
    jobs: usize,
    work: F,
) -> Result<Vec<T>, String>
where
    T: Send,
    F: Fn(usize) -> Result<T, String> + Sync,
{
    if item_count == 0 {
        return Ok(Vec::new());
    }
    if jobs == 0 {
        return Err("parallel worker count must be positive".to_owned());
    }
    let worker_count = jobs.min(item_count);
    let next = AtomicUsize::new(0);
    let (sender, receiver) = mpsc::channel::<(usize, Result<T, String>)>();
    let worker_result = thread::scope(|scope| {
        let mut handles = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            let sender = sender.clone();
            let next = &next;
            let work = &work;
            handles.push(scope.spawn(move || {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if index >= item_count {
                        break;
                    }
                    if sender.send((index, work(index))).is_err() {
                        break;
                    }
                }
            }));
        }
        for handle in handles {
            handle
                .join()
                .map_err(|_| "parallel worker panicked".to_owned())?;
        }
        Ok::<(), String>(())
    });
    drop(sender);
    worker_result?;

    let mut ordered = (0..item_count).map(|_| None).collect::<Vec<_>>();
    for (index, result) in receiver {
        if ordered[index].replace(result).is_some() {
            return Err(format!("corpus row {index} completed more than once"));
        }
    }
    ordered
        .into_iter()
        .enumerate()
        .map(|(index, result)| {
            result.ok_or_else(|| format!("corpus row {index} did not complete"))?
        })
        .collect()
}

fn run_parallel_ordered_with_isolation<T, F, I>(
    item_count: usize,
    jobs: usize,
    isolated: I,
    work: F,
) -> Result<Vec<T>, String>
where
    T: Send,
    F: Fn(usize) -> Result<T, String> + Sync,
    I: Fn(usize) -> bool,
{
    let mut ordered = Vec::with_capacity(item_count);
    let mut start = 0;
    while start < item_count {
        if isolated(start) {
            ordered.push(work(start)?);
            start += 1;
            continue;
        }
        let end = (start + 1..item_count)
            .find(|index| isolated(*index))
            .unwrap_or(item_count);
        let batch = run_parallel_ordered(end - start, jobs, |offset| work(start + offset))?;
        ordered.extend(batch);
        start = end;
    }
    Ok(ordered)
}

fn execute_rows(
    workspace: &Path,
    selected: &[&'static RegistryRow],
    mode: CorpusMode,
    check_outcomes: &BTreeMap<RequiredCheck, CheckOutcome>,
    row_jobs: usize,
) -> Result<Vec<RowExecution>, String> {
    let mut tasks = Vec::new();
    let mut row_ranges = Vec::with_capacity(selected.len());
    for row in selected {
        let start = tasks.len();
        if row.is_mapped(mode) {
            let invocations = row.mode_invocations(mode).ok_or_else(|| {
                format!(
                    "mapped row {} lacks mode {} invocations after registry validation",
                    row.id, mode
                )
            })?;
            tasks.extend(invocations.iter().map(|invocation| InvocationTask {
                row_id: row.id,
                invocation,
                marker: marker_path(row.id),
            }));
        }
        row_ranges.push(start..tasks.len());
    }

    let invocation_executions = run_parallel_ordered_with_isolation(
        tasks.len(),
        row_jobs,
        |index| {
            tasks[index]
                .invocation
                .features
                .requires_process_isolation()
        },
        |index| Ok(execute_invocation(workspace, &tasks[index], mode)),
    )?;

    let mut rows = Vec::with_capacity(selected.len());
    for (row, range) in selected.iter().zip(row_ranges) {
        if !row.is_mapped(mode) {
            rows.push(RowExecution {
                result: RowResult {
                    id: row.id,
                    mapped: false,
                    passed: false,
                    invocations: 0,
                    marker_ok: false,
                    semantic_captures: 0,
                    required_checks: row.required_checks,
                    required_checks_validated: false,
                },
                stdout: Vec::new(),
                stderr: Vec::new(),
            });
            continue;
        }

        let invocations = row.mode_invocations(mode).ok_or_else(|| {
            format!(
                "mapped row {} lacks mode {} invocations after registry validation",
                row.id, mode
            )
        })?;
        let executions = &invocation_executions[range];
        if invocations.len() != executions.len() {
            return Err(format!(
                "mapped row {} lost an invocation during parallel execution",
                row.id
            ));
        }

        let (mut passed, mut succeeded, mut semantic_captures) = (true, 0usize, 0usize);
        let mut marker_errors = Vec::new();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        for (invocation, execution) in invocations.iter().zip(executions) {
            semantic_captures += execution.result.semantic_captures;
            if execution.result.success {
                succeeded += 1;
                if let Some(error) = &execution.marker_error {
                    marker_errors.push(format!(
                        "{} / {} {}: {error}",
                        row.id, invocation.target, invocation.filter
                    ));
                }
            } else {
                passed = false;
                let _ = writeln!(
                    stderr,
                    "--- FAIL: {} / {} {} ---",
                    row.id, invocation.target, invocation.filter
                );
                stderr.extend_from_slice(&execution.result.stderr);
                stdout.extend_from_slice(&execution.result.stdout);
            }
        }

        let required_checks_validated = row.required_checks.iter().all(|check| {
            check_outcomes
                .get(check)
                .is_some_and(|outcome| outcome.passed)
        });
        if !required_checks_validated {
            passed = false;
        }
        if mode == CorpusMode::Determinism && passed && semantic_captures == 0 {
            let _ = writeln!(
                stderr,
                "[DETERMINISM] {} produced no canonical semantic evidence",
                row.id
            );
            passed = false;
        }
        let marker_ok = passed && succeeded > 0 && marker_errors.is_empty();
        if passed && !marker_ok {
            for error in marker_errors {
                let _ = writeln!(stderr, "[MARKER] {error}");
            }
            passed = false;
        }

        rows.push(RowExecution {
            result: RowResult {
                id: row.id,
                mapped: true,
                passed,
                invocations: invocations.len(),
                marker_ok,
                semantic_captures,
                required_checks: row.required_checks,
                required_checks_validated,
            },
            stdout,
            stderr,
        });
    }
    Ok(rows)
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
    let applicable_rows = REGISTRY
        .iter()
        .filter(|registry_row| registry_row.is_applicable(parsed.mode))
        .count();
    let selected = selected_rows(&parsed);

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

    let executions = match execute_rows(
        &ws,
        &selected,
        parsed.mode,
        &check_outcomes,
        parsed.row_jobs,
    ) {
        Ok(executions) => executions,
        Err(error) => {
            eprintln!("[TOOL ERROR] {error}");
            return ExitCode::from(2);
        }
    };
    let mut results = Vec::with_capacity(executions.len());
    for execution in executions {
        eprintln!("[CORPUS] {} row {}", parsed.mode, execution.result.id);
        let _ = std::io::stderr().write_all(&execution.stderr);
        let _ = std::io::stdout().write_all(&execution.stdout);
        if execution.result.mapped {
            eprintln!(
                "[CORPUS] {} row {} {} (semantic captures: {})",
                parsed.mode,
                execution.result.id,
                if execution.result.passed {
                    "passed"
                } else {
                    "failed"
                },
                execution.result.semantic_captures
            );
        }
        results.push(execution.result);
    }
    let has_fail = results.iter().any(|row| row.mapped && !row.passed);
    let has_unmap = results.iter().any(|row| !row.mapped);
    match parsed.format {
        OutputFormat::Human => print_human(
            &results,
            parsed.mode,
            parsed.selection,
            applicable_rows,
            parsed.row_jobs,
            parsed.row_shard_index,
            parsed.row_shard_count,
        ),
        OutputFormat::Json => {
            if let Err(e) = print_json(
                &results,
                parsed.mode,
                parsed.selection,
                applicable_rows,
                parsed.row_jobs,
                parsed.row_shard_index,
                parsed.row_shard_count,
            ) {
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
