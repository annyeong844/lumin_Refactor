# REVIEW-001: Architecture v1 Adversarial Review Resolution

Document role: durable architecture review and resolution record

Status: findings addressed; independent verification pending

Reviewed revision: `637218f89d9f963590af9788e6d90259aa145e4b`

Freeze decision: blocked

## Review Input

An independent adversarial review packet supplied by the user examined every Phase 0 document and repository control file. It reviewed documents only; no implementation, corpus execution, package probe, or store benchmark existed.

This record stores decisions and owner links instead of copying the full review into the permanent context path.

## Blocker Resolution Ledger

| Finding | Decision | Canonical resolution |
| --- | --- | --- |
| F-01 dynamic DAG ambiguity | Accept | ARCH-001 fixes stage nodes from profile plus compiled capabilities; inventory changes batches only. |
| F-02 missing Vue join | Accept | ARCH-001 adds `finalize-sfc-facts` under `lumin-sfc` and external-source reference semantics. |
| F-03 identity/version ownership | Accept | ARCH-000 adds the type-owner/value-authority table. |
| F-04 snapshot and stale evidence | Accept | ARCH-001 defines `SnapshotStatus`; ARCH-002 separates latest attempt and completed run. |
| F-05 gate conflicts too narrow | Accept | ARCH-002 defines declared writes, leased writes, semantic reads, path identity, and close-time cross-gate checks. |
| F-06 uncertainty propagation absent | Accept | ARCH-001 owns limitation scopes; SLICE-001 maps every first-slice opacity class. |
| F-07 scan/resolution policy implicit | Accept | SLICE-001 now owns inventory, probe order, tsconfig, package surface, and consumer tables. |
| F-08 ownerless shape/type-escape lanes | Accept option A | SLICE-001 marks both unavailable and removes their close-out promise. |
| F-09 determinism includes runtime metrics | Accept | ARCH-001 defines the canonical semantic dump and non-semantic metric partition. |
| F-10 verification crates absent | Accept with consolidation | ARCH-000 adds one development-only `lumin-xtask`; SLICE-001 maps every AC to a proof. |
| F-11 CLI/gate decision drift | Accept | ARCH-000 owns the command table; ARCH-002 requires gate ID and defines JSON, decisions, nested bounds, and exits. |
| F-12 performance ordering cycle | Accept | SLICE-001 separates Phase 0 feasibility/targets from Phase 1 binary acceptance. |

## Additional Resolution Ledger

| Review area | Resolution |
| --- | --- |
| Public-surface fact owner | `lumin-resolve` lowers inventory metadata into model facts; graph does not parse manifests. |
| Cache owner | `lumin-store` owns physical cache persistence; capability owners own semantic keys. |
| Cold pre-write | Missing index produces focused rebuild plus visible unverifiable lanes, never a hidden full audit. |
| Stable IDs | ARCH-001 separates cross-run semantic finding IDs from run-local ordinals. |
| Store migration | ARCH-002 forbids in-place completed-run migration and preserves logical gate history. |
| Nested bounds | ARCH-002 pages evidence and relations inside single-item responses. |
| Corpus breadth/provenance | SLICE-001 expands failure fixtures and requires provenance only for copied material. |
| Concurrent latest publication | ARCH-002 uses monotonic attempt sequences and non-regressing pointers. |
| Optional Rust oracle | ARCH-000 makes it explicit opt-in, externally dependent, and visibly unavailable. |
| Review durability | This record is linked by the Workboard. |
| README/AGENTS repetition | README status is explicitly a Workboard projection; AGENTS remains a compact routed summary. |

## Remaining Freeze Gates

1. Run and record the ARCH-002 store correctness/measurement comparison.
2. Run Phase 0 OXC memory/stack and Windows/Linux packaging feasibility probes.
3. Approve numeric Phase 1 target budgets from named evidence.
4. Obtain one independent design review of the amended architecture.
5. Obtain adversarial verification that this revision actually closes the findings above.

No finding is an accepted risk yet. Architecture v1 remains draft and implementation remains blocked.
