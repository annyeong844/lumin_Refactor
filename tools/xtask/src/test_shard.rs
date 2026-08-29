//! Deterministic Windows integration-test sharding for public CI.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, ExitCode};

const CLI_TEST_DIRECTORY: &str = "crates/application/cli/tests";
const STORE_PACKAGE: &str = "lumin-store";
const STORE_LIB_TEST_MODULES: &[&str] = &["cache", "gate", "namespace", "retention"];
const FEATURE_GATED_TARGETS: &[&str] = &[
    "cache_cleanup_publication_race",
    "lifecycle_operation_idempotency",
    "lifecycle_store_migration",
    "publication_concurrency",
    "publication_faults",
    "publication_retention_race",
    "retention_faults",
    "state_namespace_initialization",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TestSuite {
    CliIntegration,
    StoreLib,
}

struct TestShardArgs {
    suite: TestSuite,
    index: usize,
    count: usize,
    jobs: usize,
}

fn parse_args(arguments: &[String]) -> Result<TestShardArgs, String> {
    let (mut suite, mut index, mut count, mut jobs) = (None, None, None, None);
    let mut cursor = 0;
    while cursor < arguments.len() {
        let flag = arguments[cursor].as_str();
        if !matches!(flag, "--suite" | "--index" | "--count" | "--jobs") {
            return Err(format!("unknown argument: {flag}"));
        }
        cursor += 1;
        let raw = arguments
            .get(cursor)
            .ok_or_else(|| format!("{} requires a value", arguments[cursor - 1]))?;
        if flag == "--suite" {
            let value = match raw.as_str() {
                "cli-integration" => TestSuite::CliIntegration,
                "store-lib" => TestSuite::StoreLib,
                _ => return Err("--suite must be cli-integration or store-lib".to_owned()),
            };
            if suite.replace(value).is_some() {
                return Err("--suite may be provided only once".to_owned());
            }
        } else {
            let value = raw
                .parse::<usize>()
                .map_err(|_| format!("{flag} requires an integer"))?;
            let target = match flag {
                "--index" => &mut index,
                "--count" => &mut count,
                "--jobs" => &mut jobs,
                _ => return Err(format!("unknown numeric argument: {flag}")),
            };
            if target.replace(value).is_some() {
                return Err(format!("{flag} may be provided only once"));
            }
        }
        cursor += 1;
    }
    let suite = suite.ok_or_else(|| "--suite is required".to_owned())?;
    let index = index.ok_or_else(|| "--index is required".to_owned())?;
    let count = count.ok_or_else(|| "--count is required".to_owned())?;
    if count == 0 || count > 16 {
        return Err("--count must be an integer from 1 through 16".to_owned());
    }
    if index >= count {
        return Err("--index must be less than --count".to_owned());
    }
    let jobs = jobs.unwrap_or(1);
    if jobs == 0 || jobs > 4 {
        return Err("--jobs must be an integer from 1 through 4".to_owned());
    }
    if suite == TestSuite::StoreLib && jobs != 1 {
        return Err("--jobs is available only for the cli-integration suite".to_owned());
    }
    Ok(TestShardArgs {
        suite,
        index,
        count,
        jobs,
    })
}

fn default_cli_test_targets(workspace: &Path) -> Result<Vec<String>, String> {
    let directory = workspace.join(CLI_TEST_DIRECTORY);
    let entries = fs::read_dir(&directory)
        .map_err(|error| format!("cannot enumerate {}: {error}", directory.display()))?;
    let gated = FEATURE_GATED_TARGETS
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut all_targets = BTreeSet::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot read CLI test entry: {error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot inspect {}: {error}", entry.path().display()))?;
        if !file_type.is_file()
            || entry.path().extension().and_then(|value| value.to_str()) != Some("rs")
        {
            continue;
        }
        let target = entry
            .path()
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| format!("CLI test target is not UTF-8: {}", entry.path().display()))?
            .to_owned();
        all_targets.insert(target);
    }
    for target in &gated {
        if !all_targets.contains(*target) {
            return Err(format!(
                "feature-gated CLI test target is missing: {target}"
            ));
        }
    }
    let targets = all_targets
        .into_iter()
        .filter(|target| !gated.contains(target.as_str()))
        .collect::<Vec<_>>();
    if targets.is_empty() {
        return Err("default CLI integration-test target set is empty".to_owned());
    }
    Ok(targets)
}

fn shard_targets(targets: &[String], index: usize, count: usize) -> Result<Vec<String>, String> {
    if count == 0 || index >= count {
        return Err("invalid test shard".to_owned());
    }
    let selected = targets
        .iter()
        .enumerate()
        .filter(|(position, _)| position % count == index)
        .map(|(_, target)| target.clone())
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err(format!(
            "test shard {index} of {count} selects zero targets from {}",
            targets.len()
        ));
    }
    Ok(selected)
}

