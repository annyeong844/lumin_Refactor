---
name: lumin
description: Use the packaged native Lumin CLI to audit repositories, query grounded evidence, and manage durable write-gate, retention, cache-cleanup, and lifecycle-store recovery workflows. Use when repository changes need Lumin evidence or authorization.
---

# Lumin

Run `lumin help-agent` from the repository root before choosing command syntax.
Treat that installed-binary output as the command and recovery contract.

- Use only the packaged `lumin` binary and its public JSON responses.
- Generate and retain a unique operation ID before every gate, retention, or
  cache-cleanup mutation. Retain every returned gate, run, finding, plan, and
  pin ID needed by the next public command.
- If any mutation result delivery is uncertain, retain its unique operation ID and never repeat the underlying edit.
- For uncertain cache-cleanup delivery, follow this exact public recovery sequence:
  1. Preserve the exact original `lumin cache clean --operation-id <operation-id> --format json` command and operation ID.
  2. Run `lumin operation show <operation-id> --format json` before any cleanup retry.
  3. If show reports a matching committed cache-clean result, consume it and do not rerun cleanup.
  4. Otherwise, only the exact same-ID cleanup command may resume as instructed by `lumin help-agent`; never mint a replacement ID.
- When the binary emits its exact migration-required diagnostic, follow this exact recovery sequence:
  1. Preserve the original public command and all arguments unchanged.
  2. Run `lumin store migrate --format json` and no other migration command.
  3. Accept only the exact migration DTO printed by `lumin help-agent`.
  4. Retry the preserved original public command with the same arguments.
- Never read, edit, infer, or repair `.lumin` internals. Missing, failed, stale,
  unsupported, or truncated evidence is not clean evidence.

Keep responses concise: cite concrete IDs and the public command result that
supports each recommendation.
