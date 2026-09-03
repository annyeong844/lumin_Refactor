use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::path::Path;

use super::super::{
    downgrade_store_as_prior, expect_migration_ready, expect_migration_required, expect_status,
    expect_string, expect_success, parse_json, run_binary, run_binary_with_broken_stdout,
    scratch_directory_for, validate_help_output,
};
use super::{CLAUDE_SKILL, CODEX_SKILL, read_skill, validate_adapter};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum AdapterAction {
    Audit,
    Overview,
    Findings,
    Explain,
    Related,
    PreWrite,
    PostWrite,
    GateAbandon,
    RunPin,
    RunUnpin,
    RunPrunePlan,
    RunPruneConfirm,
    GatePrunePlan,
    GatePruneConfirm,
    CacheCleanup,
    OperationShow,
    Migration,
}

const ADAPTER_ACTIONS: &[AdapterAction] = &[
    AdapterAction::Audit,
    AdapterAction::Overview,
    AdapterAction::Findings,
    AdapterAction::Explain,
    AdapterAction::Related,
    AdapterAction::PreWrite,
    AdapterAction::PostWrite,
    AdapterAction::GateAbandon,
    AdapterAction::RunPin,
    AdapterAction::RunUnpin,
    AdapterAction::RunPrunePlan,
    AdapterAction::RunPruneConfirm,
    AdapterAction::GatePrunePlan,
    AdapterAction::GatePruneConfirm,
    AdapterAction::CacheCleanup,
    AdapterAction::OperationShow,
    AdapterAction::Migration,
];

impl AdapterAction {
    fn selector(self) -> &'static [&'static str] {
        match self {
            Self::Audit => &["audit"],
            Self::Overview => &["overview"],
            Self::Findings => &["findings"],
            Self::Explain => &["explain"],
            Self::Related => &["related"],
            Self::PreWrite => &["pre-write"],
            Self::PostWrite => &["post-write"],
            Self::GateAbandon => &["gate", "abandon"],
            Self::RunPin => &["runs", "pin"],
            Self::RunUnpin => &["runs", "unpin"],
            Self::RunPrunePlan => &["runs", "prune", "plan"],
            Self::RunPruneConfirm => &["runs", "prune", "confirm"],
            Self::GatePrunePlan => &["gate", "prune", "plan"],
            Self::GatePruneConfirm => &["gate", "prune", "confirm"],
            Self::CacheCleanup => &["cache", "clean"],
            Self::OperationShow => &["operation", "show"],
            Self::Migration => &["store", "migrate"],
        }
    }
}

struct AgentHelp {
    commands: BTreeMap<AdapterAction, String>,
    option_prefixed_operation_show: String,
}

impl AgentHelp {
    fn load(relative: &str, source: &str, binary: &Path, root: &Path) -> Result<Self, String> {
        let arguments = help_bootstrap_arguments(relative, source)?;
        let help = expect_success(
            run_adapter_command(binary, root, &arguments),
            &format!("{relative} help-agent route"),
        )?;
        validate_help_output(&help.stdout)?;
        let stdout = String::from_utf8(help.stdout)
            .map_err(|error| format!("{relative} help-agent output is not UTF-8: {error}"))?;
        let mut commands = BTreeMap::new();
        for action in ADAPTER_ACTIONS {
            let matches = stdout
                .lines()
                .map(str::trim)
                .filter(|line| command_matches(*action, line))
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                return Err(format!(
                    "{relative} installed help must expose exactly one `{}` command, found {}",
                    action.selector().join(" "),
                    matches.len()
                ));
            }
            commands.insert(*action, matches[0].to_owned());
        }
        let operation_help_arguments =
            command_help_bootstrap_arguments(relative, source, AdapterAction::OperationShow)?;
        let operation_help = expect_success(
            run_adapter_command(binary, root, &operation_help_arguments),
            &format!("{relative} installed operation command help"),
        )?;
        let operation_help = String::from_utf8(operation_help.stdout).map_err(|error| {
            format!("{relative} installed operation help is not UTF-8: {error}")
        })?;
        let option_prefixed_operation_show = operation_help
            .lines()
            .map(str::trim)
            .filter(|line| {
                command_matches(AdapterAction::OperationShow, line)
                    && line
                        .split_ascii_whitespace()
                        .collect::<Vec<_>>()
                        .windows(2)
                        .any(|pair| pair == ["--", "<operation-id>"])
            })
            .collect::<Vec<_>>();
        if option_prefixed_operation_show.len() != 1 {
            return Err(format!(
                "{relative} installed operation help must expose exactly one option-prefixed recovery command, found {}",
                option_prefixed_operation_show.len()
            ));
        }
        Ok(Self {
            commands,
            option_prefixed_operation_show: option_prefixed_operation_show[0].to_owned(),
        })
    }

    fn command_arguments(
        &self,
        relative: &str,
        action: AdapterAction,
        replacements: &[(&str, &str)],
    ) -> Result<Vec<String>, String> {
        let command = self.commands.get(&action).ok_or_else(|| {
            format!(
                "{relative} installed help omitted `{}`",
                action.selector().join(" ")
            )
        })?;
        command_arguments(relative, command, replacements)
    }

    fn operation_show_arguments(
        &self,
        relative: &str,
        operation_id: &str,
    ) -> Result<Vec<String>, String> {
        if operation_id.starts_with("--") {
            command_arguments(
                relative,
                &self.option_prefixed_operation_show,
                &[("<operation-id>", operation_id)],
            )
        } else {
            self.command_arguments(
                relative,
                AdapterAction::OperationShow,
                &[("<operation-id>", operation_id)],
            )
        }
    }
}

