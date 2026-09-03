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
    "Use `lumin <command> --help` when a returned cursor or workflow step needs\n",
    "options not shown in the short agent help. The installed binary is the only\n",
    "command-syntax and DTO authority.\n",
    "\n",
    "- Use only the packaged `lumin` binary and its public JSON responses.\n",
);
pub(super) const QUERY_WORKFLOW: &str = concat!(
    "- Audit with the deterministic single-worker setting, retain its concrete run\n",
    "  ID, then query its overview, relevant findings, explanations for chosen IDs,\n",
    "  and related evidence when relationships matter.\n",
    "  When a bounded response has a `nextCursor`, use that command's installed help\n",
    "  and follow the cursor until `truncated` is false when exhaustive output is\n",
    "  required.\n",
);
pub(super) const MUTATION_WORKFLOW: &str = concat!(
    "- Generate and retain a unique operation ID before every gate, retention, or\n",
    "  cache-cleanup mutation. Retain every returned gate, run, plan, and pin ID.\n",
    "- For a write, request pre-write authorization for the exact repository paths,\n",
    "  edit only after decision `allow` or `allow-with-warnings`; `deny`, `incomplete`,\n",
    "  and `stale` never authorize editing. Then close that gate with post-write.\n",
    "  Use gate abandon with its returned gate ID when the authorized edit is\n",
    "  cancelled.\n",
    "- For retention, pin and unpin by returned IDs. Create a plan before confirming\n",
    "  run or terminal-gate pruning, and confirm only the exact returned plan ID.\n",
);
pub(super) const MIGRATION_WORKFLOW: &str = concat!(
    "- When the binary emits its exact migration-required diagnostic, follow this exact recovery sequence:\n",
    "  1. Preserve the original public command and all arguments unchanged.\n",
    "  2. Run only the lifecycle-store migration command named by installed help.\n",
    "  3. Accept only the exact migration DTO printed by the public agent help.\n",
    "  4. Retry the preserved original public command with the same arguments.\n",
);
pub(super) const OPERATION_RECOVERY_WORKFLOW: &str = concat!(
    "- If any mutation result delivery is uncertain, retain its unique operation ID and never repeat the underlying edit.\n",
    "- For uncertain cache-cleanup delivery, preserve the exact original request and\n",
    "  query operation show with that operation ID before any retry. Consume a\n",
    "  matching committed result without rerunning cleanup; otherwise resume only as\n",
    "  instructed by installed help, with the same ID and no replacement ID.\n",
);
const ADAPTER_SUFFIX: &str = concat!(
    "- Never read, edit, infer, or repair `.lumin` internals. Missing, failed, stale,\n",
    "  unsupported, or truncated evidence is not clean evidence.\n",
    "\n",
    "Keep responses concise: cite concrete IDs and the public command result that\n",
    "supports each recommendation.\n",
);
const ADAPTER_COMMAND_BOOTSTRAPS: &[&str] = &["lumin help-agent", "lumin <command> --help"];

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
    if source.matches(MIGRATION_WORKFLOW).count() != 1 {
        return Err(format!(
            "{relative} must contain exactly one canonical migration recovery workflow"
        ));
    }
    if source.matches(OPERATION_RECOVERY_WORKFLOW).count() != 1 {
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
    if authored_commands != ADAPTER_COMMAND_BOOTSTRAPS {
        return Err(format!(
            "{relative} must defer command syntax to the installed binary"
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
