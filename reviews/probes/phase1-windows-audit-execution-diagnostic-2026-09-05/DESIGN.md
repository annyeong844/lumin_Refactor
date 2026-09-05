# Windows Cold-Audit Execution Diagnostic

Candidate: W2. Owner and review disposition: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).

## Purpose and authority

Locate the serial or poorly scaling portion of the Windows cold-audit path on
the actual four-worker CI host. This is a development diagnostic, not the
missing permanent ARCH-001 runtime-metrics implementation and not AC 16 PASS
evidence. The Windows scaling limit remains `default / jobs=1 <= 0.75`.

Only cold `audit --include ** --format json`, with default jobs or `--jobs 1`,
is measured here. Warm audits, gates, cache policy, allocator selection,
namespace durability, supported platforms, and numeric budgets are unchanged.
No public DTO, evidence/store schema, analysis identity, skill workflow, or
production command is extended. Permanent run/gate metrics and the WSL `/mnt`
decision remain separate open REVIEW-005 obligations.

## Two binaries, one product path

Build the normal release control and an optimized diagnostic executable from
the same exact checkout, lockfile, toolchain, target, allocator, and release
settings, in separate Cargo target directories. The sole difference is the
non-default `audit-execution-test-profile` feature. It must not be combined
with crash, fault, or completion-perturbation features. This feature observes
the existing audit; it cannot choose another analyzer, pool, scheduler,
store protocol, cache policy, or error path.

The normal staged package remains the distribution under test. The diagnostic
binary is kept outside that package, identified as unshippable, and never
replaces `LUMIN_PACKAGE_BINARY` or a required benchmark input. Both payload
hashes are checked before and after use; build IDs alone cannot distinguish
feature builds. Record the exact feature sets and build commands as well.

No runtime opt-in environment variable, profiling file inside `.lumin`, or
new public flag is needed. A successful audit in the diagnostic build always
produces the diagnostic frame below. Other commands retain their normal
transport. The ordinary package probe and `benchmark foundation` continue to
require empty success stderr and therefore reject the diagnostic executable.

## Ownership and timing boundaries

`lumin-engine` owns the collection lifetime and one existing local Rayon pool.
Project-owned, feature-gated observation values in `lumin-model` cross existing
engine/CLI/protocol dependency edges. `lumin-protocol` owns frame encoding and
the CLI owns transport. There is no new crate, dependency, third-party public
type, shared mutable evidence, timer thread, global collector, or worker log.

Record elapsed monotonic time around the existing calls, without moving them
across a validation, transaction, flush, or release boundary. Timings of
parallel calls are the caller's elapsed time for the whole batch, not the sum
of worker CPU times. The engine observes store calls as opaque calls; their
elapsed time is not claimed as backend commit time or lock-wait time.

The following ordered phase set is closed. Parent names define inclusive
nesting, not additional execution stages or a proposed scheduler graph.

| Phase | Parent | Exact measured interval |
| --- | --- | --- |
| `command` | none | CLI audit handling through normal stdout write and flush, excluding diagnostic encoding/transport. |
| `pool-create` | `command` | Existing `ThreadPoolBuilder::build` call. |
| `audit-work` | `command` | Entry to and return from `audit_in_current_pool`. |
| `pool-release` | `command` | Drop of the existing local pool after `install` returns; not a claim that OS thread teardown has completed. |
| `admission` | `audit-work` | Lexical/caller validation and `repository_admission`, before opening the store. |
| `store-open` | `audit-work` | `RepositoryStore::open`, including bootstrap/recovery and its validations. |
| `entry-identities` | `audit-work` | Context construction and caller-entry physical/reserved-state checks before attempt allocation. |
| `attempt-begin` | `audit-work` | `begin_attempt`, including running-envelope/latest publication. |
| `capture` | `audit-work` | Complete `capture_admitted_repository_in_current_pool` call. |
| `inventory` | `capture` | Initial inventory begin/finish in `RepositoryAnalysisSession::start`. |
| `profiles` | `capture` | Every `select_resolution_profiles` call in `next_step`. |
| `extraction` | `capture` | Complete `extract_facts` call, including owned reduction and cache work. |
| `resolution` | `capture` | Every `resolve_all` call. |
| `demand-capture` | `capture` | Every `session.capture_demands` call between owner steps. |
| `finish` | `capture` | Complete `session.finish` call, including projection and snapshot sealing. |
| `graph` | `finish` | `lumin_graph::build`. |
| `dead-code` | `finish` | `lumin_dead::analyze`. |
| `publication` | `audit-work` | Complete `audit_publication::publish` call. |
| `evidence-prepare` | `publication` | `prepare_run_evidence`, before entering store publication. |
| `store-publish` | `publication` | `publish_run_with_preflight`, including preflight, physical publication, catalog/latest commits, and liveness release. |
| `final-inputs` | `store-publish` | Complete `validate_snapshot` preflight callback, including the joined parallel freshness checks. |
| `response` | `command` | Public audit DTO projection and serialization after the engine returns. |
| `stdout` | `command` | Existing product-result write and flush. |

