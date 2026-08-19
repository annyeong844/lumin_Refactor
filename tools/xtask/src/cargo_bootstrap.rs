//! Structural CI routing checks for the separate dependency-admission guard.
//!
//! This module prevents accidental workflow drift. It deliberately does not
//! authenticate repository files, invoke Cargo, or recreate the Python guard's
//! dependency verdict.

use std::collections::BTreeMap;
use std::path::Path;

const WORKFLOW_DIRECTORY: &str = ".github/workflows";
const WORKFLOW_PATH: &str = ".github/workflows/ci.yml";
const CHECKOUT: &str = "actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0 # v7.0.0";
const SETUP_PYTHON: &str = "actions/setup-python@5fda3b95a4ea91299a34e894583c3862153e4b97 # v7.0.0";
const INSTALL_TOOLS: &str =
    "taiki-e/install-action@07b4745e0c39a41822af610387492e3e53aa222b # v2.83.4";
const GUARD_PREFIX: &str = concat!(
    "& \"$env:PINNED_PYTHON\" -I -S ",
    "tools/xtask/bootstrap/source_provenance.py"
);
const TEST_COMMAND: &str = concat!(
    "& \"$env:PINNED_PYTHON\" -I -S ",
    "tools/xtask/bootstrap/test_source_provenance.py"
);
const CI_POLICY_COMMAND: &str = concat!(
    "& \"$env:PINNED_PYTHON\" -I -S ",
    "tools/xtask/bootstrap/ci_policy.py github"
);
const CI_POLICY_TEST_COMMAND: &str = concat!(
    "& \"$env:PINNED_PYTHON\" -I -S ",
    "tools/xtask/bootstrap/test_ci_policy.py"
);
const FULL_SCOPE_CONDITION: &str = "if: ${{ needs.policy.outputs.run_full == 'true' }}";
const PRIVATE_CARGO_HOME: &str =
    "\"CARGO_HOME=$env:RUNNER_TEMP/lumin-cargo-home\" >> $env:GITHUB_ENV";
const PRIVATE_TARGET: &str =
    "\"CARGO_TARGET_DIR=$env:RUNNER_TEMP/lumin-target\" >> $env:GITHUB_ENV";
const DIRECT_AUDIT: &str = "& \"$env:PINNED_CARGO_AUDIT\" audit --deny warnings";
const DIRECT_DENY: &str = "& \"$env:PINNED_CARGO_DENY\" --locked check bans licenses sources";
const AUDIT_INSTALL: &str = "cargo-audit@0.22.2";
const DENY_INSTALL: &str = "cargo-deny@0.20.2";
const STRUCTURAL_CHECK: &str = concat!(
    "& (Join-Path $env:CARGO_TARGET_DIR ",
    "'debug/lumin-xtask') architecture-check"
);
const CORPUS_RUN: &str = concat!(
    "& \"$env:PINNED_PYTHON\" -I -S tools/xtask/bootstrap/source_provenance.py ",
    "-- cargo run --locked -p lumin-xtask -- corpus ${{ matrix.arguments }}"
);
const WINDOWS_INTEGRATION_RUN: &str = concat!(
    "& \"$env:PINNED_PYTHON\" -I -S tools/xtask/bootstrap/source_provenance.py ",
    "-- cargo run --locked -p lumin-xtask -- ci-test-shard ",
    "--index ${{ matrix.shard }} --count 6 --jobs 4"
);
const ALL_TARGET_TEST: &str = concat!(
    "& \"$env:PINNED_PYTHON\" -I -S tools/xtask/bootstrap/source_provenance.py ",
    "-- cargo test --workspace --all-targets --locked"
);
const CORE_TARGET_TEST: &str = concat!(
    "& \"$env:PINNED_PYTHON\" -I -S tools/xtask/bootstrap/source_provenance.py ",
    "-- cargo test --workspace --lib --bins --locked"
);
const ENGINE_INTEGRATION_TEST: &str = concat!(
    "& \"$env:PINNED_PYTHON\" -I -S tools/xtask/bootstrap/source_provenance.py ",
    "-- cargo test --locked -p lumin-engine --tests"
);
const SFC_INTEGRATION_TEST: &str = concat!(
    "& \"$env:PINNED_PYTHON\" -I -S tools/xtask/bootstrap/source_provenance.py ",
    "-- cargo test --locked -p lumin-sfc --tests"
);
const UBUNTU_MAPPED_CORPUS_CASES: &[(&str, &str)] = &[
    ("mapped-standard", "foundation --mapped-only --row-jobs 8"),
    (
        "mapped-determinism",
        "foundation --determinism --mapped-only --row-jobs 2",
    ),
];
const CRASH_CORPUS_CASES: &[(&str, &str)] = &[
    (
        "retention-crash-protocol",
        "foundation --store-crash --row retention-crash-protocol",
    ),
    (
        "crash-publication",
        "foundation --store-crash --row crash-publication",
    ),
    (
        "concurrent-latest-publication",
        "foundation --store-crash --row concurrent-latest-publication",
    ),
    (
        "publication-retention-race",
        "foundation --store-crash --row publication-retention-race",
    ),
];

#[derive(Debug, Default)]
pub struct CargoBootstrapResult {
    pub violations: Vec<String>,
    pub tool_errors: Vec<String>,
}

