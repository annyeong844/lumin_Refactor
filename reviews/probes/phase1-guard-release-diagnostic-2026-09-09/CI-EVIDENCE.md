# W9 Hosted Evidence and Structural-Policy Correction

Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
Authority: [W9 review](REVIEW.md#authority); the frozen [design](DESIGN.md)
and all numeric budgets remain unchanged.

## Completed hosted run

[Run 34481869325](https://github.com/annyeong844/lumin_Refactor/actions/runs/34481869325)
completed on 2026-09-10 with **47 successful and 11 failed jobs**. Its first and
only attempt used PR head `90e67d5e2e4a437ac1d6178b4f992b225fb7d74e`; the
diagnostic build record binds merge checkout
`1354fc273e8883d0ba577a0abb56f654b60b99fc`. Overall CI and Required fail.
No run was cancelled, retried or reclassified.

The [Windows package job](https://github.com/annyeong844/lumin_Refactor/actions/runs/34481869325/job/102886547456)
and [Linux package job](https://github.com/annyeong844/lumin_Refactor/actions/runs/34481869325/job/102886547606)
pass their actual staged binary/adapter probes and unchanged ordinary benchmark.
Windows also passes the isolated W9 build, actual public-child probe and
diagnostic capture. Both ordinary reports return numeric `PASS` with no target
misses for these samples:

| Ordinary environment | Cold default median | Cold jobs=1 median | Ratio, maximum 0.75 |
| --- | ---: | ---: | ---: |
| Windows NTFS, four workers | 1,422,572,300 ns | 2,099,897,300 ns | 0.6774485114105342 |
| Linux release, four workers | 298,755,572 ns | 574,573,314 ns | 0.5199607512575846 |

This records the hosted reports, not an independently reconstructed performance
verdict or a causal improvement over W8. Earlier misses remain valid evidence.
The separate W9 report remains `DIAGNOSTIC_ONLY`; its counterbalanced control
ratio is `1.2063708172011771` and diagnostic ratio `0.9962965366186889`.
Neither replaces the ordinary benchmark or establishes an optimization benefit.
Permanent runtime worker/stage evidence, allocator approval and the WSL `/mnt`
disposition remain open, as does Phase 1 exit.

## Failure and corrective scope

The workflow moved to the frozen W9 diagnostic, but
`tools/xtask/src/cargo_bootstrap.rs` still required W7's complete shell body,
commands, feature closure and artifact paths. The structural checker therefore
rejected the workflow. This omission also failed the same two xtask tests on
Windows/Linux and the five corpus rows that share `architecture-check`:
`resolver-config-registry-artifact`, `pnpm-workspace-registry-and-precedence`,
`limitation-scope-exhaustiveness`, `capability-availability-authority` and
`gate-lifecycle-effects`. Their structural failure propagated through seven
corpus jobs and Required. The complete failed-job logs contain no separate
product-test failure.

The pre-publication Python and inverse-workflow checks did not cover this stale
Rust policy. Earlier local structural PASS predates the workflow edit and is
not evidence that the published workflow passed that check. The omission is
corrected in that policy only; no workflow, product, dependency, lockfile,
feature activation, lifetime or budget is changed. Exact-body matching remains
indivisible, with no new general shell-command allowance. Regression tests
reject complete predecessor wiring and missing/extra features in every one of
the five owner closures. [Local correction evidence](IMPLEMENTATION.md#hosted-structural-policy-correction)
is separate from the next hosted verdict.

## Retained evidence

Downloaded artifacts remain under
`D:/lumin-w9-boundary-diagnostic-20260910/hosted-34481869325/`.
All 745 Windows ordinary, 711 Linux ordinary and 379 diagnostic captures match
their manifest sizes and SHA-256 values; the physical file inventories contain
no unlisted captures. The manifests mark 34/34/14 complete cells respectively.
This read-only check authenticates archive integrity, not full semantic truth
or numeric acceptance. Its receipt is
`ci-policy-fix/hosted-archive-integrity-r1.json`, SHA-256
`3b1b65eda58d1ab2d88ff703b7cce078352ba134bc7e305428f6870fec702591`.

| Artifact | SHA-256 |
| --- | --- |
| Windows ordinary report | `52f7acc5eec1b030322105521b8820d51cea8a9b1b41f6943d3807b36b74df6b` |
| Linux ordinary report | `1d310ec5093cf96af5219c6d373521c0a9109b27d9bca26ec1492b43b72e1b2e` |
| W9 diagnostic report | `d4033361580c5a7c5f6f55802abe98599f00cabde46f7b996c65053b92463ecd` |
| W9 diagnostic build record | `02a70526124da281915176d44103488156cc4a43435a26967c1b5c464d712217` |
| Complete failed-job log | `a41ff28634adea55fd58542b91ab325f0147602115011c142c13591c9c1fb3df` |

The completed job inventory is retained in
`ci-policy-fix/ci-34481869325-completed.json`. PR #135 remains Draft. The owner
approved the narrow policy correction and one new-head CI run only after this
failed run completed; no same-head rerun, automatic merge or local cold
benchmark is authorized. Local correction checks cannot establish that next
hosted result.