fn command_matches(action: AdapterAction, line: &str) -> bool {
    let tokens = line.split_ascii_whitespace().collect::<Vec<_>>();
    tokens.first() == Some(&"lumin")
        && tokens
            .get(1..1 + action.selector().len())
            .is_some_and(|candidate| candidate == action.selector())
}

fn help_bootstrap_arguments(relative: &str, source: &str) -> Result<Vec<String>, String> {
    let commands = source
        .split('`')
        .enumerate()
        .filter_map(|(index, text)| (index % 2 == 1 && text.starts_with("lumin ")).then_some(text))
        .collect::<Vec<_>>();
    let command = commands
        .first()
        .filter(|command| **command == "lumin help-agent")
        .ok_or_else(|| format!("{relative} omitted the installed help bootstrap"))?;
    command_arguments(relative, command, &[])
}

fn command_help_bootstrap_arguments(
    relative: &str,
    source: &str,
    action: AdapterAction,
) -> Result<Vec<String>, String> {
    let commands = source
        .split('`')
        .enumerate()
        .filter_map(|(index, text)| (index % 2 == 1 && text.starts_with("lumin ")).then_some(text))
        .collect::<Vec<_>>();
    let command = commands
        .get(1)
        .filter(|command| **command == "lumin <command> --help")
        .ok_or_else(|| format!("{relative} omitted the installed command-help bootstrap"))?;
    let mut arguments = Vec::new();
    for token in command.split_ascii_whitespace().skip(1) {
        if token == "<command>" {
            arguments.push(action.selector()[0].to_owned());
        } else {
            arguments.push(token.to_owned());
        }
    }
    Ok(arguments)
}