pub fn check_cargo_bootstrap(workspace_root: &Path) -> CargoBootstrapResult {
    let mut result = CargoBootstrapResult::default();
    validate_workflow_directory(workspace_root, &mut result);
    let workflow = match std::fs::read_to_string(workspace_root.join(WORKFLOW_PATH)) {
        Ok(source) => source,
        Err(error) => {
            result
                .tool_errors
                .push(format!("cannot read {WORKFLOW_PATH}: {error}"));
            return result;
        }
    };
    validate_workflow(&workflow, &mut result.violations);
    validate_no_nested_dependency_admission(workspace_root, &mut result);
    result
}

fn validate_workflow_directory(root: &Path, result: &mut CargoBootstrapResult) {
    let directory = root.join(WORKFLOW_DIRECTORY);
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) => {
            result
                .tool_errors
                .push(format!("cannot enumerate {}: {error}", directory.display()));
            return;
        }
    };
    let mut workflows = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => {
                let path = entry.path();
                if matches!(
                    path.extension().and_then(|value| value.to_str()),
                    Some("yml" | "yaml")
                ) {
                    workflows.push(entry.file_name().to_string_lossy().into_owned());
                }
            }
            Err(error) => result
                .tool_errors
                .push(format!("cannot enumerate workflow entry: {error}")),
        }
    }
    workflows.sort();
    if workflows != ["ci.yml"] {
        result.violations.push(format!(
            "review every workflow or retain only ci.yml; found {workflows:?}"
        ));
    }
}

fn validate_workflow(source: &str, violations: &mut Vec<String>) {
    if source.contains("actions/cache")
        || source.contains("rust-cache")
        || source
            .lines()
            .any(|line| line.trim_start().starts_with("cache:"))
    {
        violations.push("public Cargo jobs must not restore a dependency cache".to_owned());
    }
    validate_actions(source, violations);
    let jobs = job_blocks(source);
    for required in [
        "policy",
        "formatting",
        "architecture-check",
        "bootstrap-tests",
        "dependency-policy",
        "platform",
        "windows-integration",
        "corpus",
        "documentation",
        "release",
        "required",
    ] {
        if !jobs.contains_key(required) {
            violations.push(format!("required CI job is missing: {required}"));
        }
    }
    for (name, block) in &jobs {
        validate_job(name, block, violations);
    }
    validate_architecture_job(&jobs, violations);
    validate_policy_job(&jobs, violations);
    validate_dependency_job(&jobs, violations);
    validate_platform_job(&jobs, violations);
    validate_windows_integration_job(&jobs, violations);
    validate_corpus_job(&jobs, violations);
    validate_bootstrap_test_job(&jobs, violations);
    validate_required_job(&jobs, violations);
}

fn validate_actions(source: &str, violations: &mut Vec<String>) {
    let allowed = [CHECKOUT, SETUP_PYTHON, INSTALL_TOOLS];
    for line in source.lines() {
        let Some(action) = line.trim().strip_prefix("uses: ") else {
            continue;
        };
        if !allowed.contains(&action) {
            violations.push(format!("unreviewed or unpinned CI action: {action}"));
        }
    }
}

fn job_blocks(source: &str) -> BTreeMap<String, String> {
    let mut jobs = BTreeMap::new();
    let mut in_jobs = false;
    let mut current: Option<(String, String)> = None;
    for line in source.lines() {
        if line == "jobs:" {
            in_jobs = true;
            continue;
        }
        if !in_jobs {
            continue;
        }
        let indent = line.len() - line.trim_start_matches(' ').len();
        let trimmed = line.trim();
        if indent == 2 && trimmed.ends_with(':') {
            if let Some((name, block)) = current.take() {
                jobs.insert(name, block);
            }
            current = Some((trimmed.trim_end_matches(':').to_owned(), String::new()));
        } else if let Some((_, block)) = current.as_mut() {
            block.push_str(line);
            block.push('\n');
        }
    }
    if let Some((name, block)) = current {
        jobs.insert(name, block);
    }
    jobs
}

fn validate_job(name: &str, block: &str, violations: &mut Vec<String>) {
    let guarded = block.contains("tools/xtask/bootstrap/source_provenance.py");
    let cargo_job = guarded || block.contains("$env:PINNED_CARGO");
    let checkout = block.find(&format!("uses: {CHECKOUT}"));
    let private_home = block.find(PRIVATE_CARGO_HOME);
    let private_target = block.find(PRIVATE_TARGET);
    if cargo_job
        && (!matches!((private_home, checkout), (Some(home), Some(checkout)) if home < checkout)
            || !matches!((private_target, checkout), (Some(target), Some(checkout)) if target < checkout)
            || block.matches(PRIVATE_CARGO_HOME).count() != 1
            || block.matches(PRIVATE_TARGET).count() != 1)
    {
        violations.push(format!(
            "Cargo job {name} must initialize one job-private Cargo home and target directory before checkout"
        ));
    }
    if name != "required" && !block.contains(&format!("uses: {CHECKOUT}")) {
        violations.push(format!("CI job {name} lacks the pinned checkout action"));
    }
    if guarded && !block.contains(&format!("uses: {SETUP_PYTHON}")) {
        violations.push(format!("guarded job {name} lacks pinned Python setup"));
    }

    for line in run_commands(block) {
        if line.contains("tools/xtask/bootstrap/source_provenance.py") {
            validate_guard_command(name, &line, violations);
        }
        if line.contains("test_source_provenance.py") && line != TEST_COMMAND {
            violations.push(format!(
                "bootstrap tests in {name} must use the pinned isolated Python command"
            ));
        }
        if line.contains("test_ci_policy.py") && line != CI_POLICY_TEST_COMMAND {
            violations.push(format!(
                "CI policy tests in {name} must use the pinned isolated Python command"
            ));
        }
        if is_unwrapped_dependency_command(&line) {
            violations.push(format!(
                "dependency-resolving command in {name} bypasses the guard: {line}"
            ));
        }
        if line.starts_with("& \"$env:PINNED_CARGO\"")
            && line != "& \"$env:PINNED_CARGO\" fmt --all --check"
        {
            violations.push(format!(
                "only non-resolving fmt may invoke PINNED_CARGO directly in {name}: {line}"
            ));
        }
        if !is_reviewed_run_command(&line) {
            violations.push(format!(
                "unreviewed or reconstructed run command in {name}: {line}"
            ));
        }
    }
}

