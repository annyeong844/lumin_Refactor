# Lifecycle Boundary Diagnostic

Candidate: W7. Status: **review candidate; not frozen; no implementation authority**.
Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
Review disposition: [REVIEW.md](REVIEW.md).
Examined source: `964c91c6c89ec84a5b2b9bb42851caf664464dfa`.

## Decision and limits

Measure existing lifecycle-backend and publication boundaries inside eight
specific cold-audit store calls. The [W6 hosted packet](../phase1-attempt-session-read-2026-09-08/CI-EVIDENCE.md)
places about 61-63% of diagnostic engine command time in store-owned regions;
the [source investigation](../phase1-attempt-session-read-2026-09-08/COST-INVESTIGATION.md)
finds repeated writable opens but does not establish their cost. W7 separates
observed construction, validation, transaction, release and publication work
before another optimization is selected.

This does not observe the whole store, hardware flush latency, kernel lock
wait, or every internal redb transaction. Unmeasured work remains explicit.
No validation, receipt, generation fence, file/transaction/guard lifetime,
destruction order, flush, crash barrier, error priority, recovery, allocator,
pool, worker policy, cache or semantic input changes. No backend reuse,
read-only substitution, optimization, public product command/flag/DTO, durable
schema or dependency change is authorized. The ordinary Windows scaling
requirement remains `default / jobs=1 <= 0.75`; its current miss is
`0.8858605752999565`. Permanent metrics, P1-60 and P1-70 remain open.

## Build, output and inherited proof

Add the non-default, unshippable `audit-lifecycle-test-profile` feature. It
implies W3's `audit-store-test-profile`, which already implies W2. Route its
feature-gated owned values through the existing model/store/engine/protocol/CLI
edges. Neither a new dependency nor a timer thread, collector thread, pool or scheduler
is introduced. The normal package has none of these features. With W7 disabled,
its clocks, counters, observation arguments and dispatch disappear at compile
time; W2 v1 and W3 v2 keep their exact shapes and meaning.

