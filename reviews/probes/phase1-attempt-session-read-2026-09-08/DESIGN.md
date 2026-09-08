# Generation-Bound Attempt-Session Read

Candidate W6. Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
Status: **design candidate; not frozen; no implementation authority**.

## Decision and evidence

Limit the next optimization to the two consecutive lifecycle database lifetimes
inside `AttemptSession::validate`. At source
`95163f586f2d9ea9dcae7f356d94e10469ec84e9`,
`publication/liveness.rs:286` opens the database for the session generation and
immediately drops it. `records::read` then opens the same canonical database
again to read and authenticate that session's lease. Successful run publication
invokes this validation in prepare, finalize, and release. There is no product
mutation between the two opens within a validation call.

The pinned redb 4.1.0 `Database::drop` calls
`ensure_allocator_state_table_and_trim`, which performs a quick-repair commit,
and then closes transactional memory. Its writable-open/close paths also flush
backend metadata. These are source-proven operations, not measurements of
individual disk waits. An unchanged application read does not imply a
write-free backend lifetime.

[Hosted run 34135740840](https://github.com/annyeong844/lumin_Refactor/actions/runs/34135740840)
passes 56 of 58 jobs; Windows package scaling and dependent Required fail.
The ordinary four-worker median is `2,856,558,900 ns`, jobs=1 median
`3,286,151,800 ns`, ratio `0.8692717421027233` against `0.75`. Every other
ordinary Windows numeric budget passes. All Windows integration jobs pass;
that does not prove the cause of the earlier gate-watchdog failures.

The separately captured W3 diagnostic places 83.8735% to 90.6347% of each
four-worker command in the summed self-times of store-open, attempt-begin,
and store-publish. `publish-session` is an enclosing observation, not the cost
of the duplicated backend lifetime. No isolated saving or scaling PASS is
claimed. W6 is a bounded reduction of provably repeated work, not a promise
to close the entire remaining gap.

The latest ordinary Windows report SHA-256 is
`6102b65c4d4635ca2caa0947c46fc82ecf260d559cd3a851798ba5f47c99e293`;
diagnostic report SHA-256 is
`a14c6d24160576cedc21c230457cbdb97add6ea1964fe3d30eed27208198262a`.
The run retains `lumin-foundation-benchmark-windows-x64` and
`lumin-audit-store-diagnostic-windows-x64`. Both capture inventories and every
file hash/length were checked locally (745 and 379 captures respectively).
The diagnostic is not numeric-budget authority and must never be subtracted
from ordinary process time.

## Unchanged owner boundary and non-goals

ARCH-002 Sections 2.0, 2.3, 2.4, and 12 remain authoritative. Keep one short,
transaction-scoped backend lifetime for this logical read. End the read and
drop that backend before existing external liveness-lock validation returns
and before any later physical mutation, latest-pointer publication, lock
release, or result transport. Preserve the original lock acquisition order.

No guard-wide or process-wide database cache, shared transaction, detached
snapshot substitute, read-only backend substitution, generation memoization,
new lock, pool, thread policy, schema, DTO, dependency, durability mode, flush
removal from an actual committed write, or numeric-budget change is permitted.
All other callers of `records::read` keep their existing behavior. Existing
write transactions, catalog publication, migration, and recovery ordering are
outside the implementation scope.

## Algorithm and failure authority

1. Keep the existing Active-session check and the caller's acquired namespace
   guard. Open exactly one database through
   `guard.open_database_for_generation(session.generation())`; retain its
   original held file identity and generation. If this open fails, mark the
   existing guard backend-rejected before propagating its original error.
   There is no second open to recover the failed read. This cannot undo
   backend metadata writes already performed by the failed open itself; it
   forbids a subsequent teardown reopen through a substituted canonical name.
2. Read the requested lease through one ordinary guarded read transaction on
   that database. Reuse the existing lease-row decoder and exact key, schema,
   nonce, state, physical-lock binding, and canonical-byte checks. Missing
   table/row remains the existing missing-session failure, never an empty
   successful lease. Retain the complete read result instead of using an early
   return from the outer operation. Close the read transaction on every path,
   including begin-read, row-read, missing-row, and decoding failures.
3. For both successful and failed read results, revalidate the complete bound namespace, the
   **same held** database entry/path/link count/volume/generation, and its current
   validation-receipt set. Repeat the bound-namespace check after those checks.
   A newly opened backend in enclosing final admission is not evidence that
   the read object remained canonical. Keep this finish-read validation local
   to this path; do not weaken or replace any existing admission checks.
   The final-turn test barrier is after this complete proof, while the original
   database is still held. After that barrier, repeat the same complete proof
   before dropping that database or releasing the owned result. A feature-only
   early check cannot stand in for this post-turn original-object proof.
4. If original-object, namespace, generation, or receipt validation fails,
   set the existing guard's sticky rejected-backend state before unwinding.
   Its existing rejection-aware teardown must not reopen a foreign canonical
   replacement for writing. No retry or fallback inside this read is allowed.
   A finish-read validation error takes precedence over the retained read
   error, matching the enclosing guard's validation-first error ordering.
   If finishing succeeds, propagate any retained read/decoding/missing-row
   error unchanged; cleanup may not convert it to success. Natural backend
   destruction can still change its own recovery
   metadata; no byte-immutable lifecycle.store claim is made.
5. Drop the database, compare the complete owned lease against the session's
   lease, and perform the existing held process-liveness lock identity/content
   validation. Do not replace the persisted-row comparison with the in-memory
   session or change Active/Releasing/Allocating semantics.

Expected implementation ownership: `publication/liveness.rs`, its existing
`records.rs` owner, a narrowly visible original-database finish-read validation
method in `namespace/database.rs`, and focused tests using the existing fault
feature. Do not create a second parser or change unrelated records::read users.

## Verification and decision gate

- Supporting owner tests count actual database construction at this path,
  proving one open, one generation-bound **lease-row** read transaction, and no application write commit
  for a successful validation. Exercise current/wrong generation, missing and
  malformed lease, changed owner/nonce/record, and failed original-object or
  receipt validation. Header and receipt validation have their own existing
  read transactions and are not included in the one lease-row count. Counts
  alone are not public correctness evidence.
- Public fault children stop after the generation-bound open before the row
  read, and separately after all final read validations immediately before
  returning its owned result. Attempt to substitute lifecycle.store with a
  closed, otherwise admissible object at each exact turn. Its valid namespace
  binding, generation and receipt set must be established independently before
  the fault; a malformed replacement does not prove original-object checking.
  Capture the foreign bytes and identity after fixture preparation is complete.
  Successful substitution must produce the original held-identity integrity
  rejection, not an unrelated lease/schema failure. A post-barrier check of the
  original held binding is mandatory; placing a barrier only before the final
  validations does not prove the final-turn substitution case.
- Linux must actually perform substitution at both turns. Windows may instead
  deny the authentic-file rename because the backend remains open. Accept this
  branch only with a captured Windows sharing/lock/access-denied OS result,
  controlled unchanged permissions, and proof that the same prepared replacement
  is permitted after the child releases its handles. Unexpected fixture errors
  fail the test. In this branch require unchanged foreign bytes/identity and
  the authentic audit's normal completed lifecycle with exactly one catalog
  insertion and coherent terminal/latest state. Do not call it a successful
  substitution test or require a mutating audit's authentic snapshot to remain
  unchanged. Restoring the tested closed placement must preserve that completed
  logical state before public follow-up queries.
- Also combine a deterministic lease-read/decoding error with substitution at
  the after-open turn. Finish-read validation and sticky rejection must still
  run, and enclosing teardown must preserve foreign bytes and identity. Cover
  generation-bound open failure plus an otherwise admissible foreign canonical
  replacement with a supporting exact fault test; no failed-open path may
  enter the enclosing writable teardown. These are distinct from separate
  healthy-read substitution and malformed-row-only cases. On Windows retain
  the exact OS-denied outcome if substitution is prevented; a forced read
  error must still fail, not be changed into the healthy-audit success branch.
- Compare the authentic logical store snapshot, foreign bytes/physical identity,
  attempt/run directories, and latest pointer before and after each rejected
  transition. Account explicitly for the already committed Running attempt and
  backend-only recovery metadata. After restoring the authentic object, prove
  normal public recovery and no invented completed run or duplicate catalog
  insertion. Test both Windows and Linux with exact barriers, never sleeps.
- Preserve public fresh/repeated audit, interrupted/failed attempt recovery,
  current/old-generation late publisher, reversed publication order, and
  publication-versus-retention outcomes. Reuse the existing required-feature
  public fault runners; register and lock the complete new invocation mapping
  if a new public test is introduced. An unexecuted test is not evidence.
- Before Rust edits, obtain the external Rust pre-write advisory. Run focused
  locked tests, matching post-write, scoped Clippy/fmt and affected publication,
  namespace, generation, and retention checks. Preserve public package evidence
  on Windows and Linux for this store-path change. Do not launch broad builds
  or benchmarks while host memory is constrained.
- Measure fresh control and candidate ordinary seven-mode packets with the
  existing fixture, full semantic truth, worker policy, conditioning schedule,
  observer, and three repetitions. Use a separately declared four-worker local
  comparison if reproducing the hosted topology; retain the unmodified host
  policy run separately. Preserve all misses and all samples. A local ratio or
  this source-level optimization cannot replace new blocking CI authority.

Before implementation, obtain an author design review, an independent
adversarial review of these exact bytes, and explicit owner approval. If this
candidate does not provide useful measured improvement, preserve the evidence
and return to candidate selection rather than enlarging its scope implicitly.
P1-60/P1-70 remain open.
