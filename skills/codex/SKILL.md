---
name: lumin
description: Use the packaged native Lumin CLI to audit repositories, query grounded evidence, and manage durable write-gate, retention, cache-cleanup, and lifecycle-store recovery workflows. Use when repository changes need Lumin evidence or authorization.
---

# Lumin

Run `lumin help-agent` from the repository root before choosing command syntax.
Use `lumin <command> --help` when a returned cursor or workflow step needs
options not shown in the short agent help. The installed binary is the only
command-syntax and DTO authority.

- Use only the packaged `lumin` binary and its public JSON responses.
- Audit with the deterministic single-worker setting, retain its concrete run
  ID, then query its overview, relevant findings, explanations for chosen IDs,
  and related evidence when relationships matter.
  When a bounded response has a `nextCursor`, use that command's installed help
  and follow the cursor until `truncated` is false when exhaustive output is
  required.
- Generate and retain a unique operation ID before every gate, retention, or
  cache-cleanup mutation. Retain every returned gate, run, plan, and pin ID.
- For a write, request pre-write authorization for the exact repository paths,
  edit only after an authorizing decision, then close that gate with post-write.
  Use gate abandon with its returned gate ID when the authorized edit is
  cancelled.
- For retention, pin and unpin by returned IDs. Create a plan before confirming
  run or terminal-gate pruning, and confirm only the exact returned plan ID.
- If any mutation result delivery is uncertain, retain its unique operation ID and never repeat the underlying edit.
- For uncertain cache-cleanup delivery, preserve the exact original request and
  query operation show with that operation ID before any retry. Consume a
  matching committed result without rerunning cleanup; otherwise resume only as
  instructed by installed help, with the same ID and no replacement ID.
- When the binary emits its exact migration-required diagnostic, follow this exact recovery sequence:
  1. Preserve the original public command and all arguments unchanged.
  2. Run only the lifecycle-store migration command named by installed help.
  3. Accept only the exact migration DTO printed by the public agent help.
  4. Retry the preserved original public command with the same arguments.
- Never read, edit, infer, or repair `.lumin` internals. Missing, failed, stale,
  unsupported, or truncated evidence is not clean evidence.

Keep responses concise: cite concrete IDs and the public command result that
supports each recommendation.
