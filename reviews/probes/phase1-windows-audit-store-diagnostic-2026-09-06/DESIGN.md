# Windows Cold-Audit Store Diagnostic

Candidate: W3. Status: **review candidate; not frozen; no implementation authority**.
Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
Review disposition: [REVIEW.md](REVIEW.md).

## Decision and unchanged authority

Extend the isolated audit diagnostic with store-owned, elapsed call-boundary
observations. The [actual W2 Windows packet](../phase1-windows-audit-execution-diagnostic-2026-09-05/CI-EVIDENCE.md)
places about 79% of each default-worker command in store-open, attempt-begin,
and publication excluding final-input validation. W2 cannot distinguish
namespace admission, lease transitions, immutable evidence construction, or
catalog/latest publication inside that time. W3 measures those existing units;
it does not assume which one is expensive or optimize any of them.

The ordinary Windows ratio remains `default / jobs=1 <= 0.75`; it currently
fails. No validation, receipt, lock, generation fence, transaction, flush,
publication, recovery, source scope, allocator, or supported platform changes.
No permanent metric, product DTO/store schema, public command/flag, dependency,
pool, or scheduler is added. Gates and warm benchmarks are outside this packet.
P1-60/P1-70 remain open regardless of diagnostic success.

## Build and version boundary

Keep W2's `audit-execution-test-profile` feature and v1 frame unchanged. Add
the non-default `audit-store-test-profile` extension, which enables W2 plus
feature-gated store observation and its model/protocol routing through existing
crate edges. Without the extension, no store clock reads, accumulators,
observation arguments, or branch selection survive compilation. Neither
diagnostic feature enters a distributed executable. W3 retains the same
compile-time exclusions for crash, fault, and collection-order perturbation.

The extension emits exactly one `lumin.audit-execution-diagnostic.v2` frame
after successful audit stdout, not a second frame or v1 with extra fields.
Its ordered top-level fields are W2's exact fields, with the distinct v2
`schemaVersion` and one final `storePhases` array. The original 23 `phases`
and their parents, intervals, counts, and residual meanings stay unchanged.
The closed v1 decoder must reject v2; the new closed v2 decoder must reject v1.
There is no schema negotiation, environment-controlled product path, or fallback.

The frame, worker/build/PID/run binding, output-failure semantics, and raw
capture requirements otherwise inherit the [frozen W2 design](../phase1-windows-audit-execution-diagnostic-2026-09-05/DESIGN.md).
Only owned scalar observations reach transport after all engine/store/session
scopes and runtime locks have ended. Original product failures retain their
original error and no completed diagnostic frame. Observation failure is sticky
and fails diagnostic delivery after normal lifecycle/resource handling; it
cannot short-circuit a commit, skip release, or re-execute a completed audit.

## Ownership and implementable collection

`lumin-model` owns only feature-gated phase/aggregate values. `lumin-store`
owns its clock, bounded recorder, and the store intervals below; the engine
owns the enclosing W2 measurement and joins returned owned observations.
Backend types remain inside store. Protocol encodes; CLI transports; xtask
checks independently authored expectations. No store-to-engine dependency,
global/TLS collector, timer thread, worker event stream, or shared lock for
collecting timings is introduced.

Use three invocation-local recorders, one for each store entry call. Explicitly
reborrow each mutable recorder down that call's synchronous owner path. It is
not attached to `RepositoryStore`, `NamespaceState`, `NamespaceGuard`, a
database handle, or `AttemptSession`. Collection follows the existing
coordinating audit call, which can run on a Rayon worker inside `pool.install`.
The mutable store recorder must not be passed to, captured by, or shared with
parallel extraction or preflight child tasks; no extra task is scheduled for
measurement. This preserves those types' existing ownership and thread-sharing
properties without relocating store calls. The preflight closure keeps its
existing engine recorder; the store recorder
measures around the whole joined callback without borrowing engine state.
Each recorder accepts only its root subtree. The engine merges exactly one
owned result for each root in root order; duplicate roots, missing results,
foreign-subtree rows, and merge overflow invalidate the combined observation.
Only the named call sites receive these scopes; reuse of a helper during
recovery does not inject a publication child beneath a recovery parent.

