use std::ffi::OsString;
use std::fs;
use std::path::Path;

use super::locate_fixture_binary;

mod behavior;

pub(super) const CODEX_SKILL: &str = "skills/codex/SKILL.md";
const CLAUDE_SKILL: &str = "skills/claude-code/SKILL.md";
const ADAPTER_PREFIX: &str = concat!(
    "---\n",
    "name: lumin\n",
    "description: Use the packaged native Lumin CLI to audit repositories, query grounded evidence, and manage durable write-gate, retention, cache-cleanup, and lifecycle-store recovery workflows. Use when repository changes need Lumin evidence or authorization.\n",
    "---\n",
    "\n",
    "# Lumin\n",
    "\n",
    "Run `lumin help-agent` from the repository root before choosing command syntax.\n",
    "Treat that installed-binary output as the command and recovery contract.\n",
    "\n",
    "- Use only the packaged `lumin` binary and its public JSON responses.\n",
);
pub(super) const QUERY_WORKFLOW: &str = concat!(
    "- Follow these public command templates exactly; they are checked projections\n",
    "  of the installed binary's agent help, not a second command contract:\n",
    "  1. `lumin audit --jobs 1 --format json`\n",
    "  2. `lumin overview --format json`\n",
    "  3. `lumin findings --run <run-id> --area dead-code --format json`\n",
    "  4. `lumin explain --run <run-id> <finding-id> --format json`\n",
);
pub(super) const MUTATION_WORKFLOW: &str = concat!(
    "- Generate and retain a unique operation ID before each command below. Retain\n",
    "  every returned gate, run, plan, and pin ID needed by the next command:\n",
    "  1. `lumin pre-write --operation-id <operation-id> --path <repo-path> --format json`\n",
    "  2. `lumin post-write <gate-id> --operation-id <operation-id> --format json`\n",
    "  3. `lumin gate abandon <gate-id> --operation-id <operation-id> --reason <reason> --format json`\n",
    "  4. `lumin runs pin <run-id> --operation-id <operation-id> --reason <reason> --format json`\n",
    "  5. `lumin runs unpin <pin-id> --operation-id <operation-id> --format json`\n",
    "  6. `lumin runs prune plan --before <unix-millis> --operation-id <operation-id> --format json`\n",
    "  7. `lumin runs prune confirm <plan-id> --operation-id <operation-id> --format json`\n",
    "  8. `lumin gate prune plan --terminal-before <unix-millis> --operation-id <operation-id> --format json`\n",
    "  9. `lumin gate prune confirm <plan-id> --operation-id <operation-id> --format json`\n",
);
pub(super) const MIGRATION_WORKFLOW: &str = concat!(
    "- When the binary emits its exact migration-required diagnostic, follow this exact recovery sequence:\n",
    "  1. Preserve the original public command and all arguments unchanged.\n",
    "  2. Run `lumin store migrate --format json` and no other migration command.\n",
    "  3. Accept only the exact migration DTO printed by the public agent help.\n",
    "  4. Retry the preserved original public command with the same arguments.\n",
);
pub(super) const OPERATION_RECOVERY_WORKFLOW: &str = concat!(
    "- If any mutation result delivery is uncertain, retain its unique operation ID and never repeat the underlying edit.\n",
    "- For uncertain cache-cleanup delivery, follow this exact public recovery sequence:\n",
    "  1. Preserve the exact original `lumin cache clean --operation-id <operation-id> --format json` command and operation ID.\n",
    "  2. Run `lumin operation show <operation-id> --format json` before any cleanup retry.\n",
    "  3. If show reports a matching committed cache-clean result, consume it and do not rerun cleanup.\n",
    "  4. Otherwise, only the exact same-ID cleanup command may resume as instructed by the public agent help; never mint a replacement ID.\n",
);
const ADAPTER_SUFFIX: &str = concat!(
    "- Never read, edit, infer, or repair `.lumin` internals. Missing, failed, stale,\n",
    "  unsupported, or truncated evidence is not clean evidence.\n",
    "\n",
    "Keep responses concise: cite concrete IDs and the public command result that\n",
    "supports each recommendation.\n",
);
const HELP_AGENT_COMMAND: &str = "lumin help-agent";
const AUDIT_COMMAND: &str = "lumin audit --jobs 1 --format json";
const OVERVIEW_COMMAND: &str = "lumin overview --format json";
const FINDINGS_COMMAND: &str = "lumin findings --run <run-id> --area dead-code --format json";
const EXPLAIN_COMMAND: &str = "lumin explain --run <run-id> <finding-id> --format json";
const PRE_WRITE_COMMAND: &str =
    "lumin pre-write --operation-id <operation-id> --path <repo-path> --format json";
