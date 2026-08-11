//! CI Cargo bootstrap integrity and workflow routing policy.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

const GUARD_PATH: &str = "tools/xtask/bootstrap/source_provenance.py";
const GUARD_SHA256: &str = "89060d980e0194a9d70990a5454e1f72ae70278ff3c36d18ddefbcc03892b163";
const TEST_PATH: &str = "tools/xtask/bootstrap/test_source_provenance.py";
const TEST_SHA256: &str = "c2f56e4b9e8276a534c6bf7c07bef50314f653fac83f02986813b87ba844ecc4";
const WORKFLOW_DIRECTORY: &str = ".github/workflows";
const WORKFLOW_PATH: &str = ".github/workflows/ci.yml";
const WORKFLOW_SHA256: &str = "4ca2610501059a8ae6eacffd0b93547cb69115c221460eb012b2aa5465dbfc4c";
const SETUP_PYTHON: &str = "actions/setup-python@5fda3b95a4ea91299a34e894583c3862153e4b97 # v7.0.0";
const PYTHON_VERSION: &str = "3.13.14";
const CHECKOUT: &str = "actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0 # v7.0.0";
const DEPENDENCY_TOOL_INSTALL: &str =
    "taiki-e/install-action@07b4745e0c39a41822af610387492e3e53aa222b # v2.83.4";
const STEP_PRIVATE_CARGO_ENV: &str = concat!(
    "        env:\n",
    "          CARGO_HOME: ${{ runner.temp }}/lumin-cargo-home\n",
    "          CARGO_TARGET_DIR: ${{ runner.temp }}/lumin-target",
);
const CARGO_JOBS: &[&str] = &[
    "formatting",
    "architecture-check",
    "bootstrap-tests",
    "dependency-policy",
    "platform",
    "corpus",
    "documentation",
    "release",
];
const EXPECTED_USES: &[(&str, usize)] = &[
    (CHECKOUT, 8),
    (SETUP_PYTHON, 8),
    (DEPENDENCY_TOOL_INSTALL, 1),
];
const TERMINAL_RUNTIME_RUNS: &[(&str, &str)] = &[
    (
        "architecture-check",
        "${{ runner.temp }}/lumin-target/debug/lumin-xtask architecture-check",
    ),
    (
        "bootstrap-tests",
        "python -I -S tools/xtask/bootstrap/test_source_provenance.py",
    ),
    (
        "platform",
        "python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo test --workspace --all-targets --locked",
    ),
    (
        "corpus",
        "python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo run --locked -p lumin-xtask -- corpus ${{ matrix.case.arguments }}",
    ),
    (
        "documentation",
        "python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo test --workspace --doc --locked",
    ),
];
const EXPECTED_RUN_COMMANDS: &[(&str, usize)] = &[
    (
        "rustup toolchain install 1.96.0 --profile minimal --component rustfmt --no-self-update",
        1,
    ),
    (
        "python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo fmt --all --check",
        1,
    ),
    (
        "rustup toolchain install 1.96.0 --profile minimal --no-self-update",
        5,
    ),
    (
        "python -I -S tools/xtask/bootstrap/test_source_provenance.py",
        1,
    ),
    (
        "python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo build --locked -p lumin-xtask",
        1,
    ),
    (
        "python -I -S tools/xtask/bootstrap/source_provenance.py --check-only",
        2,
    ),
    (
        "${{ runner.temp }}/lumin-target/debug/lumin-xtask architecture-check",
        1,
    ),
    (
        "python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo audit --deny warnings",
        1,
    ),
    (
        "python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo deny --locked check bans licenses sources",
        1,
    ),
    (
        "rustup toolchain install 1.96.0 --profile minimal --component clippy,rustfmt --no-self-update",
        1,
    ),
    ("rustc --version --verbose\ncargo --version", 1),
    (
        "python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo clippy --workspace --all-targets --locked -- -D warnings",
        1,
    ),
    (
        "python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo clippy -p lumin-cli --test retention_faults --features retention-test-crash --locked -- -D warnings",
        1,
    ),
    (
        "python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo clippy -p lumin-cli --test lifecycle_operation_idempotency --features lifecycle-test-fault --locked -- -D warnings",
        1,
    ),
    (
        "python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo clippy -p lumin-cli --test publication_faults --features publication-test-crash --locked -- -D warnings",
        1,
    ),
    (
        "python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo clippy -p lumin-cli --test publication_concurrency --features publication-test-crash --locked -- -D warnings",
        1,
    ),
    (
        "python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo clippy -p lumin-cli --test publication_retention_race --features publication-test-crash,retention-test-crash --locked -- -D warnings",
        1,
    ),
    (
        "python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo test --workspace --all-targets --locked",
        1,
    ),
    (
        "python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo run --locked -p lumin-xtask -- corpus ${{ matrix.case.arguments }}",
        1,
    ),
    (
        "python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo test --workspace --doc --locked",
        1,
    ),
    (
        "python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo build -p lumin-cli --release --locked",
        1,
    ),
    (
        concat!(
            "test \"$FORMATTING_RESULT\" = success\n",
            "test \"$ARCHITECTURE_CHECK_RESULT\" = success\n",
            "test \"$BOOTSTRAP_TESTS_RESULT\" = success\n",
            "test \"$DEPENDENCY_POLICY_RESULT\" = success\n",
            "test \"$PLATFORM_RESULT\" = success\n",
            "test \"$CORPUS_RESULT\" = success\n",
            "test \"$DOCUMENTATION_RESULT\" = success\n",
            "test \"$RELEASE_RESULT\" = success",
        ),
        1,
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
    verify_digest(workspace_root, GUARD_PATH, GUARD_SHA256, &mut result);
    verify_digest(workspace_root, TEST_PATH, TEST_SHA256, &mut result);
    verify_digest(workspace_root, WORKFLOW_PATH, WORKFLOW_SHA256, &mut result);

    let workflow = workspace_root.join(WORKFLOW_PATH);
    match std::fs::read_to_string(&workflow) {
        Ok(source) => validate_workflow(&source, &mut result.violations),
        Err(error) => result
            .tool_errors
            .push(format!("cannot read {WORKFLOW_PATH}: {error}")),
    }
    result
}

fn validate_workflow_directory(root: &Path, result: &mut CargoBootstrapResult) {
    let directory = root.join(WORKFLOW_DIRECTORY);
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) => {
            result.tool_errors.push(format!(
                "cannot enumerate workflow directory {}: {error}",
                directory.display()
            ));
            return;
        }
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                result
                    .tool_errors
                    .push(format!("cannot read workflow directory entry: {error}"));
                return;
            }
        };
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => {
                result
                    .violations
                    .push("WORKFLOW SURFACE: non-UTF-8 entry is forbidden".to_owned());
                continue;
            }
        };
        match entry.file_type() {
            Ok(file_type) if file_type.is_file() => names.push(name),
            Ok(_) => result.violations.push(format!(
                "WORKFLOW SURFACE: redirected or non-file entry is forbidden: {name}"
            )),
            Err(error) => result.tool_errors.push(format!(
                "cannot inspect workflow directory entry {name}: {error}"
            )),
        }
    }
    names.sort();
    if names != ["ci.yml"] {
        result.violations.push(format!(
            "WORKFLOW SURFACE: expected only ci.yml, found {names:?}"
        ));
    }
}