fn continuation_arguments_from_installed_help(
    relative: &str,
    source: &str,
    binary: &Path,
    root: &Path,
    action: AdapterAction,
    replacements: &[(&str, &str)],
) -> Result<Vec<String>, String> {
    let help_arguments = command_help_bootstrap_arguments(relative, source, action)?;
    let help = expect_success(
        run_adapter_command(binary, root, &help_arguments),
        &format!(
            "{relative} installed {} command help",
            action.selector().join(" ")
        ),
    )?;
    let stdout = String::from_utf8(help.stdout).map_err(|error| {
        format!(
            "{relative} installed {} command help is not UTF-8: {error}",
            action.selector().join(" ")
        )
    })?;
    let matches = stdout
        .lines()
        .map(str::trim)
        .filter(|line| {
            command_matches(action, line)
                && line
                    .split_ascii_whitespace()
                    .collect::<Vec<_>>()
                    .windows(2)
                    .any(|pair| pair == ["--cursor", "<cursor>"])
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(format!(
            "{relative} installed {} help must expose exactly one cursor continuation, found {}",
            action.selector().join(" "),
            matches.len()
        ));
    }
    command_arguments(relative, matches[0], replacements)
}

fn command_arguments(
    relative: &str,
    command: &str,
    replacements: &[(&str, &str)],
) -> Result<Vec<String>, String> {
    let mut tokens = command.split_ascii_whitespace();
    if tokens.next() != Some("lumin") {
        return Err(format!("{relative} command does not invoke packaged lumin"));
    }
    let mut used = vec![0_usize; replacements.len()];
    let mut arguments = Vec::new();
    for token in tokens {
        if token.starts_with('<') && token.ends_with('>') {
            let (index, (_, value)) = replacements
                .iter()
                .enumerate()
                .find(|(_, (placeholder, _))| placeholder == &token)
                .ok_or_else(|| {
                    format!("{relative} installed command has unresolved placeholder {token}")
                })?;
            used[index] += 1;
            arguments.push((*value).to_owned());
        } else {
            arguments.push(token.to_owned());
        }
    }
    for ((placeholder, _), count) in replacements.iter().zip(used) {
        if count != 1 {
            return Err(format!(
                "{relative} installed command must use {placeholder} exactly once"
            ));
        }
    }
    Ok(arguments)
}

pub(super) fn validate_binary_agent_contract(binary: &Path) -> Result<(), String> {
    let scratch = scratch_directory_for("skills")?;
    fs::create_dir(&scratch)
        .map_err(|error| format!("cannot create package-check scratch directory: {error}"))?;
    let result = expect_success(run_binary(binary, &scratch, &["help-agent"]), "help-agent")
        .and_then(|output| validate_help_output(&output.stdout))
        .and_then(|()| {
            expect_success(
                run_binary(binary, &scratch, &["findings", "--help"]),
                "findings command help",
            )
        })
        .and_then(|output| {
            let stdout = String::from_utf8(output.stdout)
                .map_err(|error| format!("packaged findings help is not UTF-8: {error}"))?;
            let required =
                "lumin findings --run <run-id> --area dead-code --cursor <cursor> --format json";
            if stdout.lines().any(|line| line.trim() == required) {
                Ok(())
            } else {
                Err(format!(
                    "packaged findings help omitted its continuation form: {stdout}"
                ))
            }
        });
    let created_state = scratch.join(".lumin").exists();
    let cleanup = fs::remove_dir_all(&scratch)
        .map_err(|error| format!("cannot remove package-check scratch directory: {error}"));
    result?;
    cleanup?;
    if created_state {
        return Err("packaged lumin help-agent created repository state".to_owned());
    }
    Ok(())
}

pub(super) fn validate_packaged_adapter_migration_workflows(
    package_root: &Path,
    binary: &Path,
    fixture_binary: &Path,
) -> Result<(), String> {
    let scratch = scratch_directory_for("skill-migration")?;
    fs::create_dir(&scratch)
        .map_err(|error| format!("cannot create skill migration scratch directory: {error}"))?;
    let result = [(CODEX_SKILL, "codex"), (CLAUDE_SKILL, "claude-code")]
        .into_iter()
        .try_for_each(|(relative, name)| {
            let source = read_skill(package_root, relative)?;
            validate_adapter(relative, &source)?;
            execute_adapter_migration_workflow(
                relative,
                &source,
                binary,
                fixture_binary,
                &scratch.join(name),
            )
        });
    let cleanup = fs::remove_dir_all(&scratch)
        .map_err(|error| format!("cannot remove skill migration scratch directory: {error}"));
    result?;
    cleanup
}

fn execute_adapter_migration_workflow(
    relative: &str,
    source: &str,
    binary: &Path,
    fixture_binary: &Path,
    root: &Path,
) -> Result<(), String> {
    fs::create_dir_all(root.join("src"))
        .map_err(|error| format!("cannot create {relative} migration fixture: {error}"))?;
    fs::write(
        root.join("package.json"),
        br#"{"name":"lumin-skill-migration","private":true,"type":"module"}"#,
    )
    .map_err(|error| format!("cannot write {relative} migration manifest: {error}"))?;
    let lib_source = format!(
        "export const used = 1;\n{}",
        (0..101)
            .map(|index| format!("export const dead{index:03} = {index};\n"))
            .collect::<String>()
    );
    fs::write(root.join("src/lib.ts"), lib_source)
        .map_err(|error| format!("cannot write {relative} migration source: {error}"))?;
    fs::write(
        root.join("src/main.ts"),
        b"import { used } from './lib.js'; console.log(used);\n",
    )
    .map_err(|error| format!("cannot write {relative} migration entry: {error}"))?;

    let help = AgentHelp::load(relative, source, binary, root)?;

    let audit_arguments = help.command_arguments(relative, AdapterAction::Audit, &[])?;
    let audit = expect_success(
        run_adapter_command(binary, root, &audit_arguments),
        &format!("{relative} fixture audit"),
    )?;
    let audit_json = parse_json(&format!("{relative} fixture audit"), &audit.stdout)?;
    let run_id = audit_json
        .pointer("/runId")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("{relative} fixture audit omitted runId"))?
        .to_owned();

    let intervening_audit = expect_success(
        run_adapter_command(binary, root, &audit_arguments),
        &format!("{relative} intervening audit"),
    )?;
    let intervening_run_id = parse_json(
        &format!("{relative} intervening audit"),
        &intervening_audit.stdout,
    )?
    .pointer("/runId")
    .and_then(serde_json::Value::as_str)
    .ok_or_else(|| format!("{relative} intervening audit omitted runId"))?
    .to_owned();
    if intervening_run_id == run_id {
        return Err(format!(
            "{relative} intervening audit did not publish a distinct latest run"
        ));
    }

    let overview_arguments = help.command_arguments(
        relative,
        AdapterAction::Overview,
        &[("<run-id>", run_id.as_str())],
    )?;
    let overview = expect_success(
        run_adapter_command(binary, root, &overview_arguments),
        &format!("{relative} overview query"),
    )?;
    let overview_json = parse_json(&format!("{relative} overview query"), &overview.stdout)?;
    expect_string(&overview_json, "/schemaVersion", "lumin.overview.v2")?;
    expect_string(&overview_json, "/scope/id", &run_id)?;

    let findings_arguments = help.command_arguments(
        relative,
        AdapterAction::Findings,
        &[("<run-id>", run_id.as_str())],
    )?;
    let findings = expect_success(
        run_adapter_command(binary, root, &findings_arguments),
        &format!("{relative} findings query"),
    )?;
    let findings_json = parse_json(&format!("{relative} findings query"), &findings.stdout)?;
    expect_string(&findings_json, "/schemaVersion", "lumin.collection.v1")?;
    if findings_json
        .get("total")
        .and_then(serde_json::Value::as_u64)
        != Some(101)
        || findings_json
            .get("items")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len)
            != Some(100)
        || findings_json
            .get("truncated")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
    {
        return Err(format!(
            "{relative} findings fixture did not produce the required first page: {findings_json}"
        ));
    }
    let cursor = findings_json
        .get("nextCursor")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("{relative} findings first page omitted nextCursor"))?;
    let continuation_arguments = continuation_arguments_from_installed_help(
        relative,
        source,
        binary,
        root,
        AdapterAction::Findings,
        &[("<run-id>", run_id.as_str()), ("<cursor>", cursor)],
    )?;
    let continuation = expect_success(
        run_adapter_command(binary, root, &continuation_arguments),
        &format!("{relative} findings continuation"),
    )?;
    let continuation_json = parse_json(
        &format!("{relative} findings continuation"),
        &continuation.stdout,
    )?;
    expect_string(&continuation_json, "/schemaVersion", "lumin.collection.v1")?;
    if continuation_json
        .get("total")
        .and_then(serde_json::Value::as_u64)
        != Some(101)
        || continuation_json
            .get("items")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len)
            != Some(1)
        || continuation_json
            .get("truncated")
            .and_then(serde_json::Value::as_bool)
            != Some(false)
        || continuation_json
            .get("nextCursor")
            .is_some_and(|value| !value.is_null())
    {
        return Err(format!(
            "{relative} installed command help did not drive the exact final findings page: {continuation_json}"
        ));
    }
    let finding_ids = findings_json
        .get("items")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .chain(
            continuation_json
                .get("items")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten(),
        )
        .map(|item| {
            item.get("findingId")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| format!("{relative} findings page item omitted findingId"))
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if finding_ids.len() != 101 {
        return Err(format!(
            "{relative} findings continuation skipped or repeated an item"
        ));
    }
    let finding_id = findings_json
        .pointer("/items/0/findingId")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("{relative} findings query omitted findingId"))?;
    let explain_arguments = help.command_arguments(
        relative,
        AdapterAction::Explain,
        &[("<run-id>", run_id.as_str()), ("<finding-id>", finding_id)],
    )?;
    let explain = expect_success(
        run_adapter_command(binary, root, &explain_arguments),
        &format!("{relative} explain query"),
    )?;
    let explain_json = parse_json(&format!("{relative} explain query"), &explain.stdout)?;
    expect_string(&explain_json, "/schemaVersion", "lumin.run-explain.v1")?;
    expect_string(&explain_json, "/finding/findingId", finding_id)?;

    let related_arguments = help.command_arguments(
        relative,
        AdapterAction::Related,
        &[("<run-id>", run_id.as_str()), ("<finding-id>", finding_id)],
    )?;
    let related = expect_success(
        run_adapter_command(binary, root, &related_arguments),
        &format!("{relative} related-evidence query"),
    )?;
    let related_json = parse_json(
        &format!("{relative} related-evidence query"),
        &related.stdout,
    )?;
    expect_string(&related_json, "/schemaVersion", "lumin.collection.v1")?;
    expect_string(&related_json, "/ordering", "relations.v1")?;
    if related_json
        .get("items")
        .and_then(serde_json::Value::as_array)
        .is_none()
    {
        return Err(format!(
            "{relative} related-evidence query omitted its bounded collection"
        ));
    }

    downgrade_store_as_prior(fixture_binary, root, None)?;

    let blocked = run_adapter_command(binary, root, &overview_arguments)?;
    expect_migration_required(&blocked, &format!("{relative} original command"))?;
    let migration_arguments = help.command_arguments(relative, AdapterAction::Migration, &[])?;
    let migration_arguments = command_argument_refs(&migration_arguments);
    expect_migration_ready(
        binary,
        root,
        &migration_arguments,
        &format!("{relative} migration command"),
    )?;
    let retried = expect_success(
        run_adapter_command(binary, root, &overview_arguments),
        &format!("{relative} unchanged original-command retry"),
    )?;
    let retried_json = parse_json(
        &format!("{relative} unchanged original-command retry"),
        &retried.stdout,
    )?;
    expect_string(&retried_json, "/schemaVersion", "lumin.overview.v2")?;
    expect_string(&retried_json, "/scope/id", &run_id)?;
    Ok(())
}