const POST_WRITE_COMMAND: &str =
    "lumin post-write <gate-id> --operation-id <operation-id> --format json";
const GATE_ABANDON_COMMAND: &str = concat!(
    "lumin gate abandon <gate-id> --operation-id <operation-id> ",
    "--reason <reason> --format json",
);
const RUN_PIN_COMMAND: &str = concat!(
    "lumin runs pin <run-id> --operation-id <operation-id> ",
    "--reason <reason> --format json",
);
const RUN_UNPIN_COMMAND: &str =
    "lumin runs unpin <pin-id> --operation-id <operation-id> --format json";
const RUN_PRUNE_PLAN_COMMAND: &str = concat!(
    "lumin runs prune plan --before <unix-millis> ",
    "--operation-id <operation-id> --format json",
);
const RUN_PRUNE_CONFIRM_COMMAND: &str =
    "lumin runs prune confirm <plan-id> --operation-id <operation-id> --format json";
const GATE_PRUNE_PLAN_COMMAND: &str = concat!(
    "lumin gate prune plan --terminal-before <unix-millis> ",
    "--operation-id <operation-id> --format json",
);
const GATE_PRUNE_CONFIRM_COMMAND: &str =
    "lumin gate prune confirm <plan-id> --operation-id <operation-id> --format json";
const CACHE_CLEANUP_COMMAND: &str = "lumin cache clean --operation-id <operation-id> --format json";
const OPERATION_SHOW_COMMAND: &str = "lumin operation show <operation-id> --format json";
const MIGRATION_COMMAND: &str = "lumin store migrate --format json";
const PUBLIC_COMMAND_TEMPLATES: &[&str] = &[
    HELP_AGENT_COMMAND,
    AUDIT_COMMAND,
    OVERVIEW_COMMAND,
    FINDINGS_COMMAND,
    EXPLAIN_COMMAND,
    PRE_WRITE_COMMAND,
    POST_WRITE_COMMAND,
    GATE_ABANDON_COMMAND,
    RUN_PIN_COMMAND,
    RUN_UNPIN_COMMAND,
    RUN_PRUNE_PLAN_COMMAND,
    RUN_PRUNE_CONFIRM_COMMAND,
    GATE_PRUNE_PLAN_COMMAND,
    GATE_PRUNE_CONFIRM_COMMAND,
    CACHE_CLEANUP_COMMAND,
    OPERATION_SHOW_COMMAND,
    MIGRATION_COMMAND,
];

pub(super) fn canonical_adapter_source() -> String {
    [
        ADAPTER_PREFIX,
        QUERY_WORKFLOW,
        MUTATION_WORKFLOW,
        OPERATION_RECOVERY_WORKFLOW,
        MIGRATION_WORKFLOW,
        ADAPTER_SUFFIX,
    ]
    .concat()
}

pub(super) fn check() -> Result<(), String> {
    let package = super::artifact::load_for_host()?;
    let package_root = &package.root;
    validate_skill_sources(package_root)?;
    let binary = &package.binary;
    let fixture_binary = locate_fixture_binary()?;
    behavior::validate_binary_agent_contract(binary)?;
    behavior::validate_packaged_adapter_migration_workflows(package_root, binary, &fixture_binary)?;
    behavior::validate_packaged_adapter_operation_workflows(package_root, binary, &fixture_binary)?;
    Ok(())
}

pub(super) fn stage_skill_sources(workspace: &Path, package_root: &Path) -> Result<(), String> {
    validate_skill_sources(workspace)?;
    if package_root.exists() {
        return Err(format!(
            "staged package root already exists: {}",
            package_root.display()
        ));
    }
    for relative in ["skills/codex", "skills/claude-code"] {
        let destination = package_root.join(relative);
        fs::create_dir_all(&destination).map_err(|error| {
            format!(
                "cannot create staged skill directory {}: {error}",
                destination.display()
            )
        })?;
        fs::copy(
            workspace.join(relative).join("SKILL.md"),
            destination.join("SKILL.md"),
        )
        .map_err(|error| format!("cannot stage {relative}/SKILL.md: {error}"))?;
    }
    validate_skill_sources(package_root)
}

