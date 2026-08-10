//! CI Cargo bootstrap integrity and workflow routing policy.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

const GUARD_PATH: &str = "tools/xtask/bootstrap/source_provenance.py";
const GUARD_SHA256: &str = "a150ac2b775485c338fd10236db698b3fd2b3d93196154526d83c495788ded58";
const TEST_PATH: &str = "tools/xtask/bootstrap/test_source_provenance.py";
const TEST_SHA256: &str = "1791b29d9c523c15589a7334188c4ec7ef097dd7eebb2fa9c722e9161d0d5396";
const WORKFLOW_PATH: &str = ".github/workflows/ci.yml";
const WORKFLOW_SHA256: &str = "7f38d74de95b360f00808b422e08581eccfc9668bfa6c1e97648e738771abd21";
const SETUP_PYTHON: &str = "actions/setup-python@5fda3b95a4ea91299a34e894583c3862153e4b97 # v7.0.0";
const PYTHON_VERSION: &str = "3.13.14";
const CARGO_JOBS: &[&str] = &[
    "formatting",
    "architecture-check",
    "dependency-policy",
    "platform",
];
const EXPECTED_USES: &[(&str, usize)] = &[
    (
        "actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0 # v7.0.0",
        4,
    ),
    (SETUP_PYTHON, 4),
    (
        "taiki-e/install-action@07b4745e0c39a41822af610387492e3e53aa222b # v2.83.4",
        1,
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
        2,
    ),
    (
        "python -I -S tools/xtask/bootstrap/test_source_provenance.py",
        1,
    ),
    (
        "python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo run --locked -p lumin-xtask -- architecture-check",
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
        "python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo run --locked -p lumin-xtask -- corpus foundation --store-crash --row retention-crash-protocol",
        1,
    ),
    (
        "python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo run --locked -p lumin-xtask -- corpus foundation --row lifecycle-operation-idempotency",
        1,
    ),
    (
        "python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo run --locked -p lumin-xtask -- corpus foundation --store-crash --row crash-publication",
        1,
    ),
    (
        "python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo run --locked -p lumin-xtask -- corpus foundation --store-crash --row concurrent-latest-publication",
        1,
    ),
    (
        "python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo run --locked -p lumin-xtask -- corpus foundation --store-crash --row publication-retention-race",
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
            "test \"$DEPENDENCY_POLICY_RESULT\" = success\n",
            "test \"$PLATFORM_RESULT\" = success",
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
        validate_python_setup(source, job, violations);
    }
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

fn validate_python_setup(source: &str, job: &str, violations: &mut Vec<String>) {
    let lines = source.lines().collect::<Vec<_>>();
    let marker = format!("  {job}:");
    let Some(start) = lines.iter().position(|line| *line == marker) else {
        violations.push(format!("MISSING CARGO WORKFLOW JOB: {job}"));
        return;
    };
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find_map(|(index, line)| {
            (line.starts_with("  ") && !line.starts_with("    ") && line.trim_end().ends_with(':'))
                .then_some(index)
        })
        .unwrap_or(lines.len());
    let block = lines[start..end].join("\n");
    let snippet = format!(
        "      - name: Install exact Python\n        uses: {SETUP_PYTHON}\n        with:\n          python-version: \"{PYTHON_VERSION}\""
    );
    let count = block.matches(&snippet).count();
    if count != 1 {
        violations.push(format!(
            "PYTHON BOOTSTRAP ROUTING: job {job} must contain one exact Python {PYTHON_VERSION} setup, found {count}"
        ));
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
}
