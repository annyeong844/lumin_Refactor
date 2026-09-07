# W5 Implementation and Verification

Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
Product base: `d63281bb613a907219f3e78348f29f7c777f102a`.
The exact [design](DESIGN.md), [rejected-backend extension](REJECTED-BACKEND.md),
and [independent review](REVIEW.md) are retained separately.

## Implemented boundary

`publication/latest.rs` compares both optional pointer IDs within its original
write transaction. Equality consumes an explicit abort; a difference retains the
existing atomic two-key update, receipt refresh and guarded immediate commit.
Aborting a provisionally opened empty pointer table does not publish it.

The abort authenticates the original database binding before and after its exact
test barrier, checks the committed receipt set, and propagates errors. A failure
irreversibly rejects backend access for that guard acquisition. Wrapped open,
read, write and commit operations then refuse access, and teardown cannot reopen
a substituted writable store or convert a caught failure into success. It still
validates namespace bindings and unlocks. Other error and success paths retain
their full existing final validation. No backend handle is cached or retained
beyond its existing transaction/lock lifetime.

The private owner test seam counts actual commits; public tests independently
verify complete durable snapshots, exact store replacement, and the supported
latest-document-replaced/index-not-yet-committed recovery boundary. The new
replacement proof is also called by the existing mapped lock-replacement
aggregate. No registry, schema, DTO, dependency, durability or budget changes
are made.

The first exact Windows replacement run failed its unchanged-foreign-bytes
predicate. The retained extension records that failure and its writable-teardown
cause. The same predicate passes after the reviewed extension; the failure is
not excluded from the evidence or reinterpreted as a passing result.

## Rust advisory scope

The external legacy lab scanned Rust, including tests, with streamed intent and
artifacts outside the checkout. The first six-file pair was closed before the
extension; the second covers all seven changed Rust files. Neither pair reports
unexpected files, missing planned files or Rust parse errors.

| Scope | Pre-write ID | Matching post-write ID |
| --- | --- | --- |
| Original six-file W5 scope | `2026-09-07T07-44-00-637Z-b18a00` | `2026-09-07T08-02-12-151Z-514dc6` |
| Seven-file rejected-backend extension | `2026-09-07T08-04-35-756Z-18f49e` | `2026-09-07T08-11-01-612Z-5f7f7e` |

Artifacts remain under `D:\lumin-w5-latest-index-gate-20260907` and
`D:\lumin-w5-rejected-backend-gate-20260907`. An unrefreshed base audit and
non-applicable TS parity/type-escape lanes are not Rust absence or quality proof.

## Current verification

Rust `1.96.0`, locked dependencies, the pinned Python `3.13.14` source-provenance
guard, one build job, and serial tests are used for Cargo verification. Targets
and generated test/package/measurement output stay outside this checkout.

| Check on current implementation | Result |
| --- | --- |
| Windows latest-index owner tests | 5 passed |
| Windows exact public compared-store replacement proof | 1 passed |
| Windows locked workspace library/binary tests | 843 passed |
| Structural architecture check | Passed; separate dependency admission not claimed |
| Windows `crash-publication` store-crash corpus | Passed, all 8 mapped invocations |
| Windows `concurrent-latest-publication` store-crash corpus | Passed, both mapped invocations |
| Windows `publication-retention-race` store-crash corpus | Passed, all 5 mapped invocations |
| Windows namespace-replacement target | All 3 public tests passed |
| Windows focused native-v13/v12 migration and latest-frontier recovery checks | All 4 exact public tests passed |
| Windows Clippy: workspace library/binaries, store all targets, changed CLI targets in their required feature partitions | Passed with warnings denied |
| Windows `snapshot-and-latest` standard/determinism corpus | Both passed; determinism retained 1 semantic capture |
| Native Linux locked workspace library/binary tests | 864 passed, including all 5 latest-index owner tests |
| Native Linux publication concurrency/fault/retention targets | Passed: 2, 8 and 5 public tests respectively |
| Native Linux lock-replacement aggregate and exact compared-store replacement proof | Both passed |
| Native Linux managed-parent replacement aggregate | Passed unchanged in an unprivileged private user/mount namespace after a retained host-namespace fixture-permission failure |
| Native Linux focused native-v13/v12 migration and latest-frontier recovery checks | All 4 exact public tests passed |
| Native Linux first-failed-attempt public projection | Passed |
| Native Linux Clippy: workspace library/binaries, store all targets, changed CLI targets in their required feature partitions | Passed with warnings denied |
| Actual Windows release, staged package and both adapter probes | Passed |
| Actual Linux static-musl release, staged package and both adapter probes | Passed |
| Fresh ordinary control/candidate measurement packets | All 4 local matrices passed; 136 cells and 2,912 raw-file hashes verified; see [comparison and limits](MEASUREMENTS.md) |

