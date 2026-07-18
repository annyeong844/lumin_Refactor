# Lumin v2 Workboard

Status: active architecture draft

Revision: 2026-07-18

## One-Line Purpose

Lumin is a native repository-audit engine and Codex/Claude Code skill that gives AI agents grounded, queryable evidence before and after they change code.

## Spec Registry

| ID | Status | Document | One-line role |
| --- | --- | --- | --- |
| METHOD-000 | active | `문서(한글)/SDD.ko.md` (canonical; `SDD.md` English translation) | Defines the permanent Spec-Driven Development workflow for every change. |
| PRODUCT-000 | draft | `specs/000-product-contract.md` | Defines what Lumin v2 must guarantee and what it is not. |
| ARCH-000 | draft | `architecture/000-system-blueprint.md` | Owns the final system shape, crate boundaries, and dependency direction. |
| ARCH-001 | draft | `architecture/001-execution-and-ownership.md` | Owns the Kahn/Rayon execution model, determinism, and memory ownership. |
| ARCH-002 | draft | `architecture/002-evidence-and-write-gate.md` | Owns the evidence store, bounded query protocol, and pre/post transaction. |
| SLICE-001 | draft | `specs/001-foundation-slice.md` | Defines the JS/TS/SFC foundation, with Vue as the first production dialect. |
| REVIEW-001 | superseded | `reviews/architecture-v1-adversarial-2026-07-15.md` | Records the first adversarial findings and first amendment decisions. |
| REVIEW-002 | verifying | `reviews/architecture-v1-independent-verification-2026-07-15.md` | Owns the exact independent verification, current resolutions, and remaining freeze gates. |

## Active Work

Close `NEW-FALSE-NEGATIVE-01` by freezing the rule that grounded findings remain canonical and visible regardless of source-role or remediation disposition, then approve the remaining Phase 0 numeric targets against that unmuted contract. Static-packaging feasibility and clean pinned-upstream provenance are independently passed. Product packages, packaged skill adapters, public behavior, native path/root product round trips, and achieved-budget proofs remain Phase 1 acceptance.

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
| Phase 0: architecture | active | The current review owner marks every freeze gate that can be evidenced without a production scaffold passed or explicitly accepted risk. No product binary or Phase 1 behavior is a Phase 0 prerequisite. |
| Phase 1: foundation slice | blocked by Phase 0 | SLICE-001 passes its complete acceptance corpus on Windows and Linux prebuilt binaries. |
| Phase 2: capability growth | not started | New capabilities enter through the frozen DAG without creating a second engine. |
| Phase 3: legacy retirement | not started | Required compatibility exports and corpus parity are complete; Node analysis paths are removed. |
