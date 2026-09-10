# REVIEW-005: Phase 1 Performance Evidence Review

Status: **review candidate; not frozen; P1-60 remains open**

This review owns the decisions needed to turn the current measured performance
candidate into complete SLICE-001 AC 16 evidence. No owner approval or
whole-amendment independent PASS is claimed. The scoped diagnostic review
below does not approve the remaining decisions. Existing frozen requirements
continue to apply until an exact amendment receives both reviews.

## Measured candidate

The [Windows NTFS and WSL2 ext4 packet](probes/phase1-foundation-benchmark-local-windows-wsl2-ext4-2026-09-05/)
retains two complete numeric matrices. Both contain seven modes with three
repetitions, the frozen fixture, and the same complete authored finding truth
and stable IDs. Each measured time, RSS, executable-size, and scaling budget
passes for that local candidate, not for every supported host or a later build.

The [first public CI run](https://github.com/annyeong844/lumin_Refactor/actions/runs/33946327448)
on `2a2e9c837f7ef7723c459b574c1bc6ced4edd85d` fails the Windows scaling
budget with four default workers: cold default median `3,469,498,100 ns`,
`jobs=1` median `3,113,171,700 ns`, ratio `1.114457676716` against `0.75`.
Its other numeric budgets and complete semantic oracle pass. The retained CI
artifact `lumin-foundation-benchmark-windows-x64.json` has SHA-256
`963fafa899e67b69c8b2d268c061109deff7552298304b82a20f00465a16f1c0`.
This is a blocking miss, not measurement noise to discard.

The preceding [W8 public CI run](https://github.com/annyeong844/lumin_Refactor/actions/runs/34344138195)
on head `12004f39127208740518d64b13c11e43b8652a34` / merge checkout
`c373b27b5ee8f6fba5551b5fab319a4778e68643` passes 56 of 58 jobs, including
native Linux and both staged binary/adapter probes. Windows scaling misses at
`0.8953551885800882` against `0.75`; Linux passes at `0.7051724995962835`.
Every other numeric budget passes. Only Windows Benchmark packaged binary and
dependent Required fail. All 745 Windows, 711 Linux and 379 diagnostic capture
files match their complete manifests; every median, count vector and full
256-entry semantic map was rechecked. The [W8 hosted packet](probes/phase1-empty-latest-read-2026-09-09/CI-EVIDENCE.md)
retains the exact identities, verification bindings and remaining measurement
gaps. No rerun or merge was performed. Its separate diagnostic is not budget
authority. Different hosted machines and sample variance prevent causal
comparisons with the retained [W7](probes/phase1-lifecycle-boundary-diagnostic-2026-09-08/CI-EVIDENCE.md#lint-correction-hosted-verification)
or [W6 hosted packet](probes/phase1-attempt-session-read-2026-09-08/CI-EVIDENCE.md).
The [initial W5 packet](probes/phase1-latest-index-noop-2026-09-07/CI-EVIDENCE.md)
retains its independent numeric misses and then-failing test routing.
The [earlier W4 packet](probes/phase1-windows-process-observer-2026-09-07/CI-EVIDENCE.md)
and [invalid/missed W3 packet](probes/phase1-windows-audit-store-diagnostic-2026-09-06/CI-EVIDENCE.md)
remain retained and are not reclassified by this measurement.

The latest [W9 hosted packet](probes/phase1-guard-release-diagnostic-2026-09-09/CI-EVIDENCE.md)
on head `90e67d5e2e4a437ac1d6178b4f992b225fb7d74e` passes both staged package
jobs and their unchanged ordinary benchmark: reported Windows/Linux scaling
ratios are `0.6774485114105342` and `0.5199607512575846`. W9 capture also passes
as diagnostic-only evidence. Overall CI is **FAIL**, 47/58 successful jobs,
because the Rust structural checker still required W7's workflow bindings.
That omission affects core tests and five shared structural corpus rows; it is
not a new product-test failure. The complete failure and all 1,835 hash-verified
captures are retained. Reported numeric PASS does not supply the permanent
runtime observations or remaining approvals below, nor prove a causal speedup.

The [allocator packet](probes/phase1-musl-allocator-selection-2026-09-05/)
proposes exact `mimalloc 0.1.52` with `v2` only for the Linux-musl CLI. It
retains the system-allocator control, both candidate measurements including the
first scaling miss, the exact dependency edges, and size/RSS/build/unsafe costs.
Protected review must approve that dependency change before merge.

## Runtime observations still required

ARCH-001 Sections 4 and 12 require actual worker/stack observations and
owner-stage timings. The current harness derives `actualJobs` from its request
and local parallelism policy and checks the stack constant in source. Its
`stageTimingsNanoseconds` covers benchmark setup, the entire product process,
and truth validation. These fields do not prove the required engine execution
observations; the reports' numeric `PASS` cannot close that acceptance surface.

The implementation must expose observations produced by the one engine-owned
pool and scheduler for the measured execution, bind them to its run or gate
revision and packaged build, and consume them without changing semantic IDs or
determinism. Missing observations must remain unavailable, never be replaced by
requested values or zero times. Any new public transport or persisted shape
requires its owning contract to be reviewed before implementation.

### Windows cold-audit diagnosis

The [W2 diagnostic design](probes/phase1-windows-audit-execution-diagnostic-2026-09-05/DESIGN.md)
defines a separate feature-built, non-distributable public-audit probe and an
exact Windows CI comparison with the unchanged release package. It measures
the existing call boundaries and records pool-produced observations without
inventing scheduler ready-wait time, backend flush time, or OS stack usage.
The full semantic oracle remains mandatory; malformed/missing observations
fail the diagnostic and cannot improve a numeric budget result.

The [exact W2 review record](probes/phase1-windows-audit-execution-diagnostic-2026-09-05/REVIEW.md)
records author design review, independent adversarial scoped PASS, and the
user's 2026-09-05 approval freezing W2 for diagnostic implementation. This
narrow candidate is for bottleneck diagnosis, not an amendment that fulfills
permanent run/gate observability. It does not approve the allocator, `/mnt`
disposition, or Phase 1 exit.

The [actual W2 CI evidence](probes/phase1-windows-audit-execution-diagnostic-2026-09-05/CI-EVIDENCE.md)
now supplies diagnostic-only worker/stack and 23-phase observations. Within
each default-worker sample, store-open, attempt-begin, and publication self
time account for about 79% of the command. These remain opaque owner regions,
not measured backend flush or lock-wait time. Permanent metrics remain open.

The frozen diagnostic scope is [W3 store-call observation](probes/phase1-windows-audit-store-diagnostic-2026-09-06/DESIGN.md),
with its [review disposition](probes/phase1-windows-audit-store-diagnostic-2026-09-06/REVIEW.md).
It specifies a separate diagnostic extension/version, exact store-owned
intervals, and hosted build/capture isolation. Author and independent design
reviews pass for its corrected exact candidate; the owner approved freezing
and implementing that hash on 2026-09-06. Its review record binds the unchanged
candidate bytes. W3 grants no performance-change authority. W2's approved bytes
and the ordinary budget stay unchanged.

The [W4 observer correction](probes/phase1-windows-process-observer-2026-09-07/DESIGN.md)
is frozen through its [exact design review](probes/phase1-windows-process-observer-2026-09-07/REVIEW.md).
It replaces Windows PID-only ancestry with private inherited-job cumulative
accounting and a strict, archived lifetime receipt. Its scope is measurement
correctness, not product optimization, old-cell reclassification, Linux scaling,
or permanent run/gate metrics. Its [actual hosted evidence](probes/phase1-windows-process-observer-2026-09-07/CI-EVIDENCE.md)
validates all 48 Windows ordinary/diagnostic lifetime receipts; the remaining
Windows numeric miss is independent of that observer correctness result.

The bounded product candidate is [W5 unchanged latest-index synchronization](probes/phase1-latest-index-noop-2026-09-07/DESIGN.md),
with its [exact scoped review](probes/phase1-latest-index-noop-2026-09-07/REVIEW.md).
It removes only redundant derived-index commits while preserving the original
held-store, namespace, generation and receipt validations. It does not change
backend lifetime or budgets and is not promised to close the scaling gap.
Its [local implementation checks](probes/phase1-latest-index-noop-2026-09-07/IMPLEMENTATION.md)
and [four fresh control/candidate packets](probes/phase1-latest-index-noop-2026-09-07/MEASUREMENTS.md)
are complete. Both controls and candidates pass locally under the eight-worker
policy, with little cold-audit change and mixed changes in other modes. This is
not evidence that the hosted four-worker misses are fixed. Its
[initial hosted execution](probes/phase1-latest-index-noop-2026-09-07/CI-EVIDENCE.md)
retains both numeric misses and the separate test-routing failure. The
[routing follow-up and integration diagnosis](probes/phase1-latest-index-noop-2026-09-07/INTEGRATION-DIAGNOSTICS.md)
retains the two earlier hosted gate-barrier failures and the scoped diagnostic
change. Those tests pass in the current hosted run, without establishing the
earlier failures' cause or invalidating their evidence. At that checkpoint the
Windows scaling miss still blocked acceptance.

The preceding bounded implementation is
[W6 generation-bound attempt-session read](probes/phase1-attempt-session-read-2026-09-08/DESIGN.md),
with its [scoped review and frozen W6-03 amendment](probes/phase1-attempt-session-read-2026-09-08/REVIEW.md).
It proposes replacing two consecutive backend lifetimes inside one session
validation with one short generation-bound lease read. Original held-object,
receipt, namespace and liveness validation remain required. It introduces no
guard-wide backend cache and claims no measured speedup. On 2026-09-08 the
owner approved freezing the exact reviewed bytes for W6 implementation and
local Windows/Linux comparisons. Implementation exposed a new-guard failure
continuation that reopens a substituted store writable despite original-guard
rejection. Three exact Linux public-child barriers reproduce changed foreign
bytes, and independent review reopens the candidate. The
[blocker](probes/phase1-attempt-session-read-2026-09-08/IMPLEMENTATION-BLOCKER.md)
retains the original negative evidence. Its
[failure-continuation authority](probes/phase1-attempt-session-read-2026-09-08/FAILURE-CONTINUATION.md)
is now owner-approved with author and independent scoped PASS. The
[implementation evidence](probes/phase1-attempt-session-read-2026-09-08/IMPLEMENTATION.md)
closes the focused counterexample on both platforms and preserves all three
publication phases, exact durable outcomes and original negative evidence.
Affected Windows corpus modes and both actual packages/adapters pass. Four fresh
[local comparison packets](probes/phase1-attempt-session-read-2026-09-08/MEASUREMENTS.md)
pass local targets, with 2.83%/1.65% lower Windows/Linux cold medians and mixed
changes elsewhere. They do not establish a material causal speedup or close the
hosted four-worker gap. A separate
[four-logical-CPU Windows comparison](probes/phase1-attempt-session-read-2026-09-08/FOUR-WORKER-COMPARISON.md)
was rejected after its control: Rust 1.96.0 still reports 12 processors/eight
default workers under the launcher's four-CPU affinity. The complete control
packet is retained, the candidate was not run, and no four-worker result is
claimed. The subsequently authorized publication now has
[actual four-processor hosted evidence](probes/phase1-attempt-session-read-2026-09-08/CI-EVIDENCE.md):
all behavioral jobs pass, but Windows scaling still misses. That diagnostic
places about 61-63% of engine command time in store-owned regions without
isolating backend, validation, locking or durability costs. The
[source investigation](probes/phase1-attempt-session-read-2026-09-08/COST-INVESTIGATION.md)
routes to the prepared
[W7 lifecycle-boundary diagnostic](probes/phase1-lifecycle-boundary-diagnostic-2026-09-08/DESIGN.md).
Its [exact author and independent scoped design reviews](probes/phase1-lifecycle-boundary-diagnostic-2026-09-08/REVIEW.md)
pass, and the owner explicitly froze that exact candidate for diagnostic
implementation. Its [implementation acceptance](probes/phase1-lifecycle-boundary-diagnostic-2026-09-08/IMPLEMENTATION.md)
records completed scoped local Windows/Linux checks. The authorized
[fourteen-cell hosted packet](probes/phase1-lifecycle-boundary-diagnostic-2026-09-08/CI-EVIDENCE.md#lint-correction-hosted-verification)
is complete: its captures and frozen counts verify, both ordinary scaling
targets still miss, and both actual packages/adapters pass. The test-only
feature-off callback correction also passes its clean hosted lint verification.
W7 distinguishes eight selected
call contexts and thirteen disjoint API costs without changing existing resource
lifetimes or claiming raw flush/lock-wait attribution. It establishes no hosted
performance PASS or optimization authority. W6 grants no broader optimization, worker-policy
change, or budget amendment.

The preceding bounded implementation is
[W8 empty latest-index read-through](probes/phase1-empty-latest-read-2026-09-09/DESIGN.md).
It proposes removing only the redundant second backend/write/abort when one
exclusive call has proven canonical latest absent and the entire pointer table
empty, retaining original-held completion and rejection authority. Its
[author and independent design review](probes/phase1-empty-latest-read-2026-09-09/REVIEW.md)
passes for the exact candidate, which the owner has frozen for scoped
implementation and local Windows/Linux verification. Its
[implementation record](probes/phase1-empty-latest-read-2026-09-09/IMPLEMENTATION.md)
has independent scoped source PASS and completed scoped local correctness,
package/adapter and actual release-child verification on both platforms.
Its four fresh ordinary [comparison packets](probes/phase1-empty-latest-read-2026-09-09/MEASUREMENTS.md)
pass local numeric and integrity checks, including 2,912 hashed captures and
identical full 256-entry semantic mappings. The timings are mixed; they do not
establish a causal speedup or resolve hosted scaling. Broad backend reuse and
read-only substitution remain excluded. P1-60/P1-70 stay open. The separate
[publication approval](probes/phase1-empty-latest-read-2026-09-09/REVIEW.md#publication-approval)
permits committing/pushing W8 and checking one resulting public CI run while
PR #135 remains Draft; it grants no broader optimization or merge authority.
That [authorized hosted run](probes/phase1-empty-latest-read-2026-09-09/CI-EVIDENCE.md)
is now complete, retaining the Windows scaling miss above.

The active owner-frozen instrumentation packet is
[W9 guard and attempt-release observation](probes/phase1-guard-release-diagnostic-2026-09-09/DESIGN.md).
It selects eight existing guard entry/exit intervals and final attempt release,
which W7 does not decompose. Native admission's two memory-backed verification
calls remain opaque and distinct from writable file-backend construction/drop.
It changes no product behavior, resource lifetime or numeric budget and makes
no whole-store or speedup claim. Its
[author and independent scoped design reviews](probes/phase1-guard-release-diagnostic-2026-09-09/REVIEW.md)
pass for the exact candidate. The owner approved exact freeze, scoped
instrumentation and local verification on 2026-09-10. The
[implementation record](probes/phase1-guard-release-diagnostic-2026-09-09/IMPLEMENTATION.md)
records completed scoped local verification: both platforms' ordinary/W9
releases, actual v1/v2/v3/v4 release children, staged packages/adapters, scoped
owner tests, separated Clippy, xtask, launcher and structural checks, plus final
format, documents and source/artifact binding. It distinguishes retained
initial-source Windows feature-off/fault evidence from final-r2-source Linux
tests. Independent scoped source review also passes. New Cargo commands remain
gated on host memory headroom. The first authorized hosted run exposed the
omitted structural-policy synchronization. The owner approved correcting that
policy, preserving the completed failed run, then publishing one new-head CI.
The ordinary benchmark and Required outcome remain authoritative. Local
correction checks do not establish the new hosted verdict or close performance
acceptance; same-head reruns and further scope changes require separate approval.

## Proposed WSL `/mnt` disposition

SLICE-001 Section 12 currently requires the report-only `/mnt/<drive>` run to
report the same metrics. The [drvfs probe](probes/phase1-wsl2-mnt-rename-noreplace-2026-09-05/)
shows `EINVAL` for the no-replace file and directory rename operations on the
observed mount. A flags-zero directory rename works but allows replacement;
directory hard linking fails. The required publication protocol cannot complete
the public lifecycle matrix with the available primitives on that mount.

The proposed amendment is to let this report-only environment retain an
explicit **unsupported namespace capability** diagnostic when a required
primitive is demonstrably unavailable. The normal same-metrics matrix remains
required whenever the primitives are available. Numeric targets and all three
blocking environments remain unchanged.

The proposed diagnostic must retain:

- the exact packaged binary/build and frozen fixture identities;
- OS, kernel, mount/volume identity, and the report-only classification;
- the requested operation/flags and observed OS error from a probe on the
  same filesystem as the benchmark repository;
- the matching public-command failure and any completed measurements, marked
  separately from the unexecuted modes; and
- an explicit incomplete/unsupported status, with unavailable metrics absent
  or null, never zero or a successful lifecycle result.

Permission errors, corrupted state, a failed semantic oracle, crashes, or an
unrecognized OS error must remain failures rather than being classified as
unsupported. Failure of a blocking environment remains a slice failure. This
proposal adds no runtime fallback, no replacement-capable directory move, and
no automatic migration or repair of repository state.

An alternative is a separately reviewed crash-consistent no-replace protocol
for drvfs that preserves foreign destination winners at every crash boundary.
That is a larger product namespace change. The recommendation for this
report-only diagnostic is the explicit capability result above.

## Review and completion requirements

The design reviewer and independent adversarial reviewer must check the exact
candidate, including the dependency-cost packet and the distinction between
numeric measurements and missing execution observations. The `/mnt` review
must challenge both an unsupported mount and a supported mount, a foreign
destination winner, permission/integrity failures, and partial measurements.
No exception may turn a failed product command into completed benchmark work.

After approval, amend the owning SLICE-001 diagnostic contract and any affected
ARCH-001/ARCH-002 observation or publication contract before dependent code.
P1-60 can close only after the required observations, all blocking matrices,
the reviewed `/mnt` result, and the clean-checkout distribution checks pass.