Each row contains exactly `phase`, `calls`, `elapsedNanoseconds`, and
`selfNanoseconds`. Repeated calls accumulate into the same row, using checked
integer arithmetic; storage is fixed by the phase set, not source count or
demand iterations. No per-file timestamp collection or truncation is allowed.
An entered interval may measure zero. A phase never entered has `calls: 0`
and both times `null`, not zero. A successful sample must contain every row
once in the listed order; all phases except `demand-capture` must be entered.

For entered parents, self time is inclusive elapsed time minus the sum of
direct child elapsed times. These children are sequential at the caller;
nested and parallel worker times cannot be added twice. Checked underflow,
overflow, an unclosed interval, or an invalid parent relation invalidates the
diagnostic. Retain parent residual/self time explicitly; do not redistribute
it to manufacture full attribution. In particular, `store-publish` minus
`final-inputs` is residual store-publication time, not a measured flush span.

The process observer's launch-to-exit time also covers startup, diagnostic
encoding/transport, and exit outside `command`. Retain that difference as an
external residual, with no cross-process timestamp subtraction or claim that
it is all startup. No Kahn ready-wait, per-worker utilization, stack usage,
or separate execution/reduction measurement is fabricated from these spans.

## Worker and invocation observations

Capture `requestedJobs` from the parsed CLI request (`null` means default),
and preserve the actual `available_parallelism` observation used by the CLI
default selection before the engine runs. If observation fails, retain null
plus the error; the existing product behavior is unchanged, but the scaling
diagnostic is ineligible. Do not substitute the harness process's observation.

After successful pool construction, record `actualJobs` from that concrete
pool's `current_num_threads()`, and `configuredWorkerStackBytes` from the
exact value passed into its builder. Check the first against the selected
policy/request and the latter against `4,194,304`. A mismatch is a diagnostic
failure, not permission to change or cap the pool. Actual jobs is pool size,
not a utilization claim; configured stack bytes is applied policy, not an OS
reservation, committed-memory, or high-water measurement.

No broadcast probe, artificial worker barrier, or extra scheduled task is
inserted into the measured execution. This packet does not close the broader
permanent worker/stack and owner-stage observability requirement.

## Transport and binding

Only after the public audit result is successfully written/flushed, every
engine/store/session scope has returned, and no storage/publication/liveness
lock remains held, the diagnostic CLI writes one compact JSON value followed
by one newline to stderr. Its exact top-level fields, in order, are:

`schemaVersion: "lumin.audit-execution-diagnostic.v1"`, `diagnosticOnly: true`,
`buildId`, `processId`, `attemptId`, `runId`, `requestedJobs`,
`observedAvailableParallelism`, `parallelismObservationError`, `actualJobs`,
`configuredWorkerStackBytes`, and `phases`.