fn run_commands(block: &str) -> Vec<String> {
    let lines = block.lines().collect::<Vec<_>>();
    let mut commands = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let raw = lines[index];
        let trimmed = raw.trim();
        let value = trimmed
            .strip_prefix("run: ")
            .or_else(|| trimmed.strip_prefix("- run: "));
        let Some(value) = value else {
            index += 1;
            continue;
        };
        if value == "|" {
            let run_indent = raw.len() - raw.trim_start_matches(' ').len();
            index += 1;
            while index < lines.len() {
                let command = lines[index];
                let indent = command.len() - command.trim_start_matches(' ').len();
                if !command.trim().is_empty() && indent <= run_indent {
                    break;
                }
                if !command.trim().is_empty() {
                    commands.push(command_text(command));
                }
                index += 1;
            }
        } else {
            commands.push(command_text(raw));
            index += 1;
        }
    }
    commands
}

fn command_text(raw: &str) -> String {
    let mut line = raw.trim();
    if let Some(value) = line
        .strip_prefix("run: ")
        .or_else(|| line.strip_prefix("- run: "))
    {
        line = value;
    }
    if line.len() >= 2 && line.starts_with('\'') && line.ends_with('\'') {
        line[1..line.len() - 1].replace("''", "'")
    } else {
        line.to_owned()
    }
}

fn is_reviewed_run_command(line: &str) -> bool {
    const EXACT: &[&str] = &[
        "rustup toolchain install 1.96.0 --profile minimal --component rustfmt --no-self-update",
        "rustup toolchain install 1.96.0 --profile minimal --no-self-update",
        "rustup toolchain install 1.96.0 --profile minimal --component clippy,rustfmt --no-self-update",
        "$cargo = rustup which --toolchain 1.96.0 cargo",
        "$clippy = rustup which --toolchain 1.96.0 cargo-clippy",
        "$python = & \"$env:SETUP_PYTHON\" -I -S -c \"import pathlib,sys; print(pathlib.Path(sys.executable).resolve(strict=True))\"",
        "if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }",
        "$suffix = if ($IsWindows) { '.exe' } else { '' }",
        "$auditCommand = (Get-Command cargo-audit -CommandType Application -ErrorAction Stop).Source",
        "$denyCommand = (Get-Command cargo-deny -CommandType Application -ErrorAction Stop).Source",
        "$audit = (Resolve-Path -LiteralPath $auditCommand -ErrorAction Stop).ProviderPath",
        "$deny = (Resolve-Path -LiteralPath $denyCommand -ErrorAction Stop).ProviderPath",
        "if ($LASTEXITCODE -ne 0 -or [IO.Path]::GetFileName($audit) -cne \"cargo-audit$suffix\" -or [IO.Path]::GetFileName($deny) -cne \"cargo-deny$suffix\") { exit 1 }",
        "\"PINNED_PYTHON=$python\" >> $env:GITHUB_ENV",
        "\"PINNED_CARGO=$cargo\" >> $env:GITHUB_ENV",
        "\"PINNED_CARGO_CLIPPY=$clippy\" >> $env:GITHUB_ENV",
        "\"PINNED_CARGO_AUDIT=$audit\" >> $env:GITHUB_ENV",
        "\"PINNED_CARGO_DENY=$deny\" >> $env:GITHUB_ENV",
        PRIVATE_CARGO_HOME,
        PRIVATE_TARGET,
        "& \"$env:PINNED_CARGO\" fmt --all --check",
        TEST_COMMAND,
        DIRECT_AUDIT,
        DIRECT_DENY,
        STRUCTURAL_CHECK,
        CI_POLICY_COMMAND,
        CI_POLICY_TEST_COMMAND,
    ];
    line.starts_with(GUARD_PREFIX)
        || EXACT.contains(&line)
        || matches!(
            line,
            "test \"$ARCHITECTURE_CHECK_RESULT\" = success"
                | "test \"$POLICY_RESULT\" = success"
                | "test \"$RUN_FULL\" = \"$EXPECTED_RUN_FULL\""
                | "test \"$POLICY_SCOPE\" = \"$EXPECTED_SCOPE\""
                | "test \"$FORMATTING_RESULT\" = \"$EXPECTED_HEAVY_RESULT\""
                | "test \"$BOOTSTRAP_TESTS_RESULT\" = \"$EXPECTED_HEAVY_RESULT\""
                | "test \"$DEPENDENCY_POLICY_RESULT\" = \"$EXPECTED_HEAVY_RESULT\""
                | "test \"$PLATFORM_RESULT\" = \"$EXPECTED_HEAVY_RESULT\""
                | "test \"$WINDOWS_INTEGRATION_RESULT\" = \"$EXPECTED_HEAVY_RESULT\""
                | "test \"$CORPUS_RESULT\" = \"$EXPECTED_HEAVY_RESULT\""
                | "test \"$DOCUMENTATION_RESULT\" = \"$EXPECTED_HEAVY_RESULT\""
                | "test \"$RELEASE_RESULT\" = \"$EXPECTED_HEAVY_RESULT\""
        )
}

