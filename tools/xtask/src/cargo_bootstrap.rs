//! CI Cargo bootstrap integrity and workflow routing policy.

use sha2::{Digest, Sha256};
use std::path::Path;

const GUARD_PATH: &str = "tools/xtask/bootstrap/source_provenance.py";
const GUARD_SHA256: &str = "f9129a92f80477a8fa0df2e9583596e2acf172b782c000acfe3ced197d6b350e";
const TEST_PATH: &str = "tools/xtask/bootstrap/test_source_provenance.py";
const TEST_SHA256: &str = "7f1d995616d8fc299b22287c28498c701c7aed7308c2a0f412c59c9a76b9c8d8";
const WORKFLOW_PATH: &str = ".github/workflows/ci.yml";
const WRAPPER_TOKENS: &[&str] = &[
    "python",
    "-I",
    "-S",
    "tools/xtask/bootstrap/source_provenance.py",
    "--",
];
const TEST_COMMAND: &str = "python -I -S tools/xtask/bootstrap/test_source_provenance.py";

#[derive(Debug, Default)]
pub struct CargoBootstrapResult {
    pub violations: Vec<String>,
    pub tool_errors: Vec<String>,
}

pub fn check_cargo_bootstrap(workspace_root: &Path) -> CargoBootstrapResult {
    let mut result = CargoBootstrapResult::default();
    verify_digest(workspace_root, GUARD_PATH, GUARD_SHA256, &mut result);
    verify_digest(workspace_root, TEST_PATH, TEST_SHA256, &mut result);

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
    let mut cargo_commands = 0_usize;
    let mut bootstrap_test_commands = 0_usize;

    for (index, raw_line) in source.lines().enumerate() {
        let line_number = index + 1;
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let command = trimmed
            .strip_prefix("run:")
            .map(str::trim)
            .unwrap_or(trimmed);
        if command == TEST_COMMAND {
            bootstrap_test_commands += 1;
        }

        let tokens = command.split_whitespace().collect::<Vec<_>>();
        let cargo_positions = tokens
            .iter()
            .enumerate()
            .filter_map(|(position, token)| (*token == "cargo").then_some(position))
            .collect::<Vec<_>>();
        if cargo_positions.is_empty() {
            continue;
        }
        cargo_commands += cargo_positions.len();

        if tokens == ["cargo", "--version"] {
            continue;
        }
        if cargo_positions.len() != 1 {
            violations.push(format!(
                "UNWRAPPED CARGO: {WORKFLOW_PATH}:{line_number} must contain one exact wrapped Cargo invocation"
            ));
            continue;
        }
        let cargo_position = cargo_positions[0];
        if cargo_position != WRAPPER_TOKENS.len()
            || tokens.get(..cargo_position) != Some(WRAPPER_TOKENS)
        {
            violations.push(format!(
                "UNWRAPPED CARGO: {WORKFLOW_PATH}:{line_number} must start with `{}`",
                WRAPPER_TOKENS.join(" ")
            ));
        }
    }

    if cargo_commands == 0 {
        violations.push(format!(
            "CARGO WORKFLOW EMPTY: {WORKFLOW_PATH} contains zero Cargo commands"
        ));
    }
    if bootstrap_test_commands != 1 {
        violations.push(format!(
            "CARGO BOOTSTRAP TEST ROUTING: {WORKFLOW_PATH} must contain exactly one `{TEST_COMMAND}` command, found {bootstrap_test_commands}"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
      - name: Test bootstrap
        run: python -I -S tools/xtask/bootstrap/test_source_provenance.py
      - name: Version
        run: cargo --version
      - name: Test
        run: python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo test --locked
"#;

    #[test]
    fn exact_wrapper_and_version_exception_pass() {
        let mut violations = Vec::new();
        validate_workflow(GOOD, &mut violations);
        assert!(violations.is_empty(), "{violations:?}");
    }

    #[test]
    fn unwrapped_or_weakened_cargo_fails() {
        for bad in [
            GOOD.replace(
                "python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo test --locked",
                "cargo test --locked",
            ),
            GOOD.replace("python -I -S", "python -S"),
            GOOD.replace(
                "cargo test --locked",
                "cargo test --locked && cargo build --locked",
            ),
        ] {
            let mut violations = Vec::new();
            validate_workflow(&bad, &mut violations);
            assert!(
                violations
                    .iter()
                    .any(|violation| violation.contains("UNWRAPPED CARGO")),
                "{violations:?}"
            );
        }
    }

    #[test]
    fn missing_bootstrap_test_command_fails() {
        let mut violations = Vec::new();
        validate_workflow(
            &GOOD.replace(TEST_COMMAND, "python -I -S unrelated.py"),
            &mut violations,
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("BOOTSTRAP TEST ROUTING")),
            "{violations:?}"
        );
    }
}