pub(super) fn validate_skill_sources(package_root: &Path) -> Result<(), String> {
    let codex = read_skill(package_root, CODEX_SKILL)?;
    let claude = read_skill(package_root, CLAUDE_SKILL)?;
    validate_adapter(CODEX_SKILL, &codex)?;
    validate_adapter(CLAUDE_SKILL, &claude)?;
    if codex != claude {
        return Err(
            "Codex and Claude Code skills diverge from the shared behavior contract".into(),
        );
    }
    validate_skill_directory(package_root, "skills/codex")?;
    validate_skill_directory(package_root, "skills/claude-code")?;
    Ok(())
}

fn read_skill(package_root: &Path, relative: &str) -> Result<String, String> {
    fs::read_to_string(package_root.join(relative))
        .map_err(|error| format!("cannot read {relative}: {error}"))
}

pub(super) fn validate_adapter(relative: &str, source: &str) -> Result<(), String> {
    let normalized = source.replace("\r\n", "\n");
    let source = normalized.as_str();
    for required in [
        "name: lumin",
        "description:",
        "lumin help-agent",
        "unique operation ID",
        "operation show",
        "Never read, edit, infer, or repair `.lumin` internals",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "{relative} omitted required workflow text: {required}"
            ));
        }
    }
    if source.matches(MIGRATION_WORKFLOW).count() != 1
        || source.matches("lumin store migrate").count() != 1
    {
        return Err(format!(
            "{relative} must contain exactly one canonical migration recovery workflow"
        ));
    }
    if source.matches(OPERATION_RECOVERY_WORKFLOW).count() != 1
        || source
            .matches(&format!("`{CACHE_CLEANUP_COMMAND}`"))
            .count()
            != 1
        || source
            .matches(&format!("`{OPERATION_SHOW_COMMAND}`"))
            .count()
            != 1
    {
        return Err(format!(
            "{relative} must contain exactly one canonical operation recovery workflow"
        ));
    }
    if source.matches(QUERY_WORKFLOW).count() != 1 {
        return Err(format!(
            "{relative} must contain exactly one canonical audit/query workflow"
        ));
    }
    if source.matches(MUTATION_WORKFLOW).count() != 1 {
        return Err(format!(
            "{relative} must contain exactly one canonical mutation workflow"
        ));
    }
    for command in PUBLIC_COMMAND_TEMPLATES {
        if source.matches(&format!("`{command}`")).count() != 1 {
            return Err(format!(
                "{relative} must contain exactly one public command `{command}`"
            ));
        }
    }
    for forbidden in [
        "schemaVersion",
        "lumin.lifecycle-store-migration",
        "lumin-lifecycle-store-header",
        "redb",
        "serde",
        "cargo run",
        "node ",
        ".rs`",
    ] {
        if source.contains(forbidden) {
            return Err(format!(
                "{relative} embeds forbidden implementation detail: {forbidden}"
            ));
        }
    }
    let authored_commands = source
        .split('`')
        .enumerate()
        .filter_map(|(index, text)| (index % 2 == 1 && text.starts_with("lumin ")).then_some(text))
        .collect::<Vec<_>>();
    if authored_commands != PUBLIC_COMMAND_TEMPLATES {
        return Err(format!(
            "{relative} public command sequence differs from the reviewed adapter contract"
        ));
    }
    if source != canonical_adapter_source() {
        return Err(format!(
            "{relative} differs from the canonical thin-adapter source"
        ));
    }
    Ok(())
}

fn validate_skill_directory(package_root: &Path, relative: &str) -> Result<(), String> {
    let directory = package_root.join(relative);
    let mut entries = fs::read_dir(&directory)
        .map_err(|error| format!("cannot inspect {relative}: {error}"))?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("cannot inspect {relative}: {error}"))?;
    entries.sort();
    if entries != [OsString::from("SKILL.md")] {
        return Err(format!(
            "{relative} must contain only SKILL.md; found {entries:?}"
        ));
    }
    Ok(())
}
