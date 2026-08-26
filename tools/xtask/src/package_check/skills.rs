use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use super::{
    downgrade_store_as_prior, expect_migration_ready, expect_migration_required, expect_string,
    expect_success, locate_binary, locate_fixture_binary, parse_json, run_binary,
    scratch_directory_for, validate_help_output,
};

pub(super) const CODEX_SKILL: &str = "skills/codex/SKILL.md";
const CLAUDE_SKILL: &str = "skills/claude-code/SKILL.md";
pub(super) const MIGRATION_WORKFLOW: &str = concat!(
    "- When the binary emits its exact migration-required diagnostic, follow this exact recovery sequence:\n",
    "  1. Preserve the original public command and all arguments unchanged.\n",
    "  2. Run `lumin store migrate --format json` and no other migration command.\n",
    "  3. Accept only the exact migration DTO printed by `lumin help-agent`.\n",
    "  4. Retry the preserved original public command with the same arguments.\n",
);
const MIGRATION_ARGUMENTS: &[&str] = &["store", "migrate", "--format", "json"];
const PACKAGE_ROOT_ENVIRONMENT: &str = "LUMIN_PACKAGE_ROOT";

pub(super) fn stage() -> Result<(), String> {
    let workspace = crate::metadata::find_workspace_root().map_err(|error| error.to_string())?;
    let package_root = configured_package_root()?;
    if !package_root.is_absolute() {
        return Err("staged package root must be absolute".to_owned());
    }
    let canonical_workspace = workspace
        .canonicalize()
        .map_err(|error| format!("cannot resolve workspace root: {error}"))?;
    if package_root.starts_with(&canonical_workspace) {
        return Err("staged skill package must be outside the checkout workspace".to_owned());
    }
    stage_skill_sources(&workspace, &package_root)
}

pub(super) fn check() -> Result<(), String> {
    let workspace = crate::metadata::find_workspace_root().map_err(|error| error.to_string())?;
    let package_root = locate_package_root(&workspace)?;
    validate_skill_sources(&package_root)?;
    let binary = locate_binary(&workspace)?;
    let fixture_binary = locate_fixture_binary()?;
    validate_binary_agent_contract(&binary)?;
    validate_packaged_adapter_migration_workflows(&package_root, &binary, &fixture_binary)?;
    Ok(())
}

fn configured_package_root() -> Result<PathBuf, String> {
    std::env::var_os(PACKAGE_ROOT_ENVIRONMENT)
        .map(PathBuf::from)
        .ok_or_else(|| format!("a staged package root is required; set {PACKAGE_ROOT_ENVIRONMENT}"))
}

fn locate_package_root(workspace: &Path) -> Result<PathBuf, String> {
    let configured = configured_package_root()?;
    let package_root = configured.canonicalize().map_err(|error| {
        format!(
            "cannot open staged package root {}: {error}",
            configured.display()
        )
    })?;
    let workspace = workspace
        .canonicalize()
        .map_err(|error| format!("cannot resolve workspace root: {error}"))?;
    if package_root.starts_with(&workspace) {
        return Err("staged skill package must be outside the checkout workspace".to_owned());
    }
    Ok(package_root)
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

fn validate_binary_agent_contract(binary: &Path) -> Result<(), String> {
    let scratch = scratch_directory_for("skills")?;
    fs::create_dir(&scratch)
        .map_err(|error| format!("cannot create package-check scratch directory: {error}"))?;
    let result = expect_success(run_binary(binary, &scratch, &["help-agent"]), "help-agent")
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

fn validate_packaged_adapter_migration_workflows(
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
    fs::write(
        root.join("src/lib.ts"),
        b"export const skillMigration = 1;\n",
    )
    .map_err(|error| format!("cannot write {relative} migration source: {error}"))?;

    let audit = expect_success(
        run_binary(binary, root, &["audit", "--jobs", "1", "--format", "json"]),
        &format!("{relative} fixture audit"),
    )?;
    let audit_json = parse_json(&format!("{relative} fixture audit"), &audit.stdout)?;
    let run_id = audit_json
        .pointer("/runId")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("{relative} fixture audit omitted runId"))?
        .to_owned();
    downgrade_store_as_prior(fixture_binary, root, None)?;

    let original_arguments = ["overview", "--format", "json"];
    let blocked = run_binary(binary, root, &original_arguments)?;
    expect_migration_required(&blocked, &format!("{relative} original command"))?;
    expect_migration_ready(
        binary,
        root,
        MIGRATION_ARGUMENTS,
        &format!("{relative} migration command"),
    )?;
    let retried = expect_success(
        run_binary(binary, root, &original_arguments),
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
