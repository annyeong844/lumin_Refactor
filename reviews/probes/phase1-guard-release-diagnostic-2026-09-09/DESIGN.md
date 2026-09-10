# Guard and Attempt-Release Diagnostic

Candidate: W9. Status: **review candidate; not frozen; no implementation authority**.
Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
Review disposition: [REVIEW.md](REVIEW.md).
Examined source: `12004f39127208740518d64b13c11e43b8652a34`.

## Decision and limits

Measure the existing eight audit guard-entry/exit intervals and successful
attempt-session release. The [W8 hosted evidence](../phase1-empty-latest-read-2026-09-09/CI-EVIDENCE.md)
shows that W7's eight selected contexts cover only 30-35% of store-owned time.
The proposed nine regions account for another 219-465 ms in the three default
samples, but their internal costs are not measured. The remaining 413-522 ms
outside both sets stays explicitly unattributed; this is not whole-store tracing.

W9 changes observation only. No backend/transaction/file/guard/session lifetime,
field destruction order, admission, receipt, generation proof, sticky rejection,
commit, flush, unlink, recovery, crash hook or error priority may change. No
backend cache, read-only replacement, worker/allocator/dependency change,
public product command, durable schema, scope reduction or budget amendment.
In particular, `Active -> Releasing -> lock removal/parent sync -> lease removal`
retains its two distinct durable transactions. Neither a measured cost nor a
design PASS authorizes removing a check or combining these transitions.

Bootstrap, lease recovery/allocation/activation, earlier session validations,
catalog insertion and evidence-file work retain W3's coarser observations. They
do not acquire a W9 recorder simply because they share a helper. Migration's
terminal-chain path also remains opaque; W9's precise admission subcall coverage
is the native, journal-absent branch, not a claim about every admission mode.
P1-60/P1-70 and permanent run/gate metrics remain open.

## Version, build and ownership

Add non-default, unshippable `audit-boundary-test-profile`, implying W7's
`audit-lifecycle-test-profile` and its W3/W2 closure. Ordinary packages enable
none of these. W9-off erases its clock calls, counters, arguments and dispatch;
existing v1/v2/v3 feature builds retain their own exact shapes and W8's current
v3 count oracle. Older frozen packets keep their original source-bound oracles.

W9 emits `lumin.audit-execution-diagnostic.v4`: the exact ordered v3 fields
with the v4 schema, then one appended `storeBoundaryContexts` array. Its report
is `lumin.phase1-cold-audit-diagnostic.v4`, `DIAGNOSTIC_ONLY`, numeric verdict
null. Decoders accept only their exact version and closed inventories, never
guess from counts or fall back to another version. Existing 23 engine, 52 store
and eight lifecycle rows keep their meanings and boundaries.

Reuse the current store-owned synchronous recorder machinery and explicit
optional mutable lending. The nine new contexts have a separate closed model
inventory; their ten common database-cost leaves share the existing instrumented
database body through an explicit local observer adapter. Do not duplicate the
database/publication implementation or introduce a second timing engine. Model
owns bounded scalars, store measures, engine merges, protocol encodes and CLI
transports. The recorder is never stored on a backend, guard, session, held file
or transaction, and never reaches parallel capture/final-input workers. No
TLS/global state, shared/atomic collector, timer thread or new pool is allowed.
Each common leaf borrow selects exactly one destination: the existing lifecycle
context or a new boundary context, never both. No phase-name inference or global
active-recorder lookup chooses that destination. Opaque whole-call leaves lend
no observer to their nested work.

## Closed context inventory

Each row contains only `context`, `calls`, `elapsedNanoseconds`,
`selfNanoseconds`, `costs`, in that order. These nine rows are emitted in the
following execution order, each once on a successful audit:

| Context | Existing containing W3 interval and boundary |
| --- | --- |
| `open-recovery-enter` | Admission-exclusive `with_lock_core` entry, before its product closure. |
| `open-recovery-exit` | The same wrapper's final validation/unlock interval. |
| `attempt-enter` | Ordinary-exclusive attempt-begin wrapper entry. |
| `attempt-exit` | The same wrapper's final validation/unlock interval. |
| `publish-prepare-enter` | Ordinary-shared publication-prepare wrapper entry. |
| `publish-prepare-exit` | The same wrapper's final validation/unlock interval. |
| `publish-finalize-enter` | Ordinary-exclusive publication-finalize wrapper entry. |
| `finalize-release` | `release_session` inside `finalize_under_guard`, after latest publication. |
| `publish-finalize-exit` | Finalize wrapper's final validation/unlock interval. |

Start/end inside the existing W3 span; never move that span to fit a timer.
Finish an entry recorder before passing the existing StoreProfiler into the
product closure, and start a new exit recorder only afterward. W3 ends its exit
span before `NamespaceGuard`'s natural final destruction: W9 preserves this and
does not claim to measure that later held-object teardown. A guard-construction
call transfers a live guard, not a destroyed one. Caller results and guard drops
stay at their original boundaries.

Each context has one interval and disjoint, sequential cost leaves. Its elapsed
time must fit its namesake W3 interval. The v3 and v4 context arrays cover
disjoint selected regions but both are nested views of W3/engine time. Never
add their durations to their ancestors or sum individual medians into a timeline.

## Closed cost inventory

Every row contains exactly these 19 costs in order, including absent rows.
Each cost has only `cost`, `calls`, `elapsedNanoseconds`. Whole-call leaves are
opaque: do not also profile their nested reads, opens, validations or drops.

| Cost | Exact observed boundary in the nine contexts |
| --- | --- |
| `namespace-validation` | Explicit `validate_bound_entries` calls in profiled database open/commit/held-read completion. Other nested namespace checks stay in their enclosing whole-call leaf. |
| `store-handle-open` | The existing held lifecycle-file open plus volume check in `open_database`. |
| `backend-open` | Its writable `Database::builder().create_file` expression, including clone/builder work. Detached memory backends are excluded. |
| `store-validation` | The existing explicit header, current-object, generation, receipt-refresh and receipt-completion helper calls in those database paths, each as a whole call. |
| `read-admission` | `begin_read` for the session's attempt-lease lookup only; receipt/internal validation reads are not separate application reads. |
| `write-admission` | `begin_write` in `mark_releasing` and `records::remove`. |
| `backend-commit` | The underlying `write.commit()` expression in those two guarded commits, excluding surrounding checks. |
| `backend-abort` | Underlying application `abort` when actually selected; none on the successful measured path. |
| `database-explicit-drop` | Already explicit `drop(database)` in `validate_complete` and `read_session_with_hooks`, unchanged in place. |
| `database-return-tail` | Only the successful natural returns of `mark_releasing` and `records::remove`, paired with their immediate callers as below. |
| `guard-prevalidation` | Entire `open_prevalidated_lock` call, including its temporary handles and proofs. |
| `lifecycle-lock-acquire` | The existing single shared or exclusive `FileExt` acquisition expression; elapsed API time, not isolated contention wait. |
| `guard-construction` | Entire `NamespaceGuard::acquire_without_store(self.clone(), lock)` expression; the returned live guard remains owned by its caller. |
| `lifecycle-lock-release` | Existing explicit `FileExt::unlock` expression, before result combination. |
| `native-store-verification` | Each whole `create_or_verify_store` and `open_current_canonical` call within journal-absent `admit_ordinary`. No nested W9 leaves inside either call. |
| `attempt-lock-validation` | Two whole `records::validate_lock` calls (session validation and release), and release's separate `lock.validate_path` call. Their nested namespace checks stay here. |
| `attempt-lock-drop` | Already explicit `drop(lock)` in successful `finish_releasing`; no separate unlock is inserted. |
| `attempt-lock-remove` | Existing `fs::remove_file` expression in that release. |
| `directory-sync` | Existing state-parent `sync_directory` immediately after lock removal. |

