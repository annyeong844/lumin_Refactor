use std::ffi::OsString;
use std::fs;
use std::path::Path;

use super::{
    downgrade_store_as_prior, expect_migration_ready, expect_migration_required, expect_status,
    expect_string, expect_success, locate_fixture_binary, parse_json, run_binary,
    run_binary_with_broken_stdout, scratch_directory_for, validate_help_output,
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
pub(super) const OPERATION_RECOVERY_WORKFLOW: &str = concat!(
    "- If any mutation result delivery is uncertain, retain its unique operation ID and never repeat the underlying edit.\n",
    "- For uncertain cache-cleanup delivery, follow this exact public recovery sequence:\n",
    "  1. Preserve the exact original `lumin cache clean --operation-id <operation-id> --format json` command and operation ID.\n",
    "  2. Run `lumin operation show <operation-id> --format json` before any cleanup retry.\n",
    "  3. If show reports a matching committed cache-clean result, consume it and do not rerun cleanup.\n",
    "  4. Otherwise, only the exact same-ID cleanup command may resume as instructed by `lumin help-agent`; never mint a replacement ID.\n",
);
const CACHE_CLEANUP_COMMAND: &str = "lumin cache clean --operation-id <operation-id> --format json";
const OPERATION_SHOW_COMMAND: &str = "lumin operation show <operation-id> --format json";
const MIGRATION_ARGUMENTS: &[&str] = &["store", "migrate", "--format", "json"];
pub(super) fn check() -> Result<(), String> {
    let package = super::artifact::load_for_host()?;
    let package_root = &package.root;
    validate_skill_sources(package_root)?;
    let binary = &package.binary;
    let fixture_binary = locate_fixture_binary()?;
    validate_binary_agent_contract(binary)?;
    validate_packaged_adapter_migration_workflows(package_root, binary, &fixture_binary)?;
    validate_packaged_adapter_operation_workflows(package_root, binary, &fixture_binary)?;
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

fn validate_packaged_adapter_operation_workflows(
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
        b"export const skillOperation = 1;\n",
    )
    .map_err(|error| format!("cannot write {relative} operation source: {error}"))?;
    expect_success(
        run_binary(binary, root, &["audit", "--jobs", "1", "--format", "json"]),
        &format!("{relative} operation fixture audit"),
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

    let operation_id = format!("skill-{adapter_name}-cache-clean-0001");
    let cleanup_arguments =
        adapter_command_arguments(relative, source, CACHE_CLEANUP_COMMAND, &operation_id)?;
    let cleanup_arguments = cleanup_arguments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let show_arguments =
        adapter_command_arguments(relative, source, OPERATION_SHOW_COMMAND, &operation_id)?;
    let show_arguments = show_arguments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let failed = run_binary_with_broken_stdout(binary, root, &cleanup_arguments)?;
    expect_status(
        &failed,
        Some(1),
        &format!("{relative} uncertain cleanup delivery"),
    )?;
    if !failed.stdout.is_empty() || !failed.stderr.is_empty() {
        return Err(format!(
            "{relative} BrokenPipe cleanup did not use the canonical empty transport"
        ));
    }

    let shown = expect_success(
        run_binary(binary, root, &show_arguments),
        &format!("{relative} operation-show recovery"),
    )?;
    let response = parse_json(
        &format!("{relative} operation-show recovery"),
        &shown.stdout,
    )?;
    expect_string(
        &response,
        "/schemaVersion",
        "lumin.cache-cleanup-operation.v2",
    )?;
    expect_string(&response, "/operationId", &operation_id)?;
    expect_string(&response, "/kind", "cache-clean")?;
    expect_string(&response, "/status", "committed")?;
    expect_string(&response, "/lastDeliveryStatus", "failed")?;
    expect_string(&response, "/result/operationId", &operation_id)?;
    expect_string(&response, "/result/status", "clean")?;

    let repeated_show = expect_success(
        run_binary(binary, root, &show_arguments),
        &format!("{relative} repeated operation-show recovery"),
    )?;
    if repeated_show.stdout != shown.stdout {
        return Err(format!(
            "{relative} read-only operation recovery changed the committed DTO"
        ));
    }
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

fn adapter_command_arguments(
    relative: &str,
    source: &str,
    command_template: &str,
    operation_id: &str,
) -> Result<Vec<String>, String> {
    let marker = format!("`{command_template}`");
    if source.matches(&marker).count() != 1 {
        return Err(format!(
            "{relative} must contain exactly one adapter command `{command_template}`"
        ));
    }
    let start = source
        .find(&marker)
        .ok_or_else(|| format!("{relative} omitted adapter command `{command_template}`"))?;
    let authored = &source[start + 1..start + marker.len() - 1];
    let mut tokens = authored.split_ascii_whitespace();
    if tokens.next() != Some("lumin") {
        return Err(format!(
            "{relative} adapter command does not invoke packaged lumin"
        ));
    }
    let mut placeholder_count = 0_usize;
    let arguments = tokens
        .map(|token| {
            if token == "<operation-id>" {
                placeholder_count += 1;
                operation_id.to_owned()
            } else {
                token.to_owned()
            }
        })
        .collect::<Vec<_>>();
    if placeholder_count != 1 {
        return Err(format!(
            "{relative} adapter command must contain exactly one operation-ID placeholder"
        ));
    }
    Ok(arguments)
}