fn verify_digest(root: &Path, relative: &str, expected: &str, result: &mut CargoBootstrapResult) {
    let path = root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            result
                .tool_errors
                .push(format!("cannot read {relative}: {error}"));
            return;
        }
    };
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        result.violations.push(format!(
            "CARGO BOOTSTRAP DIGEST MISMATCH: {relative} expected {expected} got {actual}"
        ));
    }
}

fn validate_workflow(source: &str, violations: &mut Vec<String>) {
    let runs = extract_run_commands(source, violations);
    validate_exact_multiset("run", &runs, EXPECTED_RUN_COMMANDS, violations);
    let uses = extract_scalar_values(source, "uses:");
    validate_exact_multiset("uses", &uses, EXPECTED_USES, violations);
    for job in CARGO_JOBS {
        validate_job_bootstrap(source, job, violations);
    }
    validate_terminal_runtime_boundaries(source, violations);
}

fn extract_run_commands(source: &str, violations: &mut Vec<String>) -> Vec<String> {
    let lines = source.lines().collect::<Vec<_>>();
    let mut commands = Vec::new();
    let mut index = 0_usize;
    while index < lines.len() {
        let raw = lines[index];
        let trimmed = raw.trim();
        let Some(value) = trimmed.strip_prefix("run:").map(str::trim) else {
            index += 1;
            continue;
        };
        if value != "|" {
            if value.is_empty() {
                violations.push(format!(
                    "EMPTY WORKFLOW RUN COMMAND: {WORKFLOW_PATH}:{}",
                    index + 1
                ));
            } else {
                commands.push(value.to_owned());
            }
            index += 1;
            continue;
        }

        let parent_indent = raw.len() - raw.trim_start().len();
        let mut block = Vec::new();
        index += 1;
        while index < lines.len() {
            let nested = lines[index];
            if nested.trim().is_empty() {
                index += 1;
                continue;
            }
            let nested_indent = nested.len() - nested.trim_start().len();
            if nested_indent <= parent_indent {
                break;
            }
            block.push(nested.trim().to_owned());
            index += 1;
        }
        if block.is_empty() {
            violations.push(format!(
                "EMPTY WORKFLOW RUN BLOCK: {WORKFLOW_PATH}:{}",
                index + 1
            ));
        } else {
            commands.push(block.join("\n"));
        }
    }
    commands
}