pub(super) fn validate_packaged_adapter_operation_workflows(
    package_root: &Path,
    binary: &Path,
    fixture_binary: &Path,
) -> Result<(), String> {
    let scratch = scratch_directory_for("skill-operation")?;
    fs::create_dir(&scratch)
        .map_err(|error| format!("cannot create skill operation scratch directory: {error}"))?;
    let result = [(CODEX_SKILL, "codex"), (CLAUDE_SKILL, "claude-code")]
        .into_iter()
        .try_for_each(|(relative, name)| {
            let source = read_skill(package_root, relative)?;
            validate_adapter(relative, &source)?;
            execute_adapter_operation_workflow(
                relative,
                name,
                &source,
                binary,
                fixture_binary,
                &scratch.join(name),
            )
        });
    let cleanup = fs::remove_dir_all(&scratch)
        .map_err(|error| format!("cannot remove skill operation scratch directory: {error}"));
    result?;
    cleanup
}

fn execute_adapter_operation_workflow(
    relative: &str,
    adapter_name: &str,
    source: &str,
    binary: &Path,
    fixture_binary: &Path,
    root: &Path,
) -> Result<(), String> {
    fs::create_dir_all(root.join("src"))
        .map_err(|error| format!("cannot create {relative} operation fixture: {error}"))?;
    fs::write(
        root.join("src/lib.ts"),
        b"export const used = 1; export const dead = 2;\n",
    )
    .map_err(|error| format!("cannot write {relative} operation source: {error}"))?;
    fs::write(
        root.join("src/main.ts"),
        b"import { used } from './lib.js'; console.log(used);\n",
    )
    .map_err(|error| format!("cannot write {relative} operation entry: {error}"))?;

    let help = AgentHelp::load(relative, source, binary, root)?;
    let audit_arguments = help.command_arguments(relative, AdapterAction::Audit, &[])?;
    let audit = expect_success(
        run_adapter_command(binary, root, &audit_arguments),
        &format!("{relative} operation fixture audit"),
    )?;
    let audit_json = parse_json(
        &format!("{relative} operation fixture audit"),
        &audit.stdout,
    )?;
    let run_id = audit_json
        .pointer("/runId")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("{relative} operation fixture audit omitted runId"))?
        .to_owned();

    let pre_write_id = operation_id(adapter_name, "pre-write");
    let post_write_id = operation_id(adapter_name, "post-write");
    let second_pre_write_id = operation_id(adapter_name, "abandon-pre-write");
    let abandon_id = operation_id(adapter_name, "abandon");
    let pin_id = operation_id(adapter_name, "run-pin");
    let unpin_id = operation_id(adapter_name, "run-unpin");
    let run_plan_id = operation_id(adapter_name, "run-prune-plan");
    let run_confirm_id = operation_id(adapter_name, "run-prune-confirm");
    let gate_plan_id = operation_id(adapter_name, "gate-prune-plan");
    let gate_confirm_id = operation_id(adapter_name, "gate-prune-confirm");
    let cleanup_id = format!("--{}", operation_id(adapter_name, "cache-clean"));
    require_unique_operation_ids(
        relative,
        [
            pre_write_id.as_str(),
            post_write_id.as_str(),
            second_pre_write_id.as_str(),
            abandon_id.as_str(),
            pin_id.as_str(),
            unpin_id.as_str(),
            run_plan_id.as_str(),
            run_confirm_id.as_str(),
            gate_plan_id.as_str(),
            gate_confirm_id.as_str(),
            cleanup_id.as_str(),
        ],
    )?;

    let pre_write_arguments = help.command_arguments(
        relative,
        AdapterAction::PreWrite,
        &[
            ("<operation-id>", pre_write_id.as_str()),
            ("<repo-path>", "src/lib.ts"),
        ],
    )?;
    let pre_write = recover_uncertain_mutation(
        relative,
        "pre-write",
        &help,
        binary,
        root,
        &pre_write_arguments,
        &pre_write_id,
    )?;
    let gate_id = validate_gate_operation(
        relative,
        &pre_write,
        &pre_write_id,
        "pre-write",
        "active",
        &["allow", "allow-with-warnings"],
    )?;

    let post_write_arguments = help.command_arguments(
        relative,
        AdapterAction::PostWrite,
        &[
            ("<gate-id>", gate_id.as_str()),
            ("<operation-id>", post_write_id.as_str()),
        ],
    )?;
    let post_write = recover_uncertain_mutation(
        relative,
        "post-write",
        &help,
        binary,
        root,
        &post_write_arguments,
        &post_write_id,
    )?;
    validate_gate_operation(
        relative,
        &post_write,
        &post_write_id,
        "post-write",
        "closed",
        &["allow", "allow-with-warnings"],
    )?;

    let second_pre_write_arguments = help.command_arguments(
        relative,
        AdapterAction::PreWrite,
        &[
            ("<operation-id>", second_pre_write_id.as_str()),
            ("<repo-path>", "src/main.ts"),
        ],
    )?;
    let second_pre_write = recover_uncertain_mutation(
        relative,
        "abandon pre-write",
        &help,
        binary,
        root,
        &second_pre_write_arguments,
        &second_pre_write_id,
    )?;
    let abandoned_gate_id = validate_gate_operation(
        relative,
        &second_pre_write,
        &second_pre_write_id,
        "pre-write",
        "active",
        &["allow", "allow-with-warnings"],
    )?;
    let abandon_arguments = help.command_arguments(
        relative,
        AdapterAction::GateAbandon,
        &[
            ("<gate-id>", abandoned_gate_id.as_str()),
            ("<operation-id>", abandon_id.as_str()),
            ("<reason>", "adapter-proof"),
        ],
    )?;
    let abandon = recover_uncertain_mutation(
        relative,
        "gate abandon",
        &help,
        binary,
        root,
        &abandon_arguments,
        &abandon_id,
    )?;
    validate_gate_operation(
        relative,
        &abandon,
        &abandon_id,
        "gate-abandon",
        "abandoned",
        &["deny"],
    )?;

    let pin_arguments = help.command_arguments(
        relative,
        AdapterAction::RunPin,
        &[
            ("<run-id>", run_id.as_str()),
            ("<operation-id>", pin_id.as_str()),
            ("<reason>", "adapter-proof"),
        ],
    )?;
    let pin = recover_uncertain_mutation(
        relative,
        "run pin",
        &help,
        binary,
        root,
        &pin_arguments,
        &pin_id,
    )?;
    validate_retention_operation(relative, &pin, &pin_id, "run-pin", "pin-created")?;
    let retained_pin_id = pin
        .pointer("/operation/result/pin/pinId")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("{relative} recovered pin result omitted pinId"))?
        .to_owned();

    let unpin_arguments = help.command_arguments(
        relative,
        AdapterAction::RunUnpin,
        &[
            ("<pin-id>", retained_pin_id.as_str()),
            ("<operation-id>", unpin_id.as_str()),
        ],
    )?;
    let unpin = recover_uncertain_mutation(
        relative,
        "run unpin",
        &help,
        binary,
        root,
        &unpin_arguments,
        &unpin_id,
    )?;
    validate_retention_operation(relative, &unpin, &unpin_id, "run-unpin", "pin-removed")?;

    let run_plan_arguments = help.command_arguments(
        relative,
        AdapterAction::RunPrunePlan,
        &[
            ("<unix-millis>", "9999999999999"),
            ("<operation-id>", run_plan_id.as_str()),
        ],
    )?;
    let run_plan = recover_uncertain_mutation(
        relative,
        "run prune plan",
        &help,
        binary,
        root,
        &run_plan_arguments,
        &run_plan_id,
    )?;
    validate_retention_operation(
        relative,
        &run_plan,
        &run_plan_id,
        "run-prune-plan",
        "retention",
    )?;
    let retained_run_plan_id = retention_plan_id(relative, &run_plan, "run")?;
    let run_confirm_arguments = help.command_arguments(
        relative,
        AdapterAction::RunPruneConfirm,
        &[
            ("<plan-id>", retained_run_plan_id.as_str()),
            ("<operation-id>", run_confirm_id.as_str()),
        ],
    )?;
    let run_confirm = recover_uncertain_mutation(
        relative,
        "run prune confirm",
        &help,
        binary,
        root,
        &run_confirm_arguments,
        &run_confirm_id,
    )?;
    validate_retention_operation(
        relative,
        &run_confirm,
        &run_confirm_id,
        "run-prune-confirm",
        "retention",
    )?;

    let gate_plan_arguments = help.command_arguments(
        relative,
        AdapterAction::GatePrunePlan,
        &[
            ("<unix-millis>", "9999999999999"),
            ("<operation-id>", gate_plan_id.as_str()),
        ],
    )?;
    let gate_plan = recover_uncertain_mutation(
        relative,
        "gate prune plan",
        &help,
        binary,
        root,
        &gate_plan_arguments,
        &gate_plan_id,
    )?;
    validate_retention_operation(
        relative,
        &gate_plan,
        &gate_plan_id,
        "gate-prune-plan",
        "retention",
    )?;
    let retained_gate_plan_id = retention_plan_id(relative, &gate_plan, "gate")?;
    let gate_confirm_arguments = help.command_arguments(
        relative,
        AdapterAction::GatePruneConfirm,
        &[
            ("<plan-id>", retained_gate_plan_id.as_str()),
            ("<operation-id>", gate_confirm_id.as_str()),
        ],
    )?;
    let gate_confirm = recover_uncertain_mutation(
        relative,
        "gate prune confirm",
        &help,
        binary,
        root,
        &gate_confirm_arguments,
        &gate_confirm_id,
    )?;
    validate_retention_operation(
        relative,
        &gate_confirm,
        &gate_confirm_id,
        "gate-prune-confirm",
        "retention",
    )?;

    let seeded = expect_success(
        run_binary(
            fixture_binary,
            root,
            &["cache", "test-write", "skill-payload.bin", adapter_name],
        ),
        &format!("{relative} cache fixture writer"),
    )?;
    if !seeded.stdout.is_empty() {
        return Err(format!("{relative} cache fixture writer emitted stdout"));
    }

    let cleanup_arguments = help.command_arguments(
        relative,
        AdapterAction::CacheCleanup,
        &[("<operation-id>", cleanup_id.as_str())],
    )?;
    let response = recover_uncertain_mutation(
        relative,
        "cache cleanup",
        &help,
        binary,
        root,
        &cleanup_arguments,
        &cleanup_id,
    )?;
    expect_string(
        &response,
        "/schemaVersion",
        "lumin.cache-cleanup-operation.v2",
    )?;
    expect_string(&response, "/operationId", &cleanup_id)?;
    expect_string(&response, "/kind", "cache-clean")?;
    expect_string(&response, "/status", "committed")?;
    expect_string(&response, "/lastDeliveryStatus", "failed")?;
    expect_string(&response, "/result/operationId", &cleanup_id)?;
    expect_string(&response, "/result/status", "clean")?;
    let active_names = fs::read_dir(root.join(".lumin/cache"))
        .map_err(|error| format!("cannot inspect {relative} active cache: {error}"))?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("cannot inspect {relative} active cache: {error}"))?;
    if active_names != [OsString::from("namespace.anchor")] {
        return Err(format!(
            "{relative} operation recovery left unexpected active-cache entries"
        ));
    }
    Ok(())
}

