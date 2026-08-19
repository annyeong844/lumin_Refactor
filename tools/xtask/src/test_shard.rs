//! Deterministic Windows integration-test sharding for public CI.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::Path;
use std::process::{Command, ExitCode};

const CLI_TEST_DIRECTORY: &str = "crates/application/cli/tests";
const FEATURE_GATED_TARGETS: &[&str] = &[
    "cache_cleanup_publication_race",
    "lifecycle_operation_idempotency",
    "publication_concurrency",
    "publication_faults",
    "publication_retention_race",
    "retention_faults",
];

struct TestShardArgs {
    index: usize,
    count: usize,
}

fn parse_args(arguments: &[String]) -> Result<TestShardArgs, String> {
    let (mut index, mut count) = (None, None);
    let mut cursor = 0;
    while cursor < arguments.len() {
        let target = match arguments[cursor].as_str() {
            "--index" => &mut index,
            "--count" => &mut count,
            unknown => return Err(format!("unknown argument: {unknown}")),
        };
        cursor += 1;
        let raw = arguments
            .get(cursor)
            .ok_or_else(|| format!("{} requires a value", arguments[cursor - 1]))?;
        let value = raw
            .parse::<usize>()
            .map_err(|_| format!("{} requires an integer", arguments[cursor - 1]))?;
        if target.replace(value).is_some() {
            return Err(format!(
                "{} may be provided only once",
                arguments[cursor - 1]
            ));
        }
        cursor += 1;
    }
    let index = index.ok_or_else(|| "--index is required".to_owned())?;
    let count = count.ok_or_else(|| "--count is required".to_owned())?;
    if count == 0 || count > 16 {
        return Err("--count must be an integer from 1 through 16".to_owned());
    }
    if index >= count {
        return Err("--index must be less than --count".to_owned());
    }
    Ok(TestShardArgs { index, count })
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
        "[TEST SHARD] {}/{} targets: {}",
        arguments.index,
        arguments.count,
        targets.join(", ")
    );
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let mut command = Command::new(cargo);
    command
        .current_dir(&workspace)
        .arg("test")
        .arg("--locked")
        .arg("-p")
        .arg("lumin-cli");
    for target in &targets {
        command.arg("--test").arg(target);
    }
    match command.status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::from(1),
        Err(error) => {
            eprintln!("[TEST SHARD ERROR] cannot run Cargo: {error}");
            ExitCode::from(2)
        }
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
            "--index".to_owned(),
            "4".to_owned(),
            "--count".to_owned(),
            "5".to_owned(),
        ])?;
        assert_eq!(arguments.index, 4);
        assert_eq!(arguments.count, 5);
        for invalid in [
            vec!["--index", "5", "--count", "5"],
            vec!["--index", "0", "--count", "0"],
            vec!["--index", "0"],
            vec!["--unknown", "0", "--count", "5"],
        ] {
            assert!(
                parse_args(&invalid.into_iter().map(str::to_owned).collect::<Vec<_>>()).is_err()
            );
        }
        Ok(())
    }

    #[test]
    fn five_shards_cover_each_default_cli_target_once() -> Result<(), String> {
        let workspace = crate::metadata::find_workspace_root()?;
        let targets = default_cli_test_targets(&workspace)?;
        let mut observed = Vec::new();
        for index in 0..5 {
            observed.extend(shard_targets(&targets, index, 5)?);
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
