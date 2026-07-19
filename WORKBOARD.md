# Lumin v2 Workboard

Status: Phase 1 foundation implementation active

Revision: 2026-07-19

## One-Line Purpose

Lumin is a native repository-audit engine and Codex/Claude Code skill that gives AI agents grounded, queryable evidence before and after they change code.

## Spec Registry

| ID | Status | Document | One-line role |
| --- | --- | --- | --- |
| METHOD-000 | active | `문서(한글)/SDD.ko.md` (canonical; `SDD.md` English translation) | Defines the permanent Spec-Driven Development workflow for every change. |
| PRODUCT-000 | frozen | `specs/000-product-contract.md` | Defines what Lumin v2 must guarantee and what it is not. |
| ARCH-000 | frozen | `architecture/000-system-blueprint.md` | Owns the final system shape, crate boundaries, and dependency direction. |
| ARCH-001 | frozen | `architecture/001-execution-and-ownership.md` | Owns the Kahn/Rayon execution model, determinism, and memory ownership. |
| ARCH-002 | frozen | `architecture/002-evidence-and-write-gate.md` | Owns the evidence store, bounded query protocol, and pre/post transaction. |
| SLICE-001 | active | `specs/001-foundation-slice.md` | Defines the JS/TS/SFC foundation, with Vue as the first production dialect. |
| REVIEW-001 | superseded | `reviews/architecture-v1-adversarial-2026-07-15.md` | Records the first adversarial findings and first amendment decisions. |
| REVIEW-002 | complete, monitoring | `reviews/architecture-v1-independent-verification-2026-07-15.md` | Preserves the exact independent verification and may reopen only on a concrete regression or counterexample. |

## Active Work

Implement the first production Rust vertical path from native repository admission through JS/TS inventory, OXC lowering, deterministic graph construction, canonical zero-fan-in findings, and machine output. The numeric targets under `reviews/probes/phase0-numeric-target-selection-2026-07-18/` are approved Phase 1 budgets, not achieved-product claims. Their external audit is non-blocking and may reopen the contract only with a concrete counterexample. Product packages, packaged skill adapters, public behavior, native path/root product round trips, and achieved-budget proofs remain Phase 1 acceptance.

## Routing Rules

- Starting any implementation, refactor, review, or specification change: read METHOD-000.
- Product identity, user-visible guarantees, supported environments: read PRODUCT-000.
- Adding, removing, or changing a crate dependency: read ARCH-000.
- Rayon, task ordering, cancellation, locks, memory, or determinism: read ARCH-001.
- Store, queries, SARIF, pre-write, post-write, or parallel agents: read ARCH-002.
- Any Phase 1 implementation or test: read SLICE-001.
- Architecture freeze or review resolution: read REVIEW-002, then follow any explicit predecessor link it cites.
- Repository working rules and close-out discipline: read `문서(한글)/AGENTS.ko.md` (canonical) or `AGENTS.md` (English translation).

## Phase Ledger

| Phase | State | Exit condition |
| --- | --- | --- |
| Phase 0: architecture | frozen | Independent document/design review, standalone feasibility, clean provenance, the unmuted finding amendment, and owner-approved numeric targets are complete. External target audit is advisory unless it supplies a concrete counterexample. |
| Phase 1: foundation slice | active | SLICE-001 passes its complete acceptance corpus on Windows and Linux prebuilt binaries. |
| Phase 2: capability growth | not started | New capabilities enter through the frozen DAG without creating a second engine. |
| Phase 3: legacy retirement | not started | Required compatibility exports and corpus parity are complete; Node analysis paths are removed. |
