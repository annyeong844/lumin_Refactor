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

The [latest completed public CI matrix](https://github.com/annyeong844/lumin_Refactor/actions/runs/33960297104)
on `a83ee602dfaf1d62c16f8f9f68f6111c81c203fb` passes the functional, crash,
determinism, and native Linux package checks. Windows still misses only the
numeric scaling target: default-four-worker median `1,756,281,200 ns` versus
`2,023,961,300 ns` for `jobs=1`, ratio `0.8677444573668479`. Its report SHA-256
is `456b57e95910007f921a5be6887534385e9b51b36e5efe93c1f7f53ac3928399`.
Native Linux completes all seven modes with ratio `0.7237935075487433` and no
numeric target miss; its report SHA-256 is
`43a5abc3c60da3ba9f37cd1fe8e08c3cc87edaca86fa4fe183535de579802ec6`.
Neither these matrices nor the earlier local reports establish the outcome of
subsequent product changes. Fresh blocking measurements remain required.

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

### Windows cold-audit diagnostic candidate

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
