# W6 Hosted CI Evidence

Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
Status: **complete observation; Windows scaling FAIL; no Phase 1 exit**.

## Exact execution

The authorized W6 head `964c91c6c89ec84a5b2b9bb42851caf664464dfa`
ran once in [CI 34192705416](https://github.com/annyeong844/lumin_Refactor/actions/runs/34192705416).
The merge checkout was `422a0d48ced4057890b069dcb5f4ed580ed56314`, with
parents main `09ab415546890241de3434b102fde90ee650b214` and that W6 head.
The run was created at `2026-09-08T05:58:35Z`; all 58 jobs completed.

56 jobs passed. Windows package job `101953946587` failed only its ordinary
`Benchmark packaged binary` step; dependent Required job `101957692383`
also failed. Windows/Linux builds, lint, tests, corpus, determinism, and both
actual staged binary/adapter behavioral probes passed. This is not a compiler
or package-behavior failure. No rerun, budget change, default-worker-policy
change, PR ready transition, or merge was performed.

## Ordinary budget authority

Both seven-mode reports contain all 34 expected cells, including 21 measured
samples. Both hosts report four available logical processors; the default
commands have no explicit `--jobs`. Ordinary worker fields remain derived
policy observations, not the still-required permanent pool-produced metrics.

| Measurement | Windows NTFS | Native Linux | Requirement |
| --- | ---: | ---: | --- |
| Cold default median | 1,651,498,000 ns | 400,813,520 ns | <= 30,000 ms |
| Cold jobs=1 median | 1,864,286,600 ns | 1,163,246,331 ns | Scaling control |
| Default/jobs=1 ratio | **0.8858605752999565** | 0.3445646113969991 | <= 0.75 |
| Peak RSS | 66,961,408 B | 143,847,424 B | <= 536,870,912 B |
| Packaged executable | 10,499,584 B | 11,938,544 B | <= 12,582,912 B |

Every other Windows numeric budget and every Linux budget passes. Windows
therefore has one valid numeric miss, not invalid or missing measurements.
The shared build ID is
`build_720137d95f5e5c9e2e7e261ea498c86c061f6f9863524f9d66b92ea1be56e17f`.
Windows executable SHA-256 is
`b7721255a5351efe84e7f22bfc56f5a59cb29f14237b432b8be329373bd59e62`;
Linux is `b3fdfda311268739d1dde734ae10377deddd34f462dd180d5470e430a4c60e8d`.

The earlier W5 measurements remain separate evidence. Hosted processor models
and sample variance differ; these absolute times do not establish a controlled
W5/W6 causal speedup. The invalid local
[affinity comparison](FOUR-WORKER-COMPARISON.md) remains invalid, with its
candidate unrun. This hosted result supplies a real four-processor execution,
not retroactive validation of that local packet.

## Diagnostic localization, not a budget verdict

The separate 14-cell Windows report is
`lumin.phase1-cold-audit-diagnostic.v2`, status `DIAGNOSTIC_ONLY`, with
`numericBudgetVerdict: null`. Its diagnostic executable SHA-256 is
`f2bba3918631ccddfdb4299f14c6396553f20edce9aeceae9d797b9c42b09057`.
Its build record binds the same merge checkout and ordinary Windows executable.

| Measured default cell | Engine command ns | Store-owned self ns | Fraction |
| --- | ---: | ---: | ---: |
| `round-1-diagnostic-default` | 1,555,223,100 | 956,307,500 | 61.4901% |
| `round-2-diagnostic-default` | 1,519,200,100 | 926,545,000 | 60.9890% |
| `round-3-diagnostic-default` | 1,649,229,900 | 1,042,839,300 | 63.2319% |

Each numerator sums only that sample's engine `store-open`, `attempt-begin`,
and `store-publish` self times. Nested store rows are not added. These regions
include several kinds of serial work; they do not isolate backend teardown,
lock wait, namespace validation, or filesystem durability. The
[bounded follow-up investigation](COST-INVESTIGATION.md) records the remaining
uncertainty, not a promised saving or authorized product optimization.

## Raw provenance and verification

The complete downloaded artifacts and read-only verifier outputs remain in
`D:\lumin-w6-ci-34192705416`. Artifact IDs are Windows `10043179901`,
Linux `10042869064`, and diagnostic `10043180953`. `run-summary.json`
retains every job ID/result; `artifacts.json` retains the three-artifact API
inventory. No full failed-job log archive is claimed.

All 1,835 capture files match their manifest lengths and hashes: Windows
745 files/34 cells, Linux 711/34, diagnostic 379/14. The verifiers recomputed
ordinary medians, checked complete process observations and Windows inherited-job
lifetime receipts, and checked the exact semantic tuple-to-ID maps against the
independently authored 256-finding truth. The shared recomputed map SHA-256 is
`93b1b99df26f8b7a5872425cc84f0f2dcb36e5af65f53eb37756e32a8532c523`.
All 14 diagnostic samples match their complete captured `sample.json`, and
the four diagnostic/control medians were recomputed from all 12 measured cells.

| Retained record | SHA-256 |
| --- | --- |
| Windows report | `5c3b1791b14560c44dad0a4c6a737ccf750d8fda93081e7eee17476435c213cc` |
| Windows manifest | `4449a2b102fe0581b19216aa5ac1ced9a8c8041a3363148daba7b1188b6c95ac` |
| Linux report | `800f1c3faf7c6c86f3c028323827c5c38a7e5c53d40b41637ba84687d26c14a1` |
| Linux manifest | `879193953cf6536942370a9511e74c7011c3ee2de01d34012ec5415a1c69130c` |
| Diagnostic report | `4ae1a71be4293dc4d39bcf67b4a612aec491e5692ec37908b4836f7724fcea1b` |
| Diagnostic manifest | `517a8876b974204e9d2bd1b5cac92832f84cb286a9e1a3d80a7fee03c8fe53c6` |
| Diagnostic build record | `f5928259644af56ae38044e654c97be165b289ed91fbdbced713afc061d93a16` |
| Ordinary verifier | `b264ed6767d870311233f585b84fa83454374e2014754b2a6d8aeaa0d117e510` |
| Diagnostic verifier | `0a9ae8eae5e6df91eca69b4ba61d0c727f589669c3e2f719834b4cb133a32b3a` |

The raw verification results are `windows-verification.json`,
`linux-verification.json`, and `diagnostic-verification.json`. The scoped W6
[implementation](IMPLEMENTATION.md) and [local comparisons](MEASUREMENTS.md)
remain their own evidence, not replacements for this hosted miss. P1-60/P1-70
remain open; no numeric target or permanent-observation requirement is relaxed.