Native admission uses `detached_database`: read the held file bytes, copy them
to `InMemoryBackend`, then `create_with_backend`. Its two whole verification
calls include byte copies, schema/logical validation and memory-backend teardown;
they are not two writable file-backend opens. Returned `CurrentStore` held-entry
lifetime is unchanged. W9 intentionally leaves those calls internally opaque.
Journal enumeration, remaining guard glue and other uninstrumented work stay in
context self time. Directory sync counts its existing API even on Windows where
that implementation is a no-op; do not add a flush or label it device time.

For each return tail, evaluate the existing successful owned result, mark the
start just before that helper's existing return, and end at its immediate caller
before assignment, a subsequent validation or product action. Keep original
table/write-transaction scopes and local drop order. Do not introduce an earlier
drop, resource wrapper, extra read or moved return scope. `read_session_with_hooks`
already explicitly drops its database after original-held completion and before
propagating either result: measure that drop in place, including the error path.
Its guard/session rejection must still reach the existing failure continuation.

Counts/sums and elapsed-minus-cost residual use checked arithmetic. Zero calls
require null elapsed; positive calls may have zero elapsed. Wrong/missing/order-
changed rows, overlap, unclosed tails, clock regression, overflow or negative
residual invalidate observations, with no clipping or dynamic event log. Each
observed writable open balances exactly one explicit drop or return tail; opaque
native verification backends are not included in that balance.

## Independently authored count oracle

The fresh cold fixture completes namespace bootstrap before these wrappers and
has no migration journal. Each of eight guard contexts performs one ordinary
`validate_complete`, hence one observed writable open/explicit drop. Native
admission additionally performs the two opaque verification calls. Release does
one generation-bound session read/explicit drop, then two separate write/commit/
natural-return scopes. These expectations follow those owner transitions, not
an observed frame. Native healthy-seeded and W7's exact fresh-first
`after-latest-temp` recovery fixture have the same **new** vectors: lease recovery
occurs outside the selected contexts. Their inherited v3 vectors remain governed
by W7/W8's respective authored cases.

Vector order: backend-open / read-admission / write-admission / backend-commit /
backend-abort / database-explicit-drop / database-return-tail.

| Context | Exact vector | Other exact leaf counts |
| --- | --- | --- |
| `open-recovery-enter` | `1 / 0 / 0 / 0 / 0 / 1 / 0` | prevalidation=1, acquire=1, construction=1, native verification=2 |
| `open-recovery-exit` | `1 / 0 / 0 / 0 / 0 / 1 / 0` | lock release=1 |
| `attempt-enter` | `1 / 0 / 0 / 0 / 0 / 1 / 0` | prevalidation=1, acquire=1, construction=1 |
| `attempt-exit` | `1 / 0 / 0 / 0 / 0 / 1 / 0` | lock release=1 |
| `publish-prepare-enter` | `1 / 0 / 0 / 0 / 0 / 1 / 0` | prevalidation=1, acquire=1, construction=1 |
| `publish-prepare-exit` | `1 / 0 / 0 / 0 / 0 / 1 / 0` | lock release=1 |
| `publish-finalize-enter` | `1 / 0 / 0 / 0 / 0 / 1 / 0` | prevalidation=1, acquire=1, construction=1 |
| `finalize-release` | `3 / 1 / 2 / 2 / 0 / 1 / 2` | attempt-lock validation=3, drop=1, remove=1, directory sync=1 |
| `publish-finalize-exit` | `1 / 0 / 0 / 0 / 0 / 1 / 0` | lock release=1 |

Totals: `11 / 1 / 2 / 2 / 0 / 9 / 2`; store-handle-open equals backend-open
per row. Namespace/store validation must be entered in every row. Every other
listed non-validation cost not named for a row is exactly zero/null. There are
four acquisitions (three exclusive, one shared) and four explicit releases.
These 11 writable opens plus W8's selected 15 are not a whole-audit count.
The cold runner rejects a different vector even with correct semantic truth;
general v4 decoding enforces structural balance, not fresh-state assumptions.

## Failure, distribution and required verification

