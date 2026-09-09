# W8 Empty Latest-Index Implementation Evidence

Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
Authority: the exact frozen [design](DESIGN.md), SHA-256
`14d0fc5df67dc2fe7a2ffdea2020b1a277b4fcd268e7ad43e74ea9954345623b`,
and [owner approval](REVIEW.md#authority-and-closeout).
Source base: `4f03cc1f8a25e83265a81c0fb1b2c604ce6ee8bf`.

## Current disposition

Implementation and independent scoped source review pass. Both platforms pass
all eight new public boundary cases and the complete 15-test W5/W6/W8 namespace
replacement target. Scoped ordinary/diagnostic regression and lint checks pass.
The actual Windows/native Linux packages, adapters and three external
release-child probes per platform also pass. All four fresh ordinary
control/candidate packets pass their local numeric targets and complete capture
verification. Their [mixed timings](MEASUREMENTS.md#results-and-interpretation)
do not establish a causal speedup or resolve hosted scaling. The authorized
scoped local verification is complete; P1-60/P1-70 remain open. A separate
[publication approval](REVIEW.md#publication-approval) now permits committing
and pushing W8 to Draft PR #135 and checking one resulting public CI run.
PR-ready, merge, further reruns and broader optimization remain unapproved.

Local verification has resumed in small serial groups as host memory headroom
returned, with the owner's browser and other applications left open. Each new
command checks headroom and uses one build job; the 3 GiB launcher preflight is
a local resource-safety choice, not a product requirement or relaxed budget.
`memory-pause.json` and `ubuntu-restart.json` retain the earlier pause and
owner-authorized Ubuntu-only restart. The remaining Linux xtask/W7 checks now
pass. Both staged release/probe groups pass after resuming only the unstarted
steps at two further Linux preflights; no application closure or further WSL
restart was needed. All four declared measurement packets then completed once,
in the declared order, without concurrent builds or tests.
No unfinished partition is counted as passed, and completed partitions are not
rerun merely to resume the launcher.

## Implemented boundary

Only an absent canonical latest document plus the entire absent/empty POINTERS
table can omit the second backend/write/unchanged-abort lifetime. Pending-first
recovery, canonical-present sync, nonempty pointer linkage and preexisting
attempt recovery retain their owner paths. Unknown keys, malformed rows and
failed reads never become an empty witness.

The original backend holds the complete read result through the shared W6
finishing proof. Table/read borrows end first. The original guard's authority,
bound namespace, held store, generation and validation receipts are checked on
both sides of the final turn; failed opens or finishing proofs poison subsequent
backend admission before outer-lock teardown. An ordinary read failure with a
successful proof retains its original error without poisoning that guard.

Empty success checks canonical and pending absence through the held parent
before and after the final barrier. Windows case aliases and opaque non-ASCII
direct names are rejected, preserved and never adopted. Resource destruction
order, W6 session-failure continuation and locks during transport are unchanged.

W7 keeps the v3 shape and thirteen cost meanings. Only the two eligible fresh
rows change from `2/1/1/0/1/0/2/0/0/0` to `1/1/0/0/0/0/1/0/0/0` in the frozen
ten-cost order. The strict candidate oracle rejects the old fresh vector;
healthy and pending fixtures retain their prior vectors. No prior packet is
reinterpreted. Actual ordinary/W7 release-child checks confirm these vectors
on both platforms, as recorded below.

## Independent adversarial review

The existing read-only `latest_index_noop_review` reviewer returned scoped PASS
after verifying the exact frozen design and final source. It required three
corrections before PASS:

- The Windows case-alias fixture first reproduced an incorrectly successful
  audit for a valid `LATEST.JSON` arrival. The corrected held-parent absence
  check rejects uppercase canonical and pending arrivals at both entry points.
- Public generation/receipt failures are injected after the corresponding real
  checks in the second finishing proof, not as a generic barrier error. W6 and
  the first proof cannot select them. The separate owner test actually changes
  generation/receipt fields through the original backend and verifies rejection.
- The foreign store is independently admitted, then refreshed from and compared
  byte-for-byte with the canonical source immediately before child launch;
  admitting it cannot leave the substitution fixture with modified backend bytes.

The reviewer inspected the retained Windows RED and final 8/8 logs, found no
remaining material source/design-conformance blocker, and performed no edits,
builds, tests or further delegation. That source PASS does not by itself
establish matrix, package or performance evidence.

The same reviewer subsequently returned scoped evidence/claims PASS for the
completed local packet. It independently checked report/manifest and candidate
binary hashes and compared the full semantic mappings; exhaustive capture
hashing relies on the retained separate verification records. Package/public
child logs and the mixed-timing interpretation agree with the stated claims.
It caught the missing zero-output formatting log; the explicit pinned-command
and exit-status artifact in `windows-format-final-r3.txt` resolves that gap.
The review reran no builds, tests or measurements and grants no hosted,
performance-causality or publication authority.

## Public and owner evidence

Raw files are retained externally at
`D:/lumin-w8-empty-latest-20260909/`. Windows is the actual host; Linux is native
WSL ext4 for build output and runtime fixtures, not a timed `/mnt` repository.
Cargo is pinned to Rust 1.96.0, uses the checked source-provenance launcher,
`--locked` and one build job. Tests/builds on the two platforms run serially.

| Evidence | Result and retained log |
| --- | --- |
| Windows case-alias negative control | Incorrect audit success reproduced before the fix; `windows-case-alias-red.txt` remains RED evidence. |
| New public faults/recovery/ordering | Windows 8/8 and Linux 8/8 PASS, including all six actual store substitutions on each platform; `windows-public-empty-r3.txt`, `linux-public-empty-r2.txt`. |
| W5/W6/W8 full replacement regression | Windows 15/15 and Linux 15/15 PASS; `windows-namespace-final.txt`, `linux-namespace-final.txt`. Linux executes the freshly compiled target as root for its required mount controls; Windows exercises distinct C:/D: volumes. |
| Ordinary latest owner tests | Windows 19/19 and Linux 19/19 PASS; `windows-owner-latest-r2.txt`, `linux-owner-latest-r4.txt`. |
| Ordinary affected libraries | Windows engine 55, model 27, protocol 20 and store 295 PASS (397 tests); Linux engine 57, model 27, protocol 20 and store 304 PASS (408 tests). Logs: `windows-ordinary-affected-libraries.txt`, `linux-ordinary-affected-libraries.txt`. |
| Ordinary lint | Both platforms' full-workspace/all-target and exact namespace fault-target Clippy PASS with `-D warnings`; `windows-workspace-clippy-r1.txt`, `windows-namespace-clippy-r1.txt`, `linux-workspace-clippy.txt`, `linux-namespace-clippy.txt`. |
| Public publication and recovery | Both platforms' publication 1, concurrency 2, fault 8 and publication/retention race 5 PASS; `windows-publication-regressions.txt`, `linux-publication-regressions.txt`. Each combined test partition emits four existing unused support-helper warnings; this is not its warnings-denied Clippy verdict. |
| Public migration | Windows 13/13 PASS, including old-generation rejection and every process-death boundary; `windows-migration-regressions.txt`. Both separately publication-feature-gated pointer fixtures also pass, one executed test each, in `windows-public_migration_recovers_after_latest_replace_before_index_sync.txt` and `windows-public_migration_accepts_the_exact_catalog_before_latest_frontier.txt`. Linux runs the explicit lifecycle/publication feature union: 14/14 PASS in `linux-migration-regressions.txt`, including both pointer fixtures. |
| W7 diagnostic owners and lint | Windows model/engine/store `audit_` 26/26, store `latest` 25/25 and protocol 2/2 PASS; Linux corresponding selections 27/27, 25/25 and 2/2 PASS. Logs: `windows-w7-owner-tests-r2.txt`, `windows-w7-latest-tests-r2.txt`, `windows-w7-protocol-tests.txt`, `linux-w7-owner-tests.txt`, `linux-w7-latest-tests.txt`, `linux-w7-protocol-tests.txt`. Both all-target diagnostic-owner Clippy runs PASS with `-D warnings`: `windows-w7-owner-clippy-r2.txt`, `linux-w7-owner-clippy.txt`. |
| Registry and strict diagnostic runner | Windows 193/193 and Linux 192/192 xtask tests PASS, including the complete ordered mappings and candidate rejection of the old fresh vector; `windows-xtask-r2.txt`, `linux-xtask.txt`. |
| Final Rust formatting | Pinned Rust 1.96.0 `cargo fmt --all --check` PASS via the direct command used by CI; `windows-format-final-r3.txt` explicitly retains the command, toolchain and zero exit status. The earlier source-provenance launcher rejects `fmt` as outside its admitted build commands; that setup rejection remains in `windows-format-final.txt`, not a source-formatting failure. |
| Actual distributions and W7 release children | Both platforms' ordinary release, fixture and separate W7 release builds PASS; staged `windows-x64`/`linux-x64` and both skill adapters PASS in `<platform>-package-{stage,platform,skills}.txt`. Each platform's three external release-child tests and their warnings-denied Clippy PASS in `<platform>-public-lifecycle.txt` and `<platform>-public-lifecycle-clippy.txt`. Linux tests the static musl release shape. These are not a new fourteen-cell W7 hosted packet. |
| Structural architecture boundary | `architecture-final.txt` reports STRUCTURAL PASS through the fresh xtask executable. Dependency admission remains the separate checked launcher/CI authority, not this structural command. |
| Fresh ordinary control/candidate packets | Four numeric PASS results, 84 measured samples, 136 conditioning/setup/measured cells and 2,912 hashed captures verify. All four complete 256-entry semantic mappings are identical. [Measurements](MEASUREMENTS.md) binds every report, manifest, package and limitation; no selective rerun was used. |
| Source and document closeout | All 17 changed Rust files match the five completed pre/post bindings in `source-bindings-final.txt`; HEAD and the frozen design remain unchanged. Eight changed/new Markdown documents and 73 local link targets pass, with anchor slugs source-reviewed separately; tracked/new whitespace checks pass. No Cargo, CI or Workboard edits, and the original user worktree was not edited. |

Public proofs compare full canonical logical observations, physical identities,
recursive payloads and preserved foreign objects. Snapshots at a held-reader
barrier come from that original backend, never another writable observer.
Generation/receipt public error injection is composed with the actual-corruption
owner proof; it is not described as an external process rewriting a live redb
header. The writer/migration waiter acknowledges actual failed exclusive-lock
acquisition before the designated release. Recovery, no-new-allocation and
stable query/migration retries are checked separately from public success bytes.

All eight new public cases are mapped in the existing lock-replacement corpus
row's ordinary/determinism/crash lanes, and the parent-binding case is also in
the managed-parent row. Complete ordered mapping assertions are updated; no
previous required invocation is removed. Both affected rows pass their actual
Windows standard, determinism and store-crash invocations (six row/mode
combinations), distinct from the registry unit proof. Determinism retains 55
semantic captures for lock replacement and 34 for managed-parent replacement.
Logs are `windows-corpus-<mode>-<row>.txt` under the external evidence root;
this does not claim a Linux corpus-runner invocation.

## External Rust lifecycle evidence

Three matching external legacy-lab pairs cover all 17 changed Rust files:

| Scope | Pre-write invocation | Post-write invocation |
| --- | --- | --- |
| Main implementation, 15 files | `2026-09-08T17-11-58-723Z-05372b` | `2026-09-08T18-04-17-498Z-e6ede3` |
| Exact contended-lock ordering, one file | `2026-09-08T17-31-44-517Z-2d1d5c` | `2026-09-08T18-04-20-444Z-18d4be` |
| Existing complete snapshot helper visibility, one file | `2026-09-08T17-33-25-069Z-736d68` | `2026-09-08T18-04-23-237Z-e76d25` |

Artifacts are in `implementation/`, `ordering/` and `shared-snapshot/` under
the external evidence root. Every planned file was observed, the two new files
were planned, and no unexpected file or parse error was reported. These are
Rust lifecycle/file-delta observations, not a refreshed full base audit or a
whole-repository absence/quality claim. The sidecar's reported capability scope
is retained without treating unavailable TypeScript lanes as Rust proof.
`post-source-hashes.json` binds all 17 post-checked files. Any subsequent Rust
edit requires a new matching pre/post pair and affected verification.

The W7 model run exposed two stale aggregate expectations in its test-only
fresh-vector sum, retained as a failure in `windows-w7-owner-tests.txt`.
The individual vectors already matched the frozen design. The correction uses
the independently authored W8 totals (15 opens, 2 writes, zero aborts and 7
return tails), with no production or frame-shape change. It has a separate
pre-write `2026-09-08T18-30-02-730Z-d92164` and post-write
`2026-09-08T18-31-57-953Z-d4c99c` under `aggregate-test/`, following a passing
3/3 focused model run. The one planned file was observed without new files or
reported parse errors. Its final model-file SHA-256 is
`384c8d8414a565be14fbc4a862d27cb68858fdd66491ebc934ab600931c7d0ec`,
superseding only that entry in the earlier 17-file hash record.
The independent reviewer verified this exact model hash and unchanged design
before/after a read-only follow-up, confirmed the totals against all eight
authored vectors, and retained the scoped source PASS: no validation weakening
or production change. The resumed Windows model/engine/store `audit_` selection
passes all 26 tests in `windows-w7-owner-tests-r2.txt`.

The W7 Clippy run then identified a needless reborrow at the profiler's final
use in `read_derived_latest`. Moving that optional reference directly changes
neither the profiler/backend ownership nor validation, counts or destruction
order. The independent reviewer confirmed scoped PASS and the unchanged design
hash. The separate `profiler-final-use/` pair is pre-write
`2026-09-08T18-38-54-898Z-623191` and post-write
`2026-09-08T18-40-51-052Z-98a724`, with the one planned file observed and no
unexpected file or reported parse error. Its 25/25 focused W7 latest tests pass
in `windows-w7-latest-tests-r2.txt`; the original Clippy failure remains in
`windows-w7-owner-clippy.txt`. Final `latest.rs` SHA-256 is
`0928e6e4750e9a7a0b5df6c739a7e555ffa9b72baf14aae148c56bdea98dca66`,
superseding only that file's entry in the earlier hash record.
