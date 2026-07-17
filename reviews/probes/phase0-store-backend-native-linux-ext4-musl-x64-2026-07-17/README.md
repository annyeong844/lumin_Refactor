# Phase 0 Store Backend Probe: Native Linux ext4 GNU/musl x64

Status: **execution evidence PASS; native Linux store comparison complete**

## Identity

- Architecture commit: `65e60216891bb3d826a4778f84cb8aaa377abe92`
- Architecture manifest SHA-256:
  `66925583362be22257dd7357072d176cd3f1ae3d4f99874a53f6689b7cf535e0`
- Probe source manifest SHA-256:
  `30be1edac70bb27f78c626bf7099aee1f825f2a0f59797a72b9acdc781e9725e`
- Native runner commit:
  `0b5988c8176c73e9d6d8936cbcc90eebcac3c2a5`
- GitHub Actions run: `29584914108`, job `87899395856`
- GitHub artifact ID: `8408726489`
- GitHub artifact SHA-256:
  `9ffc3fd385c1d6b8af748eda20c26f623f4a18420a3e9a540cb91b6f0f7706e4`
- Evidence manifest SHA-256:
  `0544d252540e14b8f9392d8a83a37209748af0995d6dd2240397ec4941e75046`
- Summary SHA-256:
  `49c3a4f7bc7191ee85ae2f3ce15bec35f4bf48a17619f7574598794dd951e5a9`
- Runner provenance SHA-256:
  `204b055dab77a68b0623f6e8d7f4b7a8b74719ef6ec21d8b2a4d1e3b3d9db7a6`

The host was a non-WSL Ubuntu 24.04 x64 GitHub runner. The source worktree was on
ext4 at `/dev/sda1`. The exact Rust toolchain was `1.96.0`; musl builds used exact
Zig `0.16.0` and `cargo-zigbuild 0.23.0`.

The harness source remains under the Windows packet's `source/` directory. It is a
standalone Phase 0 evidence harness, not an approved product scaffold.

## Correctness

| Surface | GNU redb | GNU SQLite | musl redb | musl SQLite |
| --- | ---: | ---: | ---: | ---: |
| Conflicting admission | 32/32 PASS | 32/32 PASS | 32/32 PASS | 32/32 PASS |
| Disjoint admission | 32/32 PASS | 32/32 PASS | 32/32 PASS | 32/32 PASS |
| Forced death before/after commit | 2/2 PASS | 2/2 PASS | 2/2 PASS | 2/2 PASS |
| Backend/fault cases | 47/47 PASS | 47/47 PASS | 47/47 PASS | 47/47 PASS |
| Namespace replacement cases | 19/19 detected | 19/19 detected | 19/19 detected | 19/19 detected |

All 188 backend/fault cases passed. All 76 namespace replacements were injected and
detected after displacement; none relied on Windows kernel prevention. The GNU and
musl runs used the same 19-file source identity. All three musl executables were
static ELF binaries with no interpreter or dynamic `NEEDED` entry.

## Measurements

Deterministic workload: 10,000 ordered records, 256 payload bytes each, 100 reopen
query samples, and 200 durable admission transactions.

| Mode/backend | Binary | Clean build | Query p50/p95/p99 | Durable p50/p95/p99 | Peak RSS | Store |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| GNU redb | 2,158,424 B | 27,610 ms | 473 / 617 / 843 us | 557 / 726 / 1,341 us | 13,004,800 B | 8,425,472 B |
| GNU SQLite | 3,458,200 B | 62,181 ms | 357 / 385 / 446 us | 1,394 / 1,654 / 2,483 us | 12,320,768 B | 3,182,592 B |
| musl redb | 2,143,256 B | 27,256 ms | 481 / 548 / 786 us | 577 / 803 / 976 us | 23,576,576 B | 8,425,472 B |
| musl SQLite | 3,173,920 B | 21,739 ms | 402 / 429 / 469 us | 1,387 / 1,635 / 2,172 us | 16,240,640 B | 3,182,592 B |

The first reopen query is not an operating-system cold-cache measurement. Build
measurements reused registry/compiler caches and are same-runner comparisons, not
approved product budgets.

## Independent Replay

After artifact download, an independent local replay verified:

- all 37 `SHA256SUMS` entries;
- no unlisted evidence payload;
- byte identity between the packaged and raw uploaded evidence copies;
- all 17 JSON architecture/source identities;
- 128 conflicting/disjoint admission rounds and four forced-death cases per mode;
- 94/94 fault cases per mode;
- static musl linkage for the all-features and both feature-specific binaries.

## Decision Boundary

This packet closes the native Linux ext4 GNU/musl portion of the store comparison.
The cross-platform backend choice is recorded in the sibling
`phase0-store-backend-selection-2026-07-17` packet. It does not approve numeric
budgets, packaged skills, native path/root behavior, or the overall Phase 0 freeze.
