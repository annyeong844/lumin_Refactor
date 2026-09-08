# W7 diagnostic implementation acceptance

Status: **local checks and hosted diagnostic observation complete; blocking CI remains red**.
This is not a performance, full-CI or merge PASS.

Authority is the explicitly frozen [DESIGN.md](DESIGN.md), SHA-256
`eaf2133f6ac6aeaa9f89538cc173a31788e0211e1a4a9bf77a9fde131198ecb8`,
and [REVIEW.md](REVIEW.md). Source base is
`964c91c6c89ec84a5b2b9bb42851caf664464dfa` in the dedicated
`agent/phase1-performance-budgets` worktree. After local verification, the owner
separately approved committing and pushing this packet and running clean-checkout
CI. No budget, optimization, worker, durability or merge authority is inferred.

## Independent ownership comparison

The independent `lifecycle_boundary_design_review` reviewer compared the eight
core instrumentation files with the source base and returned scoped **PASS**.
All eight file hashes were stable during that read-only review. The frozen
design hash was independently rechecked. A final read-only follow-up also
returned scoped **PASS** after formatting, final-use recorder-borrow
simplifications and the feature-erased callback adapter correction.

- `StoreDatabase`, read/write transaction and guard field layouts are unchanged.
  Failed pre-construction opens retain backend-before-entry local teardown;
  constructed databases retain their original field order.
- Both derived-document returns, catalog callback returns, lease read returns
  and index abort/commit returns retain their original ownership scopes.
  Successful return-tail markers start after the owned result exists and end
  in the immediate caller before subsequent work. The explicit table drop
  before abort is unchanged.
- Final generation validation still runs on mutation failure and retains error
  priority. Guard poisoning and W6 continuation rejection remain intact.
- JSON/staging declarations, validation, callback and publication ordering are
  unchanged. The optional recorder is synchronously borrowed, not stored in a
  resource. Feature-off arguments/clocks are erased and the core body is shared.

This closes only the source/ownership comparison in acceptance item 2. Supporting
canaries are not observations of redb/HeldEntry destructors. Actual helper,
public-child, fault and package results remain distinct acceptance requirements.

Final independently reviewed hashes, relative to `crates/application/store/src/`:

| File | SHA-256 |
| --- | --- |
| `namespace/database.rs` | `487fb178d2f13239c64bf397fddee838f363d9b43d50d9b7e6f3a1ed30c3be8f` |
| `publication/latest.rs` | `62bc5f333deaf71dfb7ea0832fa6fc298fa0bca06f594dd4fd048ebbe713b85a` |
| `publication/liveness.rs` | `afbbc36f8f356faff98f2e29d00871de28314261b56284e3832ba4a2df8fbd8b` |
| `publication/liveness/records.rs` | `6b5455b4ffef7095cf159942c684701fa572437eb977aa36ff8ee8ba27b7448f` |
| `publication/files.rs` | `38afa348adcb0a9a947ccdddf37fa54a9031341f5298b3b450797fc2cafe1f05` |
| `publication/run.rs` | `e01e4a55c9700cc457fc1acfd3e587b416781422da0dd292e67ee6252bce62a3` |
| `lib.rs` | `9bd97f4a7aacc9d3fb6327cd2a826cfc0b9e354a99c9f20449b078259683cec7` |
| `audit_lifecycle_profile.rs` | `8cda2216946341605c8cb7508b4a4f5ea6077a5f491c6aa973ec896249ad7479` |

## Verification boundary

The external Rust pre-write invocation is
`2026-09-08T11-43-16-415Z-090893`, paired with post-write
`2026-09-08T13-06-11-652Z-a6fb6a`; its exact advisory and all local raw logs are
outside the repository under `D:/lumin-w7-lifecycle-diagnostic-20260908/`.
The advisory covers the 26 planned Rust files, includes tests, and reports
Rust lifecycle evidence with opaque macro/configuration surfaces; it is not a
full absence or quality proof. Matching post-write observed all 26 planned
files, five planned new files, no unexpected new files and no reported parse
errors. It completed before broader Cargo regression checks; base audit
evidence was not refreshed. The [hosted W7 evidence](CI-EVIDENCE.md) separately
records the complete observation, ordinary Windows miss and lint correction.

The structural check exposed a remaining W3-only orchestration constant in
`tools/xtask/src/cargo_bootstrap.rs`. The follow-up changes only that routing
owner: the complete approved W7 body, exact target/archive/fixture bindings,
and required public probe/runner commands replace the prior W3 expectations.
It does not admit shell fragments separately. Mutation tests reject removal or
weakening of fixture copying, hash comparison, probe selection and isolation.
The separate Rust pair is pre-write `2026-09-08T13-34-16-052Z-5866f6` and
post-write `2026-09-08T13-39-14-870Z-91cf2e`, with the one planned file observed,
no new files and no reported parse errors. Its raw evidence is retained under
`D:/lumin-w7-ci-binding-gate-20260908/`; the original failed structural log is
not reclassified as passing. Final source SHA-256 for that routing owner is
`f75bf31bc25546d0a4eaa8c1e32737b49e7033a94155d098c180dde7c09a1e21`.

