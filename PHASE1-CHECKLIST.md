# Phase 1 Completion Checklist

Status: active

Owner: PLAN-001

Revision: 2026-08-08

## Purpose and Ownership

This document is the sole owner of Phase 1 execution order, dependency order,
and verified progress. It does not redefine product or architecture contracts.

- Phase 1 acceptance is owned by
  [`SLICE-001`](specs/001-foundation-slice.md#14-acceptance-criteria).
- Corpus truth is owned by
  [SLICE-001 Section 9](specs/001-foundation-slice.md#9-truth-corpus); executable
  mapping is owned by
  [`lumin-xtask`](tools/xtask/src/corpus/registry.rs).
- Architecture and gate behavior remain owned by the documents routed from
  [`WORKBOARD.md`](WORKBOARD.md).
- Per-change verification and closeout remain owned by
  [`AGENTS.ko.md`](문서(한글)/AGENTS.ko.md).

`WORKBOARD.md` routes here but does not copy this checklist's counts or status.
README files may describe the product checkpoint but are not progress ledgers.

## Update Rules

1. Change a checkbox or count only from named, reproducible evidence.
2. A focused row pass proves only that row. It does not advance the last full
   aggregate run.
3. `mapped` means the orchestrator selects a real public-binary invocation and
   the invocation proves its exact row marker. An existing test or source file
   alone is not mapped evidence.
4. Missing, unsupported, degraded, stale, or unrun evidence remains open.
5. Do not copy acceptance prose here. Link the owning AC or corpus row and state
   only the work packet, dependency, and required proof.
6. Close the first open priority before starting a later one unless its owner
   proves that the work is independent and cannot invalidate the earlier proof.

## Verified Starting Point

These are execution-matrix counts, not a percentage estimate of product code.

| Lane | Applicable | Mapped | Remaining | Verified aggregate |
| --- | ---: | ---: | ---: | --- |
| Standard | 86 | 53 | 33 | Last full aggregate: 40 passed, 0 failed, 46 unmapped. All 13 later mappings passed focused public-binary markers; P1-70 owns the next full aggregate. |
| Determinism | 86 | 53 | 33 | Last full aggregate: 40 passed, 0 failed, 46 unmapped. The same 13 mappings passed focused nonempty semantic-capture comparison across repeated default jobs and `jobs=1`; P1-70 owns the next full aggregate. |
| Store crash | 10 | 4 | 6 | Mapping count only; Phase 1 exit still requires the complete lane. |
| **Total execution obligations** | **182** | **110** | **72** | This total deliberately counts each required lane execution. |

Known non-corpus exit gaps:

- Windows/Linux packages and packaged Codex/Claude Code adapters are not yet
  accepted as products;
- approved performance and memory budgets have not yet been achieved and
  proven by the completed public binary.

## Priority Order

The ordering rule is: close in-flight truth, then safety and ownership, then
semantic behavior, determinism, crash recovery, distribution, performance, and
finally release authority. A later proof cannot make an earlier incomplete
contract clean.

### P1-00 — Close the Current In-Flight Packet

Owner routes: SLICE-001, repository closeout workflow.

- [x] Map `source-role-classification` to a real public-binary invocation and
  pass its focused marker-backed behavior test.
- [x] Complete the current source-role packet's required closeout checks without
  mixing a new feature into the dirty worktree.
- [x] Run the full standard orchestrator once at the then-current mapping and
  record the exact pass/unmapped result here.

Exit: the current packet is closed, and aggregate evidence no longer trails the
registry count.

### P1-10 — Close Safety, Ownership, and Resolution Prerequisites

Owner routes: ARCH-000, ARCH-001, ARCH-002, SLICE-001 AC 19, 21, 35, 37, and 38.

- [x] Implement the exact actual-write set and physical-alias attribution; keep
  the current fail-closed `UnplannedWrite` behavior until exact evidence exists.
- [x] Implement the owned relative-directory package/index extension-probe
  precedence required by the `extension-probe-precedence` corpus row.
- [x] Enforce path ownership in `architecture-check`.
- [x] Enforce generated-table ownership and drift in `architecture-check`.
- [x] Enforce exhaustive limitation-registry ownership in
  `architecture-check`.
- [x] Enforce the path/root codec runtime boundary in `architecture-check`.
- [x] Enforce the third-party command re-export boundary in
  `architecture-check`.

Exit: no known semantic or structural prerequisite is being hidden by an
unmapped corpus row. Corpus, package, and benchmark proof stay with their own
commands rather than being faked inside `architecture-check`.

### P1-20 — Make Determinism a Paired Acceptance Lane

Owner routes: ARCH-001 and SLICE-001 AC 5, 6, 22, 24, 33, and 35.

- [x] Keep determinism invocations paired with all 51 currently mapped standard
  rows.
- [x] Require every later standard-row packet to add its applicable determinism
  invocation in the same packet.
- [x] Prove repeated default jobs and `jobs=1` produce identical semantic
  evidence and finding IDs; exclude only contract-named runtime/store bytes.

Exit: determinism is no longer an end-of-phase retrofit, and its mapped count
cannot trail the standard count.

### P1-30 — Complete the Standard Public-Behavior Corpus

Owner route: SLICE-001 Section 9 and AC 1-15, 17, 19-23, 26-29, 32, and 37.

- [ ] Close the remaining inventory, native-path, configuration, and resolution
  rows first.
- [ ] Close parsing, SFC, graph, finding, and uncertainty rows on that foundation.
- [ ] Close cache, query, gate, concurrency, lifecycle, publication, retention,
  and migration rows only after their input identities are final.
- [ ] For every row, prove authored truth through the packaged public binary and
  its exact marker; do not count private tests as corpus completion.
- [ ] Reach standard `86/86` and determinism `86/86`.

Exit: every applicable standard and determinism row passes exhaustively with no
unmapped row or silent fallback.

### P1-40 — Complete Store-Crash and Recovery Proof

Owner routes: ARCH-002 and SLICE-001 AC 17, 18, 25, 28, 30, 31, 34, and 36.

- [ ] Map and pass the remaining six store-crash rows through real public child
  processes and named fault points.
- [ ] Prove operation retry, publication, retention, migration, namespace, and
  latest-pointer recovery agree with public lookup state after every death.
- [ ] Reach store-crash `10/10`.

Exit: every named crash point has one durable, publicly queryable outcome.

### P1-50 — Prove Distribution as Product Behavior

Owner routes: PRODUCT-000 and SLICE-001 Sections 11 and 14, AC 8, 9, and 35.

- [ ] Build and probe locked prebuilt Windows and Linux packages.
- [ ] Run package behavior with Cargo and Node unavailable.
- [ ] Prove Codex and Claude Code adapters invoke the same public commands and
  DTOs without embedded semantic tables or source fallbacks.
- [ ] Prove native path/root, NUL-input, cursor, and physical-alias round trips
  through the packaged binaries on both platforms.

Exit: packages and adapters are the tested product, not wrappers around a source
checkout.

### P1-60 — Prove the Frozen Performance and Memory Budgets

Owner route: SLICE-001 Sections 12 and 14, AC 16.

- [ ] Run the blocking Windows and native-Linux benchmark matrix against the
  completed public binary and frozen fixtures.
- [ ] Record cold, warm, `jobs=1`, default-jobs, package-size, and peak-memory
  results with the required toolchain and fixture identities.
- [ ] Run the `/mnt/<drive>` WSL diagnostic as report-only evidence.
- [ ] Treat a missed blocking target as Phase 1 failure or reopen the contract
  through explicit review; never relax a number after observing the result.

Exit: every blocking target passes and the separately labeled WSL diagnostic is
present.

### P1-70 — Obtain Phase 1 Exit Authority

Owner routes: AGENTS.ko.md closeout, SLICE-001 AC 15, and public CI.

- [ ] Pass locked formatting, lint, workspace, architecture, corpus,
  determinism, store-crash, dependency, package, and benchmark checks from a
  clean checkout.
- [ ] Pass the complete Windows/Linux prebuilt-binary matrix in public CI.
- [ ] Confirm no grounded merge-blocking or required-fix external audit item
  remains for the final Phase 1 change.
- [ ] Change PLAN-001 and the WORKBOARD Phase Ledger only after public CI owns
  the complete exit proof.
- [ ] Route Phase 2 through a separately frozen active slice; do not extend this
  checklist into a Phase 2 roadmap.

Exit: SLICE-001 is complete on Windows and Linux prebuilt binaries, Phase 1 is
closed, and Phase 2 may begin without inheriting hidden foundation debt.

## Non-Goals

- This is not a second product specification or architecture document.
- This is not a historical changelog of every implemented feature.
- This is not a product-completion percentage.
- This does not authorize Phase 2 capability work, a Rust capability lane, or a
  replacement lighthouse before Phase 1 exits.
