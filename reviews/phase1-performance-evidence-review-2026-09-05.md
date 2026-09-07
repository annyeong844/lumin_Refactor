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

The [latest completed public CI packet](probes/phase1-windows-process-observer-2026-09-07/CI-EVIDENCE.md)
on head `d63281bb613a907219f3e78348f29f7c777f102a` / merge checkout
`c38c5e07bc17a974678e6a130eee3aff26be2ea8` passes both builds and staged
binary/adapter behavior probes. Both platforms complete all 34 benchmark cells
with valid semantic truth and process observations. Native Linux passes its
numeric matrix; Windows misses scaling at `0.842191828905966` against `0.75`,
so its package job and Required remain failed. The separate diagnostic is not
budget authority. The [earlier invalid/missed W3 packet](probes/phase1-windows-audit-store-diagnostic-2026-09-06/CI-EVIDENCE.md)
remains retained and is not reclassified by this new measurement.

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
not evidence that the hosted four-worker Windows miss is fixed. Fresh blocking
CI evidence for the candidate remains required before acceptance.

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
