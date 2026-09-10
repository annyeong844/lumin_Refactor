# W8 Hosted Evidence

Status: **verification complete; Windows scaling FAIL; P1-60 remains open**.
Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
Authority: [W8 publication approval](REVIEW.md#publication-approval).

## Identity and verdict

The authorized [CI run 34344138195](https://github.com/annyeong844/lumin_Refactor/actions/runs/34344138195)
completed attempt 1 on head
`12004f39127208740518d64b13c11e43b8652a34`, using merge checkout
`c373b27b5ee8f6fba5551b5fab319a4778e68643`. Its parents are main
`09ab415546890241de3434b102fde90ee650b214` and that W8 head.

56 of 58 jobs pass. Windows package job `102441701704` fails only
`Benchmark packaged binary`; dependent Required job `102446756431` fails.
Linux package job `102441701684` passes. Both actual staged platform and
adapter probes, all build/lint/test/corpus/dependency jobs, and the separate
Windows diagnostic build, public-child probes and fourteen-cell runner pass.
No rerun, budget change, PR-ready transition or merge was performed; PR #135
remained Draft at verification. This record publishes the already retained
result during the next authorized design packet, not a new CI execution.

| Ordinary metric | Windows NTFS | Native Linux |
| --- | ---: | ---: |
| Cold default median ns | 1599358200 | 342951882 |
| Cold jobs=1 median ns | 1786283500 | 486337573 |
| Default/jobs=1; required <= 0.75 | 0.8953551885800882 FAIL | 0.7051724995962835 PASS |
| Peak RSS bytes | 67067904 | 149639168 |
| Ordinary executable bytes | 10507776 | 11950832 |

Windows scaling is the sole numeric miss; all other absolute-time, RSS and
executable-size targets pass on both platforms. Both hosted environments expose
four processors/default workers. Different hosted machines and sample variance
prevent a causal W7/W8 speedup claim. Permanent observations, allocator approval,
the WSL `/mnt` disposition and P1-60/P1-70 remain open.

## Capture and verification binding

The complete external packet is retained at `D:/lumin-w8-ci-34344138195/`:
`SUMMARY.md`, `run-summary.json`, `artifacts.json`, both package-job records,
three verification results and all downloaded artifact contents. Windows
artifact `10101588094` contains 34 cells/745 captures; Linux `10101126216`
contains 34/711; diagnostic `10101589110` contains 14/379. All 1,835 capture
lengths, SHA-256 hashes and exact inventories verify. Every ordinary median,
verdict, process receipt and complete authored 256-tuple mapping was checked.
The common semantic mapping hash is
`93b1b99df26f8b7a5872425cc84f0f2dcb36e5af65f53eb37756e32a8532c523`.

| Retained artifact | SHA-256 |
| --- | --- |
| Windows report | `cf1e8f80a96362434ff7da114b369fc46412237b2a2308e48da1c90836b11e99` |
| Windows capture manifest | `dd1e80b448db8e77f91994f9fa5bb605a305e2e86558f8fc2c59d9fffc75620a` |
| Linux report | `6677fa459f6ff4607b8e4ad294015893080041b1d1be37bddee2cda7c686fbb1` |
| Linux capture manifest | `8ad737fd62bba81f3323300d04e79bc84bb31a084852282c6c773eb4d025f7a9` |
| Diagnostic report | `88fa412335c1b7ec913aa0b87aa7468739c8bb9696d34e58394bade7e2a72892` |
| Diagnostic capture manifest | `8a673122626231bf548fd94eb4757f66be819e7a6d189f8ef6fa7d6a8c5c5306` |
| Diagnostic build record | `4bb6c53ecbaa20a4388dbcb4643b3889430bc0c542aaafdef26882ef4f16de09` |

The unchanged ordinary verifier is
`D:/lumin-w6-ci-34192705416/verify-ordinary.ps1`, hash
`b264ed6767d870311233f585b84fa83454374e2014754b2a6d8aeaa0d117e510`.
The retained W8 `verify-lifecycle.ps1` has hash
`c71365da2df0c77e106f6ca151b345652a7f3481b18661868160ed734a7b3282`.
It preserves the earlier verification algorithm and amends only the first two
fresh vectors to the independently frozen W8 expectations. It verifies v3 raw
frame order, all eight vectors, null/zero distinctions, residuals, containment,
process/build identity and complete semantic truth. Integrity VERIFIED is not
numeric PASS; the diagnostic remains `DIAGNOSTIC_ONLY` with a null verdict.

Ordinary build ID:
`build_301199be77ac4e365978bce55bc23c1f5b8fafd9fdb3368d61b6767a8a8953b8`.
Windows binary SHA-256:
`4c85db2eaf4995c175b9599cea4e70d6948603316b792a72890f1e892fd2944b`.
Linux binary SHA-256:
`9f82f9e01fc3c2baa402541ac5e69f177e7c22ff988579a204d5247d81f53ae4`.
Windows diagnostic binary SHA-256:
`abd2ce18aa77141d66ac43a86536cd8103584ae2270e304acf5c43989bd39879`.

## Read-only attribution and next scope

These are per-sample calculations from the hashed diagnostic report, not new
measurements. Select the three `round-N-diagnostic-default` samples. Store-owned
time is the sum of **engine self** times for `store-open`, `attempt-begin` and
`store-publish`; nested final-input validation is not added. Selected context
time sums the eight `lifecycleContexts`. Guard time sums the eight W3 enter/exit
rows for open recovery, attempt begin, publish prepare and publish finalize.
All columns below are ns; guard and release are outside the eight W7 contexts.

| Round | Store-owned | W7 selected | Eight guards | Finalize release | Outside all these regions |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 1324679500 | 396936200 | 391064300 | 74336800 | 462342200 |
| 2 | 1132136300 | 391306200 | 149373300 | 69418000 | 522038800 |
| 3 | 1054703500 | 373567500 | 201979300 | 65888000 | 413268700 |

Store-owned time is 67.61-72.72% of engine command time. The selected W7
contexts cover only 29.96-35.42% of that store-owned time. Within those contexts,
backend construction takes 58.84-70.33 ms, while explicit-drop plus return-tail
intervals take 206.85-214.01 ms. Return tails include other local destruction
and return overhead. These are not pure lock wait, redb internals, antivirus
cost, or hardware flush measurements; their medians must not be added together.

The next [W9 design](../phase1-guard-release-diagnostic-2026-09-09/DESIGN.md)
selects the eight guards and final release, whose currently opaque intervals
total 218.79-465.40 ms in these samples. It still leaves 413.27-522.04 ms outside
both selected sets. W9 cannot claim whole-store coverage or predict savings.
Source distinguishes writable held-file backends from admission's detached
in-memory verification backends; the latter must not be labelled disk opens.
