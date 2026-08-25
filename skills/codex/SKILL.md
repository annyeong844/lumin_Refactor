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
- If mutation delivery is uncertain, use the binary-owned `operation show`
  workflow with the same operation ID. Never repeat the underlying edit.
- When the binary emits its exact migration-required diagnostic, invoke only
  public `lumin store migrate`, accept only the exact DTO documented by
  `lumin help-agent`, and retry the original command unchanged.
- Never read, edit, infer, or repair `.lumin` internals. Missing, failed, stale,
  unsupported, or truncated evidence is not clean evidence.

Keep responses concise: cite concrete IDs and the public command result that
supports each recommendation.
