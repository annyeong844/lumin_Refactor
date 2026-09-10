# W6 Failure-Continuation Authority

Status: **design candidate; implementation awaits scoped review PASS**.

This amendment addresses [W6-03](IMPLEMENTATION-BLOCKER.md), supplementing the
unchanged [W6 design](DESIGN.md), SHA-256
`853fc908deab61e351a05ba099b9b4d5c36829e0134166d4940a4f8620d8fa3a`.
On 2026-09-08 the owner approved extending the design to prevent the same
attempt's subsequent error handling from reopening a rejected store. The
extension is limited to that in-memory authority, its entry-point checks, and
the required verification; the remaining W6 scope and performance decision
gate are unchanged.

## Failure boundary

The prototype correctly poisons its original namespace guard, but the engine
then calls `RepositoryStore::fail_attempt`. That call acquires a new exclusive
guard whose ordinary admission opens the canonical backend writable before
validating the session. Three public Linux barriers show that an otherwise
admissible substituted store retains its physical identity but changes bytes.

Rejection must survive through the same attempt's continuation, before new
guard acquisition, not merely through the old guard's final validation.

## Design

1. Add one private, initially clear, sticky rejected-backend flag to
   `AttemptSession`. Use an atomic flag so the change does not remove existing
   Send/Sync capability. No serialized row, schema, public DTO, repository-wide
   poison state, backend cache, lock or durable reservation is added. The flag
   does not alter the process-liveness handle's lifetime.
2. At the start of `AttemptSession::validate`, reject an already flagged
   session. Preserve the Active check and W6's one generation-bound database
   lifetime. Retain the result of `records::read_session`; before propagating
   any result, inspect the acquired guard's existing rejected-backend state.
   If the generation-bound open or original-held finishing rejected backend
   access, set the session's flag. Preserve the original error and its
   validation-first ordering. Never infer rejection by parsing diagnostics or
   by classifying all `Integrity` errors as the same failure.
3. A read, missing-row, decoder, complete-lease comparison or external-liveness
   validation error whose original-held proof succeeds does not by itself
   set this new flag. Its existing diagnostic and supported failure handling
   remain unchanged. This is not permission to accept malformed durable rows;
   a later attempt to persist failure must still pass all existing checks.
4. `finish_failed` checks the session flag after the existing nonempty-failure
   and repository-ownership checks but **before** `with_exclusive_lock`.
   A rejected session returns a store integrity error without acquiring any
   new namespace guard or opening a database. The engine's existing combined
   publication/persistence error preserves the initial rejection diagnostic;
   no engine-side recovery or error-selection rewrite is required.
5. The common `run::publish` entry also checks this same flag after its
   existing repository-ownership check and before preparation/shared-guard
   acquisition. All public publication wrappers therefore refuse an explicit
   caller retry through the same rejected session before backend admission.
   Existing checks inside `validate` and `release_session` remain in place.
6. The flag never resets, even if the canonical file is restored. Dropping
   the session releases its existing process-liveness handle. Later normal
   admission of the authentic namespace follows the unchanged recovery
   protocol: a Running attempt becomes Interrupted after owner death; an
   already terminal attempt/run remains authoritative. This change does not
   roll back previously committed state, invent a completion, or authorize a
   foreign namespace. It claims no protection against separate future
   commands or wholesale forgery of every canonical ownership record.

The flag is authority learned from W6's rejected generation-bound read, not a
general solution for every unrelated publication or namespace race. No other
guard/failure transition, physical publication ordering, pointer merge,
retention behavior, migration policy, or transaction durability is changed.
Production changes remain in `publication/liveness.rs` and
`publication/run.rs`. W6 records, database and fault-hook owners, the existing
logical-observation serializer in `namespace/migration/snapshot.rs`, and the
CLI namespace-barrier support may be refined solely for the exact verification
below, without changing the production all-exits proof or store format.

## Exact verification

