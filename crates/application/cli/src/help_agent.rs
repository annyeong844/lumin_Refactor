use crate::{Arguments, CliError};

const AGENT_HELP: &str = r#"Lumin agent workflow

Read before acting
  lumin help-agent
  Run repository commands from the repository root. Treat JSON stdout as the
  machine result and stderr as diagnostics. Never read or modify .lumin.

Audit and query
  lumin audit --jobs 1 --format json
  lumin overview --format json
  lumin findings --run <run-id> --area dead-code --format json
  lumin explain --run <run-id> <finding-id> --format json
  Retain concrete run, finding, gate, plan, pin, and operation IDs returned by
  the binary. Follow nextCursor until truncated is false when exhaustive output
  is required.

Write gate
  Generate and retain a unique operation ID before every mutation.
  lumin pre-write --operation-id <operation-id> --path <repo-path> --format json
  Rust is inferred from planned .rs paths. Add
  --capability-at <declared-repo-path> <shape|clone|type-escape> for those typed intents.
  An unavailable requested owner returns an incomplete gate; never substitute another lane.
  Apply edits only when the returned decision authorizes them, and retain the
  returned gateId.
  lumin post-write <gate-id> --operation-id <operation-id> --format json
  lumin gate abandon <gate-id> --operation-id <operation-id> --reason <reason> --format json

Retention
  Use a new operation ID for every command below.
  lumin runs pin <run-id> --operation-id <operation-id> --reason <reason> --format json
  lumin runs unpin <pin-id> --operation-id <operation-id> --format json
  lumin runs prune plan --before <unix-millis> --operation-id <operation-id> --format json
  lumin runs prune confirm <plan-id> --operation-id <operation-id> --format json
  lumin gate prune plan --terminal-before <unix-millis> --operation-id <operation-id> --format json
  lumin gate prune confirm <plan-id> --operation-id <operation-id> --format json

Cache cleanup
  lumin cache clean --operation-id <operation-id> --format json
  Recovery through operation show returns a lumin.cache-cleanup-operation.v2
  object. Inspect its status, result, and lastDeliveryStatus. Delivery status is
  not-attempted, unknown, succeeded, or failed; unknown means the greatest
  allocated delivery attempt has no durable completion, so recover with the
  same operation ID rather than starting another cleanup.

Delivery recovery
  If a mutating command may have committed without delivering its result, do
  not invent a new operation ID. Run:
  lumin operation show <operation-id> --format json
  Then retry the identical mutation with the same operation ID only when the
  returned operation state requires it. Never repeat the underlying edit.

Lifecycle-store migration
  When an ordinary repository-state command exits 1 with exactly:
  lumin: lifecycle store migration requires 'lumin store migrate'
  run:
  lumin store migrate --format json
  Accept migration only when it exits 0 with exactly:
  {"schemaVersion":"lumin.lifecycle-store-migration.v1","storeSchema":"lumin-lifecycle-store-header.v13","status":"ready"}
  Then retry the original public command unchanged. Do not inspect private
  store records or implement migration in the adapter.
"#;

const AUDIT_HELP: &str = r#"Lumin command help: audit
  lumin audit --jobs <count> --format json
  lumin audit --include <pattern> --exclude <pattern> --entry <repo-path> --role-at <pattern> <role> --resolution-profile <profile> --jobs <count> --format json
  --include, --exclude, --entry, and --role-at may be repeated."#;

const OVERVIEW_HELP: &str = r#"Lumin command help: overview
  lumin overview --format json
  lumin overview --run <run-id> --format json"#;

const FINDINGS_HELP: &str = r#"Lumin command help: findings
First page
  lumin findings --run <run-id> --area dead-code --format json
Continuation
  lumin findings --run <run-id> --area dead-code --cursor <cursor> --format json"#;

const EXPLAIN_HELP: &str = r#"Lumin command help: explain
First page
  lumin explain --run <run-id> <finding-id> --format json
Evidence continuation
  lumin explain --run <run-id> <finding-id> --evidence-cursor <cursor> --format json
Relation continuation
  lumin explain --run <run-id> <finding-id> --relations-cursor <cursor> --format json"#;

const RELATED_HELP: &str = r#"Lumin command help: related
First page
  lumin related --run <run-id> <finding-id> --format json
Continuation
  lumin related --run <run-id> <finding-id> --cursor <cursor> --format json"#;

const FILES_HELP: &str = r#"Lumin command help: files
First page
  lumin files --run <run-id> <repo-path> --format json
