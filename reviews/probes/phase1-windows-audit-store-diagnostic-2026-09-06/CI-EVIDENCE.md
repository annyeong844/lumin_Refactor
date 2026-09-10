# W3 Actual CI Evidence

[CI run 34033040113, attempt 1](https://github.com/annyeong844/lumin_Refactor/actions/runs/34033040113).
PR head: `a0f5032dfa19dcfc3546f17ff1a4b13e6b7b42d6`.
Actual merge checkout: `15a4de8ffc89cad8cb67d125cc078d9807b4f5ef`.
Scope: frozen [W3](DESIGN.md), diagnostic only; no optimization or exit authority.

## Blocking verdicts

54 jobs pass; Windows package, Linux package, and `Required` fail. Both release
builds and staged binary/adapter behavior probes pass; the benchmark failures
remain blocking independently of those functional results.

- Windows: seven capture cells complete, `warm-pre-write-default-1-measured`
  is invalid, and 26 cells are not run. No complete numeric report exists.
  The observer reports children `[6456, 6504, 6556, 6588]`; the public product
  exits 0 with empty stderr and a sealed allow-with-warnings result. The old
  v1 receipt lacks lifetime-bound ancestry, so the exact cause is unproven.
  [W4](../phase1-windows-process-observer-2026-09-07/DESIGN.md) corrects the
  observer prospectively; it does not turn this cell into a pass.
- Linux: all 34 cells complete. Cold default median `348,140,123 ns` divided
  by jobs=1 median `401,097,462 ns` is `0.8679689002868833`, above the unchanged
  `0.75` limit. Other numeric criteria and the semantic oracle pass. Earlier
  Linux results cannot replace this checkout's failure.
- The isolated Windows W3 build, public-child probes, and all 14 diagnostic
  cells pass. Its `DIAGNOSTIC_ONLY` result has no numeric-budget verdict.

## Diagnostic interpretation

All seven diagnostic frames preserve the exact 23 execution phases and 52
store-call phases, observed PIDs/build/run/attempt bindings, actual one/four
workers, and the configured 4,194,304-byte stack. All 14 cells have the same
complete 256-finding authored truth and tuple-to-ID map, SHA-256
`93b1b99df26f8b7a5872425cc84f0f2dcb36e5af65f53eb37756e32a8532c523`.

| Three-sample median | jobs=1 (ms) | Default, four workers (ms) |
| --- | ---: | ---: |
| External process | 2141.3742 | 1876.4551 |
| Store open, inclusive | 346.4902 | 218.7934 |
| Attempt begin, inclusive | 325.2329 | 356.5575 |
| Store publish, inclusive | 987.4905 | 922.1777 |
| Publish preflight | 314.1945 | 241.1312 |
| Finalize release | 101.6746 | 96.7849 |
| Finalize latest | 91.9745 | 89.2401 |

The per-sample sum of the three disjoint store roots has median 1620.0794 ms
(one worker) and 1497.5286 ms (default), median process shares 75.656% and
79.806%. The roots include final-input revalidation. Nested rows overlap and
individual medians are not additive. These are owner-call elapsed intervals,
not measured backend flush or lock wait. Diagnostic-minus-control median
differences are +111.5551 / -126.1347 ms; no stable overhead correction follows.

## Retained packet binding

Exact file inventories, sizes, and SHA-256 values were checked independently:
365 W3 captures, 164 incomplete Windows captures, and 711 Linux captures.
W3's frozen design and W2's frozen design remain unchanged. The workflow's
raw artifacts are `lumin-audit-store-diagnostic-windows-x64`,
`lumin-foundation-benchmark-windows-x64`, and
`lumin-foundation-benchmark-linux-x64`, with 90-day retention.

| Artifact | SHA-256 |
| --- | --- |
| W3 report | `10933476dd6a52f6d54c004155c5b31d94e44be460dffad34bff7e0526ccbb8e` |
| W3 capture manifest | `4fbdf423158616909ca93ce2f1984cf5a4543f846d01ce1926911208e3a53e68` |
| W3 build record | `696a6c0ac3243638eddbeefc5e50e091ecfa80a88394aa3decf65cd4c3114056` |
| Windows incomplete manifest | `3c9e1ea0de0782c6c8c34e4851376eab7768c90b3f871692ea95ae2a88a0d01e` |
| Linux report | `6d789098a1b1191ba50cd79693759d6d1c1fa60d0a77356319e9a7ae3a317531` |
| Linux capture manifest | `e0491360d1e494503b64167ac68f0019b0cbaa5f23654dff88c022f088df7e7f` |

External copies are `D:\lumin-w3-ci-diagnostic-34033040113`,
`D:\lumin-w3-ci-windows-34033040113`, and
`D:\lumin-w3-ci-linux-34033040113`. Paths are retrieval hints; the run,
source, and hashes bind the evidence. Missing raw packets remain unavailable,
never reconstructed from this summary. P1-60 and REVIEW-005 remain open.
