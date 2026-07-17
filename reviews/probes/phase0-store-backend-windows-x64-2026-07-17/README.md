# Phase 0 Store Backend Probe: Windows x64

Status: **partial execution evidence PASS; backend selection and Phase 0 freeze remain blocked**

## Identity

- Architecture commit: `65e60216891bb3d826a4778f84cb8aaa377abe92`
- Architecture manifest SHA-256:
  `66925583362be22257dd7357072d176cd3f1ae3d4f99874a53f6689b7cf535e0`
- Probe date: 2026-07-17
- Host/target: Windows x64 / `x86_64-pc-windows-msvc`
- Toolchain: Rust `1.96.0`
- Backends: exact `redb 4.1.0`; exact `rusqlite 0.39.0` with bundled SQLite
- Evidence manifest SHA-256:
  `b370b648e2d2a1d5840e9e1ed4ec7bf2646fc5421d1765924f168df6ac82d4a0`
- Summary SHA-256:
  `35de7a5f8cebbd5a62f070565307d7ef51d2cba8abba2c57484a5b27fa77dbf0`

The `source/` directory is a standalone evidence harness, not an approved Phase 1
product scaffold. Its oracle is recorded in `source/PROBE-CONTRACT.md` and was derived
from the frozen architecture before the cases were run.

## Correctness Result

| Surface | redb | bundled SQLite |
| --- | ---: | ---: |
| Conflicting two-process admission | 32/32 PASS | 32/32 PASS |
| Disjoint two-process admission | 32/32 PASS | 32/32 PASS |
| Forced death before/after commit | 2/2 PASS | 2/2 PASS |
| Backend/fault cases | 42/42 PASS | 42/42 PASS |

The 42 cases per backend cover bounded indexed traversal, visible header corruption,
eight publication death points, reverse and same-sequence latest publication, both
publication-retention race orders, six retention death points, both-or-neither
canonical/trash hard stops, five migration death points, stale-generation rejection,
and 14 Windows namespace replacement/content/link races. The raw observations are in
`evidence/admission-windows-x64.json` and
`evidence/fault-matrix-windows-x64.json`.

## Measurements

Deterministic workload: 10,000 ordered records, 256 payload bytes each, 100 reopen
query samples, and 200 durable admission transactions.

| Metric | redb | bundled SQLite |
| --- | ---: | ---: |
| Clean release build | 102,067 ms | 121,813 ms |
| Incremental release build | 1,330 ms | 702 ms |
| Release binary | 1,895,936 B | 2,808,320 B |
| Transitive packages | 28 | 41 |
| Selected dependency `unsafe` keyword lines | 13,545 | 18,046 |
| Bundled native source | 0 files / 0 B | 10 files / 20,340,001 B |
| Initialize | 14,920 us | 49,476 us |
| Bulk insert | 79,226 us | 142,547 us |
| First query after handle reopen | 8,002 us | 20,282 us |
| Reopen query p50 / p95 / p99 | 5,534 / 8,152 / 8,793 us | 16,336 / 28,066 / 32,521 us |
| Durable admission p50 / p95 / p99 | 6,993 / 10,784 / 15,914 us | 19,594 / 27,069 / 47,166 us |
| Peak working set | 15,343,616 B | 11,816,960 B |
| Store size | 8,425,472 B | 3,182,592 B |

The first reopen sample is not an operating-system cold-cache measurement. The
`unsafe` count is a reproducible comparison surface, not a safety audit. These are
single-host observations and are not approved product budgets.

## Verification

The final source state passed:

```text
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

The executable reports all carried the same 19-file source-hash list. The evidence
packager independently rejected candidate-ID mismatches, failed status, wrong fault
counts, stringified build metrics, and feature-binary size disagreement before writing
`evidence/windows-x64-summary.json` and `evidence/SHA256SUMS`.

## Limits

This packet does **not** select a backend or pass the complete store gate. It does not
provide Linux ext4/musl results, all required filesystem durable-flush semantics,
namespace bootstrap death recovery, product-level operation delivery/idempotency,
public Lumin commands, native path/package/skill probes, OXC memory evidence, or
approved cross-platform numeric budgets. Both backends therefore remain candidates and
Architecture v1 remains blocked.
