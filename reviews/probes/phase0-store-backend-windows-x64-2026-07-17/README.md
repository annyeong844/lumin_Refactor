# Phase 0 Store Backend Probe: Windows x64

Status: **partial execution evidence PASS; later backend selection recorded; Phase 0 freeze remains blocked**

Subsequent cross-platform decision: exact `redb 4.1.0` was selected in
[`../phase0-store-backend-selection-2026-07-17/`](../phase0-store-backend-selection-2026-07-17/).
The packet-local `backend_selected: false` remains unchanged because it records the
state when this Windows-only evidence was sealed.

## Identity

- Architecture commit: `65e60216891bb3d826a4778f84cb8aaa377abe92`
- Architecture manifest SHA-256:
  `66925583362be22257dd7357072d176cd3f1ae3d4f99874a53f6689b7cf535e0`
- Probe date: 2026-07-17
- Host/target: Windows x64 / `x86_64-pc-windows-msvc`
- Toolchain: Rust `1.96.0`
- Backends: exact `redb 4.1.0`; exact `rusqlite 0.39.0` with bundled SQLite
- Evidence manifest SHA-256:
  `6d404cfc4b25ed581a9f021fc6248c6e7a94c2fdc38b668872c009c0f747ef2d`
- Summary SHA-256:
  `6623d3425294bcf8d5949348dd660a9fa9762d7808a453cb0f1b60df9ea385b3`
- Embedded 19-file source manifest SHA-256:
  `30be1edac70bb27f78c626bf7099aee1f825f2a0f59797a72b9acdc781e9725e`
- All-features harness executable SHA-256:
  `a6d58e49c8cc40856c2e97e64e3a49bed827843098799ed1af19bfba33eea151`

The `source/` directory is a standalone evidence harness, not an approved Phase 1
product scaffold. Its oracle is recorded in `source/PROBE-CONTRACT.md` and was derived
from the frozen architecture before the cases were run.

## Correctness Result

| Surface | redb | bundled SQLite |
| --- | ---: | ---: |
| Conflicting two-process admission | 32/32 PASS | 32/32 PASS |
| Disjoint two-process admission | 32/32 PASS | 32/32 PASS |
| Forced death before/after commit | 2/2 PASS | 2/2 PASS |
| Backend/fault cases | 47/47 PASS | 47/47 PASS |

The 47 cases per backend cover bounded indexed traversal, visible header corruption,
eight publication death points, deterministic reverse and same-sequence latest
publication, both publication-retention race orders, six retention death points,
both-or-neither canonical/trash hard stops, five migration death points, a real
late-writer process rejected after generation change, and 19 Windows namespace
replacement/content/link races. Across both backends, 18 namespace faults were
injected and detected after displacement; Windows prevented the other 20 before
displacement while the child held the bound directory domain. Kernel prevention is
recorded separately and is not treated as a substitute for injected-and-detected
coverage on platforms that permit displacement. The raw observations are in
`evidence/admission-windows-x64.json` and
`evidence/fault-matrix-windows-x64.json`.

## Measurements

Deterministic workload: 10,000 ordered records, 256 payload bytes each, 100 reopen
query samples, and 200 durable admission transactions.

| Metric | redb | bundled SQLite |
| --- | ---: | ---: |
| Clean release build | 134,033 ms | 139,292 ms |
| Incremental release build | 783 ms | 798 ms |
| Release binary | 2,333,696 B | 3,247,104 B |
| Transitive packages | 28 | 41 |
| Selected dependency `unsafe` keyword lines | 13,545 | 18,046 |
| Bundled native source | 0 files / 0 B | 10 files / 20,340,001 B |
| Initialize | 32,583 us | 56,954 us |
| Bulk insert | 85,874 us | 137,236 us |
| First query after handle reopen | 12,184 us | 17,031 us |
| Reopen query p50 / p95 / p99 | 12,133 / 15,090 / 15,849 us | 13,758 / 18,075 / 21,205 us |
| Durable admission p50 / p95 / p99 | 15,236 / 18,328 / 20,460 us | 21,770 / 26,594 / 28,071 us |
| Peak working set | 15,257,600 B | 13,189,120 B |
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

Feature-specific Clippy runs also passed for both backend builds. Every report carried
the same compile-time embedded 19-file source identity. Before packaging, the verifier
recomputed the current source manifest, checked every report and measured executable,
invoked each release executable's live `identity` command, and bound the reported
bytes and SHA-256 to the build records. Negative controls confirmed that a renamed
fault case and a tampered executable SHA-256 are rejected before summary publication.

## Limits

This packet does **not** select a backend or pass the complete store gate. A sibling
packet records WSL2 ext4 GNU/musl evidence, but neither packet provides native Linux
filesystem/package proof. The remaining gaps include all required filesystem
durable-flush semantics, namespace bootstrap death recovery, product-level operation
delivery/idempotency, public Lumin commands, native path/package/skill probes, OXC
memory evidence, and approved cross-platform numeric budgets. Both backends therefore
remain candidates and Architecture v1 remains blocked.
