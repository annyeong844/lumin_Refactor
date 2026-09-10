# W2 Actual Windows CI Evidence

Measurement: 2026-09-06 UTC, [CI run 34010008338, attempt 1](https://github.com/annyeong844/lumin_Refactor/actions/runs/34010008338).
Scope: the frozen [W2 diagnostic](DESIGN.md), not permanent metrics or Phase 1 exit.

## Tested identity and verdict

PR head: `062964192a1d3f16f5b7739f0d98eb85ac5bef4d`.
Actual merge checkout: `e126a73d5af15d77e9bfd454fd431e9b4ead6f89`.
Lockfile SHA-256: `c82640e677c8c602c90f9ee8577a4286d52e65e621636067bf59f8b0257185fb`.
Public build scope: `build_4a9dcf5e038fa238182a13c613af92eab60d4b8c1bb1862e0ba9d2c264e5c187`.

The isolated Windows diagnostic build, eight feature-enabled unit tests,
three actual release-child transport tests, and complete W2 packet pass.
The ordinary Windows benchmark fails its scaling criterion; the package job
and `Required` remain failed. The other 55 jobs, including Linux packaging,
pass. Diagnostic success does not override that failure.

| Ordinary release measurement | Windows, four default workers | Linux, four default workers |
| --- | ---: | ---: |
| Cold default median (ns) | 3,116,713,800 | 319,440,909 |
| Cold jobs=1 median (ns) | 3,169,005,900 | 475,386,529 |
| Default/one ratio | 0.9834988947164788 | 0.671960372272139 |
| Unchanged ratio limit | 0.75 | 0.75 |
| Numeric verdict | FAIL | PASS |

Other ordinary Windows time, RSS, size, and semantic criteria pass. Cold
means a fresh process/repository/state, not flushed machine caches. These
results do not authorize a comparison with a different CI host or checkout.

## Measured diagnostic, with limits

The report is `lumin.phase1-cold-audit-diagnostic.v1`, `DIAGNOSTIC_ONLY`, with
`numericBudgetVerdict: null`. Both conditioning and all twelve measured cells
complete. The seven feature-built children return the exact 23 phases,
observer-matched PIDs, matching build/run/attempt identities, four observed
available processors, and actual one/default-four workers. All fourteen full
256-finding truth maps agree; tuple-to-ID map SHA-256 is
`93b1b99df26f8b7a5872425cc84f0f2dcb36e5af65f53eb37756e32a8532c523`.

| Instrumented phase median (ns) | jobs=1 | Default |
| --- | ---: | ---: |
| Command | 3,214,975,800 | 2,958,408,700 |
| Store open | 386,368,600 | 379,116,600 |
| Attempt begin | 694,449,300 | 751,113,400 |
| Inventory | 315,187,200 | 255,673,000 |
| Extraction | 157,802,700 | 74,048,700 |
| Store publication, inclusive | 1,497,181,600 | 1,447,569,200 |
| Final inputs, inside publication | 323,446,700 | 234,545,700 |
| Store-publication self residual | 1,176,111,100 | 1,213,023,500 |

These medians are not an additive timeline. Within each actual default
sample, the disjoint store-open, attempt-begin, and store-publication self
intervals sum to 79.3776%, 79.4050%, and 79.2493% of command duration.
Those three owner regions warrant finer measurement. This does not prove
backend flush, lock wait, antivirus, or any specific system call is the cause.

The packet's control medians are 3,212,515,400 ns (one) and 2,898,349,500 ns
(default); diagnostic medians are 3,222,521,900 and 2,965,369,100 ns.
Per-round diagnostic-minus-control differences have mixed signs: one,
-396,474,800 / +48,787,600 / -124,195,200 ns; default,
-186,169,900 / +122,278,900 / +39,705,400 ns. Neither a stable probe overhead
nor a budget correction can be inferred from differences of these medians.

## Retained raw packet

All downloaded capture-manifest entries were independently checked by size
and SHA-256; no unlisted capture files were present. Each ordinary archive
has 34 completed cells / 711 files; W2 has 14 / 365. Before/after executable
hashes match. The CI artifacts have the workflow's 90-day retention:

| Artifact name / ID | Report SHA-256 | Capture-manifest SHA-256 |
| --- | --- | --- |
| `lumin-foundation-benchmark-windows-x64` / `9982397456` | `1414676bf8d7f477b5fbba6a4d8f084c80d030956a9ae76e7f634f4535c78575` | `c05412ade8fac51210da04baab1e68125bdc07eea2016a0011090fc295911ed6` |
| `lumin-foundation-benchmark-linux-x64` / `9982216642` | `6a8ccefd72ea3310164d4323773a98f26d78cf3caee54b2cba52b07cf3ae1f20` | `18155b287241f86dd036a8ac7352679c42831e7d676be8d11d175d9beddaedad` |
| `lumin-audit-diagnostic-windows-x64` / `9982397771` | `6a15a47b1092f52d59c50ff8defb047cee1f0efc2df7c3d7fe43f0fa7ee1e138` | `2da920c320ad4a3e86154bc81eaa8a2b3c1145a99aa25390a53b5475c3d5772a` |

Windows control SHA-256: `5572ecf6945280e08b37d83650be7f917cff3ee871346153ef05cfdd173916e0`.
Windows diagnostic SHA-256: `e4e28bb4eeb764fcac9d6a76edadc40bd9e6498fe9af9a10fb4fc4e02b7f8e48`.
Linux control SHA-256: `22c2a308616776fdc5f7eb58f211b850496d462ee4dfea385fde45a671f585dc`.
Diagnostic build-record SHA-256: `c5ceeec02db4dab161f379008d808d81ea070ae5a0c4dfdd1cf724d7e0dbe3e1`.

Preserved external copies are under `D:\lumin-w2-ci-windows-34010008338`,
`D:\lumin-w2-ci-linux-34010008338`, and `D:\lumin-w2-ci-diagnostic-34010008338`.
These locations are retrieval hints; hashes and CI identities bind the packet.
Expired or unavailable raw captures must be reported as unavailable, not
reconstructed from this summary. W2's frozen design is unchanged.
