# W9 Guard and Attempt-Release Implementation Evidence

Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
Authority: the exact frozen [design](DESIGN.md) and
[owner approval](REVIEW.md#authority).
Source base: `12004f39127208740518d64b13c11e43b8652a34`.
Design SHA-256: `e0abf1d62c5fe5349bf621acac07d8728381d6c3c8dc236cbdd930ff2da035ca`.

## Current disposition

Instrumentation is implemented. Final-source Windows scoped verification passes:
ordinary/W9 release builds, the fresh fault fixture, all three actual W9
public-child tests, staged package/adapters, the exact 30-test W9 owner vector,
separated Clippy, formatting and structural checks. Four unchanged protocol
tests, 197 Windows xtask tests and 45 launcher tests also pass. The initial-source
feature-off owner tests (397), namespace fault tests (15) and publication/retention
fault tests (16) remain retained evidence, not claimed reruns. Independent
implementation source review passes within its read-only scope. Linux ordinary
and W9 releases, fresh fixture, all three actual W9 public-child tests and staged
package/adapter checks pass. Linux's isolated 45-test launcher suite,
existing-binary structural check, 35 W9 owner tests, 408 feature-off owner tests,
15 namespace and 16 publication/retention fault tests also pass. Linux W9,
ordinary-workspace and namespace Clippy partitions and exact incompatible-feature
rejection pass. Linux child-driver Clippy and all 196 applicable xtask tests pass.
Current-source v1/v2/v3 releases and all three release-child tests per version
pass on both platforms. The Windows/Linux xtask difference is exactly two
Windows-only cases versus one Unix-only case, with 195 shared cases and no
ignored/filtered tests. Scoped local instrumentation verification is complete,
including formatting, document checks and source/artifact binding. These local
checks preceded the authorized workflow edit. The [completed hosted run](CI-EVIDENCE.md)
passes both package/ordinary benchmark jobs and W9 capture, but overall CI fails
on the omitted structural-policy synchronization described below. P1-60/P1-70
remain open; neither those local checks nor the hosted reports close Phase 1.

The implementation adds the private v4 diagnostic and its strict runner decoder.
It preserves the earlier diagnostic envelopes, ordinary product surfaces and
numeric budgets. Hosted verification remains separate from this local evidence.
All builds/checks are serialized with one build job. User applications remain
running; no application closure, WSL restart or unrelated target deletion is used.
`memory-pause.json` retains both completed executable hashes, the previously
unstarted step and the post-stop memory reading. No Cargo/rustc process remained
at that reading. The initial resumed checks reused those builds; final-source
executables are separately bound in `windows-verification-r2.json` after the
diagnostic-only lint correction. Earlier binaries/packages and their evidence
remain separate from the final-source `windows-package-r2` and fault fixture.
The 3 GiB preflight is a local safety choice, not a product budget or relaxed
acceptance criterion. Resume only unstarted checks after headroom returns; the
launcher refusal is not a failed Cargo build or a passing skipped test.

The retained Linux receipt, `linux-verification-r2.json`, binds the r2 Rust sources,
both musl releases, native-Linux staged package, separate fixture, current-source
xtask executable and ten completed logs through the low-memory pause. Only the
isolated Python fixture suite and the existing xtask's non-Cargo structural
command ran below the preflight. Host headroom later returned above 7 GiB without
manual intervention, and serialized Cargo checks resumed with the passing W9
owner vector. No WSL cache reclamation command, configuration change, restart or
app closure was performed. Later launcher refusals likewise occur before Cargo;
checks resume only after headroom naturally returns above the preflight. Pending
checks are not failed builds or passing skips; no performance verdict follows
from the completed verification. `continuation-verification-r1.json` binds the
ten subsequent Linux build/test/lint logs, four rebuilt Windows v1/v2 logs,
predecessor archive and executable hashes without replacing the earlier
receipts. That receipt preserves its then-pending checks; the additional
predecessor release children, Linux child-driver Clippy and xtask now pass.
No instrumentation Rust source changed after the independently reviewed r2
bindings. The later CI-policy correction below is separately scoped and checked.

`final-local-verification-r1.json` binds the three preceding verification
receipts, the final continuation logs, 17 current Windows/Linux artifacts, and
the final format/document checks. It resolves the preceding receipts' pending
checks without rewriting their historical state. The complete changed Rust
inventory and all 26 hashes still match r2. At that local closeout the source
base, frozen design, Cargo lockfile and CI were unchanged. Final formatting ran
only after host headroom naturally returned above the 3 GiB preflight.

The subsequently authorized CI wiring replaces only the prior W7 diagnostic
steps with W9's exact commands, five-owner feature closure and distinct artifact
names. The ordinary benchmark, prerequisites, failed-job propagation, upload
policy and Required block are unchanged. No Rust source or frozen design bytes
change in this publication step, so the completed local Rust evidence remains
applicable. No local cold benchmark or additional Cargo build is needed for the
workflow-only wiring. However, the separate Rust structural policy still named
W7. The hosted failure shows that the earlier structural PASS did not cover
that workflow edit; the corrective verification below does not rewrite it.

## Hosted structural-policy correction

The owner approved correcting `tools/xtask/src/cargo_bootstrap.rs` after the
first hosted run exposed its stale W7 binding. The correction admits exactly
the already-reviewed W9 workflow body, public probe, runner, five-owner feature
closure and artifact paths. It preserves indivisible matching, every failure
check, ordinary benchmark and isolated target. No product, workflow, manifest,
lockfile, frozen design or budget changes accompany this correction.

Both former failures and the new predecessor/feature-closure rejection tests
pass: all **199 Windows** and **198 Linux** xtask tests, with zero ignored or
filtered tests. The difference remains two Windows-only versus one Unix-only
case. Windows scoped all-target Clippy passes with `-D warnings`, and the
rebuilt Windows xtask returns STRUCTURAL PASS against the published W9 workflow.
The isolated Python CI policy suite also passes all 12 tests.
The affected `resolver-config-registry-artifact` public corpus row passes both
standard and determinism, including its required architecture check; determinism
retains one nonempty semantic capture. Final pinned `cargo fmt --all --check`
passes without edits. Five changed/new Markdown files and 71 local link targets
pass the path/whitespace check; new section-anchor links were source-reviewed.

Raw corrective evidence is under
`D:/lumin-w9-boundary-diagnostic-20260910/ci-policy-fix/`: `windows-xtask-r2.txt`,
`windows-clippy-r1.txt`, `ci-policy-r1.txt`, `windows-corpus-standard-r1.txt`,
`windows-corpus-determinism-r1.txt`, `fmt-r1.txt` and `documents-r2.txt`.
The parent evidence directory retains `linux-xtask-r2.txt` and
`windows-resume-architecture-r2.txt`.
An initial PowerShell wrapper invocation rejected ambiguous `-p` before Cargo
launched; it is not counted as an executed or passing test.

The new Rust pre-write is `2026-09-10T13-47-38-586Z-01984a`; its matching
post-write is `2026-09-10T13-52-50-071Z-471875`. Both bind intent SHA-256
`20bbe19cebe7858cb6b4eec05b780430c762428f695d7dd6b06f496e78c7a8d7`.
The one declared Rust file is observed, with no missing, new, removed or
unexpected file. Its post-check SHA-256 is
`36a68f23b684cadb73857c84a20c66ab409df8511af66fb505a8bbae2a0ba265`.
This is lifecycle inventory evidence, not refreshed Rust semantic analysis:
type-escape and scan/capability parity remain not applicable, and the incidental
quick audit's JS/Python output cannot certify Rust. The 26 instrumentation
bindings remain unchanged and do not purport to cover this additional file.

The first failed CI and all of its successful package/diagnostic artifacts are
preserved in [CI-EVIDENCE.md](CI-EVIDENCE.md). The approved next-head CI is
separate from these local checks; no same-head rerun or automatic merge follows.

## Source boundary

The existing synchronous lifecycle recorder is parameterized over its closed
row model. W7 and W9 share that timing engine. Common database costs select one
explicit, locally borrowed observer destination; no product resource retains it.
The new contexts start/end within the original W3 spans, not around the guard's
later natural destruction. Native admission's two detached-memory verification
calls remain opaque, distinct from writable file-backend construction.

Final release retains the existing session read and explicit database drop,
Releasing commit and natural return, held-lock validation/drop/removal, parent
sync, and separate lease-removal commit and natural return. Each successful tail
ends at its immediate caller before the next product action. Failed opens,
held-read finishing proofs, sticky rejection and original result precedence
retain their existing continuations. The separate
[independent source review](REVIEW.md#independent-implementation-source-review)
confirms these boundaries for the exact r2 source hashes; neither comparison is
a platform-runtime or performance verdict.

The v4 frame appends exactly nine ordered boundary rows, each with 19 costs.
Fresh, native healthy-seeded and exact pending-recovery fixtures use the frozen
new total `11 / 1 / 2 / 2 / 0 / 9 / 2` (open/read/write/commit/abort/explicit-drop/
return-tail); their inherited v3 fixture vectors remain unchanged. Strict
decoding rejects version crossover, opaque/duplicate/reordered fields, missing
rows, invalid arithmetic and noncanonical transport. No dynamic log, fallback
version or diagnostic subtraction from ordinary budget time is introduced.

The namespace lint correction groups the existing W3/W9 phase pairs in the
feature-gated `LockProfilePhases` descriptor and removes one unnecessary final
observer reborrow. The descriptor retains neither an observer nor a product
resource. Guard/backend declarations, product callbacks, unlock/drop order and
error continuations are unchanged in the author's source comparison. The
Windows feature-off/fault results below precede that correction and are not
claimed as new runs; the Linux counterparts use the final r2 source. Current
Rust hashes and lifecycle bindings are in
`source-bindings-r2.json`, which supersedes only the namespace binding in r1.

## Verification

Raw evidence is retained at `D:/lumin-w9-boundary-diagnostic-20260910/`.
Rust 1.96.0 and the source-provenance launcher are pinned; Cargo uses `--locked`.
The launcher tests use its isolated pinned Python. Completed checks below are
scoped local evidence, separate from hosted CI and numerical acceptance.

| Evidence | Current result |
| --- | --- |
| Windows W9 `audit_` owners | Post-correction exact model/engine/store vector 30/30 PASS (7/6/17), `windows-boundary-owners-r2.txt`; four unchanged protocol tests also PASS in `windows-owner-r5.txt`. These overlap the initial owner runs rather than adding 30 new distinct tests. |
| Linux W9 `audit_` owners | Final-source 35/35 PASS: engine 7, model 7, protocol 4, store 17; `linux-boundary-owners-r1.txt`. This scoped filter intentionally leaves unrelated owner tests uncollected. |
| xtask, strict runner and previous-version regression | Windows 197/197 and Linux 196/196 PASS, including the 19 diagnostic tests and scripted invalid-frame raw capture preservation; `windows-resume-xtask-r1.txt` and `linux-xtask-r1.txt`. `xtask-platform-inventory-r1.txt` verifies all 195 shared cases and the exact two Windows-only/one Unix-only difference against their source `cfg` declarations. |
| Source-provenance admission/inverse matrix | 45/45 PASS separately on Windows and Linux, including the exact W9 vectors and feature/target crossovers; `source-provenance-r1.txt` and `linux-source-provenance-r1.txt`. These isolated tests use mocked Cargo, not an actual incompatible-feature compile. |
| Windows release builds | Final-source ordinary and W9 PASS, `windows-resume-control-build-r2.txt` and `windows-resume-boundary-build-r2.txt`; exact hashes/lengths are retained in `windows-verification-r2.json`. |
| Windows fixture and staged distribution | Final-source fresh fault-fixture build, create-new hash-checked fixture copy and package staging PASS, `windows-resume-fixture-build-r2.txt` and `windows-resume-stage-r2.txt`. |
| Actual Windows W9 release children | Final-source 3/3 PASS for jobs=1/default fresh, native seeded and exact pending recovery, final durable/public state, failed output delivery and original audit failure, `windows-resume-children-r2.txt`. |
| Actual Windows staged package and adapters | Final-source package and staged Codex/Claude adapter probes PASS, `windows-resume-package-r2.txt` and `windows-resume-skills-r2.txt`. |
| Linux ordinary release and staged distribution | Final-source musl release, separate fresh fault fixture, package staging, actual packaged binary and both staged adapters PASS; `linux-control-build-r1.txt`, `linux-fixture-build-r1.txt`, `linux-fixture-copy-r1.txt`, `linux-stage-r1.txt`, `linux-package-r1.txt`, `linux-skills-r1.txt`. |
| Actual Linux W9 release children | Final-source musl W9 release and 3/3 public-child tests PASS, with no ignored/filtered tests; exact jobs=1/default fresh, native healthy and pending-recovery vectors, durable/public states, failed output delivery and original audit failure. `linux-boundary-build-r1.txt`, `linux-children-r1.txt`; hashes in `linux-verification-r1.json`. |
| Linux namespace fault partition | Final-source 15/15 PASS, no ignored/filtered tests; `linux-namespace-build-r1.txt` and `linux-namespace-r1.txt`. Cargo's JSON artifact record binds the exact test executable and CLI; only the already-built test process runs as root for the fixture-owned cross-volume mount, never Cargo. |
| Linux W9 Clippy partitions | Final-source owner all-targets and CLI bin PASS separately with `-D warnings`; `linux-boundary-owner-clippy-r1.txt` and `linux-boundary-clippy-r1.txt`. |
| Linux feature-off owners | Final-source 408/408 PASS: engine 57, model 27, protocol 20, store 304; `linux-ordinary-owners-r1.txt`, with no ignored/filtered tests. |
| Linux publication/retention fault partition | Final-source 16/16 PASS across four targets (1/2/8/5), `linux-publication-r1.txt`. The same four unchanged unused-support-function warnings as Windows remain; this is test PASS, not lint PASS for that partition. |
| Linux ordinary and namespace Clippy | Final-source ordinary workspace all-targets and separate namespace fault target PASS with `-D warnings`; `linux-ordinary-clippy-r1.txt` and `linux-namespace-clippy-r1.txt`. |
| Linux child-driver Clippy | Final-source external W9 child-test target PASS with `-D warnings`; `linux-children-clippy-r1.txt`. |
| Predecessor release children | Current-source Windows and Linux v1/v2/v3 releases and 3/3 children per version PASS, no ignored/filtered cases. Build/test logs are `windows-resume-{execution,store,lifecycle}-{build,children}-r1.txt` and `linux-{execution,store,lifecycle}-{build,children}-r1.txt`. All six earlier binaries were preserved by create-new, hash-verified copies in `predecessor-archive-r1.txt`, not reused as current-source proof. |
| Windows feature-off owners | 397/397 PASS: engine 55, model 27, protocol 20, store 295; `windows-resume-ordinary-owners-r1.txt`. |
| Windows separate namespace fault partition | 15/15 PASS, including held-read/generation, lock and parent replacement, cross-volume and exact durable/retry state; `windows-resume-namespace-r1.txt`. |
| Windows separate publication/retention fault partition | 16/16 PASS across four targets (1/2/8/5), `windows-resume-publication-r1.txt`. Four existing unused-support-function warnings remain in the feature-selected `publication` harness; its source is unchanged from HEAD. This is test PASS, not lint PASS for that partition. |
| Windows separated Clippy partitions | Post-correction PASS with `-D warnings`: W9 owner all-targets, W9 CLI bin, ordinary workspace all-targets, external-child driver and namespace fault target. Logs: `windows-resume-boundary-owner-clippy-r1.txt`, `windows-resume-boundary-clippy-r3.txt`, `windows-resume-ordinary-clippy-r1.txt`, `windows-resume-children-clippy-r1.txt`, `windows-resume-namespace-clippy-r1.txt`. |
| Incompatible W9/fault feature union | Exact negative vector reaches Cargo and fails with the owned incompatibility diagnostic on both platforms; `windows-resume-negative-feature-union-r1.txt` and `linux-negative-feature-union-r1.txt`. This is expected rejection, not a positive build PASS. |
| Architecture before workflow publication | Windows and Linux STRUCTURAL PASS, `windows-resume-architecture-r1.txt` and `linux-architecture-direct-r1.txt`; Linux executes the already-built current-source xtask directly without Cargo. These checks predate the W9 workflow edit and cannot establish its routing verdict. Neither command establishes separate dependency-admission or corpus/package/benchmark verdicts. |
| Hosted wiring admission and control | Pinned isolated Python: 12 CI policy tests and 45 source-provenance tests PASS, `hosted-ci-policy-r1.txt` and `hosted-source-provenance-r1.txt`. The exact inverse W9-to-W7 workflow transformation matches the source-base workflow and the changed build block parses as PowerShell; `hosted-workflow-scope-r1.txt`. These are local wiring checks, not a hosted result. |
| Independent resource-lifetime/failure source comparison | Scoped SOURCE PASS with no actionable P1/P2, `w9_lifetime_source_review`; exact 26-file r2 source bindings and frozen design verified. This is not binary-erasure, platform-runtime or performance certification. |
| Matching external Rust lifecycle post-write | All three pairs complete within their inventory-only scope; all 26 current changed Rust paths bind in `source-bindings-r2.json`. See limitations below. |
| Formatting | Final pinned `cargo fmt --all --check` PASS with exit 0 and no edits, `windows-format-r4.txt`. This actual log supersedes the earlier receipt's `windows-format-r3.txt` reference, whose raw file is absent. The dependency launcher rejects `fmt`; this non-resolving check uses pinned Cargo directly. |
| Document links/whitespace | Six changed/new documents and 69 local link targets PASS; tracked/new whitespace checked. Anchor slugs were source-reviewed separately. `documents-r12.txt`. |

The retained compile-failure logs are intermediate development evidence, not
passing checks; no assertion was weakened to replace them with a green result.
The first W9 Clippy invocation found the two corrected namespace issues. The
second incorrectly combined CLI all-targets with instrumentation, activating
the CLI dev dependency's `gate-test-fault` feature. That rejected mixed partition
is not passing evidence. The corrected positive commands separate W9 owner
all-targets from the W9 CLI bin and from the ordinary external-child driver;
the product's incompatible-feature guard remains intact.

## External Rust lifecycle scope

The main Rust pre-write is invocation `2026-09-10T06-58-33-469Z-7141e7` under
`pre/`; the separately added scripted-child dispatch is covered by invocation
`2026-09-10T07-16-41-625Z-7d3be0` under `scripted-child/`. Both report Rust scope
from `lumin-rust-analyzer`, with no producer failures or unavailable Rust scope.
Their matching post-write invocations are `2026-09-10T07-53-51-987Z-30e1cd` and
`2026-09-10T07-54-24-953Z-1d2c86`. Both preserve the exact pre-write invocation and
intent hash, report complete file delta and no missing planned paths, and require
no TS acknowledgements. The main pair observes 27/27 planned paths, including all
five new files. The secondary pair observes its one planned path and also reports
`boundary_tests.rs` as unexpected in that narrower directory inventory; this
exact file is explicitly planned/observed by the main pair, not unapproved work.

The namespace correction uses pre-write `2026-09-10T08-56-11-561Z-ad2708` and
post-write `2026-09-10T09-00-31-165Z-fc0ba9` under `clippy-namespace/`. Its exact
intent hash matches, its one planned file is observed, and no file is missing,
new, removed or unexpected. The Rust producer is available; fuzzy name hints
remain degraded search hints, not reuse or absence proof. This pair supersedes
the earlier namespace binding without broadening the Rust edit scope.

The 26 instrumentation Rust files are covered by these three pairs and have
post-write hashes recorded in `source-bindings-r2.json`. That is a separate
local binding check, not a claim that the external tool attests those hashes.
The tool marks Rust post-write type escapes, capability parity and scan parity
not applicable and does not refresh Rust/base analysis. The pre-write root,
Rust owner, included-test setting and empty exclusion list were checked directly.
Lifecycle-only inventory evidence is not whole-repository quality, absence,
freshness or semantic-equivalence evidence. No missing capability is called clean.