fn operation_id(adapter_name: &str, kind: &str) -> String {
    format!("skill-{adapter_name}-{kind}-0001")
}

fn require_unique_operation_ids<'a>(
    relative: &str,
    operation_ids: impl IntoIterator<Item = &'a str>,
) -> Result<(), String> {
    let operation_ids = operation_ids.into_iter().collect::<Vec<_>>();
    if operation_ids.iter().copied().collect::<BTreeSet<_>>().len() != operation_ids.len() {
        return Err(format!(
            "{relative} adapter workflow reused a mutation operation ID"
        ));
    }
    Ok(())
}

fn recover_uncertain_mutation(
    relative: &str,
    label: &str,
    help: &AgentHelp,
    binary: &Path,
    root: &Path,
    mutation_arguments: &[String],
    operation_id: &str,
) -> Result<serde_json::Value, String> {
    let mutation_arguments = command_argument_refs(mutation_arguments);
    let failed = run_binary_with_broken_stdout(binary, root, &mutation_arguments)?;
    expect_status(
        &failed,
        Some(1),
        &format!("{relative} uncertain {label} delivery"),
    )?;
    if !failed.stdout.is_empty() || !failed.stderr.is_empty() {
        return Err(format!(
            "{relative} BrokenPipe {label} did not use the canonical empty transport"
        ));
    }

    let show_arguments = help.operation_show_arguments(relative, operation_id)?;
    let shown = expect_success(
        run_adapter_command(binary, root, &show_arguments),
        &format!("{relative} {label} operation-show recovery"),
    )?;
    let response = parse_json(
        &format!("{relative} {label} operation-show recovery"),
        &shown.stdout,
    )?;
    expect_string(&response, "/operationId", operation_id)?;

    let repeated_show = expect_success(
        run_adapter_command(binary, root, &show_arguments),
        &format!("{relative} repeated {label} operation-show recovery"),
    )?;
    if repeated_show.stdout != shown.stdout {
        return Err(format!(
            "{relative} read-only {label} recovery changed the committed DTO"
        ));
    }
    Ok(response)
}