Observation failure is sticky scalar state, never a product early return.
Continue the original product body, its error precedence, cleanup and transport.
Original product failure emits no complete diagnostic frame. Successful product
output with an invalid observer must fail diagnostic delivery afterward without
rerunning the audit. Preserve W2/W3/W4/W7 output order, process/lifetime receipts,
raw-capture completeness and exact stdout/build/run/PID/worker bindings.

Extend the existing runner with developer-only
`lumin-xtask benchmark foundation --diagnose-cold-audit-boundary` and its exact
v4 decoder. Reuse the fixed two-conditioning/twelve-measured schedule, ordinary
staged control, full 256-tuple oracle and create-new archives. No second runner,
measurement engine or diagnostic subtraction from ordinary budget times.

After independent review, exact freeze and implementation approval, admit only
these source-provenance vectors to job-private
`RUNNER_TEMP/lumin-audit-boundary-diagnostic-target`, with the existing private
Cargo home:

```text
cargo build -p lumin-cli --release --features audit-boundary-test-profile --locked
cargo test -p lumin-model -p lumin-engine -p lumin-store --lib --features audit-boundary-test-profile audit_ --locked
cargo check -p lumin-cli --bin lumin --features audit-boundary-test-profile,lifecycle-test-fault --locked
```

The last vector must reach Cargo and reject the incompatible union. Exact
feature closure: CLI/engine/model/protocol have W2/W3/W7/W9; store has W3/W7/W9.
Preserve the ordinary xtask-to-model/protocol decoder-only edge and the existing
`audit-execution-profile-probe` feature; neither may enable product profiling.
The new external-child target `audit_boundary_diagnostic` uses that probe and the
ordinary target; pending-fixture setup uses the separate fault-enabled fixture
binary, never the measured binary. Preserve W7's before-Cargo rejection matrix
on ordinary and diagnostic targets, including equivalent reordered/qualified/
short/repeated selectors, all-features and hidden feature-edge injection.

1. Deterministic recorder tests prove all 9/19 ordered inventories, counts,
   absent/zero, containment, overflow, regression, overlap and paired tails.
   Drop canaries prove recorder mechanics only. Independent source comparison
   must separately verify real declaration/return/drop scopes, failed opens,
   guarded commit errors, sticky rejection and the two durable release stages.
2. Real Windows/Linux public children, jobs=1/default, must prove exact fresh,
   native healthy-seeded and precisely prepared pending-recovery vectors and
   complete final durable/public states. Keep v1/v2/v3 child tests and rejection
   crossovers. A skipped platform case or zero collected tests is not evidence.
3. In separate fault-feature partitions run the existing W5/W6/W8 held-read,
   namespace replacement, publication/lease crash and retry tests at their exact
   barriers. Compare complete authentic/foreign logical and physical state;
   observer errors must not acquire an extra guard/backend or repair product
   failures. Diagnostic-success/output-failure probes must retain exactly one
   publicly queryable completed run, with no extra release transaction.
4. Verify feature-off scoped tests/Clippy and actual staged Windows/Linux
   binaries/adapters: ordinary stdout/stderr and package inventory stay clean.
   Perform a new matching external Rust pre/post transaction before future Rust
   work. Keep checks serialized and proportionate to available memory; leave
   user applications running. This design packet runs no builds or benchmarks.
5. Any later hosted authorization replaces only W7 diagnostic build/probe/run
   steps with W9, after the unchanged ordinary benchmark under its existing
   prerequisite/failure conditions. Archive distinct
   `lumin-audit-boundary-diagnostic-{build,report}.json` and
   `lumin-audit-boundary-diagnostic-captures/` as
   `lumin-audit-boundary-diagnostic-windows-x64`, including partial failures.
   Preserve red ordinary/Required outcomes and previous artifacts; no reruns or
   continue-on-error. Commit, push and starting that CI require separate approval.

## Decision gate

This packet authorizes preparing/reviewing a design only. Independent adversarial
review and owner approval of the exact candidate are still required before
freeze or implementation. No runtime PASS or optimization follows from a design
review. If the explicit borrowing, lifetime proof or count oracle cannot be
preserved without changing product behavior, revise the candidate before coding.
