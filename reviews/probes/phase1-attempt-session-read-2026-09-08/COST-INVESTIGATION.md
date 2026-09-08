# Post-W6 Store-Cost Investigation

Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
Status: **read-only findings and next-task proposal; not a frozen design**.
Source: `964c91c6c89ec84a5b2b9bb42851caf664464dfa`.

## What the actual packet establishes

The [current hosted evidence](CI-EVIDENCE.md) leaves Windows scaling at
`0.8858605752999565`, above `0.75`. The three measured default diagnostic
samples place 60.9890-63.2319% of engine command time in the store-owned
regions after excluding nested engine final-input validation. That establishes
an investigation area, not the cost of any one backend or filesystem primitive.

The following are elapsed owner-call observations from those three samples,
in round order. Individual phase medians are summaries, not an additive timeline.

| Store phase | Observed ns, rounds 1/2/3 | Median ns |
| --- | --- | ---: |
| `finalize-latest` | 69,377,000 / 73,900,800 / 72,507,300 | 72,507,300 |
| `finalize-release` | 73,365,600 / 63,515,700 / 63,398,100 | 63,515,700 |
| `publish-terminal` | 56,363,700 / 49,981,700 / 56,244,100 | 56,244,100 |
| `staging-move` | 49,893,900 / 51,533,000 / 50,559,300 | 50,559,300 |
| `attempt-latest` | 49,488,800 / 48,873,700 / 53,096,200 | 49,488,800 |
| `open-recovery-latest` | 33,722,800 / 32,893,800 / 33,236,600 | 33,236,600 |
| `attempt-recover-latest` | 31,101,900 / 35,629,200 / 31,724,100 | 31,724,100 |
| `publish-session` | 18,349,800 / 17,465,400 / 18,988,000 | 18,349,800 |

These are diagnostic executable observations, not ordinary budget samples.
In particular, `staging-move` includes generation validation and publication
checks, not just a rename. The large `publish-preflight` row contains engine
final-input validation; it must not be counted again as backend work.

## Source-backed candidates and uncertainty

Paths and line numbers below describe the source head above.

1. `crates/application/store/src/publication/latest.rs:382` opens a database
   in the catalog-read closure of `validate_document`. Its active-lease closure
   calls `publication/liveness.rs:253`, whose `records::read`
   (`publication/liveness/records.rs:196`) opens another database. Both are
   conditional on the document shape; this is not a claim that every call opens
   both. `latest.rs:509` separately opens a backend for derived-index sync.
   W5 already aborts an unchanged index instead of committing it. The remaining
   call durations do not show how much time these individual opens consume.
2. `crates/application/store/src/namespace/database.rs:42` constructs the
   writable redb backend between held-entry, namespace, header and generation
   checks. `validate_generation` at line 148 opens and drops a backend;
   `mutate_for_generation` at line 134 invokes it before and after a physical
   mutation. These are existing freshness fences, not redundant checks proved
   safe to delete. Backend open/close cost and bound-entry validation cost are
   currently combined in the containing owner interval.
3. `crates/application/store/src/namespace.rs:949` opens/drops a backend in
   `validate_complete`; `with_lock_core` at line 461 performs complete admission
   and final validation for accepted ordinary operations. `validate_bound_entries`
   at line 965 separately authenticates the directory, lock, marker, top-level
   managed parents and nested quarantine. Current wrapper spans also include
   lock acquisition and other work; they are not lock-wait measurements.
4. `crates/application/store/src/namespace/database.rs:100` retains checks and
   receipt refresh around `write.commit`. Measuring its whole caller cannot
   establish the latency of that backend API or the hardware flush beneath it.
   Natural writable-backend destruction can also perform metadata work, as the
   pinned-source analysis in the [W6 design](DESIGN.md) records. No observed
   syscall-time attribution or safe lifetime/readonly substitution is claimed.

## Recommended next bounded task

The prepared [W7 design](../phase1-lifecycle-boundary-diagnostic-2026-09-08/DESIGN.md)
and its [exact review](../phase1-lifecycle-boundary-diagnostic-2026-09-08/REVIEW.md)
now supply this route. Author and independent scoped design reviews pass;
explicit owner freeze and implementation approval remain pending.

Prepare a separately reviewed diagnostic-only design that distinguishes
namespace/generation validation, backend construction, read/transaction work,
natural backend teardown, and explicit publication sync/API intervals inside
the existing store calls. Prioritize latest validation/index synchronization
and generation-guarded publication; retain unmeasured residuals rather than
assigning them to an assumed bottleneck. The design must first establish which
boundaries can be observed without moving any natural resource lifetime.

This is not authorization for a backend cache, shared transaction, read-only
backend substitution, deleted validation, flush removal, or a new optimization.
W6's exact session-read approval does not cover those changes. A diagnostic
extension also needs its own author/independent review and owner approval,
including explicit recorder ownership, exact interval/count semantics, strict
versioned output and isolated build/capture routing. Reuse neither a frozen
W2/W3 frame version nor its PASS for a changed shape.

Any approved probe must compile out of ordinary distributed binaries; preserve
the current public audit, complete semantic oracle, locks, generation/receipt
fences, fault barriers, result/error handling and resource release. No timing
collector may change thread-sharing, backend lifetime, or product scheduling.
API elapsed time must not be called physical disk/flush or lock-wait time without
the corresponding direct observation. Compare a normal control and isolated
diagnostic under an authored fixed schedule, retaining failures and overhead;
do not change worker policy or rerun an ordinary miss until green.

Only after valid observations identify a material cost should a new bounded
product optimization be proposed. This investigation ran no Rust build,
benchmark, product command, new CI job, commit, or push and provides no new
performance PASS. Permanent runtime observations, allocator approval, and the
WSL `/mnt` decision remain separately open under REVIEW-005.
