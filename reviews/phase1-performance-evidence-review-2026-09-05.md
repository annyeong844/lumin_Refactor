# REVIEW-005: Phase 1 Performance Evidence Review

Status: **author design-review candidate; not frozen; P1-60 remains open**

This review owns the decisions needed to turn the current measured performance
candidate into complete SLICE-001 AC 16 evidence. No owner approval or
independent adversarial PASS is claimed. Existing frozen requirements continue
to apply until an exact amendment receives both reviews.

## Measured candidate

The [Windows NTFS and WSL2 ext4 packet](probes/phase1-foundation-benchmark-local-windows-wsl2-ext4-2026-09-05/)
retains two complete numeric matrices. Both contain seven modes with three
repetitions, the frozen fixture, and the same complete authored finding truth
and stable IDs. Each measured time, RSS, executable-size, and scaling budget
passes. Native Linux CI must still run the staged Linux-musl distribution.

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
