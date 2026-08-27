use crate::{Arguments, CliError};

const AGENT_HELP: &str = r#"Lumin agent workflow

Read before acting
  lumin help-agent
  Run repository commands from the repository root. Treat JSON stdout as the
  machine result and stderr as diagnostics. Never read or modify .lumin.

Audit and query
  lumin audit --format json
  lumin overview --format json
  lumin findings --run <run-id> --area dead-code --format json
  lumin explain --run <run-id> <finding-id> --format json
  Retain concrete run, finding, gate, plan, pin, and operation IDs returned by
  the binary. Follow nextCursor until truncated is false when exhaustive output
  is required.

Write gate
  Generate and retain a unique operation ID before every mutation.
  lumin pre-write --operation-id <operation-id> --path <repo-path> --format json
  Apply edits only when the returned decision authorizes them, and retain the
  returned gateId.
  lumin post-write <gate-id> --operation-id <new-operation-id> --format json
  Use a new operation ID for abandon, pin, unpin, prune-plan creation,
  prune confirmation, and cache cleanup as well.

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

pub(super) fn execute(arguments: &mut Arguments) -> Result<String, CliError> {
    if let Some(argument) = arguments.next_utf8("help-agent argument")? {
        return Err(CliError::UnknownArgument(argument));
    }
    Ok(AGENT_HELP.trim_end().to_owned())
}
