use std::ffi::OsString;
use std::fs;
use std::path::Path;

use super::{
    expect_success, locate_binary, run_binary, scratch_directory_for, validate_help_output,
};

pub(super) const CODEX_SKILL: &str = "skills/codex/SKILL.md";
const CLAUDE_SKILL: &str = "skills/claude-code/SKILL.md";

pub(super) fn check() -> Result<(), String> {
    let workspace = crate::metadata::find_workspace_root().map_err(|error| error.to_string())?;
    validate_skill_sources(&workspace)?;
    validate_binary_agent_contract(&workspace)?;
    Ok(())
}

pub(super) fn validate_skill_sources(workspace: &Path) -> Result<(), String> {
    let codex = read_skill(workspace, CODEX_SKILL)?;
    let claude = read_skill(workspace, CLAUDE_SKILL)?;
    validate_adapter(CODEX_SKILL, &codex)?;
    validate_adapter(CLAUDE_SKILL, &claude)?;
    if codex != claude {
        return Err(
            "Codex and Claude Code skills diverge from the shared behavior contract".into(),
        );
    }
    validate_skill_directory(workspace, "skills/codex")?;
    validate_skill_directory(workspace, "skills/claude-code")?;
    Ok(())
}

fn read_skill(workspace: &Path, relative: &str) -> Result<String, String> {
    fs::read_to_string(workspace.join(relative))
        .map_err(|error| format!("cannot read {relative}: {error}"))
}

pub(super) fn validate_adapter(relative: &str, source: &str) -> Result<(), String> {
    for required in [
        "name: lumin",
        "description:",
        "lumin help-agent",
        "unique operation ID",
        "operation show",
        "lumin store migrate",
        "retry the original command unchanged",
        "Never read, edit, infer, or repair `.lumin` internals",
    ] {
        if !source.contains(required) {
            return Err(format!(
                "{relative} omitted required workflow text: {required}"
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
    Ok(())
}

fn validate_skill_directory(workspace: &Path, relative: &str) -> Result<(), String> {
    let directory = workspace.join(relative);
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

fn validate_binary_agent_contract(workspace: &Path) -> Result<(), String> {
    let binary = locate_binary(workspace)?;
    let scratch = scratch_directory_for("skills")?;
    fs::create_dir(&scratch)
        .map_err(|error| format!("cannot create package-check scratch directory: {error}"))?;
    let result = expect_success(run_binary(&binary, &scratch, &["help-agent"]), "help-agent")
        .and_then(|output| validate_help_output(&output.stdout));
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