Attempt/run IDs come from the same completed `AuditResult` used for stdout,
never a latest pointer query. Build ID uses the compiled identity also exposed
by the binary-scoped capabilities query, not a nonexistent `AuditResult`
field. Parallelism error is null on success. The collector survives
pool release as owned values, not open store handles or a live session. It is
never persisted in the run/gate evidence, hashed into semantic identities, or
replayed from a prior invocation. Original audit failures keep their ordinary
diagnostic/exit and produce no completed frame. A frame write/flush failure
exits nonzero without undoing or re-executing the already committed audit.

The diagnostic runner requires one complete frame, strict closed/duplicate-key
decoding, exact phase order and presence, and the same process ID as its OS
observer. It cross-checks attempt/run IDs against raw audit stdout and
`overview --run <that-run-id>`. Separately, it compares build ID with
`capabilities --format json`'s `/scope/buildId` from the same before/after-hashed
executable, retaining that query's raw output. Audit and run-scoped DTOs have
no build ID; they must not be treated as its authority. An absent, partial,
duplicate, stale, wrong-build, wrong-process,
or contradictory frame invalidates the sample. Nonzero product exit, extra
stderr, an analysis child process, or truth mismatch always invalidates it.
No successful process may silently contribute an empty capture list.

The runner binds each frame's raw-byte hash to the executable's before/after
hash, exact source revision/feature set, staged control manifest, fixture
manifest/truth hashes, command arguments, fresh repository/root identity, host
and filesystem observation, process measurement, stdout, and full semantic
truth result. The diagnostic must not rely solely on self-reported identities.

The existing process helper v1 does not contain a product PID. Add a distinct
`measure-audit-diagnostic` helper subcommand that reuses the existing observer
and measurement path, but emits `lumin.phase1-process-measurement.v2`: the
exact v1 fields plus mandatory `processId` captured directly from the launched
`Popen.pid`. Keep the ordinary `measure` v1 result unchanged. The diagnostic
runner accepts only v2 for its feature-built cells and tests a stale/different
PID as a hard failure. It cannot copy the frame PID, use its own PID, or guess
an identity from process names. Control-cell worker/stack observations remain
unavailable; the new report must not relabel v1 harness-derived values as
engine observations.

## Runner and Windows CI placement

Proposed developer command (not implemented by this design):

```text
lumin-xtask benchmark foundation --diagnose-cold-audit
```

`LUMIN_PACKAGE_ROOT` selects the unchanged staged control. A separate
`LUMIN_AUDIT_DIAGNOSTIC_BINARY` selects the feature build. Existing pinned
Python/process-observation and fixture/truth owners are reused, with scrubbed
child environments and no runtime Cargo/Node. Use the exact frozen fixture,
unfiltered 256-finding oracle and tuple-to-ID mapping, not only counts. Fresh
repositories are used for every cell; never reuse a control's `.lumin`.

Run one disclosed unmeasured `jobs=1` conditioning audit for each binary, then
three fixed rounds of four cells. Round 1 and 3 order is control-1,
control-default, diagnostic-1, diagnostic-default; round 2 reverses that order.
This order is authored before results. Record every raw sample and semantic
dump, including conditioning, failures, and misses. No trimming, median-picking,
cache flushing, affinity tricks, or rerunning until green. Default is genuinely
omitted from the CLI, not replaced by `--jobs 4`.

Compute separate descriptive control and diagnostic medians/ratios and
feature-overhead deltas by worker policy; never pool the two binaries' samples,
subtract probe overhead from a budget result, or report a numeric PASS. A host
with fewer than four observed workers has no scaling diagnostic authority.
Keep per-round differences visible: a large residual or unstable overhead
means the proposed bottleneck is unresolved, not proven by local timings.

On the existing Windows package job, build/run this packet after the ordinary
seven-mode benchmark. Explicit step conditions must run it even when that
benchmark misses a number, provided the normal build/stage/behavior steps
succeeded. Keep the benchmark's failure in the job and `Required`; do not add
`continue-on-error`. Upload raw normal and diagnostic captures with `always()`
even when profiling fails. The profiling build uses a separate target/output
path so it cannot change the earlier or a later packaged control. Do not
derive a four-worker CI explanation from an eight-worker local/WSL run.

