# W5 Initial Hosted Evidence

Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
Run: [34126574222, attempt 1](https://github.com/annyeong844/lumin_Refactor/actions/runs/34126574222).
Head: `4ec0af7d3923f93cb23cd8c8f3121121b3bad039`.
Merge checkout: `12bc25042407e34a9a1aa766c81930f13514ecc8`.

## Verdict and distinct failures

The run completed with 49 successful jobs and 8 failures. Both clean release
builds, staged platform probes, and staged adapter probes passed. Windows and
native Linux completed their ordinary benchmark matrices, but each failed the
unchanged `0.75` cold default/jobs=1 ratio. Required failed accordingly.

All five Windows store-unit partitions also failed before executing their
selected tests. Their closed inventory omitted the new top-level publication
unit module. The actual failure was:

```text
[TEST SHARD ERROR] store library test matches 0 module filters: publication::latest::tests::changed_index_commits_both_exact_fields_once_and_preserves_other_rows
```

The prior local full-library run did execute these tests, but did not exercise
the CI-only shard inventory. Neither result substitutes for the other. The
[routing correction](IMPLEMENTATION.md) retains the exact inventory check and
adds the missing executable partition; it does not waive the performance misses
or change the measured product source. A new hosted run is still required.

## Ordinary measurement results

Both matrices retain 34 completed cells and 21 measured samples, with three
samples per mode. All seven medians were independently recomputed. Every
measured public process exits zero without an analysis child, and every sample
retains the exact 256-finding truth-map SHA-256
`93b1b99df26f8b7a5872425cc84f0f2dcb36e5af65f53eb37756e32a8532c523`.
The unchanged fixture has 780 files and 7,461,511 bytes; its content, manifest
and truth identities match the [local packet](MEASUREMENTS.md).

| Metric | Windows x64 | Native Linux x64 |
| --- | ---: | ---: |
| Default worker policy | 4 | 4 |
| Cold audit default median, ns | 1,735,140,200 | 919,151,436 |
| Cold audit jobs=1 median, ns | 1,980,378,000 | 551,593,899 |
| Default/jobs=1 ratio | 0.8761661662571489 | 1.666355334361666 |
| Frozen ratio maximum | 0.75 | 0.75 |
| Executable bytes | 10,498,048 | 11,938,544 |
| Peak process RSS bytes | 66,588,672 | 147,369,984 |

Both reports pass all absolute time, executable-size and RSS targets. Their only
numeric miss is scaling. No sample is removed, rerun to green, or reclassified
as noise. These are separate hosted measurements, not controlled before/after
evidence of a W5 speedup or slowdown. The Windows diagnostic completes 14 cells
with `DIAGNOSTIC_ONLY` status; it has no numeric budget authority.

Both ordinary packages carry build ID
`build_891d5b65f288a606088645df26f133c9948cbc3ca092f5e645917341176c3128`.
Windows executable SHA-256 is
`c77b480551fe253dc50b0812ede9e65bb02c7f3d5100b19216014f0dcb0bd6cf`;
Linux executable SHA-256 is
`3779234054940833a0244e4936b7c17c9fa9d8a29da5b961ef9e586870738657`.

## Retained raw evidence

Downloaded artifacts remain under `D:\lumin-w5-ci-34126574222`, in `windows`,
`linux`, and `diagnostic`. Every ordinary capture's byte count and SHA-256,
plus the exact ordinary file inventory, were checked: 745 Windows and 711 Linux
capture files. All 379 diagnostic capture hashes also match its complete
14-cell terminal manifest. The terminal `manifest.json`, not the initial
`inventory.json`, owns completion.

| Packet | Report SHA-256 | Terminal manifest SHA-256 |
| --- | --- | --- |
| Windows ordinary | `ced259470cf5c76a84df14636a49c3049b3442b912faf0769e6a80e8b17f17d5` | `1e5c7d971e608eb9ad0b084b36a86316d758e0d935011b2092ab9a4c1327e1ed` |
| Linux ordinary | `6a8a97b7edaeb1494d142456cee5bfd732a1dfba78830bc037e45c0f1473ed74` | `881f0535f2fa7d4d719399415981daf9d26d7a4d7dd73aee424669f3bffa9c8c` |
| Windows diagnostic | `9c63843607539913ccaf093f57552970975df6525881e64f766a1726172e2bbd` | `73bf8340b44978d28606f9dcc834d46954d2f4e336f4aa5a5e22298978464390` |

The diagnostic build-record SHA-256 is
`8702e89b2414fb28d2bcecf4b5d1f049c647447b914317409684b807387c1ddb`.
P1-60/P1-70 remain open; no draft/merge action or budget relaxation follows from
package correctness or diagnostic completion.
