# Unchanged Latest-Index Synchronization

Candidate W5. Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
Status: design candidate; author and independent review precede implementation.

## Problem and scope

The [complete W4 hosted packet](../phase1-windows-process-observer-2026-09-07/CI-EVIDENCE.md)
still misses Windows scaling. At `d63281b`,
`publication/latest.rs::sync_index` always opens a write transaction, inserts
or removes both pointer keys, and commits, even when both persisted values
already match the authenticated latest document. A fresh audit reaches this
path in open recovery and again before attempt allocation with neither pointer
present. This is source-proven redundant work; its isolated runtime saving has
not been measured. W3's containing intervals are not measurements of the commit
alone, and this change is not promised to meet the 0.75 scaling target.

Limit product changes to this private synchronization body, an owned abort
method on the existing private `StoreWriteTransaction` wrapper, and focused
tests. The wrapper must consume its backend transaction and propagate redb's
abort error; invoking a consuming backend method through `Deref` is not valid.
An exact equality-to-abort barrier uses the existing namespace test feature.
No public command, DTO, schema, pointer authority, lock order, guard or
backend-handle lifetime, recovery ordering, validation rule, durability mode,
pool, allocator, dependency, or numeric budget changes. In particular, no
guard-wide database cache or read-only backend substitution is introduced.

## Existing owner contract

ARCH-002 Sections 2.0/2.3 and 12 remain authoritative. The caller holds the
existing exclusive namespace/catalog guard, authenticates the canonical latest
document and its referenced attempt/run, and synchronizes its derived pointer
index. Both fields still merge under the same exclusive guard. A crash after
latest replacement but before index commit must be repaired on next admission.
An index is never independent authority for overwriting a valid newer document.

## Algorithm

1. Open the existing transaction-scoped `StoreDatabase` through the same guard
   and begin the same write transaction, including current header/generation
   and validation-receipt checks. Do not compare an unlocked cached projection.
2. Within that transaction, compare each optional expected ID's exact bytes
   with its owned pointer key. Absence equals only absence; a missing table is
   the supported empty inventory, not an invented ID or swallowed read error.
3. If both fields match, make no pointer-entry mutations. The owned abort method
   validates the bound namespace and the original `StoreDatabase`'s held
   lifecycle.store identity/generation before abort, propagates the abort error,
   then revalidates that same original identity/generation, the committed receipt
   set, and bound namespace before returning. A newly opened backend in final
   admission is not a substitute for authenticating the original held object.
   The existing enclosing guard's final full validation and normal backend
   destruction remain. No durable user-state commit is needed
   to store values that are already committed. A provisionally opened empty
   table also disappears with that abort.
4. Otherwise retain the existing two-key update and `guard.commit` path,
   including receipt refresh, all pre/post checks and immediate durability.
   Both updates remain atomic, and errors remain errors. Never remove unknown
   rows or attempt to repair an invalid namespace as part of this shortcut.

Only the equality branch is new. The backend may still write its private
recovery/allocator metadata when opened or dropped: this is not a claim of
byte-immutable lifecycle.store, zero disk writes, or removed backend flushes.
Absent and empty owned tables already represent the same empty logical
inventory; an aborted first access must not durably create an empty index.

## Verification

- Supporting owner tests exercise absent/empty, exact populated, differing
  attempt, differing completed-run, insertion/removal, and single-field drift.
  Assert complete pointer rows and unchanged unrelated logical state. A
  private injected commit callback wraps the real guard commit and proves zero
  calls on equality and one on change; it is not a production bypass or public
  feature. A fault injected before the required real commit must propagate
  failure and leave the prior index intact. This does not claim rollback when
  the real commit succeeded but its subsequent validation failed.
- Stop a public child after the unchanged comparison and final pre-abort
  validation; substitute the lifecycle.store entry at that exact turn. The
  original held-object check must reject the substitute. Assert the authentic
  logical snapshot, foreign bytes/identity, and successful recovery after
  restoring the authentic namespace; never treat a fresh final reopen as proof
  that the compared object stayed canonical.
- Public CLI tests prove fresh and repeated audits, latest failed-attempt versus
  completed-run projection, and recovery after exact latest replacement before
  index commit. Existing exact concurrent publisher/retention, namespace
  replacement, migration, and committed-operation retry tests remain required.
  Public behavior and durable snapshots, not callback counts alone, are proof.
- Follow the external Rust pre/post workflow. Run locked focused store/CLI
  tests and affected publication/retention/namespace corpus, then affected
  workspace checks, Clippy, formatting, and actual package smoke on Windows and
  Linux in proportion to this shared-store change.
- Use fresh, separately retained local control/candidate measurements with the
  unchanged observer, fixture truth, jobs policy, three-repetition schedule,
  and all raw outcomes. Do not subtract diagnostic timers from the budget or
  infer hosted improvement from local samples. A later separately authorized
  push must receive new complete blocking Windows/Linux CI evidence.

P1-60/P1-70 remain open. The user requested continuing the Windows performance
work; no approval of an unseen design hash or of a relaxed budget is claimed.