The final developer help correction in `tools/xtask/src/main.rs` has its own
pair: pre-write `2026-09-08T13-56-26-150Z-e3751c` and post-write
`2026-09-08T13-58-33-310Z-a3824a`. The one planned file was observed with no new
files or reported parse errors. A real `lumin-xtask` invocation requires usage
exit `2` and the new selector in its output. Source SHA-256 is
`63fa3999ad822ee09f67233b430090720b6e072a0f00f0664b25713ea537e0c9`;
the advisory, focused probe and subsequent xtask checks are retained under
`D:/lumin-w7-help-gate-20260908/`.

## Hosted feature-off lint correction

The first hosted run, [34238854239](https://github.com/annyeong844/lumin_Refactor/actions/runs/34238854239),
found `clippy::redundant_closure` in the feature-off, test-only
`publication/latest.rs::sync_index_with_commit` adapter. The prior scoped
Clippy checks did not cover this ordinary store-test configuration; their
success was not full-workspace lint evidence. Both hosted lint failures are
retained, not relabelled passes.

The correction selects the original two-argument closure only when W7 is
enabled and passes `commit` directly otherwise. Only this `#[cfg(test)]`
adapter changes. The independent ownership reviewer returned scoped **PASS**
against published head `945f373a65d6fe79fba98c5dc4e535d3b8c045ee`: no
production body, callback order, resource lifetime or failure behavior changes.
The corrected `publication/latest.rs` SHA-256 is
`b980aa8128571fa82880825607d522bd92167bc2be77619661103272b21d5797`;
the earlier table records the originally published source comparison.

The new external Rust pair is pre-write
`2026-09-08T14-35-34-770Z-917b98` and post-write
`2026-09-08T14-43-46-910Z-6d244e`. It observed the one planned file, no new
files and no reported parse errors; base audit evidence was not refreshed.
Before post-write, the exact latest-index tests passed: Windows five ordinary
and ten W7 tests, Linux five ordinary tests. After post-write, ordinary and W7
store all-target Clippy passed on both platforms with `-D warnings`, and
pinned formatting passed. The exact failing hosted command, locked
`cargo clippy --workspace --all-targets -- -D warnings`, subsequently passed
on Windows and native Linux as well. Raw advisories and logs are retained outside the
repository under `D:/lumin-w7-feature-off-lint-20260908/`.

## Local execution evidence

All Cargo commands use `--locked`, the pinned Rust 1.96.0 toolchain and the
source-provenance launcher, except direct pinned `cargo fmt`. Builds use one
Cargo job. Windows runs on the actual Windows host. Linux runs in WSL Ubuntu
with build output and runtime fixtures on its native filesystem; this is not
the unresolved `/mnt/<drive>` benchmark. Python policy checks use 3.13.14.
The ordinary, W2, W3 and W7 binaries have separate target directories.

Raw log names below are relative to `D:/lumin-w7-lifecycle-diagnostic-20260908/`
unless another evidence root is named. Counts are executed tests, not an
aggregate corpus-completeness or clean-CI claim.

| Proof | Verified result and raw evidence |
| --- | --- |
| Actual W7 release children | Windows and Linux each pass the three `audit_lifecycle_diagnostic` tests: fresh default/jobs=1, healthy seeded and exact fresh-first pending-latest recovery, ordinary stdout, diagnostic transport failure and original audit failure. Logs: `windows-public-lifecycle-final.txt`, `linux-fixture-final-build-public-v3.txt`. |
| W2-only compatibility | Both platforms pass all three `audit_diagnostic` public-child tests against separately built W2-only release binaries. Logs: `windows-v1-public-probe-final.txt`, `linux-v1-build-and-public-probe.txt`. |
| W3-only compatibility | Both platforms pass all three `audit_store_diagnostic` public-child tests against separately built W3-only release binaries. Logs: `windows-v2-public-probe-final.txt`, `linux-v2-build-and-public-probe.txt`. |
| Model, recorder and real selected helpers | Windows passes 25 and Linux 26 `audit_` library tests across model, engine and store, including the recorder canaries and actual missing/empty POINTERS, lease/catalog returns, commit/abort, failed open/decode/commit paths. Logs: `windows-audit-owner-tests-r3.txt`, the test portion of `linux-w7-owner-tests-clippy-final.txt`. |
| v3 protocol | Both Windows protocol tests pass, including closed ordering, arithmetic and version separation. Log: `windows-lifecycle-protocol-tests.txt`. |
| Ordinary affected libraries | Windows passes engine 55, model 27, protocol 20 and store 288 tests, total 390. Log: `windows-ordinary-affected-libraries.txt`. |
| Ordinary publication and recovery | Both platforms pass `publication` (1), `publication_concurrency` (2), `publication_faults` (8) and `publication_retention_race` (5), with their required fault features and no measured feature. Logs: `windows-publication-regressions.txt`, `linux-publication-regressions.txt`. |
| Ordinary public freshness | Both platforms pass all four failed/stale pre/post capture binding tests selected by `capture_retains_and_rechecks_its_semantic_bindings`, including exact-barrier rechecks and durable retries. Windows log: `windows-public-freshness.txt`. Linux log: `linux-final-freshness-xtask-architecture.txt` in the help evidence root. |
| W5/W6 and namespace replacement | Both platforms pass all seven `state_namespace_replacement` tests, including the unchanged-index abort and four generation-bound session-read cases. Logs: `windows-namespace-regressions.txt`, `linux-namespace-regressions.txt`. Linux executes the compiled test target as root only for its required mount fixtures; Windows exercises the separate C:/D: volume control. |
| Staged ordinary distribution | Windows and Linux stage the normal release package and pass the platform probe plus both packaged skill adapters. Logs: `windows-package-stage.txt`, `windows-package-platform.txt`, `windows-package-skills.txt`, `linux-package-adapters.txt`. Neither staged binary enables diagnostics. |
| Incompatible feature union | Windows reaches Cargo and gets the expected owner compile rejection, exit 101, for W7 plus `lifecycle-test-fault`. Log: `windows-incompatible-feature-check.txt`; this is a negative proof, not a successful binary build. |
| Hosted routing and policy | Source-provenance tests pass 40/40 on each platform; CI-policy tests pass 12/12. Logs: `windows-source-provenance-tests-final.txt`, `linux-source-provenance-tests-final.txt`, `windows-ci-policy-tests-final.txt`. The actual hosted execution is recorded separately in [CI-EVIDENCE.md](CI-EVIDENCE.md). |
| Windows lint and runner | W7 model/protocol/store library and test Clippy, ordinary diagnostic-probe Clippy and final xtask all-target Clippy pass with `-D warnings`. Logs: `windows-diagnostic-owner-clippy-final-verified.txt`, `windows-public-probe-clippy-r2.txt`, and the final xtask logs in the help evidence root. All 192 Windows xtask tests pass there. |
| Linux lint and runner | W7 model/protocol/store library and test Clippy passes with the exact `-D warnings` argument. Log: `linux-w7-owner-clippy-final-verified.txt`. All 191 Linux xtask tests and all-target Clippy pass in `linux-final-freshness-xtask-architecture.txt` in the help evidence root. |
| Structural and document checks | Final Linux architecture check is STRUCTURAL PASS, explicitly not dependency admission. Log: `linux-architecture-final.txt` in the help evidence root. Pinned `cargo fmt --all -- --check`, `git diff --check`, and the owner document checker over all 88 tracked/new live Markdown files pass; document log: `documentation-final.txt`. |

The ordinary publication/namespace feature partition emits four existing
unused support-helper warnings, also present in W6. Its passing tests are not
a warning-free Clippy claim for that partition. Initial diagnostic fixture
failures and the failed pre-fix architecture log are retained separately, not
counted as passes. The fixture issue was the ordinary test build replacing
`debug/lumin`; preparation now uses a separately copied, hash-bound fault
binary. Both final public-child probes pass without a fault-feature fallback.
One Linux shell handoff appended a carriage return to the final argument.
Its malformed `-D warnings` and `architecture-check` turns are not passing
evidence; their retained logs still contain the earlier successful tests.
The separately named final lint and structural logs use direct native argument
handoff, pass, and contain no such malformed selector.

## Remaining authority and measurement

The fourteen-cell runner requires an exact clean source checkout. An uncommitted
implementation cannot supply that build binding; local helper/transport checks
must not be relabelled a complete diagnostic packet. In particular, the local
public-child fixtures do not substitute for the runner's full 256-tuple oracle,
fixed two-conditioning/twelve-measured schedule, process-lifetime receipts or
archived hosted observations.

The authorized publication and clean-checkout fourteen-cell measurement are
complete; the exact packet is retained in [CI-EVIDENCE.md](CI-EVIDENCE.md).
The feature-off test adapter correction has passed local ordinary/W7 and exact
workspace Clippy checks, but still requires its clean hosted verification.
Numeric-budget relaxation and product optimization are not authorized.
P1-60 and P1-70 remain open; no
four-worker performance conclusion, permanent-metric completion or merge
authority follows from these diagnostic checks.