### Capture retention, including early failure

The current normal benchmark deletes its scratch directory before writing a
numeric PASS or FAIL report; a new upload glob cannot recover those captures.
The Windows packet must set a separate `LUMIN_BENCHMARK_CAPTURE_ROOT` for the
normal matrix and another for the diagnostic command, before either starts.
Each is a create-new archive outside the checkout, staged package, and
repository scratch trees. Reusing/overwriting an archive is an error.

When configured, both runners write measured/conditioning process stdout,
stderr, helper measurements, and truth-query raw responses directly into that
archive before decoding, validation, or an error return. Reuse the existing
process/query/truth owners with explicit capture destinations, not a second
measurement implementation. Normal scratch cleanup must not remove an archive.
Keep normal measurement v1 and diagnostic measurement v2 distinguishable.

The archive records an expected cell inventory, per-cell completed/invalid/
not-run state, failure reason, and hashes of every retained capture. An
interrupted helper may leave no complete measurement; preserve its raw prefix
as incomplete, never fill missing metrics. Decoder, truth, helper, archive-I/O,
and publication failures exit nonzero. Upload the available archive prefix
even if no report/manifest could be completed, and flag that prefix as
incomplete. No valid-diagnostic summary is emitted unless both conditioning
audits and all twelve measured cells pass their required checks. Numeric
misses remain visible in the separate ordinary report.

CI uploads the two archives as well as both reports. Tests force a normal
numeric miss, malformed successful stdout, a truth-query failure, and a
diagnostic framing error, then inspect retained bytes after cleanup/error
unwinding. A configured archive that is absent or empty is failed evidence,
not a successful upload with no matching files.

## Acceptance before using the diagnostic

1. Feature-off Windows/Linux packages retain empty audit success stderr and
   unchanged DTO/schema/semantic behavior; substituting the feature binary into
   a normal package/benchmark probe is rejected. Diagnostic builds do not enter
   release artifacts and incompatible feature combinations fail to build.
2. Actual feature-built public audit children produce one valid frame for
   `jobs=1` and default. Wrong/missing worker observations fail the runner,
   rather than falling back to requested jobs or a source constant.
3. A deterministic test clock and owned recorder exercise exact nested and
   repeated intervals, zero-duration entered work, absent demand work,
   checked arithmetic failures, and every required phase. These unit checks
   supplement, not replace, the public-process fixture.
4. A fake public-child transport fixture independently exercises malformed,
   missing, duplicate-key, duplicate-frame, stale-ID, wrong-process/build,
   extra-stderr, truncated-output, and child-process rejection. Force output
   failure at an explicit boundary and prove committed run lookup remains
   valid without another audit. A watchdog only detects stuck tests.
   The build-mismatch fixture changes only the frame identity while the
   hashed binary's capabilities identity stays fixed; the PID fixture uses
   the launched child's actual observer identity, not a supplied expectation.
5. The complete authored finding/role/limitation truth and stable IDs agree
   across both binaries, both worker policies, and all rounds. Query results
   are run-pinned; cold/warm/final-freshness hard stops are not bypassed for
   telemetry. No profile result is accepted only because it matches itself.
6. Inject a normal benchmark budget miss and a diagnostic failure separately
   in the CI-step control test: diagnostics/uploads must still run where
   prerequisites succeeded, and the original failing job stays failed.
   Inspect archived process/query bytes after the normal cleanup and every
   named early-error path; upload configuration alone is not this proof.
7. Retain the actual Windows packet and its remaining uncertainty before
   selecting a performance change. If an opaque phase dominates, propose a
   separately reviewed finer owner measurement; do not label its residual as
   a proven internal cause or change correctness checks to improve scaling.

Implementation follows exact design/adversarial review and owner approval.
Focused checks will cover the recorder, strict decoder, CLI public process,
runner failure paths, Windows diagnostic packet and unaffected package smoke.
The ordinary seven-mode benchmark remains the numeric authority. This design
alone marks no Phase 1 checkbox complete.