Feature-only observed entry adapters may return a store result plus owned
timings, but must call the same private core as the ordinary entry with an
absent observer. There must be one bootstrap, attempt, and publication body,
not copied profiled implementations. Lock wrappers pass an optional recorder
explicitly to the same operation closure. The ordinary caller supplies none;
no timing callback changes an operation's arguments, result, or error mapping.
Do not move natural database/guard destruction to obtain a measurement.

The store recorder uses checked, bounded LIFO intervals with a deterministic
test clock, following W2's recorder semantics. Product code runs even if a
recorder becomes invalid. Zero-duration entered work differs from absent
work. No per-file/per-row timestamps, dynamic phase names, truncation, sampling,
or foreign library telemetry is needed.

## Closed store phase inventory

`storePhases` contains exactly these 52 rows in this order, each with exactly
`phase`, `calls`, `elapsedNanoseconds`, and `selfNanoseconds`. Parent links
describe caller nesting, not a new execution graph. Source anchors refer to
the examined product head `062964192a1d3f16f5b7739f0d98eb85ac5bef4d`.

| Phase | Parent | Existing interval to observe |
| --- | --- | --- |
| `store-open` | none | Shared core of `RepositoryStore::open`, including publication recovery. |
| `namespace-open` | `store-open` | `NamespaceState::open`. |
| `bootstrap-setup` | `namespace-open` | Fresh `bootstrap_namespace` body through lock/global-header setup, before `finish_bootstrap`. |
| `bootstrap-parents` | `namespace-open` | `finish_bootstrap` parent/anchor creation or validation through the all-parents directory sync. |
| `bootstrap-marker` | `namespace-open` | `publish_repository_marker`. |
| `bootstrap-store` | `namespace-open` | `create_or_verify_store` and following state-directory sync. |
| `bootstrap-validation` | `namespace-open` | Following `guard.validate_complete`; unlock and remaining natural drops stay residual. |
| `open-recovery` | `store-open` | `recover_publication` / `liveness::recover`. |
| `open-recovery-enter` | `open-recovery` | Admission-exclusive lock wrapper entry, as defined below. |
| `open-recovery-latest` | `open-recovery` | Its `latest::ensure`. |
| `open-recovery-leases` | `open-recovery` | Its `recovery::recover_under_guard`. |
| `open-recovery-exit` | `open-recovery` | Admission-exclusive wrapper tail, as defined below. |
| `attempt-begin` | none | Shared core of `begin_attempt` / `liveness::begin`. |
| `attempt-enter` | `attempt-begin` | Exclusive wrapper entry. |
| `attempt-recover-latest` | `attempt-begin` | `latest::ensure` before allocation. |
| `attempt-recover-leases` | `attempt-begin` | `recovery::recover_under_guard` before allocation. |
| `attempt-reserve` | `attempt-begin` | `records::reserve`, including database lifetime and guarded transaction. |
| `attempt-lock` | `attempt-begin` | `create_state_file` and its following `try_lock_exclusive`. |
| `attempt-activate` | `attempt-begin` | `records::activate`, including lock self-binding and guarded transaction. |
| `attempt-directory` | `attempt-begin` | `create_attempt_directory`, including its generation checks and parent sync. |
| `attempt-envelope` | `attempt-begin` | Open attempt directory and `files::write_json` for the running envelope. |
| `attempt-latest` | `attempt-begin` | `latest::publish_attempt` for that running attempt. |
| `attempt-exit` | `attempt-begin` | Exclusive wrapper tail. |
| `store-publish` | none | Shared core of `publish_run_with_preflight` / `run::publish`. |
| `publish-prepare` | `store-publish` | `prepare_publication`. |
| `publish-prepare-enter` | `publish-prepare` | Shared wrapper entry. |
| `publish-session` | `publish-prepare` | Initial `session.validate`. |
| `publish-envelope` | `publish-prepare` | Read running attempt and `session.require_running`. |
| `publish-identities` | `publish-prepare` | `guard.reserved_state_identities`. |
| `publish-preflight` | `publish-prepare` | Whole existing engine preflight callback, including final-input validation. |
| `publish-directory` | `publish-prepare` | `publish_directory`. |
| `staging-create` | `publish-directory` | Existing generation-guarded create-directory/parent-sync call. |
| `evidence-write` | `publish-directory` | `write_evidence_store` from `write_staging`. |
| `evidence-create` | `evidence-write` | `Database::create` for immutable evidence, not lifecycle.store. |
| `evidence-begin-write` | `evidence-write` | That database's `begin_write`. |
| `evidence-rows` | `evidence-write` | Existing evidence table scope and complete chunk insertion loop. |
| `evidence-commit` | `evidence-write` | That write transaction's `commit`. |
| `evidence-close` | `evidence-write` | Its already explicit `drop(database)`. |
| `evidence-bind-flush-hash` | `publish-directory` | Following evidence-entry open/binding, sync, read, and catalog-record hash construction. |
| `run-envelope` | `publish-directory` | `files::write_json` for `run.json`. |
| `staging-flush` | `publish-directory` | Following `staging_entry.sync_directory`. |
| `staging-move` | `publish-directory` | Complete generation-guarded staging validation/move/sync/reopen call. |
| `published-validation` | `publish-directory` | Following `revalidate_directory_identity`. |
| `publish-terminal` | `publish-prepare` | `liveness::write_terminal`. |
| `publish-prepare-exit` | `publish-prepare` | Shared wrapper tail. |
| `publish-finalize` | `store-publish` | `finalize_publication`. |
| `publish-finalize-enter` | `publish-finalize` | Exclusive wrapper entry. |
| `finalize-candidate` | `publish-finalize` | Session validation, retention availability, and `revalidate_publication_candidate`. |
| `finalize-catalog` | `publish-finalize` | Database open, `insert_catalog_record`, and its existing explicit database drop. |
| `finalize-latest` | `publish-finalize` | `latest::publish_attempt` for the completed run, including index synchronization. |
| `finalize-release` | `publish-finalize` | `liveness::release_session`, including retained releasing-state recovery. |
| `publish-finalize-exit` | `publish-finalize` | Exclusive wrapper tail. |

