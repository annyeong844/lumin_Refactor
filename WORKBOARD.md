# Lumin v2 Workboard

Status: active architecture draft

Revision: 2026-07-15

## One-Line Purpose

Lumin is a native repository-audit engine and Codex/Claude Code skill that gives AI agents grounded, queryable evidence before and after they change code.

## Source-of-Truth Order

When documents disagree, use this order:

1. `specs/000-product-contract.md`
2. `architecture/000-system-blueprint.md`
3. The owning focused architecture document
4. The active vertical-slice specification
5. `AGENTS.ko.md` (canonical; `AGENTS.md` is the English translation)

An implementation is not allowed to redefine a higher-level contract implicitly.

## Spec Registry

| ID | Status | Document | One-line role |
| --- | --- | --- | --- |
| METHOD-000 | active | `SDD.md` | Defines the permanent spec-anchored workflow for every change. |
| PRODUCT-000 | draft | `specs/000-product-contract.md` | Defines what Lumin v2 must guarantee and what it is not. |
| ARCH-000 | draft | `architecture/000-system-blueprint.md` | Owns the final system shape, crate boundaries, and dependency direction. |
| ARCH-001 | draft | `architecture/001-execution-and-ownership.md` | Owns the Kahn/Rayon execution model, determinism, and memory ownership. |
| ARCH-002 | draft | `architecture/002-evidence-and-write-gate.md` | Owns the evidence store, bounded query protocol, and pre/post transaction. |
| SLICE-001 | draft | `specs/001-foundation-slice.md` | Defines the first production-grade JS/TS/SFC vertical slice. |
| REVIEW-001 | superseded | `reviews/architecture-v1-adversarial-2026-07-15.md` | Records the first adversarial findings and first amendment decisions. |
| REVIEW-002 | verifying | `reviews/architecture-v1-independent-verification-2026-07-15.md` | Owns the exact independent verification, current resolutions, and remaining freeze gates. |

## Active Work

1. Independently re-verify the exact REVIEW-002 resolution candidate.
2. Run the store, OXC memory/stack, and package feasibility probes recorded by REVIEW-002.
3. Select the store backend and approve named Phase 1 performance targets from measured evidence.
4. Complete the separate independent design review.
5. Freeze Architecture v1 only when REVIEW-002 or its explicit successor marks every remaining gate passed or accepted risk.

## Routing Rules

- Starting any implementation, refactor, review, or specification change: read METHOD-000.
- Product identity, user-visible guarantees, supported environments: read PRODUCT-000.
- Adding, removing, or changing a crate dependency: read ARCH-000.
- Rayon, task ordering, cancellation, locks, memory, or determinism: read ARCH-001.
- Store, queries, SARIF, pre-write, post-write, or parallel agents: read ARCH-002.
- Any Phase 1 implementation or test: read SLICE-001.
- Architecture freeze or review resolution: read REVIEW-002, then follow any explicit predecessor link it cites.
- Repository working rules and close-out discipline: read `AGENTS.ko.md` (canonical) or `AGENTS.md` (English translation).

## Phase Ledger

| Phase | State | Exit condition |
| --- | --- | --- |
| Phase 0: architecture | active | The current review owner marks every remaining freeze gate passed or explicitly accepted risk. |
| Phase 1: foundation slice | blocked by Phase 0 | SLICE-001 passes its complete acceptance corpus on Windows and Linux prebuilt binaries. |
| Phase 2: capability growth | not started | New capabilities enter through the frozen DAG without creating a second engine. |
| Phase 3: legacy retirement | not started | Required compatibility exports and corpus parity are complete; Node analysis paths are removed. |

## Context Rule

Read this Workboard first. Do not load every specification. Follow the routing table and read only the owner documents for the current change.