W7 emits one `lumin.audit-execution-diagnostic.v3` frame: the exact ordered
W3 v2 fields, with v3 `schemaVersion` and one appended `lifecycleContexts`
array. The original 23 engine and 52 store rows are unchanged. v1/v2 decoders
reject v3, and the v3 decoder rejects v1/v2; no shape detection or fallback.
All W2/W3 binding, successful-stdout-before-frame, complete raw-capture and
failure rules remain required under their [W2](../phase1-windows-audit-execution-diagnostic-2026-09-05/DESIGN.md)
and [W3](../phase1-windows-audit-store-diagnostic-2026-09-06/DESIGN.md) owners,
with [W4's Windows observer](../phase1-windows-process-observer-2026-09-07/DESIGN.md)
and lifetime receipts. This proposal changes no frozen predecessor bytes.

The diagnostic report is `lumin.phase1-cold-audit-diagnostic.v3`, with
`DIAGNOSTIC_ONLY` status and null numeric verdict. It retains each v3 frame
and its exact raw-byte hash. Ordinary seven-mode reports remain budget authority;
diagnostic times and overhead are never subtracted from them.

## One synchronous owner, unchanged resource lifetimes

The store owns invocation-local timing, model owns only bounded scalar values,
engine joins returned values, protocol encodes and CLI transports. Each of the
eight selected calls borrows one local W7 recorder down its existing synchronous
coordinating path. W3 receives and merges its owned result by context identity
after the call returns. A selected call may run on the existing coordinating
Rayon worker; its recorder never reaches parallel extraction/preflight tasks.

Do not put the recorder or a timing callback in `RepositoryStore`,
`NamespaceState`, `NamespaceGuard`, `StoreDatabase`, any transaction,
`AttemptSession`, or a held-file object. No global/TLS state, shared collector,
atomic timing ledger, reference-counted collector, unsafe lifetime extension or
changed Send/Sync property is needed. Core helpers receive explicit optional
mutable reborrows, erased when W7 is off. Ordinary and observed callers execute
one body, not copied publication/database implementations. For the two callbacks
in `validate_document_at`, lend the recorder as an argument to each sequential
callback instead of capturing the same mutable reference in both closures.
Apply the same explicit lending to index-commit and generation-mutation closures.

Only the named contexts below enable instrumentation. Other callers, queries,
recovery paths outside these calls, bootstrap, wrapper admission/exit, lease
allocation/release and the W6 session-read path receive no W7 observer. Reuse of
a helper does not infer a context from the thread, path or phase-name substring.

Each recorder has one context interval and at most one active cost interval.
Cost intervals are disjoint, sequential leaves, never nested. A listed
validation helper is measured as one whole call: backend reads internal to that
call are not also entered as read costs. The same rule applies to nested
validations inside the listed publication APIs. Explicit reborrowing cannot
silently instrument a subcall twice. Gaps, row/JSON work, uninstrumented helpers
and bookkeeping stay in the context residual.

### Natural database release

Already explicit `drop(database)` in `validate_generation` is timed in place.
Do not insert an earlier explicit drop, wrap the backend in `Option`, implement
a new Drop wrapper, or move a transaction/table scope to make teardown measurable.

For an existing helper/closure that returns an owned value while its local
database is still live, measure a **database-return-tail** instead. Place a
start marker immediately before its existing successful return, after the
returned expression has produced its owned result; stop at the immediate caller
after that same helper/closure returns. The original local objects drop through
the normal return, at their original boundary and in their original order.
This interval includes return overhead and all locals dropped there, potentially
table/read-transaction and held-entry release, not only redb destruction. It is
not a hardware flush or a pure backend-drop measurement.

Instrument all successful exits of the selected backend-owning scopes:
`derive_legacy_document` (including missing POINTERS), the catalog-read closure
of `validate_document`, `records::read` used by its active-lease check, and
`sync_index_with_commit` (both unchanged-index abort and changed-index commit).
The caller closes the interval before any subsequent validation, row comparison,
lock check or product action. In particular, the active-lease lock proof remains
after `records::read` releases its database. No marker is placed on
`open_database`'s return, which transfers a live database rather than releases it.
Internal core/adapters may separate timing from the same original body, but may
not change a backend's original ownership/return boundary. The source-lifetime
comparison, supporting recorder tests and actual selected-call evidence below
are all required. None is a direct trace of redb's internal destruction.

## Closed context and cost inventories

`lifecycleContexts` contains exactly these eight rows in order. Each row has
exactly `context`, `calls`, `elapsedNanoseconds`, `selfNanoseconds`, and `costs`,
in that order. Its interval surrounds the named existing call inside the
namesake W3 store span; no original span is moved.

| Context | Existing selected call |
| --- | --- |
| `open-recovery-latest` | Admission recovery's `latest::ensure`. |
| `attempt-recover-latest` | Attempt begin's `latest::ensure`. |
| `attempt-directory` | `create_attempt_directory`. |
| `attempt-latest` | Running-envelope `latest::publish_attempt`. |
| `staging-create` | Staging creation's complete `mutate_for_generation` call. |
| `staging-move` | Staging movement's complete `mutate_for_generation` call. |
| `publish-terminal` | Normal publication's `liveness::write_terminal`. |
| `finalize-latest` | Completed-envelope `latest::publish_attempt`. |

Every successful audit enters all eight once, including existing-store audits.
Each context elapsed time must be <= its corresponding W3 span. Its `costs`
contains exactly these thirteen rows in order, each with exactly `cost`, `calls`,
and `elapsedNanoseconds`, in that order. These are elapsed API/owner boundaries,
not mutually exhaustive categories of CPU or device work.

| Cost | Exact boundary in the selected context |
| --- | --- |
| `namespace-validation` | Explicit `guard.validate_bound_entries()` calls in database open/commit/abort, generation validation, the latest before-replace callback, and staging-move closure. No recursive profiling inside this helper. |
| `store-handle-open` | `open_database`'s held lifecycle entry open plus the immediately following volume check, before the backend construction expression. |
| `backend-open` | `Database::builder().create_file(entry.file().try_clone()...)` expression; includes builder/clone overhead. |
| `store-validation` | Explicit `verify_store_header`, `validate_current`, `require_generation`, receipt refresh and `validate_receipt_set_current` calls in the selected database/generation paths. Nested internal reads remain within this call. |
| `read-admission` | Existing `StoreDatabase::begin_read` calls for derived POINTERS, catalog-record and attempt-lease reads; includes its receipt validation. |
| `write-admission` | Existing `StoreDatabase::begin_write` for index sync; includes its header validation. |
| `backend-commit` | The underlying `write.commit()` expression inside guarded index commit, excluding its surrounding checks. |
| `backend-abort` | The underlying `write.abort()` expression inside guarded unchanged-index abort, excluding its surrounding checks. |
| `database-explicit-drop` | The already explicit `drop(database)` in each generation validation. |
| `database-return-tail` | Only the four backend-owning scope kinds and paired return boundaries specified above. |
| `json-write-flush` | `pending_entry.replace_contents(&bytes)` in the existing JSON publication helper; whole write/truncate/sync helper, not isolated flush. |
| `publication-move` | The existing `replace_entry_atomic` or `move_entry_noreplace` call for a selected JSON publication or staging movement, including its own internal checks. |
| `directory-sync` | Direct `parent.sync_directory()` calls in selected attempt/staging creation, staging movement and JSON publication, including pending-file removal when actually reached. |

`directory-sync` counts the owner API even on Windows, where the current
implementation returns without a directory flush. Do not insert a flush or
rename this measurement as device durability time; platform differences are
part of the observed API cost, not missing samples.

For a successful context, no open cost interval may remain. Counts and sums use
checked arithmetic; a never-entered cost has count zero and null time, while an
entered zero-duration cost has positive count and zero time. Context self time
is elapsed minus the checked sum of these disjoint cost durations. Negative
residual, overflow, overlapping leaves, wrong context, duplicate/missing/out-of-
order rows, unpaired return tails or a changed clock invalidate the observation.
There is no clipping, zero-fill, dynamic name, event stream or source-sized log.

The eight results merge once each in the listed order into W3's owned return
values. Missing/duplicate/foreign contexts fail the combined observation.
Do not add W7 costs to W3 or engine timings: they are nested views of the same
execution. Nor may individual medians be summed into a sample timeline.

### Independently authored fresh-fixture counts

The fixed cold corpus starts with no prior attempt, latest document or pending
publication. Its expected counts follow the owner state transitions, not a
recorded implementation run. Entries below are ordered vectors
`backend-open / read-admission / write-admission / backend-commit /
backend-abort / database-explicit-drop / database-return-tail /
json-write-flush / publication-move / directory-sync`.

| Context | Exact fresh vector |
| --- | --- |
| `open-recovery-latest` | `2 / 1 / 1 / 0 / 1 / 0 / 2 / 0 / 0 / 0` |
| `attempt-recover-latest` | `2 / 1 / 1 / 0 / 1 / 0 / 2 / 0 / 0 / 0` |
| `attempt-directory` | `2 / 0 / 0 / 0 / 0 / 2 / 0 / 0 / 0 / 1` |
| `attempt-latest` | `2 / 1 / 1 / 1 / 0 / 0 / 2 / 1 / 1 / 1` |
| `staging-create` | `2 / 0 / 0 / 0 / 0 / 2 / 0 / 0 / 0 / 1` |
| `staging-move` | `2 / 0 / 0 / 0 / 0 / 2 / 0 / 0 / 1 / 1` |
| `publish-terminal` | `2 / 0 / 0 / 0 / 0 / 2 / 0 / 1 / 1 / 1` |
| `finalize-latest` | `3 / 2 / 1 / 1 / 0 / 0 / 3 / 1 / 1 / 1` |

For every successful context, `store-handle-open == backend-open ==
database-explicit-drop + database-return-tail`; namespace/store validation
costs must be entered. An ordinary complete frame with a wrong fresh vector
fails the cold runner, even if its times and semantic truth otherwise agree.

Two separate public-child fixtures exercise the selected helpers' other
successful return shapes. Use the same vector order; unlisted contexts retain
the fresh vector:

| Fixture | Changed context | Exact vector |
| --- | --- | --- |
| One healthy completed control audit precedes the measured audit. | `attempt-latest` | `4 / 3 / 1 / 1 / 0 / 0 / 4 / 1 / 1 / 1` |
| Same healthy seeded fixture. | `finalize-latest` | `4 / 3 / 1 / 1 / 0 / 0 / 4 / 1 / 1 / 1` |
| A separate fresh fixture's first audit dies at `after-latest-temp`; no earlier completed run exists. | `open-recovery-latest` | `3 / 2 / 1 / 0 / 1 / 0 / 3 / 0 / 0 / 1` |
| Same fresh-first pending fixture, after admission recovers its Completed run. | `attempt-latest` | `4 / 3 / 1 / 1 / 0 / 0 / 4 / 1 / 1 / 1` |
| Same fresh-first pending fixture, after admission recovers its Completed run. | `finalize-latest` | `4 / 3 / 1 / 1 / 0 / 0 / 4 / 1 / 1 / 1` |

Prepare the pending fixture with the existing ordinary fixture binary and
`LUMIN_TEST_PUBLICATION_CRASH_POINT=after-latest-temp`, never the measured
binary. Its completed run, Running canonical latest document and complete
pending successor must match that owner-defined crash boundary. Retain the
fixture invocation, nonzero exit, output, build identity and pre-recovery state;
a setup failure at another boundary is not this fixture. Then remove the fault
environment and invoke the v3 public audit once. Do not seed this case with a
previously completed audit or infer expected counts from the emitted frame.
After the observed `open-recovery-latest`, the existing unobserved
`recover_under_guard` restores that Completed run's latest pointer before
attempt begin. Its work remains outside W7; its resulting completed frontier
explains why the two later contexts have the healthy-seeded vectors.

## Failure and compatibility

Observation errors are sticky scalar state, not product errors or early-return
permission. The same product body still executes all normal validation, commit,
abort, recovery and resource release. For generation mutations, final validation
still runs even when the mutation fails and retains its existing error priority.
Original product failures emit no completed diagnostic frame. On a successful
audit with invalid observation, complete normal product transport/resource
handling, then fail diagnostic delivery; never retry the audit automatically.
Successful frame delivery requires every local recorder to be closed.

Preserve W5/W6 backend-rejection and session failure-continuation semantics,
including their exact hooks. No new guard acquisition, database opening,
transaction, header read, namespace validation or flush may exist solely to
collect timings. Diagnostic helpers must never catch a product error and
substitute an owned empty result. A diagnostic stderr failure remains nonzero;
run-pinned ordinary lookup must find the one committed run, not a second audit.

## Runner and hosted isolation

Add developer-only `lumin-xtask benchmark foundation --diagnose-cold-audit-lifecycle`.
Reuse the W2/W3 fixed two-conditioning/twelve-measured schedule, fresh corpus,
unfiltered 256-tuple oracle, run-pinned queries, PID/build/worker and Windows
lifetime-receipt binding, create-new archives, and all failure-retention rules.
`LUMIN_PACKAGE_ROOT` remains the normal staged control;
`LUMIN_AUDIT_DIAGNOSTIC_BINARY` and its build record select the isolated v3 build.
Do not create a second measurement, truth or archive engine. Old commands stay
strictly v1/v2. Feature overhead and uncertainty remain visible and descriptive.

Use the exact new job-private `RUNNER_TEMP/lumin-audit-lifecycle-diagnostic-target`
only for these source-provenance-admitted vectors, with existing job-private
Cargo home:

```text
cargo build -p lumin-cli --release --features audit-lifecycle-test-profile --locked
cargo test -p lumin-model -p lumin-engine -p lumin-store --lib --features audit-lifecycle-test-profile audit_ --locked
cargo check -p lumin-cli --bin lumin --features audit-lifecycle-test-profile,lifecycle-test-fault --locked
```

The negative check must reach Cargo and reject the incompatible feature union.
Record the exact feature closure: CLI, engine, model and protocol have W2, W3
and W7 features; store has W3 and W7. Bind normal and diagnostic executable
hashes before/after, source/toolchain/lock/allocator identity, exact commands
and build record. No substring-based command/target admission.

Hosted admission also enforces the inverse restriction **before metadata or
Cargo launch**: an unlisted instrumentation-enabling vector is rejected on
every target, including ordinary `lumin-target`. Exact W2/W3/W7 vectors are the
only commands allowed to explicitly select their reserved diagnostic features.
Detection of a forbidden selector is not normalization into an allowed vector.
Inspect Cargo arguments before the first `--`, including repeated `--features`,
`--features=...`, split/attached `-F`, comma/space feature lists and qualified
`package/feature` selectors. Reordering an otherwise equivalent command, adding
a second diagnostic selector, or using a qualified form never authorizes an
unlisted vector. Reject user-supplied `--all-features` on artifact-producing
commands as another instrumentation-enabling selector. The bootstrap's own
reviewed metadata query is not such an artifact-producing command.

Keep the complete reviewed manifest feature maps closed against new default,
alias or dependency edges that enable CLI/engine/store instrumentation through
an otherwise ordinary command; do not introduce a second Cargo resolver. The
existing xtask-to-protocol/model diagnostic **decoder-only** edge remains valid
in the ordinary target, extended to v3 through that same reviewed edge. It must
not enable CLI/engine/store instrumentation. Likewise the exactly named
`audit-execution-profile-probe` is not a diagnostic feature selector. Preserve
ordinary runner/control commands and their explicit feature-closure checks;
rejecting every transitive model/protocol decoder feature would break that
legitimate separation and is not this admission rule.

The release-child probe uses the ordinary `lumin-target` and existing
`audit-execution-profile-probe`, with a distinct `audit_lifecycle_diagnostic`
test target expecting v3. It must not enable production instrumentation in
the control/runner binary. Its pending-latest preparation uses the already
separately built `LUMIN_PACKAGE_FIXTURE_BINARY` with `lifecycle-test-fault`
(which includes `publication-test-crash`), bound to the same source identity.
Do not union these fault features into the measured release binary or count
the setup execution as a measured cell. After exact design freeze, the Windows job may
replace its W3 diagnostic build/probe/run steps with W7, after the unchanged
ordinary benchmark and under the same explicit prerequisite/after-failure
conditions. Preserve failed ordinary and Required outcomes, without reruns or
`continue-on-error`. Upload new create-new
`lumin-audit-lifecycle-diagnostic-{build,report}.json` and
`lumin-audit-lifecycle-diagnostic-captures/` under the distinct
`lumin-audit-lifecycle-diagnostic-windows-x64` artifact even on diagnostic failure.
Preserve predecessor packets; no existing ordinary/W2/W3 archive is overwritten.

## Acceptance before using W7 observations

1. Author the exact eight-context/thirteen-cost order and fresh count vectors
   independently in model, protocol and runner tests. Deterministic clocks
   cover repeated leaves, zero/absent, wrong/duplicate context, overlap,
   overflow, backward clock, unclosed tails, root containment and checked
   residuals. An invalid observer never short-circuits the product closure.
2. Keep three kinds of lifetime evidence distinct. First, independently compare
   the before/after source and ownership scopes for every backend-owning return
   shape: declarations, struct field order, table/transaction scopes, explicit
   drops, early returns and error propagation stay unchanged. Include successful
   constructed databases and failed pre-construction opens; their existing
   destruction orders differ and must not be flattened into one claimed order.
   Second, deterministic drop-canary tests of the actual recorder's start/return/
   end primitive prove its return interval and closure before the caller's next
   action. These are supporting mechanics, not observations of concrete redb
   or HeldEntry destruction; no backend wrapper or test-only Drop is introduced.
   Third, actual selected helpers and public children prove the authored counts,
   cleanup, errors and durable outcomes. Cover missing POINTERS, empty/nonempty
   lease, catalog read, abort and commit exits. Failed opens/reads/commits/aborts
   retain normal teardown and cannot emit a complete success frame. A canary
   test alone cannot close the source review or real-path proof; no sleep-based
   proof or direct-redb-drop claim is permitted.
3. Actual fresh public children on Windows/Linux, default and jobs=1, must
   produce v3 with exact fresh vectors, full authored truth and ordinary stdout.
   The precisely prepared healthy seeded and fresh-first pending-latest cases
   above separately require their exact non-fresh vectors and final durable
   recovery state; no fixture or fault-capability fallback is permitted.
   Preserve W2-only and W3-only public-child frame tests, and reject all
   version, feature, build, PID, worker, row-order and missing-observation
   crossovers. Fake frames only supplement actual child evidence.
4. Run the ordinary public publication/freshness/crash/retry, W5 latest-index
   no-op, W6 attempt-session and namespace replacement proofs separately from
   the incompatible measured features. Preserve exact barriers, foreign bytes,
   complete durable snapshots and same-operation outcomes. Also force product
   failure, invalid observation and output failure in their own partitions;
   no diagnostic error may alter the successful audit's committed result.
5. Exercise real hosted command/target admission for all three positive vectors,
   the expected Cargo rejection, redirected/shared/wrong targets, feature
   injection and every W2/W3/W7/control crossover. Repeat the unlisted reordered,
   repeated, equals/short/qualified feature and all-features attacks against
   **both diagnostic and ordinary targets**; assert rejection before metadata
   or Cargo execution, not merely failure in a later build. Prove the ordinary
   decoder/probe commands remain admitted without product instrumentation.
   An ordinary numeric miss
   must retain red CI while eligible diagnostics run. Inspect raw captures on
   malformed output, truth failure, observer/archive failure and early exit;
   a partial packet is never completed evidence.
6. Keep ordinary Windows/Linux package/adapter smoke clean with no diagnostic
   features or extra stderr. Retain all fourteen actual Windows cells under
   a fixed schedule before drawing a four-worker conclusion. High overhead,
   residuals or inconsistent samples leave attribution unresolved. No W7
   success closes a budget, permanent metric or merge requirement.

After author review and independent adversarial review of the exact candidate,
explicit owner approval is required to freeze it before Rust implementation.
Use a new matching external Rust pre/post advisory, focused tests before broad
checks, and the existing locked fmt/Clippy/package workflow. This design-only
packet performs none of those implementation or runtime acceptance checks.