Wrapper entry starts before `open_prevalidated_lock` and ends after handle
acquisition and ordinary/admission validation, immediately before the existing
operation closure. Wrapper tail starts after that closure returns and covers
the existing final validation, unlock, and `combine_lock_results`, ending
before natural guard destruction. Destruction and uninstrumented work stay
in the containing phase's residual. Neither interval is called lock-wait
time. `staging-move` is not raw rename latency; `evidence-commit` is elapsed
backend API time, not a hardware-flush measurement. Lifecycle transactions
remain included in their named owner calls; W3 does not claim per-syscall
or whole-store backend-commit attribution.

The three roots are independent sequential call observations, enclosed by
their namesake W2 intervals. Within each tree, self time is elapsed minus
direct-child elapsed, with checked arithmetic and no overlapping siblings.
Repeated sequential invocations accumulate with counts. Timers bracket calls
and existing scopes; gaps remain residual rather than redistributed. Do not
add parent medians or combine `phases` with `storePhases`: these are two views
of the same execution, not independent work. In particular, `publish-preflight`
contains W2 `final-inputs`; subtracting both would double-count the exclusion.

For every successful frame, all store rows except the five `bootstrap-*` rows
must be entered exactly once. An absent bootstrap row has count zero and null
times; existing/resumed namespaces are not silently relabeled fresh. The cold
packet's independently created fresh repository requires all 52 counts to be
one, including every bootstrap row. A fresh sample with any absent row fails.
Validate each store root elapsed <= its W2 namesake elapsed and W2 final-inputs
elapsed <= store publish-preflight elapsed. A bad parent, negative residual,
clock regression, overflow, unclosed interval, duplicate/out-of-order row, or
contradictory count rejects the frame; none becomes a zero or omitted result.

## Runner, hosted admission, and retention

Add the developer-only command:

```text
lumin-xtask benchmark foundation --diagnose-cold-audit-store
```

It reuses W2's observer v2, exact two-conditioning/twelve-measured fixed
schedule, fresh 780-file fixture, 256-tuple oracle, full run-pinned queries,
binary/build/worker binding, and create-new archive implementation. Its report
is `lumin.phase1-cold-audit-diagnostic.v2`, with v2 engine frames; status remains
`DIAGNOSTIC_ONLY` and numeric verdict null. The old command remains strictly
v1. No heuristic frame detection or second fixture/measurement implementation.
Keep normal and diagnostic medians, per-round differences, root residuals,
and every raw miss/failure; do not mix them with the blocking benchmark.

