//! Row-owned checks that complement public-binary corpus invocations.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RequiredCheck {
    ArchitectureCheck,
}

impl RequiredCheck {
    pub fn name(self) -> &'static str {
        match self {
            Self::ArchitectureCheck => "architecture-check",
        }
    }
}

const ARCHITECTURE_CHECK: &[RequiredCheck] = &[RequiredCheck::ArchitectureCheck];

/// Section 9 rows whose authored truth includes a structural invariant. The
/// public invocation remains mandatory; this check supplies only that named
/// structural part of the same row contract.
const ARCHITECTURE_CHECK_ROWS: &[&str] = &[
    "resolver-config-registry-artifact",
    "pnpm-workspace-registry-and-precedence",
    "limitation-scope-exhaustiveness",
    "capability-availability-authority",
    "gate-lifecycle-effects",
];

pub fn expected_for_row(row_id: &str) -> &'static [RequiredCheck] {
    if ARCHITECTURE_CHECK_ROWS.contains(&row_id) {
        ARCHITECTURE_CHECK
    } else {
        &[]
    }
}

pub struct CheckOutcome {
    pub passed: bool,
    pub stdout: String,
    pub stderr: String,
}

pub fn run_required_checks(
    workspace: &Path,
    checks: &BTreeSet<RequiredCheck>,
) -> Result<BTreeMap<RequiredCheck, CheckOutcome>, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot locate current lumin-xtask executable: {error}"))?;
    let mut outcomes = BTreeMap::new();

    for check in checks {
        let output = match check {
            RequiredCheck::ArchitectureCheck => Command::new(&executable)
                .current_dir(workspace)
                .arg("architecture-check")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .map_err(|error| format!("cannot run {}: {error}", check.name()))?,
        };
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        match output.status.code() {
            Some(0) => {
                outcomes.insert(
                    *check,
                    CheckOutcome {
                        passed: true,
                        stdout,
                        stderr,
                    },
                );
            }
            Some(1) => {
                outcomes.insert(
                    *check,
                    CheckOutcome {
                        passed: false,
                        stdout,
                        stderr,
                    },
                );
            }
            code => {
                return Err(format!(
                    "{} returned tool-error status {:?}\nstdout:\n{}\nstderr:\n{}",
                    check.name(),
                    code,
                    stdout,
                    stderr,
                ));
            }
        }
    }

    Ok(outcomes)
}