Continuation
  lumin files --run <run-id> <repo-path> --cursor <cursor> --format json
  Use -- before an option-shaped repository path."#;

const CAPABILITIES_HELP: &str = r#"Lumin command help: capabilities
Binary inventory
  lumin capabilities --format json
  lumin capabilities --cursor <cursor> --format json
Run inventory
  lumin capabilities --run <run-id> --format json
  lumin capabilities --run <run-id> --cursor <cursor> --format json"#;

const PRE_WRITE_HELP: &str = r#"Lumin command help: pre-write
  lumin pre-write --operation-id <operation-id> --path <repo-path> --format json
  lumin pre-write --operation-id <operation-id> --paths0-from - --format json
Optional analysis controls
  --dependency-at <repo-path> <dependency>
  --capability-at <repo-path> <shape|clone|type-escape>
  --include <pattern> --exclude <pattern> --entry <repo-path>
  --role-at <pattern> <role> --resolution-profile <profile> --jobs <count>
  --path, --dependency-at, --capability-at, --include, --exclude, --entry, and --role-at may be repeated."#;

const POST_WRITE_HELP: &str = r#"Lumin command help: post-write
  lumin post-write <gate-id> --operation-id <operation-id> --format json"#;

const GATE_HELP: &str = r#"Lumin command help: gate
  lumin gate show <gate-id> --format json
  lumin gate show <gate-id> --revision <revision> --format json
  lumin gate list --active --format json
  lumin gate list --active --cursor <cursor> --format json
  lumin gate findings <gate-id> --revision <revision> --format json
  lumin gate findings <gate-id> --revision <revision> --cursor <cursor> --format json
  lumin gate explain <gate-id> --revision <revision> <finding-id> --format json
  lumin gate explain <gate-id> --revision <revision> <finding-id> --evidence-cursor <cursor> --format json
  lumin gate explain <gate-id> --revision <revision> <finding-id> --relations-cursor <cursor> --format json
  lumin gate abandon <gate-id> --operation-id <operation-id> --reason <reason> --format json
  lumin gate prune plan --terminal-before <unix-millis> --operation-id <operation-id> --format json
  lumin gate prune plan show <plan-id> --format json
  lumin gate prune plan show <plan-id> --cursor <cursor> --format json
  lumin gate prune confirm <plan-id> --operation-id <operation-id> --format json"#;

const OPERATION_HELP: &str = r#"Lumin command help: operation
  lumin operation show <operation-id> --format json
  lumin operation show -- <operation-id> --format json"#;

const RUNS_HELP: &str = r#"Lumin command help: runs
  lumin runs list --format json
  lumin runs list --cursor <cursor> --format json
  lumin runs pin <run-id> --operation-id <operation-id> --reason <reason> --format json
  lumin runs unpin <pin-id> --operation-id <operation-id> --format json
  lumin runs prune plan --before <unix-millis> --operation-id <operation-id> --format json
  lumin runs prune plan show <plan-id> --format json
  lumin runs prune plan show <plan-id> --cursor <cursor> --format json
  lumin runs prune confirm <plan-id> --operation-id <operation-id> --format json"#;

const CACHE_HELP: &str = r#"Lumin command help: cache
  lumin cache clean --operation-id <operation-id> --format json"#;

const STORE_HELP: &str = r#"Lumin command help: store
  lumin store migrate --format json"#;

const HELP_AGENT_HELP: &str = r#"Lumin command help: help-agent
  lumin help-agent"#;

pub(super) fn execute(arguments: &mut Arguments) -> Result<String, CliError> {
    if let Some(argument) = arguments.next_utf8("help-agent argument")? {
        return Err(CliError::UnknownArgument(argument));
    }
    Ok(AGENT_HELP.trim_end().to_owned())
}

pub(super) fn command_help(command: &str) -> Option<String> {
    let help = match command {
        "audit" => AUDIT_HELP,
        "overview" => OVERVIEW_HELP,
        "findings" => FINDINGS_HELP,
        "explain" => EXPLAIN_HELP,
        "related" => RELATED_HELP,
        "files" => FILES_HELP,
        "capabilities" => CAPABILITIES_HELP,
        "pre-write" => PRE_WRITE_HELP,
        "post-write" => POST_WRITE_HELP,
        "gate" => GATE_HELP,
        "operation" => OPERATION_HELP,
        "runs" => RUNS_HELP,
        "cache" => CACHE_HELP,
        "store" => STORE_HELP,
        "help-agent" => HELP_AGENT_HELP,
        _ => return None,
    };
    Some(help.trim_end().to_owned())
}
