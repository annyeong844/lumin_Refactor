# Empty Latest-Index Read-Through

Candidate: W8. Status: **review candidate; not frozen; no implementation authority**.
Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
Review disposition: [REVIEW.md](REVIEW.md).
Examined source: `4f03cc1f8a25e83265a81c0fb1b2c604ce6ee8bf`.

## Decision and limits

Remove one redundant backend lifetime from `latest::ensure_profiled` only when
canonical `latest.json` is absent and the existing POINTERS read proves the
entire table absent or empty. The current reader returns an empty document,
closes its backend, then `sync_index_profiled` opens another backend, begins a
write and aborts the unchanged index. W5 removed the redundant commit, not this
second construction and release. The same empty case occurs twice before a
fresh audit allocates its attempt.

The [second W7 hosted packet](../phase1-lifecycle-boundary-diagnostic-2026-09-08/CI-EVIDENCE.md#lint-correction-hosted-verification)
authenticates 55/58 successful jobs. Both ordinary scaling ratios still miss
`default / jobs=1 <= 0.75`: Windows `0.9609594999583402`, Linux
`0.8562792222437678`. Backend-open plus explicit-drop and return-tail intervals
account for 62-77% of the eight selected Windows default context times, or
19-22% of engine time, in that diagnostic's three rounds. These are selected
API intervals, not isolated redb destruction, lock wait or hardware flush.
They motivate a bounded experiment; they do not predict its speedup. This
small change is not promised to satisfy the scaling budget.

Do not retain a backend across another transaction, namespace mutation,
publication, analysis, lock release or transport. No guard-wide cache,
read-only backend substitution, dependency, store schema, public DTO, worker
policy, allocator, durability or benchmark-budget change is included. W5's
changed-index two-key commit and W6's session/failure-continuation authority
remain unchanged. P1-60/P1-70 stay open.

## Source-backed choice

| Candidate | Decision |
| --- | --- |
| Cache a writable database across publication contexts | Rejected: ARCH-002's transaction/generation boundary and fresh canonical admission are not optional performance costs. |
| Substitute pinned redb's read-only database | Rejected here: redb 4.1.0 exposes its read-only builder by pathname, not Lumin's held file; its read-only open also rejects missing allocator state with `RepairAborted`. This needs a different reviewed protocol, not a fallback. |
| Reuse the already observed empty-index fact within this one exclusive call | Proposed: eliminate only the second application write/abort and backend lifetime, with final original-held proof on every exit of the first read. |

The local pinned dependency source, `redb-4.1.0/src/db.rs`, supplies the second
row's API evidence. `Database::drop` also performs allocator-state maintenance;
zero application writes therefore does not mean byte-invariant backend files.
No undocumented redb switch or raw-header parser is introduced.

## Eligibility and owned result

Keep pending-latest authentication/removal first, in its existing order.
Keep the canonical-document-present path and its index synchronization intact,
even if that document contains no latest IDs. In the canonical-absent branch,
the existing derived-document reader returns one private owned result carrying
both the derived document and whether the **whole** POINTERS table was absent
or empty in that read transaction. It retains no transaction, table or backend.

- Only `TableDoesNotExist` for POINTERS or a successfully inspected zero-row
  table proves empty. Two absent known keys in a nonempty table do not qualify.
  Read errors, malformed values, unsupported state and unknown keys never
  become an empty-index witness. Existing admission remains authoritative for
  rejecting foreign control state; the optimization must not bypass it.
- The empty witness can be consumed only by the immediately enclosing
  `ensure_profiled` call under the same held exclusive catalog guard. It is not
  persisted, cached on the guard or reused by another recovery/attempt.
- If empty, the derived default document still passes the existing document
  schema/linkage validator. No document is published, no POINTERS table is
  created, and `sync_index_profiled` is not called. No later recovery step is
  skipped: empty POINTERS is not proof of an otherwise fresh repository.
- A nonempty derivation follows the existing publication, linkage and W5 sync
  path. An error is propagated, not retried through a different backend or
  converted into an empty document.

## Original-held read completion

The new owned-result reader must not propagate any result after opening its
database until it has finished the original-held proof. Scope the actual
POINTERS read so all table/read-transaction borrows end before that proof.
Retain its `Result`, including read/parse/linkage failure, without an early `?`
that skips completion. No backend is reopened to finish the read.

Reuse W6's complete held-read finishing semantics in the database owner:
factor the existing `finish_attempt_session_read` implementation into a common
private read-finishing body, preserving its current wrapper and behavior. The
latest reader uses a narrowly named owner entry to that same body; do not copy
the header/receipt/physical validators or add a new database abstraction.

The completion requires the guard's backend authority, bound namespace,
original `StoreDatabase` entry/path identity, one-link and volume/mount rules,
header generation, complete receipt set and final bound namespace, all through
the already held objects. The exact feature-only `before-empty-latest-return`
barrier runs **after** the complete first proof; repeat the complete proof
after that barrier before any successful return. A separate
`after-latest-derivation-open` barrier runs after the backend is held and before
reading POINTERS, including a forced-read-error variant. These are controlled
turns, not timing sleeps.

For an empty success also prove canonical `latest.json` and its `.pending`
name absent with no-follow checks against the held state parent, before the
final barrier and again after it. A new or redirected entry at either name
rejects this shortcut and is preserved; it is not adopted, removed or silently
sent through a recovery fallback. The namespace and original-held proof runs
on this failure path too. Existing pending cleanup before the reader is not
reordered or relabelled as shortcut work.

If this reader's backend open fails, reject further backend access on its
original guard before returning. If held completion fails, its error takes
priority over the retained read result and sets the same sticky rejection.
The outer lock wrapper must then follow its existing no-backend final path,
not reopen an otherwise admissible foreign replacement writable. A read error
whose completion succeeds retains its original error; it alone does not poison
backend authority. No new `AttemptSession` exists at either of the two eligible
fresh-audit calls, so this introduces no second session-rejection mechanism.

The backend closes before the owned result reaches document validation,
publication or sync. The only lifetime extension is the explicit same-held
finishing proof inside this original read helper. This is not authority to
change failed-open teardown, constructed field destruction order, W6's
lifetimes, or any other caller's transaction/publication ordering.

## Observation contract for the candidate

Keep W7's v3 field shape, eight contexts, thirteen disjoint API cost meanings,
strict decoding, build/raw-byte binding and `DIAGNOSTIC_ONLY` verdict. W8 would
explicitly amend only the source-derived expected fresh counts for its own
candidate build. Keep earlier W7 design and evidence bytes; never reclassify an
old capture under a new oracle. A reader evaluating an older packet must use
that packet's pinned source/oracle, not guess a variant from its counts.

In W7's ten-cost order below, each of `open-recovery-latest` and
`attempt-recover-latest` changes from `2 / 1 / 1 / 0 / 1 / 0 / 2 / 0 / 0 / 0`
to `1 / 1 / 0 / 0 / 0 / 0 / 1 / 0 / 0 / 0`. Order: backend-open,
read-admission, write-admission, commit, abort, explicit-drop, return-tail,
JSON-write/flush, publication-move, directory-sync. The other six fresh rows
remain unchanged. New eight-context totals are
`15 / 5 / 2 / 2 / 0 / 8 / 7 / 3 / 4 / 6`; opens still balance explicit drops
plus return tails in every row. Healthy-seeded and W7's exact pending-latest
fixture vectors remain unchanged; that fixture has a canonical document.

Observe finishing namespace/store validation as disjoint whole API leaves;
their internal receipt/header reads do not become application-read counts.
End all finishing costs before the natural successful return-tail starts, and
end that tail at the immediate caller. No timing callback is retained on a
resource, and no earlier drop is inserted to manufacture an observation. Update
the model, strict decoder, runner and public count tests together. Negative
tests reject the old fresh vector for the candidate build; no count-based
fallback or weakening of contextual completeness is permitted.

## Required implementation evidence, after explicit freeze

1. Owner tests distinguish absent table, entirely empty table, canonical empty
   document, one/both pointer keys, unknown key, malformed row and read failure.
   Eligible reads have one backend construction and one application read,
   zero application writes/commits/aborts; ineligible paths retain their
   authored W5/W7 behavior. Compare complete logical observations, including
   table inventory, sequence/catalog rows and receipts; never use raw redb
   bytes as the sole logical no-mutation oracle.
2. At both new barriers run actual public `audit` children, covering each
   eligible call separately. Linux must install an otherwise admissible,
   byte-identical but physically distinct canonical store. A held-proof
   failure must leave foreign bytes/identity and the authentic paused logical
   state unchanged, emit no success, and create no new attempt/run. Also test
   forced read failure plus substitution, and ordinary forced read failure
   without substitution. Obtain the full paused logical observation from the
   already held database over the existing feature-only barrier protocol; no
   second lifecycle lock/backend or writable observer may collect it.
3. On Windows require actual substitution and the same assertions, or an
   explicitly authenticated OS sharing/lock/access denial followed by the
   identical prepared rename succeeding after child exit under unchanged
   permissions. The denied healthy branch completes one audit; a denied
   forced-read-error branch fails before new attempt allocation. An arbitrary
   fixture failure is not protection. After authentic restoration use normal
   recovery and repeat the public query: exact logical and filesystem state
   must be stable, with no invented lease, attempt or catalog revision.
4. Independently inject canonical/pending latest arrivals, state/lock/managed
   parent replacement and store extra-link/generation/receipt failure at the
   last barrier. Preserve each foreign object and the exact paused durable
   inventory. A rejected guard cannot construct a new backend during unwind.
   Ordinary read failures with a successful held proof retain supported error
   handling. Both entry points must preserve it; do not rely only on a private
   helper test or public stdout.
5. Keep preexisting Allocating/Active/Releasing or unobserved attempt recovery
   in order. Add an admissible empty-POINTERS crash fixture with a preexisting
   allocation, plus W7's nonempty pending/healthy fixtures. Assert their exact
   owner-defined recovery, attempt/latest/catalog linkage and stable retry.
   Prove exclusive lock ordering against a waiting ordinary writer and
   migration with designated barriers; do not infer order from process timing.
6. Register every new public fault case in the applicable existing corpus
   invocation set and its complete ordered mapping assertion. Run focused
   Windows/Linux ordinary and W7 builds/tests/Clippy, W5/W6 namespace tests,
   publication/retention/generation/crash cases and actual binary/adapter
   package smoke. No zero-test or cfg-skipped lane is passing evidence.
7. Use a new external Rust pre/post advisory for approved implementation.
   After correctness, compare fresh control/candidate seven-mode packets on
   both local platforms with the frozen fixture, full oracle and process
   evidence. Keep every miss; if setup cannot establish the required worker
   policy, do not call it hosted scaling evidence. W7 remains explanatory,
   never a substitute for ordinary numeric authority. Public CI and any
   commit/push require their separate existing user authorization.

## Decision gate

This document permits only author and independent adversarial design review.
If a read/namespace proof, recovery oracle or cost contract cannot be preserved
in this scope, stop and revise the candidate before implementation. Freeze and
code changes require the owner's approval of the exact reviewed candidate.
No claim of runtime correctness or measured optimization follows from a design
PASS, and no broader backend reuse is an automatic next step.
