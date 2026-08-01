# Lumin v2

Lumin v2 is an architecture-first rewrite of the Lumin repository-audit and write-gate product. It remains a native CLI and Codex/Claude Code skill, but replaces the legacy multi-producer artifact pipeline with one deterministic Rust engine, queryable canonical evidence, and durable pre-write/post-write transactions.

Status: Phase 1 foundation implementation active. This line is a projection of `WORKBOARD.md`; implementation proceeds only through production behavior owned by SLICE-001.

## Read the Blueprint

Start with [`WORKBOARD.md`](WORKBOARD.md). It routes each kind of work to the smallest owner document.

The architecture review packet is:

1. [`specs/000-product-contract.md`](specs/000-product-contract.md)
2. [`architecture/000-system-blueprint.md`](architecture/000-system-blueprint.md)
3. [`architecture/001-execution-and-ownership.md`](architecture/001-execution-and-ownership.md)
4. [`architecture/002-evidence-and-write-gate.md`](architecture/002-evidence-and-write-gate.md)
5. [`specs/001-foundation-slice.md`](specs/001-foundation-slice.md)

The current freeze ledger is [`REVIEW-002`](reviews/architecture-v1-independent-verification-2026-07-15.md); [`REVIEW-001`](reviews/architecture-v1-adversarial-2026-07-15.md) preserves the first review history.

[`문서(한글)/SDD.ko.md`](문서(한글)/SDD.ko.md) is the canonical Korean development method, and [`SDD.md`](SDD.md) is its English translation. [`문서(한글)/AGENTS.ko.md`](문서(한글)/AGENTS.ko.md) is the canonical Korean repository-agent contract, and [`AGENTS.md`](AGENTS.md) is its English translation.

## Current Decision

The destination architecture is frozen. Implementation proceeds through a narrow production-grade slice covering native JS/TS analysis, a dialect-extensible SFC pipeline with Vue as the first production dialect, resolution, export-level dead evidence, bounded queries, write-gate transactions, and prebuilt Windows/Linux delivery.

Legacy Lumin remains a compatibility and defect corpus. Its internal boundaries are not migration targets.

## Runnable Checkpoint

The native Rust vertical is executable across repository analysis, bounded evidence queries, durable write gates, and retention. The current binary scans JS/TS and Vue SFCs, routes inline Vue scripts through OXC while preserving embedded-source identity, attaches exact external scripts without copying their logical source, admits strict `package.json` and restricted `pnpm-workspace.yaml` workspace facts plus JSONC tsconfig/jsconfig inputs, resolves relative imports, supported `baseUrl`/`paths` mappings, and workspace package public surfaces under an explicit resolution profile, then persists deterministic zero-production-fan-in evidence under `.lumin`.

```text
lumin audit --format json
lumin audit --resolution-profile <bundler|node|node10|node16|nodenext> --format json
lumin overview --run <run-id> --format json
lumin capabilities --run <run-id> --format json
lumin findings --run <run-id> --area dead-code --format json
lumin explain --run <run-id> <finding-id> --format json
lumin related --run <run-id> <finding-id> --format json
lumin files --run <run-id> <repo-path> --format json
lumin runs list --format json
lumin pre-write --operation-id <operation-id> --path <repo-path> --format json
lumin post-write <gate-id> --operation-id <operation-id> --format json
lumin gate list --active --format json
lumin gate show <gate-id> --format json
lumin gate abandon <gate-id> --operation-id <operation-id> --reason <text> --format json
```

Findings, nested evidence, relations, file findings, runs, active gates, and retention-plan items use explicit versioned total orders and bounded, content-bound continuation cursors. Duplicate semantic anchors, cross-scope or nonboundary cursors, stale mutable catalog views, and active-gate record contradictions fail closed rather than restarting at page one. Run and gate retention also expose independent pins, immutable plans, tombstone-aware lookup, and idempotent plan/confirm operations.

The durable gate binds pre-write and post-write through one gate ID, retains logical and physical alias-aware leases, protects semantic reads, records monotonic worktree transitions, and supports exact retry, process-death interruption recovery, abandon, and active-gate listing. State-directory physical identity, generation-bound storage transactions, copy-on-write migration, publication crash recovery, and retention recovery are covered by the current behavioral corpus.

Generated and vendored findings remain canonical `review-only` rows. Vue template opacity, malformed decomposition, external-script conflicts, parse failures, unsupported Svelte/Astro dialects, configuration uncertainty, and resolution uncertainty remain visible incomplete or unavailable evidence; they are never converted into a clean zero.

This checkpoint is still not the completed foundation slice. Start at
[`WORKBOARD.md`](WORKBOARD.md) for owner routing and use
[`PHASE1-CHECKLIST.md`](PHASE1-CHECKLIST.md) as the sole Phase 1 execution and
progress ledger rather than inferring completion from this overview.