fn validate_guard_command(name: &str, line: &str, violations: &mut Vec<String>) {
    if !line.starts_with(GUARD_PREFIX) {
        violations.push(format!(
            "guard in {name} must use absolute PINNED_PYTHON with -I -S: {line}"
        ));
        return;
    }
    let suffix = &line[GUARD_PREFIX.len()..];
    if [";", "|", "`", "$(", "@(", "&"]
        .iter()
        .any(|token| suffix.contains(token))
    {
        violations.push(format!(
            "guard command in {name} contains shell composition: {line}"
        ));
        return;
    }
    if suffix == " --check-only" {
        return;
    }
    let Some(cargo) = suffix.strip_prefix(" -- cargo ") else {
        violations.push(format!("unsupported guard command in {name}: {line}"));
        return;
    };
    let before_runtime = cargo.split(" -- ").next().unwrap_or(cargo);
    if before_runtime
        .split_whitespace()
        .filter(|token| *token == "--locked")
        .count()
        != 1
    {
        violations.push(format!(
            "guarded Cargo command in {name} needs exactly one pre-delimiter --locked: {line}"
        ));
    }
    if cargo.starts_with("audit ") || cargo.starts_with("deny ") {
        violations.push(format!(
            "Cargo plugins in {name} must use their pinned executables directly"
        ));
    }
}

