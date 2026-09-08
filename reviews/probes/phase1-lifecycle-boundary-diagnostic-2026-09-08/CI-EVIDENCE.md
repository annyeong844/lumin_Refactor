# W7 Hosted Diagnostic Evidence

Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
Status: **complete diagnostic observation; Windows scaling FAIL; no Phase 1 exit**.

## Exact execution and separate failures

The authorized W7 head `945f373a65d6fe79fba98c5dc4e535d3b8c045ee`
ran once in [CI 34238854239](https://github.com/annyeong844/lumin_Refactor/actions/runs/34238854239),
created at `2026-09-08T14:31:54Z`. The actual merge checkout was
`43193d65a431a8a91470a412302fc9233052e314`, with parents main
`09ab415546890241de3434b102fde90ee650b214` and that W7 head.
All 58 jobs completed: 54 passed and four failed.

- Linux `102103701217` and Windows lint `102103701406` both failed ordinary
  store-test Clippy on the same redundant feature-off callback. The
  [test-only correction and its separate review/checks](IMPLEMENTATION.md#hosted-feature-off-lint-correction)
  do not alter production execution. Later steps skipped by these lint failures
  are not passing test evidence.
- Windows package `102103701455` failed its ordinary benchmark's scaling
  target. Its actual staged binary and both skill-adapter behavioral probes,
  isolated W7 build, 25 owner tests, incompatible-feature rejection, three
  public-child tests and complete diagnostic runner passed. Linux package
  `102103701385`, including its benchmark, passed.
- Dependent Required `102110858667` failed. This is not a full-CI or merge PASS.

This first run was not cancelled or rerun. The lint correction was held until
the complete measurement finished; its next source run is verification of a
real fix, not a replacement for this retained result. No budget, default-worker,
optimization, PR-ready or merge action follows from this packet.

## Ordinary budget authority

Both seven-mode reports contain all 34 expected cells, including 21 measured
samples. Both hosts report four available logical processors, with no explicit
`--jobs` on default commands. Ordinary worker fields remain derived policy
observations, not permanent pool-produced metrics.

| Measurement | Windows NTFS | Native Linux | Requirement |
| --- | ---: | ---: | --- |
| Cold default median | 1,651,152,300 ns | 346,400,442 ns | <= 30,000 ms |
| Cold jobs=1 median | 2,035,331,300 ns | 512,947,850 ns | Scaling control |
| Default/jobs=1 ratio | **0.8112449801169962** | 0.675313176573408 | <= 0.75 |
| Peak RSS | 66,764,800 B | 150,323,200 B | <= 536,870,912 B |
| Packaged executable | 10,503,168 B | 11,942,640 B | <= 12,582,912 B |

Every other Windows numeric budget and every Linux budget passes. The Windows
failure is a valid numeric miss, not missing or invalid observations. The shared
build ID is
`build_5a34cfbe11acb32e196d5f6def998d2e663d99b691acc2908110072f323a4c6c`.
Windows binary SHA-256 is
`ff8f70c5a886820328bc809b80adc9bea6e0314c221eaa1587baf23a367bc5ab`;
Linux is `bc9b3a04839c5c461cf67686e9a2d60fdaa7dfe2f16e167710ba52c0f4475fd8`.

W6 and W7 ran on separate hosted executions with sample variance. The changed
ratios do not establish a controlled speedup, especially because W7 changes
diagnostic instrumentation rather than the ordinary product's resource policy.

## What the eight contexts establish

All fourteen scheduled cells completed: two conditioning cells and twelve
measured cells. The separate report is `lumin.phase1-cold-audit-diagnostic.v3`,
status `DIAGNOSTIC_ONLY`, with `numericBudgetVerdict: null`. The diagnostic
binary SHA-256 is
`60653b20a85f81995df5a16014ac4a2670c81b38c68a231c35ed6fd8ee56c856`;
its build record binds the same merge checkout and ordinary control binary.

Every diagnostic frame has the exact ordered eight contexts and thirteen costs,
the independently frozen fresh vectors, and checked disjoint residuals. In the
selected contexts each fresh audit has 17 backend opens, eight explicit drops,
nine natural return tails, five read admissions, four write admissions, two
commits, two aborts, three JSON write/flush calls, four moves and six directory
sync API calls. These are scoped counts, not the whole audit's backend inventory.

The three measured default samples, in their fixed round order:

| Elapsed boundary | Round 1 ns | Round 2 ns | Round 3 ns |
| --- | ---: | ---: | ---: |
| Engine command | 1,585,701,100 | 2,018,897,400 | 1,721,166,900 |
| Eight selected contexts | 398,629,200 | 451,946,300 | 484,605,100 |
| Backend-open expressions | 64,944,400 | 79,335,100 | 63,293,900 |
| Existing explicit database drops | 106,123,900 | 121,312,600 | 196,029,800 |
| Natural database-owning return tails | 124,452,500 | 138,239,600 | 118,952,000 |
| Unassigned context residual | 25,348,800 | 26,766,000 | 28,242,000 |

Within each sample, the three disjoint backend/open-and-release boundaries sum
to 74.13%, 74.98% and 78.06% of the selected contexts, or 18.64%, 16.79% and
21.98% of the complete engine command. This narrows the next source-level
investigation to repeated backend construction and release on these paths;
it does not prove that any open or freshness fence can safely be removed.
The broader store-owned engine regions remain 62.16%, 70.78% and 65.96% of
the command, so much store-owned work is outside these eight contexts.

Return tails include all local teardown and return overhead, not only redb
destruction. Explicit drops and backend-open expressions are elapsed owner
boundaries, not kernel lock-wait or device-flush observations. Windows directory
sync is the existing no-op owner API. W7, W3 and engine views overlap and must
not be added; medians are not additive timelines.

The fixed schedule's control/diagnostic medians are respectively 1,884,807,100 /
1,891,904,600 ns for jobs=1 and 1,867,103,500 / 1,728,475,300 ns for default.
The latter apparent negative overhead (-7.42%, versus +0.38% at jobs=1)
demonstrates uncertainty, not free instrumentation or a causal speedup. No
diagnostic duration is subtracted from the ordinary budget measurements. Further
product optimization requires a separately reviewed and owner-approved design.

## Raw provenance and independent verification

The complete artifacts, job logs and read-only verifier outputs remain outside
the repository in `D:/lumin-w7-ci-34238854239/`. Artifact IDs are Windows
`10061935476`, Linux `10061276812`, and diagnostic `10061937731`.
`run-summary.json` retains all job IDs/results; `artifacts.json` retains the
three-artifact inventory. Logs retain both lint failures and both package jobs.

All 1,835 capture files match their manifest lengths and hashes: Windows 745
files/34 cells, Linux 711/34, diagnostic 379/14. Independent verification checks
the exact file inventory, raw process observations, Windows inherited-job
lifetime receipts, source/build/PID/worker bindings, complete sample order,
ordinary and diagnostic medians, and the authored 256-finding tuple-to-ID map.
The recomputed map SHA-256 is
`93b1b99df26f8b7a5872425cc84f0f2dcb36e5af65f53eb37756e32a8532c523`.
W7 additionally checks every raw v3 frame's hash and wire field order, every
fresh count vector, zero/null shape, exact residual and containing W3 interval.

| Retained record | SHA-256 |
| --- | --- |
| Windows report | `d855b9b890b4c8201419cd2a56751972f4aa5cc74f8e433b93cedba96645d457` |
| Windows manifest | `266202ffd1edf2cd79eb2f33d0a23549c8d03d4bc0f278a7cde29e7248efcc6b` |
| Linux report | `1d0eeda8cccc76c793255213169f28ed783374051609fd4441ada0265bb2405b` |
| Linux manifest | `7de88fb74de40baa02a0e3b7cbe95ce100c74b7f8ae66eda186ccd2e7c855f7c` |
| Diagnostic report | `747aa6ab0d54c804a5c5467074750b380d3175312ff1700a99fdcded1ba3c246` |
| Diagnostic manifest | `9df333e0ed1c645df957da7583bea2bb04d96e185d94afa986a87000f24913d8` |
| Diagnostic build record | `8cf9c6662608feae0fbeeee9f358c4bba1a620e74ed694e1c3c30049a83d7bb7` |
| Reused ordinary verifier | `b264ed6767d870311233f585b84fa83454374e2014754b2a6d8aeaa0d117e510` |
| W7 verifier | `bec10e326a65604399a5852194cb1cadac011b02698fb8759fae4be69b85808b` |

The final verifier outputs are `windows-independent-verification.json`,
`linux-independent-verification.json`, and
`diagnostic-independent-verification-final.json`. The initial diagnostic verifier
attempt is retained separately: it incorrectly compared the ordered wire object
with the report's sorted `serde_json::Value` serialization. The corrected verifier
compares complete JSON values while preserving array order, and separately
checks canonical field order on the raw wire frame. No capture, expected vector,
budget or product file was changed to obtain this verification.

P1-60/P1-70 remain open. This packet closes only the authorized W7 measurement,
not permanent runtime metrics, allocator approval, the `/mnt` disposition, clean
blocking CI or Phase 1 exit authority.