The current publication corpus independently reruns the 8 tests that also passed
before the extension. Each corpus invocation verifies its executed-test marker;
zero semantic captures are expected for store-crash mode, not zero executed tests.
Formatting, whitespace and live/new Markdown link checks have passed locally.

The first ordinary WSL managed-parent aggregate failed before its cross-volume
assertions: direct bind mounting lacked privilege and `sudo -n` required a
password. That run is not counted as passed. The exact same compiled test and
assertions subsequently passed with the guarded Cargo invocation inside
`unshare --user --map-root-user --mount --propagation private`. This grants
mount authority only in a new user-owned namespace; it does not grant host-root
access, change host mount propagation, alter sudo policy, or skip the test.
The independent compared-store substitution and lock aggregate passed in the
ordinary unprivileged namespace. No WSL shutdown was performed.

## Local packaged candidate

Both platform releases use
`LUMIN_BUILD_REVISION=d63281bb613a907219f3e78348f29f7c777f102a+w5-uncommitted-20260907`.
This is an explicit working-tree label, not a Git commit or hosted build.
The unchanged staging command produced
`D:\lumin-w5-windows-20260907\candidate-package`; the actual platform and both
staged adapter probes passed with its packaged executable and a separately built
`lifecycle-test-fault` fixture executable.

- Build ID: `build_76fc88b7dd33fafdae9224d9a5784b15b23cc378fd0aa1098df3666935aea3db`.
- Executable: 10,496,000 bytes; SHA-256
  `c2225570626fc311de7ba71998d56aef6ee05f342613e663d47d2210e6a7e953`.
- Package manifest SHA-256:
  `2aa25fe159c96f4382e15fe9e146171b003a06a8ed9b264a50f717686c5de970`.
- Both staged skill files retain SHA-256
  `a21d74d5d54f34dbaa909c9e7c82f3158b9a31c82493e93cc2ff55cf8bb0e482`.

The native Linux candidate is staged at
`/home/endof/.cache/lumin-w5-linux-20260907/candidate-package`. Its static-musl
executable, package manifest, and the unchanged ordinary controls are bound in
the [complete local comparison](MEASUREMENTS.md). Behavioral package success
and local numeric success do not establish hosted Windows budget success.

## Retained artifacts

The user requested removal of obsolete Cargo targets while validation was paused.
The active Windows target `D:\lumin-phase1-validation-target-20260905` and native
Linux target `/home/endof/.cache/lumin-phase1-p160-target-20260904` remain. Historical
release executables, including the W3 diagnostic executables formerly inside old
targets, are retained byte-identically under
`D:\lumin-retained-release-artifacts-20260907`; its `manifest.json` binds original
paths, retained names, byte counts and SHA-256 values for all 9 preserved files.
Existing staged packages and raw measurement packets are unchanged. Removal of
rebuildable caches is not a new performance measurement or a CI verdict.

P1-60 and P1-70 remain open. The local comparison does not establish a meaningful
cold-audit speedup or hosted Windows budget success. No new commit, push, hosted verdict,
ready/merge action, allocator approval or Phase 1 exit is implied.