fn is_unwrapped_dependency_command(line: &str) -> bool {
    if line.starts_with(GUARD_PREFIX) || line.starts_with("rustup ") {
        return false;
    }
    let lower = line.to_ascii_lowercase();
    [
        "cargo build",
        "cargo check",
        "cargo test",
        "cargo clippy",
        "cargo doc",
        "cargo run",
        "cargo bench",
        "cargo metadata",
        "cargo audit",
        "cargo deny",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn validate_architecture_job(jobs: &BTreeMap<String, String>, violations: &mut Vec<String>) {
    let Some(block) = jobs.get("architecture-check") else {
        return;
    };
    let commands = block.lines().map(command_text).collect::<Vec<_>>();
    let check = commands
        .iter()
        .position(|line| line == &format!("{GUARD_PREFIX} --check-only"));
    let build = commands
        .iter()
        .position(|line| line == &format!("{GUARD_PREFIX} -- cargo build --locked -p lumin-xtask"));
    let structural = commands.iter().position(|line| line == STRUCTURAL_CHECK);
    if !matches!((check, build, structural), (Some(a), Some(b), Some(c)) if a < b && b < c) {
        violations.push(
            "architecture job must expose ordered dependency admission, guarded build, and structural verdict steps"
                .to_owned(),
        );
    }
    if block.matches("--check-only").count() != 1 {
        violations.push("architecture job must expose exactly one dependency verdict".to_owned());
    }
}

fn validate_policy_job(jobs: &BTreeMap<String, String>, violations: &mut Vec<String>) {
    let Some(block) = jobs.get("policy") else {
        return;
    };
    for required_line in [
        "scope: ${{ steps.policy.outputs.scope }}",
        "run_full: ${{ steps.policy.outputs.run_full }}",
        "fetch-depth: 0",
        "PR_BASE_SHA: ${{ github.event.pull_request.base.sha }}",
        "PR_HEAD_SHA: ${{ github.event.pull_request.head.sha }}",
        "CURRENT_SHA: ${{ github.sha }}",
    ] {
        if block
            .lines()
            .map(str::trim)
            .filter(|line| *line == required_line)
            .count()
            != 1
        {
            violations.push(format!(
                "change-policy job must bind reviewed input exactly once: {required_line}"
            ));
        }
    }
    let commands = block.lines().map(command_text).collect::<Vec<_>>();
    if commands
        .iter()
        .filter(|command| **command == CI_POLICY_COMMAND)
        .count()
        != 1
    {
        violations.push("change-policy job must run the reviewed policy command once".to_owned());
    }
    if commands
        .iter()
        .filter(|command| **command == CI_POLICY_TEST_COMMAND)
        .count()
        != 1
    {
        violations.push("change-policy job must run its focused tests once".to_owned());
    }
    let test_position = commands
        .iter()
        .position(|command| *command == CI_POLICY_TEST_COMMAND);
    let policy_position = commands
        .iter()
        .position(|command| *command == CI_POLICY_COMMAND);
    if !matches!(
        (test_position, policy_position),
        (Some(test), Some(policy)) if test < policy
    ) {
        violations.push("change-policy tests must pass before classification".to_owned());
    }

    for name in [
        "formatting",
        "bootstrap-tests",
        "dependency-policy",
        "platform",
        "windows-integration",
        "corpus",
        "documentation",
        "release",
    ] {
        let Some(job) = jobs.get(name) else {
            continue;
        };
        for required_line in ["needs: policy", FULL_SCOPE_CONDITION] {
            if job
                .lines()
                .map(str::trim)
                .filter(|line| *line == required_line)
                .count()
                != 1
            {
                violations.push(format!(
                    "full CI job {name} must use the exact fail-closed policy gate: {required_line}"
                ));
            }
        }
    }
    if jobs.get("architecture-check").is_some_and(|job| {
        job.lines()
            .map(str::trim)
            .any(|line| line.starts_with("if:"))
    }) {
        violations.push("architecture-check must run for documentation scope".to_owned());
    }
}

fn validate_dependency_job(jobs: &BTreeMap<String, String>, violations: &mut Vec<String>) {
    let Some(block) = jobs.get("dependency-policy") else {
        return;
    };
    for command in [DIRECT_AUDIT, DIRECT_DENY] {
        if !block.lines().any(|line| command_text(line) == command) {
            violations.push(format!(
                "dependency-policy job lacks pinned direct tool command: {command}"
            ));
        }
    }
    if !block.contains("PINNED_CARGO_AUDIT=") || !block.contains("PINNED_CARGO_DENY=") {
        violations
            .push("dependency-policy job does not record absolute audit/deny tools".to_owned());
    }
    if !block.contains(AUDIT_INSTALL)
        || !block.contains(DENY_INSTALL)
        || !block.contains("Get-Command cargo-audit -CommandType Application")
        || !block.contains("Get-Command cargo-deny -CommandType Application")
        || !block.contains("Resolve-Path -LiteralPath $auditCommand")
        || !block.contains("Resolve-Path -LiteralPath $denyCommand")
    {
        violations.push(
            "dependency-policy job must install exact reviewed audit/deny versions and record their resolved executable paths"
                .to_owned(),
        );
    }
    let install = block.find(&format!("uses: {INSTALL_TOOLS}"));
    let record = block.find("$auditCommand = (Get-Command cargo-audit");
    if !matches!((install, record), (Some(install), Some(record)) if install < record) {
        violations.push(
            "dependency-policy tools must be installed before their executable paths are recorded"
                .to_owned(),
        );
    }
    if !block.contains(&format!("{GUARD_PREFIX} --check-only")) {
        violations
            .push("dependency-policy job lacks dependency admission before audit/deny".to_owned());
    }
}

fn validate_platform_job(jobs: &BTreeMap<String, String>, violations: &mut Vec<String>) {
    let Some(block) = jobs.get("platform") else {
        return;
    };
    if !block.contains("PINNED_CARGO_CLIPPY=") {
        violations.push("platform job does not record the absolute cargo-clippy tool".to_owned());
    }
    if !block.contains(&format!(
        "{GUARD_PREFIX} -- cargo clippy --workspace --all-targets --locked -- -D warnings"
    )) {
        violations
            .push("platform job lacks the guarded cross-platform Clippy execution".to_owned());
    }
    for required_line in [
        "test_scope: all",
        "test_scope: core",
        "if: ${{ matrix.test_scope == 'all' }}",
        "if: ${{ matrix.test_scope == 'core' }}",
    ] {
        if block
            .lines()
            .map(str::trim)
            .filter(|line| *line == required_line)
            .count()
            == 0
        {
            violations.push(format!(
                "platform job must retain the reviewed cross-platform test split: {required_line}"
            ));
        }
    }
    for command in [
        ALL_TARGET_TEST,
        CORE_TARGET_TEST,
        ENGINE_INTEGRATION_TEST,
        SFC_INTEGRATION_TEST,
    ] {
        if block
            .lines()
            .filter(|line| command_text(line) == command)
            .count()
            != 1
        {
            violations.push(format!(
                "platform job must execute each reviewed test partition exactly once: {command}"
            ));
        }
    }
}

fn validate_windows_integration_job(jobs: &BTreeMap<String, String>, violations: &mut Vec<String>) {
    let Some(block) = jobs.get("windows-integration") else {
        return;
    };
    for required_line in [
        "fail-fast: false",
        "shard: [0, 1, 2, 3, 4, 5]",
        "runs-on: windows-2022",
    ] {
        if block
            .lines()
            .map(str::trim)
            .filter(|line| *line == required_line)
            .count()
            != 1
        {
            violations.push(format!(
                "Windows integration tests must use the complete reviewed shard matrix: {required_line}"
            ));
        }
    }
    if block
        .lines()
        .filter(|line| command_text(line) == WINDOWS_INTEGRATION_RUN)
        .count()
        != 1
    {
        violations
            .push("Windows integration tests must execute all six owner-derived shards".to_owned());
    }
}

fn validate_corpus_job(jobs: &BTreeMap<String, String>, violations: &mut Vec<String>) {
    let Some(block) = jobs.get("corpus") else {
        return;
    };
    let lines = block.lines().map(str::trim).collect::<Vec<_>>();
    if lines.iter().filter(|line| **line == "include:").count() != 1
        || lines
            .iter()
            .filter(|line| **line == "fail-fast: false")
            .count()
            != 1
        || lines
            .iter()
            .filter(|line| **line == "runs-on: ${{ matrix.os }}")
            .count()
            != 1
        || lines
            .iter()
            .filter(|line| **line == "name: ${{ matrix.os }} corpus ${{ matrix.name }}")
            .count()
            != 1
    {
        violations.push(
            "corpus job must use the reviewed explicit fail-closed partition matrix".to_owned(),
        );
    }
    let partition_count = |platform: &str, name: &str, arguments: &str| {
        let os_line = format!("- os: {platform}");
        let name_line = format!("name: {name}");
        let arguments_line = format!("arguments: {arguments}");
        lines
            .windows(3)
            .filter(|partition| {
                partition[0] == os_line
                    && partition[1] == name_line
                    && partition[2] == arguments_line
            })
            .count()
    };
    for (name, arguments) in UBUNTU_MAPPED_CORPUS_CASES {
        if partition_count("ubuntu-24.04", name, arguments) != 1 {
            violations.push(format!(
                "corpus job must contain exactly one Ubuntu {name} aggregate"
            ));
        }
    }
    for (mode, mode_flag, shard_count) in
        [("standard", "", 4), ("determinism", "--determinism ", 8)]
    {
        let row_jobs = if mode == "determinism" { 2 } else { 4 };
        for index in 0..shard_count {
            let name = format!("mapped-{mode}-{index}");
            let arguments = format!(
                "foundation {mode_flag}--mapped-only --row-jobs {row_jobs} --row-shard-index {index} --row-shard-count {shard_count}"
            );
            if partition_count("windows-2022", &name, &arguments) != 1 {
                violations.push(format!(
                    "corpus job must contain exactly one Windows {name} partition"
                ));
            }
        }
    }
    for platform in ["ubuntu-24.04", "windows-2022"] {
        for (name, arguments) in CRASH_CORPUS_CASES {
            if partition_count(platform, name, arguments) != 1 {
                violations.push(format!(
                    "corpus job must contain exactly one {platform} {name} partition"
                ));
            }
        }
    }
    if lines
        .iter()
        .filter(|line| line.starts_with("- os:"))
        .count()
        != 22
    {
        violations.push("corpus job must contain exactly 22 reviewed partitions".to_owned());
    }
    if lines.iter().any(|line| {
        *line == "exclude:"
            || *line == "os:"
            || *line == "case:"
            || (line.starts_with("if:") && *line != FULL_SCOPE_CONDITION)
            || line.starts_with("continue-on-error:")
    }) {
        violations.push(
            "corpus job cannot add exclusions, implicit products, or failure bypasses".to_owned(),
        );
    }
    if lines.contains(&"name: lifecycle-operation-idempotency") {
        violations.push(
            "lifecycle-operation-idempotency is already covered by both mapped aggregates"
                .to_owned(),
        );
    }
    if block
        .lines()
        .filter(|line| command_text(line) == CORPUS_RUN)
        .count()
        != 1
    {
        violations.push(
            "corpus job must route every matrix partition through one guarded runner".to_owned(),
        );
    }
}

fn validate_required_job(jobs: &BTreeMap<String, String>, violations: &mut Vec<String>) {
    let Some(block) = jobs.get("required") else {
        return;
    };
    for required_line in [
        "if: ${{ always() }}",
        "- policy",
        "- architecture-check",
        "POLICY_RESULT: ${{ needs.policy.result }}",
        "POLICY_SCOPE: ${{ needs.policy.outputs.scope }}",
        "RUN_FULL: ${{ needs.policy.outputs.run_full }}",
        "EXPECTED_RUN_FULL: ${{ needs.policy.outputs.scope == 'full' && 'true' || 'false' }}",
        "EXPECTED_SCOPE: ${{ needs.policy.outputs.run_full == 'true' && 'full' || 'documentation' }}",
        "EXPECTED_HEAVY_RESULT: ${{ needs.policy.outputs.run_full == 'true' && 'success' || 'skipped' }}",
        "ARCHITECTURE_CHECK_RESULT: ${{ needs.architecture-check.result }}",
        "test \"$ARCHITECTURE_CHECK_RESULT\" = success",
        "test \"$POLICY_RESULT\" = success",
        "test \"$RUN_FULL\" = \"$EXPECTED_RUN_FULL\"",
        "test \"$POLICY_SCOPE\" = \"$EXPECTED_SCOPE\"",
    ] {
        if block
            .lines()
            .map(str::trim)
            .filter(|line| *line == required_line)
            .count()
            != 1
        {
            violations.push(format!(
                "Required job must bind the complete scoped result exactly once: {required_line}"
            ));
        }
    }
    for (job, result) in [
        ("formatting", "FORMATTING_RESULT"),
        ("bootstrap-tests", "BOOTSTRAP_TESTS_RESULT"),
        ("dependency-policy", "DEPENDENCY_POLICY_RESULT"),
        ("platform", "PLATFORM_RESULT"),
        ("windows-integration", "WINDOWS_INTEGRATION_RESULT"),
        ("corpus", "CORPUS_RESULT"),
        ("documentation", "DOCUMENTATION_RESULT"),
        ("release", "RELEASE_RESULT"),
    ] {
        for required_line in [
            format!("- {job}"),
            format!("{result}: ${{{{ needs.{job}.result }}}}"),
            format!("test \"${result}\" = \"$EXPECTED_HEAVY_RESULT\""),
        ] {
            if block
                .lines()
                .map(str::trim)
                .filter(|line| *line == required_line)
                .count()
                != 1
            {
                violations.push(format!(
                    "Required job must bind the complete {job} result exactly once: {required_line}"
                ));
            }
        }
    }
    if block
        .lines()
        .map(str::trim)
        .any(|line| line.starts_with("continue-on-error:"))
    {
        violations.push("Required job cannot tolerate a failed required check".to_owned());
    }
}

fn validate_bootstrap_test_job(jobs: &BTreeMap<String, String>, violations: &mut Vec<String>) {
    let Some(block) = jobs.get("bootstrap-tests") else {
        return;
    };
    if block
        .lines()
        .filter(|line| command_text(line) == TEST_COMMAND)
        .count()
        != 1
    {
        violations.push("bootstrap test suite must run once in its own process job".to_owned());
    }
}

fn validate_no_nested_dependency_admission(root: &Path, result: &mut CargoBootstrapResult) {
    let path = root.join("tools/xtask/src/metadata.rs");
    let source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) => {
            result
                .tool_errors
                .push(format!("cannot read {}: {error}", path.display()));
            return;
        }
    };
    for forbidden in ["Command::new", "std::process", "source_provenance.py"] {
        if source.contains(forbidden) {
            result.violations.push(format!(
                "structural checker must not recreate dependency admission; metadata.rs contains {forbidden}"
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workflow() -> Result<String, Box<dyn std::error::Error>> {
        let root = crate::metadata::find_workspace_root().map_err(std::io::Error::other)?;
        Ok(std::fs::read_to_string(root.join(WORKFLOW_PATH))?)
    }

    fn violations(source: &str) -> Vec<String> {
        let mut violations = Vec::new();
        validate_workflow(source, &mut violations);
        violations
    }

    #[test]
    fn checked_workflow_has_no_routing_violation() -> Result<(), Box<dyn std::error::Error>> {
        let found = violations(&workflow()?);
        assert!(found.is_empty(), "{found:#?}");
        Ok(())
    }

    #[test]
    fn removing_one_guard_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let guarded = format!("{GUARD_PREFIX} -- cargo build --locked -p lumin-xtask");
        let source = workflow()?.replace(&guarded, "cargo build --locked -p lumin-xtask");
        assert!(
            violations(&source)
                .iter()
                .any(|violation| violation.contains("bypasses the guard"))
        );
        Ok(())
    }

    #[test]
    fn dependency_cache_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let source = workflow()?.replace(
            "steps:\n",
            "steps:\n      - uses: actions/cache@0000000000000000000000000000000000000000\n",
        );
        assert!(
            violations(&source)
                .iter()
                .any(|violation| violation.contains("dependency cache"))
        );
        Ok(())
    }

    #[test]
    fn cargo_paths_must_be_initialized_before_checkout() -> Result<(), Box<dyn std::error::Error>> {
        let source = workflow()?.replacen(
            PRIVATE_CARGO_HOME,
            "\"CARGO_HOME=$env:RUNNER_TEMP/late-home\" >> $env:GITHUB_ENV",
            1,
        );
        assert!(
            violations(&source)
                .iter()
                .any(|violation| violation.contains("before checkout"))
        );
        Ok(())
    }

    #[test]
    fn reconstructed_or_delegated_cargo_commands_are_rejected()
    -> Result<(), Box<dyn std::error::Error>> {
        for command in [
            "$c = 'cargo'",
            "& $c test --locked",
            "./ci-cargo-wrapper.ps1",
        ] {
            let source = workflow()?.replace(
                "steps:\n",
                &format!("steps:\n      - run: |\n          {command}\n"),
            );
            assert!(
                violations(&source)
                    .iter()
                    .any(|violation| violation.contains("reconstructed run command")),
                "command was accepted: {command}"
            );
        }
        Ok(())
    }

    #[test]
    fn guarded_command_cannot_append_a_shell_tail() -> Result<(), Box<dyn std::error::Error>> {
        let command = format!("{GUARD_PREFIX} -- cargo build --locked -p lumin-xtask");
        let source = workflow()?.replace(&command, &format!("{command}; Write-Output bypass"));
        assert!(
            violations(&source)
                .iter()
                .any(|violation| violation.contains("shell composition"))
        );
        Ok(())
    }

    #[test]
    fn cargo_plugin_dispatch_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let source = workflow()?.replace(
            DIRECT_AUDIT,
            &format!("{GUARD_PREFIX} -- cargo audit --locked --deny warnings"),
        );
        assert!(
            violations(&source)
                .iter()
                .any(|violation| violation.contains("pinned executables directly"))
        );
        Ok(())
    }

    #[test]
    fn mapped_standard_and_determinism_corpus_partitions_are_required()
    -> Result<(), Box<dyn std::error::Error>> {
        for (name, arguments) in UBUNTU_MAPPED_CORPUS_CASES {
            let case = format!(
                "          - os: ubuntu-24.04\n            name: {name}\n            arguments: {arguments}\n"
            );
            let source = workflow()?;
            assert!(source.contains(&case), "missing Ubuntu partition {name}");
            let changed = source.replacen(&case, "", 1);
            assert!(
                violations(&changed)
                    .iter()
                    .any(|violation| violation.contains(name)),
                "removed Ubuntu partition was accepted: {name}"
            );
        }
        for (mode, mode_flag, shard_count) in
            [("standard", "", 4), ("determinism", "--determinism ", 8)]
        {
            let row_jobs = if mode == "determinism" { 2 } else { 4 };
            for index in 0..shard_count {
                let name = format!("mapped-{mode}-{index}");
                let arguments = format!(
                    "foundation {mode_flag}--mapped-only --row-jobs {row_jobs} --row-shard-index {index} --row-shard-count {shard_count}"
                );
                let partition = format!(
                    "          - os: windows-2022\n            name: {name}\n            arguments: {arguments}\n"
                );
                let source = workflow()?;
                assert!(
                    source.contains(&partition),
                    "missing Windows partition {name}"
                );
                let changed = source.replacen(&partition, "", 1);
                assert!(
                    violations(&changed)
                        .iter()
                        .any(|violation| violation.contains(&name)),
                    "removed Windows partition was accepted: {name}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn code_ci_parallel_partitions_are_complete() -> Result<(), Box<dyn std::error::Error>> {
        let source = workflow()?;
        for changed in [
            source.replacen("shard: [0, 1, 2, 3, 4, 5]", "shard: [0, 1, 2, 3, 4]", 1),
            source.replacen(WINDOWS_INTEGRATION_RUN, "& $testShard", 1),
            source.replacen("--count 6 --jobs 4", "--count 6 --jobs 3", 1),
            source.replacen(CORE_TARGET_TEST, ALL_TARGET_TEST, 1),
            source.replacen("--row-jobs 8", "--row-jobs 7", 1),
            source.replacen(
                "foundation --determinism --mapped-only --row-jobs 2",
                "foundation --determinism --mapped-only --row-jobs 1",
                1,
            ),
            source.replacen("--row-shard-count 4", "--row-shard-count 3", 1),
            source.replacen("--row-shard-count 8", "--row-shard-count 7", 1),
        ] {
            assert!(
                violations(&changed).iter().any(|violation| {
                    violation.contains("shard")
                        || violation.contains("reviewed test partition")
                        || violation.contains("mapped-standard")
                        || violation.contains("mapped-determinism")
                        || violation.contains("reconstructed run command")
                }),
                "incomplete parallel CI partition was accepted"
            );
        }
        Ok(())
    }

    #[test]
    fn documentation_scope_gate_is_exact_and_fail_closed() -> Result<(), Box<dyn std::error::Error>>
    {
        let source = workflow()?;
        for changed in [
            source.replacen(FULL_SCOPE_CONDITION, "if: ${{ false }}", 1),
            source.replacen("    needs: policy\n", "", 1),
            source.replacen(CI_POLICY_COMMAND, "& $policy github", 1),
            source.replacen(CI_POLICY_TEST_COMMAND, "& $policyTest", 1),
        ] {
            assert!(
                violations(&changed).iter().any(|violation| {
                    violation.contains("fail-closed policy gate")
                        || violation.contains("reviewed policy command")
                        || violation.contains("focused tests")
                        || violation.contains("reconstructed run command")
                }),
                "changed policy routing was accepted"
            );
        }
        Ok(())
    }

    #[test]
    fn mapped_aggregate_rows_are_not_repeated_as_standalone_cases()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = workflow()?.replace(
            concat!(
                "          - os: ubuntu-24.04\n",
                "            name: crash-publication\n"
            ),
            concat!(
                "          - os: ubuntu-24.04\n",
                "            name: lifecycle-operation-idempotency\n",
                "            arguments: foundation --row lifecycle-operation-idempotency\n",
                "          - os: ubuntu-24.04\n",
                "            name: crash-publication\n"
            ),
        );
        assert!(
            violations(&source)
                .iter()
                .any(|violation| violation.contains("already covered"))
        );
        Ok(())
    }

    #[test]
    fn required_job_cannot_drop_the_corpus_result() -> Result<(), Box<dyn std::error::Error>> {
        let source = workflow()?;
        for changed in [
            source.replacen("      - corpus\n", "", 1),
            source.replacen("      - windows-integration\n", "", 1),
            source.replacen(
                "          test \"$CORPUS_RESULT\" = \"$EXPECTED_HEAVY_RESULT\"\n",
                "          test \"$CORPUS_RESULT\" = success\n",
                1,
            ),
            source.replacen(
                "    name: Required\n",
                "    name: Required\n    continue-on-error: true\n",
                1,
            ),
            source.replacen("    if: ${{ always() }}\n", "    if: ${{ false }}\n", 1),
        ] {
            assert!(
                violations(&changed)
                    .iter()
                    .any(|violation| violation.contains("Required job"))
            );
        }
        Ok(())
    }

    #[test]
    fn direct_architecture_command_never_claims_dependency_admission()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = crate::metadata::find_workspace_root().map_err(std::io::Error::other)?;
        let source = std::fs::read_to_string(root.join("tools/xtask/src/architecture.rs"))?;
        assert!(source.contains("structural only"));
        assert!(source.contains("dependency admission not evaluated here"));
        assert!(!source.contains("cargo metadata dependency edges"));
        Ok(())
    }
}