The build record names requested CLI feature `audit-store-test-profile` and
its reviewed implication of W2/store/model/protocol features. Resolve and
verify that feature closure; the recorded exact command is not permission
to enable unrelated features. Normal control features remain empty. Check
both executable payload hashes before/after the run. Stage only the control.

After freeze, the Windows job may replace its W2 diagnostic steps with W3
steps, still after the ordinary seven-mode benchmark and under W2's explicit
after-failure prerequisite conditions. No second blocking budget or rerun
until green. Preserve the failed ordinary job and `Required`; uploads run
even when the diagnostic fails. W2 raw evidence remains separately retained.

Use a new job-private `RUNNER_TEMP/lumin-audit-store-diagnostic-target`.
Hosted source-provenance admission must bind that exact unredirected target
only to these exact command vectors, with the existing job-private Cargo home:

```text
cargo build -p lumin-cli --release --features audit-store-test-profile --locked
cargo test -p lumin-model -p lumin-engine -p lumin-store --lib --features audit-store-test-profile audit_ --locked
cargo check -p lumin-cli --bin lumin --features audit-store-test-profile,lifecycle-test-fault --locked
```

The last command must reach Cargo and fail with the owned incompatible-feature
diagnostic, not fail bootstrap admission. W2's exact target/command mapping
remains separate. The host runner and public-child probe use ordinary
`lumin-target`, not either instrumented target. Reuse the probe-only
`audit-execution-profile-probe` feature for a distinct `audit_store_diagnostic`
test target that expects v2; it launches the actual isolated release child.
Do not accidentally enable store instrumentation in the probe/control build.
Reports and captures use new create-new W3 paths/artifact names, never overwrite
the ordinary or W2 archives. Preserve missing/invalid/not-run cells and available
raw prefixes on every early error exactly as in W2.

## Acceptance before any performance decision

1. Independently authored exact 52-row order/parent/count expectations match
   the recorder, DTO, and actual release-child frame. A deterministic clock
   exercises each owner boundary, repeated calls, zero versus absent bootstrap,
   root containment, preflight nesting, arithmetic errors and failure unwind.
   A synthetic valid-looking frame cannot replace actual child evidence.
2. Actual fresh `jobs=1`/default audits on Windows/Linux produce valid v2
   frames, normal stdout and complete authored truth. Existing-store audits
   separately exercise absent bootstrap rows. Strict tests reject changed row
   order, duplicate keys, v1/v2 crossover, missing store observations, wrong
   PID/build/run/worker data, extra stderr, and truncated success. Retain W2
   base-feature v1 tests; feature-off packages keep empty success stderr.
3. Test product/preflight errors and diagnostic output failure independently.
   No failure skips owner cleanup or produces a completed diagnostic. Following
   output failure, normal run-pinned lookup finds exactly the one completed
   audit; no diagnostic retry runs a second audit automatically. Ordinary
   publication freshness/crash/retry and namespace tests remain unchanged and
   run separately, never compiled into the measured feature binary.
4. Under `GITHUB_ACTIONS=true`, test all three positive command/target mappings,
   wrong/redirected/shared paths, feature or argument injection, both-direction
   W2/W3/control crossovers, and the expected Cargo incompatibility failure.
   Exercise the real hosted wrapper, release-child probe, and archive decoder;
   successful local compilation alone is not integration evidence.
5. Fixed Windows CI conditioning/measured cells all pass full truth and binding
   before a diagnostic summary exists. Forced ordinary numeric failure still
   runs eligible diagnostics/uploads and keeps CI red. Forced diagnostic or
   archive failure retains raw bytes and missing cells, and cannot emit a
   completed summary. Test actual archive contents, not upload-glob presence.
6. Retain the actual four-worker Windows W3 packet before selecting an
   optimization. Large residuals or mixed-sign overhead remain uncertainty.
   Any later durability, validation, reuse, or allocator change requires its
   own grounded owner review and fresh ordinary Windows/Linux budget evidence.

An author design review, independent adversarial review of the exact bytes,
and explicit owner approval are required before W3 implementation. This
candidate supplies no independent PASS, freeze, numeric PASS, or merge authority.