fn validate_gate_operation(
    relative: &str,
    response: &serde_json::Value,
    operation_id: &str,
    kind: &str,
    lifecycle: &str,
    expected_decisions: &[&str],
) -> Result<String, String> {
    expect_string(response, "/schemaVersion", "lumin.operation.v1")?;
    expect_string(response, "/operationId", operation_id)?;
    expect_string(response, "/kind", kind)?;
    expect_string(response, "/status", "committed")?;
    expect_string(response, "/result/schemaVersion", "lumin.gate-mutation.v2")?;
    expect_string(response, "/result/operationId", operation_id)?;
    expect_string(response, "/result/lifecycle", lifecycle)?;
    let decision = response
        .pointer("/result/decision")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("{relative} recovered {kind} result omitted decision"))?;
    if !expected_decisions.contains(&decision) {
        return Err(format!(
            "{relative} recovered {kind} decision {decision} was not one of {expected_decisions:?}"
        ));
    }
    let gate_id = response
        .pointer("/result/gateId")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("{relative} recovered {kind} result omitted gateId"))?;
    expect_string(response, "/gateId", gate_id)?;
    Ok(gate_id.to_owned())
}

fn validate_retention_operation(
    _relative: &str,
    response: &serde_json::Value,
    operation_id: &str,
    kind: &str,
    result_kind: &str,
) -> Result<(), String> {
    expect_string(response, "/schemaVersion", "lumin.retention-operation.v1")?;
    expect_string(response, "/operationId", operation_id)?;
    expect_string(response, "/operation/operationId", operation_id)?;
    expect_string(response, "/operation/kind", kind)?;
    expect_string(response, "/operation/status", "committed")?;
    expect_string(response, "/operation/result/kind", result_kind)
}

fn retention_plan_id(
    relative: &str,
    response: &serde_json::Value,
    domain: &str,
) -> Result<String, String> {
    response
        .pointer("/operation/result/result/planId")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{relative} recovered {domain} plan omitted planId"))
}

fn run_adapter_command(
    binary: &Path,
    root: &Path,
    arguments: &[String],
) -> Result<std::process::Output, String> {
    let arguments = command_argument_refs(arguments);
    run_binary(binary, root, &arguments)
}

fn command_argument_refs(arguments: &[String]) -> Vec<&str> {
    arguments.iter().map(String::as_str).collect()
}