- Keep all five existing W6 owner checks, including the actual one-open /
  one guarded lease-row read / zero application-write counts. After a failed
  generation-bound open, call `fail_attempt` and the common publication entry
  repeatedly through the same session with an otherwise admissible foreign
  store installed. Count zero new backend constructions, reads, writes and
  commits; preserve foreign identity/bytes and authentic logical state. After
  restoration the same session remains rejected. Dropping it and opening the
  authentic store must use ordinary interrupted-attempt recovery, not flag
  reset or an invented successful run.
- Re-run the three currently red Linux public-child assertions at
  `after-attempt-session-open`, `before-attempt-session-return`, and the
  after-open turn combined with a forced lease-read error. Each requires the
  original held-identity error, no stdout, unchanged foreign bytes/identity,
  the exact authentic paused logical/directory/latest snapshot before
  recovery, and stable public recovery/retry without duplicate catalog rows.
  Parameterize each case over the normal audit's three session validations:
  prepare (Running, no new run), finalize (Completed run published, before its
  catalog/latest insertion), and release (Completed run, catalog/latest already
  published, lease still Active). Use an explicit feature-only invocation
  selector and assert the actual durable phase at the stopped turn; the counter
  alone is not proof. After authentic restoration, prepare recovers Interrupted;
  finalize/release retain Completed and recover missing linkage/lease cleanup.
  Never apply the prepare-only unchanged-run-catalog oracle to terminal phases.
- Obtain the paused complete logical observation from the **already held**
  backend through the existing snapshot serializer, as private feature-only
  data on the exact barrier connection before release. Do not acquire a second
  lifecycle lock (finalize/release hold its exclusive side), open another file
  backend, or use a writable observer. A length-framed complete snapshot must
  be consumed without dropping buffered bytes; malformed/missing frames fail
  the test. It is not a public DTO, persisted field, timing observation, or
  measurement artifact, and is emitted only for a selected W6 fault stage.
  Close this observation read transaction before awaiting the barrier release.
  Compare against the same existing complete logical serializer after child
  exit/restoration. The extra test-only snapshot read is not the production
  lease-row-read count. Filesystem snapshots cover attempts/runs/latest and
  immutable anchors separately; a Windows live lock need not be read through
  an unrelated handle to claim the full database observation.
- Add a public forced read-error control with **no substitution**. Finishing
  must succeed, the new flag must remain clear, the failure must be persisted
  as exactly one Failed attempt, and a later healthy audit must publish
  normally. Missing/malformed-row supporting controls restore the row only
  in test setup and prove they did not permanently poison the session.
- Preserve the precise Windows alternatives from the original design: an
  actual substitution must reject and preserve objects; a verified
  sharing/lock/access-denied live-handle rename must leave the foreign object
  unchanged and permit the identical prepared rename after child exit under
  unchanged fixture permissions. The healthy denied branch completes one
  authentic audit; the forced-read-error denied branch persists one Failed
  attempt instead only at prepare. At finalize/release the forced error must
  preserve the already Completed attempt, allowing normal recovery of its
  linkage/lease after child exit rather than converting it to Failed.
  Verify exact catalog, attempt and latest linkage, then
  stable retry snapshots. Do not count arbitrary fixture errors as protection.
- All new public tests must be registered in the existing required-feature
  namespace corpus runner and its complete ordered invocation-set assertion
  before that verification is claimed complete. No passing support test may
  replace the three public counterexamples or their post-restoration checks.
- Use a new external Rust pre/post advisory for this extension. Run focused
  Windows/Linux checks first, then scoped Clippy/fmt and affected publication,
  generation, namespace, retention and package evidence when memory permits.
  Retain the earlier red transcript; do not overwrite it with a green run.
  Only after correctness is established resume the unchanged W6 local
  control/candidate measurement plan. No commit, push, CI rerun, budget
  relaxation or broader optimization is authorized by this amendment.

## Review decision

The author considers this a scoped solution to W6-03 because rejected-read
authority survives both same-session guard entry points without retaining a
backend or changing durable recovery. An independent adversarial review of
these exact bytes is required before code changes. P1-60/P1-70 remain open.