fn extract_scalar_values(source: &str, key: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| line.trim().strip_prefix(key).map(str::trim))
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn validate_exact_multiset(
    label: &str,
    observed: &[String],
    expected: &[(&str, usize)],
    violations: &mut Vec<String>,
) {
    let mut counts = BTreeMap::new();
    for value in observed {
        *counts.entry(value.as_str()).or_insert(0_usize) += 1;
    }
    let expected_counts = expected.iter().copied().collect::<BTreeMap<_, _>>();
    for (value, count) in &counts {
        let allowed = expected_counts.get(value).copied().unwrap_or(0);
        if *count > allowed {
            violations.push(format!(
                "UNAPPROVED WORKFLOW {label}: `{value}` observed {count}, allowed {allowed}"
            ));
        }
    }
    for (value, count) in expected_counts {
        let actual = counts.get(value).copied().unwrap_or(0);
        if actual < count {
            violations.push(format!(
                "MISSING WORKFLOW {label}: `{value}` observed {actual}, required {count}"
            ));
        }
    }
}

fn job_block(source: &str, job: &str) -> Option<String> {
    let lines = source.lines().collect::<Vec<_>>();
    let marker = format!("  {job}:");
    let start = lines.iter().position(|line| *line == marker)?;
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find_map(|(index, line)| {
            (line.starts_with("  ") && !line.starts_with("    ") && line.trim_end().ends_with(':'))
                .then_some(index)
        })
        .unwrap_or(lines.len());
    Some(lines[start..end].join("\n"))
}

fn validate_job_bootstrap(source: &str, job: &str, violations: &mut Vec<String>) {
    let Some(block) = job_block(source, job) else {
        violations.push(format!("MISSING CARGO WORKFLOW JOB: {job}"));
        return;
    };
    let snippet = format!(
        "      - name: Install exact Python\n        uses: {SETUP_PYTHON}\n        with:\n          python-version: \"{PYTHON_VERSION}\""
    );
    let count = block.matches(&snippet).count();
    if count != 1 {
        violations.push(format!(
            "PYTHON BOOTSTRAP ROUTING: job {job} must contain one exact Python {PYTHON_VERSION} setup, found {count}"
        ));
    }
    let checkout = format!("        uses: {CHECKOUT}");
    let checkout_count = block.matches(&checkout).count();
    if checkout_count != 1 {
        violations.push(format!(
            "FRESH CHECKOUT ROUTING: job {job} must contain one exact checkout, found {checkout_count}"
        ));
    }
    let run_count = block
        .lines()
        .filter(|line| line.trim_start().starts_with("run:"))
        .count();
    let private_run_prefix = format!("{STEP_PRIVATE_CARGO_ENV}\n        run:");
    let private_run_count = block.matches(&private_run_prefix).count();
    if private_run_count != run_count {
        violations.push(format!(
            "STEP-PRIVATE CARGO ROUTING: job {job} must bind all {run_count} run steps to the exact Cargo environment, found {private_run_count}"
        ));
    }
    if job == "dependency-policy" {
        let private_install =
            format!("{STEP_PRIVATE_CARGO_ENV}\n        uses: {DEPENDENCY_TOOL_INSTALL}");
        let install_count = block.matches(&private_install).count();
        if install_count != 1 {
            violations.push(format!(
                "STEP-PRIVATE CARGO ROUTING: job {job} must bind the dependency tool installer to the exact Cargo environment, found {install_count}"
            ));
        }
    }
}

