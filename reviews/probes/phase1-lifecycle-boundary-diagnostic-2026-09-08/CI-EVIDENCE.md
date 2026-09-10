# W7 Hosted Diagnostic Evidence

Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
Status: **lint correction verified; Windows/Linux scaling FAIL; no Phase 1 exit**.

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

## Lint-correction hosted verification

The separate authorized correction head
`4f03cc1f8a25e83265a81c0fb1b2c604ce6ee8bf` completed
[CI 34241768722](https://github.com/annyeong844/lumin_Refactor/actions/runs/34241768722).
Both package checkout logs and the diagnostic build record bind actual merge
`5c13e1e5faad50cff9ed37d9243cc80723e221fc`, with unchanged main parent
`09ab415546890241de3434b102fde90ee650b214`. All 58 jobs completed: **55 passed,
three failed**. Both ordinary lint jobs now pass. Windows package
`102113639659` and Linux package `102113639716` fail only their ordinary
benchmark step; dependent Required `102120405579` fails as well. Actual
staged platform/adapter behavior passes on both, as do the Windows W7 build,
owner/public probes, incompatible-feature rejection and fourteen-cell runner.
The first run and its lint failure above remain separate retained evidence.

Both ordinary packets have all 34 expected cells and 21 measured samples,
with four available logical processors and default commands without `--jobs`.
Their complete semantic oracle and every absolute-time, RSS and executable-size
target pass. **Scaling alone fails on both platforms**, so neither package job
nor the aggregate is a PASS.

| Measurement | Windows NTFS | Native Linux | Requirement |
| --- | ---: | ---: | --- |
| Cold default median | 1,856,880,300 ns | 363,578,474 ns | <= 30,000 ms |
| Cold jobs=1 median | 1,932,319,000 ns | 424,602,705 ns | Scaling control |
| Default/jobs=1 ratio | **0.9609594999583402** | **0.8562792222437678** | <= 0.75 |
| Peak RSS | 67,010,560 B | 141,623,296 B | <= 536,870,912 B |
| Packaged executable | 10,503,168 B | 11,942,640 B | <= 12,582,912 B |

Shared ordinary build ID:
`build_9f5d0e2be352b263009f1766e67e6bc8daa904fa4698f0add16cec72dd66db8c`.
Windows binary SHA-256:
`47ddfc99cf1da7a5ad5f41e67a00046f4e67a0f296d0c28e012f2e8caa35397a`;
Linux binary SHA-256:
`2aaebdae9c56bbb65929e7f39454970e4d8ff788f9bda00e69ebe48941707c1f`.
The correction is test-only. Separate hosted runs and variable sample durations
do not establish that it caused either numeric change; both misses remain
valid blocking evidence rather than grounds for rerunning until green.

The new diagnostic remains `DIAGNOSTIC_ONLY` with null numeric verdict and
unchanged W7 fresh counts. Its binary SHA-256 is
`951cd5cb5988b80d1a9fe405c31e80f7a1c83e21f110393b2fb5a20e7fea4f13`.
Its fixed-round default observations are:

| Elapsed boundary | Round 1 ns | Round 2 ns | Round 3 ns |
| --- | ---: | ---: | ---: |
| Engine command | 1,645,762,400 | 1,902,000,600 | 1,723,913,700 |
| Eight selected contexts | 440,225,000 | 579,255,600 | 477,702,500 |
| Backend-open + explicit-drop + return-tail | 324,947,700 | 360,656,200 | 367,290,800 |

The last row comprises 73.81%, 62.26% and 76.89% of the selected contexts,
or 19.74%, 18.96% and 21.31% of engine time. It again identifies a candidate
area, not pure backend teardown, kernel wait or device-flush time. Overlapping
W7/W3/engine views are not additive. Control/diagnostic medians are respectively
2,033,847,500 / 1,976,199,900 ns for jobs=1 and 1,771,844,300 /
1,731,330,800 ns for default; no diagnostic duration is subtracted from budgets.

### Second packet provenance

Complete downloads, package job logs and verifier outputs are retained outside
the repository at `D:/lumin-w7-ci-34241768722/`. The artifact IDs are Windows
`10063080298`, Linux `10062503340` and diagnostic `10063081222`.
`run-summary.json` retains all 58 jobs; `artifacts.json` retains the download
inventory. No measurement or source file was changed during verification.

The previously retained ordinary and W7 verifiers, at their unchanged hashes
in the table above, independently verify all **1,835 capture files** again:
Windows 745/34 cells, Linux 711/34, diagnostic 379/14. Checks cover exact
inventory/bytes, frozen semantic map, every median, process and inherited-job
receipts, merge/build/PID/worker binding, raw v3 hash/field order, every count
vector, zero/null shape, residual and W3 containment. The complete 256-tuple
map has the same independently recomputed identity recorded above. Results
are `windows-verification.json`, `linux-verification.json` and
`diagnostic-verification.json`; integrity is VERIFIED, not numeric PASS.

| Second packet record | SHA-256 |
| --- | --- |
| Windows report | `df0127964b878d4af9f093ecac179ea0a96125fa702a34e3823201b0e6daaba9` |
| Windows manifest | `8de4e980a0c5cb2ba2cf934e70a3ddcc99deacd516e9eeeaade4fe22bc298a08` |
| Linux report | `d0d7de5157b3c50b8736cc24a28d5565cd0091bc012c699ff8afc74e4acd4eb5` |
| Linux manifest | `5692df83b4024e623595e982c343f1f9bc5ae2b67292954d6027a45a72326b12` |
| Diagnostic report | `89fdc75125fb6f40b2b57a92de0859647cd6cac7251df74c02d9b62bf6eb06fa` |
| Diagnostic manifest | `b1ce6d88e47f62d58998f85779a5c4e0378dbd3351afb7e9f834b64fe1a28112` |
| Diagnostic build record | `ee85d8a99f63e145065a87d5e1bda89bd80fae0e0465077786781f6b306c2a94` |

This closes the correction's clean-hosted lint verification only. P1-60/P1-70,
both scaling misses and the separately owned remaining decisions stay open.
