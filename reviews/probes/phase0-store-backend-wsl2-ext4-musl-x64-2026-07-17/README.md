# Phase 0 Store Backend Probe: WSL2 ext4 GNU/musl x64

Status: **partial execution evidence PASS; later backend selection recorded; Phase 0 freeze remains blocked**

Subsequent cross-platform decision: exact `redb 4.1.0` was selected in
[`../phase0-store-backend-selection-2026-07-17/`](../phase0-store-backend-selection-2026-07-17/).
The packet-local `backend_selected: false` remains unchanged because it records the
state when this WSL2-only evidence was sealed.

## Identity

- Architecture commit: `65e60216891bb3d826a4778f84cb8aaa377abe92`
- Architecture manifest SHA-256:
  `66925583362be22257dd7357072d176cd3f1ae3d4f99874a53f6689b7cf535e0`
- Probe date: 2026-07-17
- Host: WSL2 Ubuntu, x86_64, kernel `6.6.87.2-microsoft-standard-WSL2`
- Filesystem: ext4 on `/dev/sdd`; the probe ran under `/home/endof`, not `/mnt/c`
- Rust: `1.96.0`
- GNU target: `x86_64-unknown-linux-gnu`
- musl target: `x86_64-unknown-linux-musl`
- musl toolchain: Zig `0.16.0`, distribution SHA-256
  `70e49664a74374b48b51e6f3fdfbf437f6395d42509050588bd49abe52ba3d00`;
  `cargo-zigbuild 0.23.0`
- Backends: exact `redb 4.1.0`; exact `rusqlite 0.39.0` with bundled SQLite
- Embedded 19-file source manifest SHA-256:
  `30be1edac70bb27f78c626bf7099aee1f825f2a0f59797a72b9acdc781e9725e`
- Evidence manifest SHA-256:
  `c65a9224bfd03482ea947661c70edc7168d1662a18eb1a0447604fa58f807b3e`
- Summary SHA-256:
  `f12f91a02c3f15f2525b476c330839351054d23fe52176c91fc9adf28d9ff81f`

The single harness source owner remains
[`../phase0-store-backend-windows-x64-2026-07-17/source`](../phase0-store-backend-windows-x64-2026-07-17/source/).
It is a standalone Phase 0 evidence harness, not a Phase 1 product scaffold. The
directory name records where the harness originated; the source and oracle are now
explicitly cross-platform.

## Correctness Result

| Surface | GNU redb | GNU SQLite | musl redb | musl SQLite |
| --- | ---: | ---: | ---: | ---: |
| Conflicting admission | 32/32 PASS | 32/32 PASS | 32/32 PASS | 32/32 PASS |
| Disjoint admission | 32/32 PASS | 32/32 PASS | 32/32 PASS | 32/32 PASS |
| Forced death before/after commit | 2/2 PASS | 2/2 PASS | 2/2 PASS | 2/2 PASS |
| Backend/fault cases | 47/47 PASS | 47/47 PASS | 47/47 PASS | 47/47 PASS |
| Namespace replacement cases | 19/19 detected | 19/19 detected | 19/19 detected | 19/19 detected |

All 76 namespace observations were `injected-and-detected`; none used the Windows
`kernel-prevented-before-displacement` outcome. Linux allowed the replacement while
the original handles remained open, and the waiting child rejected the displaced
entry before a canonical commit. The matrix also proves deterministic latest races,
publication-retention ordering, retention crash recovery, migration generation
fencing, and a real late-writer process.

The musl all-features and feature-specific executables are static ELF files. `ldd`
reported `not a dynamic executable`, and `readelf` reported no interpreter or dynamic
`NEEDED` entry.

## Measurements

Deterministic workload: 10,000 ordered records, 256 payload bytes each, 100 reopen
query samples, and 200 durable admission transactions.

| Mode/backend | Binary | Clean target build | Reopen p50/p95/p99 | Durable p50/p95/p99 | Peak RSS | Store |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| GNU redb | 2,158,264 B | 151,247 ms | 14,117 / 16,751 / 20,485 us | 17,286 / 20,274 / 21,888 us | 12,451,840 B | 8,425,472 B |
| GNU SQLite | 3,458,008 B | 380,233 ms | 1,017 / 2,689 / 3,510 us | 22,849 / 32,596 / 40,166 us | 12,144,640 B | 3,182,592 B |
| musl redb | 2,143,032 B | 90,892 ms | 14,360 / 18,002 / 20,704 us | 16,307 / 20,621 / 30,840 us | 22,544,384 B | 8,425,472 B |
| musl SQLite | 3,173,744 B | 47,269 ms | 879 / 1,520 / 3,754 us | 21,081 / 23,548 / 25,657 us | 16,777,216 B | 3,182,592 B |

The build collector removed each Cargo target directory before its clean measurement,
but Cargo registry and compiler caches and Zig's global libc cache were warm. Build
times are therefore same-host comparison observations, not cold-machine package
budgets, and should not be compared naively across GNU and musl modes. The first reopen
sample is not an operating-system cold-cache measurement.

On this host SQLite had substantially faster reopen queries and the smaller store;
redb had faster durable admissions. This mixed result does not select a backend.

## Verification

The final source passed on GNU/Linux:

```text
cargo fmt --all -- --check
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
```

The live packager verified exact case IDs, admission truth tables, current source
bytes, all executable identities, build-log/dependency-tree hashes, static musl
linkage, and benchmark-to-feature-binary binding. It rejected two negative controls:
one renamed fault case and one tampered benchmark executable SHA-256. The copied packet
then passed the same verifier in `--offline` mode and an independent 37-entry
`SHA256SUMS` replay.

## Limits

This is WSL2 ext4 evidence, not native non-WSL Linux certification. It does not prove
bare-metal/container distribution behavior, every required durable-flush and lock
semantic, native path/root and packaged skills, OXC memory/stack feasibility, or
approved cross-platform numeric budgets. Both backends remain candidates and Phase 0
remains blocked.