struct TargetExecution {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn run_targets_ordered(
    workspace: &Path,
    targets: &[String],
    jobs: usize,
) -> Result<Vec<TargetExecution>, String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    crate::corpus::run_parallel_ordered(targets.len(), jobs, |index| {
        let target = &targets[index];
        let output = Command::new(&cargo)
            .current_dir(workspace)
            .arg("test")
            .arg("--locked")
            .arg("-p")
            .arg("lumin-cli")
            .arg("--test")
            .arg(target)
            .output()
            .map_err(|error| format!("cannot run Cargo for {target}: {error}"))?;
        Ok(TargetExecution {
            success: output.status.success(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    })
}

fn store_lib_test_modules(listing: &str) -> Result<BTreeSet<String>, String> {
    let mut modules = BTreeSet::new();
    let mut test_count = 0usize;
    for line in listing.lines() {
        let Some(test_name) = line.strip_suffix(": test") else {
            continue;
        };
        let Some((module, _)) = test_name.split_once("::") else {
            return Err(format!(
                "store library test is outside a module partition: {test_name}"
            ));
        };
        let matching_filters = STORE_LIB_TEST_MODULES
            .iter()
            .filter(|candidate| test_name.contains(&format!("{candidate}::")))
            .count();
        if matching_filters != 1 {
            return Err(format!(
                "store library test matches {matching_filters} module filters: {test_name}"
            ));
        }
        modules.insert(module.to_owned());
        test_count += 1;
    }
    if test_count == 0 {
        return Err("store library test listing is empty".to_owned());
    }
    let expected = STORE_LIB_TEST_MODULES
        .iter()
        .map(|module| (*module).to_owned())
        .collect::<BTreeSet<_>>();
    if modules != expected {
        return Err(format!(
            "store library test modules changed: expected {expected:?}, found {modules:?}"
        ));
    }
    Ok(modules)
}

fn run_store_lib_shard(workspace: &Path, arguments: &TestShardArgs) -> Result<(), String> {
    if arguments.count != STORE_LIB_TEST_MODULES.len() {
        return Err(format!(
            "store-lib requires exactly {} shards",
            STORE_LIB_TEST_MODULES.len()
        ));
    }
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let listing = Command::new(&cargo)
        .current_dir(workspace)
        .args([
            "test",
            "--locked",
            "-p",
            STORE_PACKAGE,
            "--lib",
            "--",
            "--list",
            "--format",
            "terse",
        ])
        .output()
        .map_err(|error| format!("cannot list store library tests: {error}"))?;
    if !listing.status.success() {
        let _ = std::io::stdout().write_all(&listing.stdout);
        let _ = std::io::stderr().write_all(&listing.stderr);
        return Err("cannot enumerate store library tests".to_owned());
    }
    let listing_stdout = String::from_utf8(listing.stdout)
        .map_err(|_| "store library test listing is not UTF-8".to_owned())?;
    store_lib_test_modules(&listing_stdout)?;

    let module = STORE_LIB_TEST_MODULES[arguments.index];
    eprintln!(
        "[TEST SHARD] store-lib {}/{}: {module}::",
        arguments.index, arguments.count
    );
    let execution = Command::new(&cargo)
        .current_dir(workspace)
        .args([
            "test",
            "--locked",
            "-p",
            STORE_PACKAGE,
            "--lib",
            &format!("{module}::"),
        ])
        .output()
        .map_err(|error| format!("cannot run store library shard {module}: {error}"))?;
    let _ = std::io::stdout().write_all(&execution.stdout);
    let _ = std::io::stderr().write_all(&execution.stderr);
    if !execution.status.success() {
        return Err(format!("store library shard failed: {module}"));
    }
    Ok(())
}

pub fn run(arguments: &[String]) -> ExitCode {
    let arguments = match parse_args(arguments) {
        Ok(arguments) => arguments,
        Err(error) => {
            eprintln!("[TEST SHARD ERROR] {error}");
            return ExitCode::from(2);
        }
    };
    let workspace = match crate::metadata::find_workspace_root() {
        Ok(workspace) => workspace,
        Err(error) => {
            eprintln!("[TEST SHARD ERROR] {error}");
            return ExitCode::from(2);
        }
    };
    if arguments.suite == TestSuite::StoreLib {
        return match run_store_lib_shard(&workspace, &arguments) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("[TEST SHARD ERROR] {error}");
                ExitCode::from(1)
            }
        };
    }
    let targets = match default_cli_test_targets(&workspace)
        .and_then(|targets| shard_targets(&targets, arguments.index, arguments.count))
    {
        Ok(targets) => targets,
        Err(error) => {
            eprintln!("[TEST SHARD ERROR] {error}");
            return ExitCode::from(2);
        }
    };
    eprintln!(
        "[TEST SHARD] {}/{} targets (jobs={}): {}",
        arguments.index,
        arguments.count,
        arguments.jobs,
        targets.join(", ")
    );
    let executions = match run_targets_ordered(&workspace, &targets, arguments.jobs) {
        Ok(executions) => executions,
        Err(error) => {
            eprintln!("[TEST SHARD ERROR] {error}");
            return ExitCode::from(2);
        }
    };
    let mut failed = false;
    for (target, execution) in targets.iter().zip(executions) {
        if execution.success {
            continue;
        }
        failed = true;
        eprintln!("--- FAIL: CLI integration target {target} ---");
        let _ = std::io::stdout().write_all(&execution.stdout);
        let _ = std::io::stderr().write_all(&execution.stderr);
    }
    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_feature_gated_targets(manifest: &str) -> BTreeSet<String> {
        let mut current_test = None;
        let mut gated = BTreeSet::new();
        for line in manifest.lines().map(str::trim) {
            if line == "[[test]]" {
                current_test = None;
            } else if let Some(value) = line.strip_prefix("name = \"") {
                if current_test.is_none() {
                    current_test = value.strip_suffix('"').map(str::to_owned);
                }
            } else if line.starts_with("required-features =") {
                if let Some(target) = &current_test {
                    gated.insert(target.clone());
                }
            } else if line.starts_with('[') {
                current_test = None;
            }
        }
        gated
    }

    #[test]
    fn arguments_are_strict() -> Result<(), String> {
        let arguments = parse_args(&[
            "--suite".to_owned(),
            "cli-integration".to_owned(),
            "--index".to_owned(),
            "4".to_owned(),
            "--count".to_owned(),
            "5".to_owned(),
            "--jobs".to_owned(),
            "4".to_owned(),
        ])?;
        assert_eq!(arguments.index, 4);
        assert_eq!(arguments.count, 5);
        assert_eq!(arguments.jobs, 4);
        assert_eq!(arguments.suite, TestSuite::CliIntegration);
        assert_eq!(
            parse_args(&[
                "--suite".to_owned(),
                "cli-integration".to_owned(),
                "--index".to_owned(),
                "0".to_owned(),
                "--count".to_owned(),
                "1".to_owned(),
            ])?
            .jobs,
            1,
        );
        for invalid in [
            vec!["--index", "0", "--count", "1"],
            vec!["--suite", "cli-integration", "--index", "5", "--count", "5"],
            vec!["--suite", "cli-integration", "--index", "0", "--count", "0"],
            vec!["--suite", "cli-integration", "--index", "0"],
            vec!["--suite", "unknown", "--index", "0", "--count", "5"],
            vec!["--unknown", "0", "--count", "5"],
            vec![
                "--suite",
                "cli-integration",
                "--index",
                "0",
                "--count",
                "5",
                "--jobs",
                "0",
            ],
            vec![
                "--suite",
                "store-lib",
                "--index",
                "0",
                "--count",
                "4",
                "--jobs",
                "2",
            ],
        ] {
            assert!(
                parse_args(&invalid.into_iter().map(str::to_owned).collect::<Vec<_>>()).is_err()
            );
        }
        Ok(())
    }

    #[test]
    fn store_library_modules_are_complete_and_fail_closed() -> Result<(), String> {
        let listing = concat!(
            "cache::tests::cleanup: test\n",
            "gate::tests::reservation: test\n",
            "namespace::tests::binding: test\n",
            "retention::tests::planning: test\n",
        );
        assert_eq!(
            store_lib_test_modules(listing)?,
            STORE_LIB_TEST_MODULES
                .iter()
                .map(|module| (*module).to_owned())
                .collect()
        );
        assert!(store_lib_test_modules(&format!("{listing}other::tests::new: test\n")).is_err());
        assert!(store_lib_test_modules("top_level_test: test\n").is_err());
        assert!(
            store_lib_test_modules(&listing.replace(
                "namespace::tests::binding",
                "namespace::cache::tests::binding"
            ))
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn six_shards_cover_each_default_cli_target_once() -> Result<(), String> {
        let workspace = crate::metadata::find_workspace_root()?;
        let targets = default_cli_test_targets(&workspace)?;
        let mut observed = Vec::new();
        for index in 0..6 {
            observed.extend(shard_targets(&targets, index, 6)?);
        }
        observed.sort();
        assert_eq!(observed, targets);
        Ok(())
    }

    #[test]
    fn feature_gated_targets_match_the_cli_manifest() -> Result<(), String> {
        let workspace = crate::metadata::find_workspace_root()?;
        let manifest = fs::read_to_string(workspace.join("crates/application/cli/Cargo.toml"))
            .map_err(|error| error.to_string())?;
        assert_eq!(
            manifest_feature_gated_targets(&manifest),
            FEATURE_GATED_TARGETS
                .iter()
                .map(|target| (*target).to_owned())
                .collect()
        );
        Ok(())
    }
}