fn validate_terminal_runtime_boundaries(source: &str, violations: &mut Vec<String>) {
    for (job, command) in TERMINAL_RUNTIME_RUNS {
        let Some(block) = job_block(source, job) else {
            continue;
        };
        let marker = format!("run: {command}");
        let lines = block.lines().collect::<Vec<_>>();
        let Some(terminal) = lines.iter().position(|line| line.trim() == marker) else {
            violations.push(format!(
                "TRUST EPOCH ROUTING: job {job} is missing exact terminal command `{command}`"
            ));
            continue;
        };
        for later in &lines[terminal + 1..] {
            let trimmed = later.trim();
            if trimmed.starts_with("run:") || trimmed.starts_with("uses:") {
                violations.push(format!(
                    "TRUST EPOCH ROUTING: job {job} executes `{trimmed}` after terminal runtime"
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = include_str!("../../../.github/workflows/ci.yml");

    #[test]
    fn exact_workflow_surface_passes() {
        let mut violations = Vec::new();
        validate_workflow(GOOD, &mut violations);
        assert!(violations.is_empty(), "{violations:?}");
    }

    #[test]
    fn workflow_directory_rejects_additional_or_redirected_entries()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let workflows = temporary.path().join(WORKFLOW_DIRECTORY);
        std::fs::create_dir_all(&workflows)?;
        std::fs::write(workflows.join("ci.yml"), GOOD)?;

        let mut clean = CargoBootstrapResult::default();
        validate_workflow_directory(temporary.path(), &mut clean);
        assert!(clean.violations.is_empty(), "{:?}", clean.violations);
        assert!(clean.tool_errors.is_empty(), "{:?}", clean.tool_errors);

        std::fs::write(workflows.join("bypass.yaml"), "name: bypass\n")?;
        let mut additional = CargoBootstrapResult::default();
        validate_workflow_directory(temporary.path(), &mut additional);
        assert!(
            additional
                .violations
                .iter()
                .any(|violation| violation.contains("expected only ci.yml")),
            "{:?}",
            additional.violations
        );
        Ok(())
    }

    #[test]
    fn reconstructed_delegated_or_weakened_cargo_fails() {
        let wrapped = "run: python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo fmt --all --check";
        for bad in [
            GOOD.replace(
                wrapped,
                "run: |\n          c=cargo\n          $c fmt --all --check",
            ),
            GOOD.replace(wrapped, "run: ./ci/run-cargo.sh"),
            GOOD.replace(wrapped, "uses: ./ci/local-cargo-action"),
            GOOD.replace(wrapped, &wrapped.replace("python -I -S", "python -S")),
        ] {
            let mut violations = Vec::new();
            validate_workflow(&bad, &mut violations);
            assert!(
                violations
                    .iter()
                    .any(|violation| violation.contains("UNAPPROVED WORKFLOW")
                        || violation.contains("MISSING WORKFLOW")),
                "{violations:?}"
            );
        }
    }

    #[test]
    fn every_cargo_job_requires_exact_python_setup() {
        for bad in [
            GOOD.replacen(
                "      - name: Install exact Python\n        uses: actions/setup-python@5fda3b95a4ea91299a34e894583c3862153e4b97 # v7.0.0\n        with:\n          python-version: \"3.13.14\"\n\n",
                "",
                1,
            ),
            GOOD.replacen("python-version: \"3.13.14\"", "python-version: \"3.10\"", 1),
        ] {
            let mut violations = Vec::new();
            validate_workflow(&bad, &mut violations);
            assert!(
                violations
                    .iter()
                    .any(|violation| violation.contains("PYTHON BOOTSTRAP ROUTING")),
                "{violations:?}"
            );
        }
    }

    #[test]
    fn terminal_runtime_is_last_executable_step_in_each_job() {
        for (job, command) in TERMINAL_RUNTIME_RUNS {
            let needle = format!("        run: {command}");
            let replacement = format!(
                "{needle}\n\n      - name: Forbidden later command\n        run: echo forbidden"
            );
            let bad = GOOD.replacen(&needle, &replacement, 1);
            let mut violations = Vec::new();
            validate_workflow(&bad, &mut violations);
            assert!(
                violations
                    .iter()
                    .any(|violation| violation.contains("TRUST EPOCH ROUTING")
                        && violation.contains(job)),
                "{job}: {violations:?}"
            );
        }
    }

    #[test]
    fn every_cargo_job_requires_its_own_checkout() {
        let checkout = format!("      - name: Check out repository\n        uses: {CHECKOUT}\n\n");
        let bad = GOOD.replacen(&checkout, "", 1);
        let mut violations = Vec::new();
        validate_workflow(&bad, &mut violations);
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("FRESH CHECKOUT ROUTING")),
            "{violations:?}"
        );
    }

    #[test]
    fn every_cargo_run_step_requires_job_private_cargo_paths() {
        let bad = GOOD.replacen(STEP_PRIVATE_CARGO_ENV, "", 1);
        let mut violations = Vec::new();
        validate_workflow(&bad, &mut violations);
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("STEP-PRIVATE CARGO ROUTING")),
            "{violations:?}"
        );
    }

    #[test]
    fn dependency_tool_install_requires_job_private_cargo_paths() {
        let private_install =
            format!("{STEP_PRIVATE_CARGO_ENV}\n        uses: {DEPENDENCY_TOOL_INSTALL}");
        let bad = GOOD.replacen(
            &private_install,
            &format!("        uses: {DEPENDENCY_TOOL_INSTALL}"),
            1,
        );
        let mut violations = Vec::new();
        validate_workflow(&bad, &mut violations);
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("STEP-PRIVATE CARGO ROUTING")),
            "{violations:?}"
        );
    }
}
