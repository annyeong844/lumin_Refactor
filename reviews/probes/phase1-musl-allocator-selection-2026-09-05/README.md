# Phase 1 Linux-musl Allocator Selection Evidence

Status: **author measurement and dependency-cost review candidate; P1-60 remains open**

## Scope

This packet measures the product value and cost of selecting exact `mimalloc
0.1.52` with feature `v2` as the global allocator only for the packaged
`x86_64-unknown-linux-musl` CLI. It does not change Windows or GNU/Linux
allocator selection, relax a performance target, or claim native-Linux release
authority.

## Controlled comparison

The candidate working tree was copied outside the repository. A complete
791-file SHA-256 comparison showed that the control source differed only by the
four-line global-allocator block recorded in
`source/system-allocator-control.patch`. The control was built first, the block
was restored, and another complete comparison showed zero source differences
before the candidate rebuild. Both builds used:

- Rust `1.96.0` and target `x86_64-unknown-linux-musl`;
- the guarded `cargo build -p lumin-cli --release --target
  x86_64-unknown-linux-musl --locked` path;
- build revision `allocator-control-20260905`;
- the same lockfile, dependency declarations, features, compiled capability
  registry, package skills, fixture, and benchmark harness.

The control deliberately retained the candidate dependency declaration. Cargo
therefore resolved and compiled the same dependency graph while the unreferenced
allocator was not linked into the control executable. This isolates runtime and
linked-binary effects; it is not a claim about clean-build time saved by omitting
the dependency.

Each report is the full `lumin-xtask benchmark foundation` matrix: seven modes,
three repetitions, 780 files, 7,461,511 bytes, and the independently authored
256-finding truth. Every product process exited successfully, spawned no analysis
child, and produced the same canonical semantic digest.

## Measurements

| Observation | system control | mimalloc run 1 | mimalloc run 2 |
| --- | ---: | ---: | ---: |
| cold default median | 1,300,867,705 ns | 858,749,903 ns | 825,709,799 ns |
| cold `jobs=1` median | 1,444,527,204 ns | 1,143,560,402 ns | 1,151,135,685 ns |
| scaling ratio | 0.900549 | 0.750944 | 0.717300 |
| scaling target | FAIL | FAIL by 0.000944 | PASS |
| executable bytes | 11,760,576 | 11,913,968 | 11,913,968 |
| maximum measured RSS | 62,029,824 | 138,981,376 | 145,108,992 |

Against the system control, the accepted second mimalloc matrix reduced the
seven mode medians by 20.31% to 39.94%. The executable grew by 153,392 bytes
(1.3043%) and remained below the frozen 12,582,912-byte limit. Peak RSS grew by
83,079,168 bytes and remained at 27.03% of the frozen 536,870,912-byte limit.

The first mimalloc matrix's marginal miss is retained rather than discarded.
The second independently complete matrix passed at 0.717300; native Linux CI
still owns the release-host verdict.

## Dependency cost review

The exact direct edge is confined to
`cfg(all(target_os = "linux", target_env = "musl"))`. The lockfile adds:

- `mimalloc 0.1.52` -> `libmimalloc-sys 0.1.49` at runtime;
- `cc 1.4.5`, `find-msvc-tools 0.1.12`, and `shlex 2.0.1` as native build
  support.

The Rust wrapper is a small `GlobalAlloc` FFI boundary. The sys crate includes
vendored mimalloc C and its build script compiles the v2 `static.c`; this is a
real native/unsafe cost, not a pure-Rust dependency. That cost is isolated to the
static musl CLI and exposes no allocator API through a Lumin crate boundary.
The extracted locked-source inventory and exact checksums are retained in
`evidence/dependency-cost.json`.

Local diagnostics with the exact CI tool versions passed:

- `cargo-audit 0.22.2 audit --deny warnings` scanned 121 locked packages after
  loading 1,239 RustSec advisories;
- `cargo-deny 0.20.2 --locked check bans licenses sources` reported
  `bans ok, licenses ok, sources ok`.

Public CI remains authority for the clean locked dependency verdict.

## Decision and limits

The measured product value justifies retaining this narrowly scoped allocator
candidate: the system allocator failed the frozen scaling target, while mimalloc
materially improved every measured mode and produced a passing complete matrix
within binary and RSS limits. Independent review must still accept the native C,
unsafe, build-tool, size, and memory costs on the exact candidate commit.

This packet does not close P1-60. Windows, WSL ext4, native Linux, and the
separately blocked WSL `/mnt` diagnostic retain their owner-defined acceptance
requirements.

## Verification

From the repository root:

```text
python reviews/probes/phase1-musl-allocator-selection-2026-09-05/source/verify.py \
  reviews/probes/phase1-musl-allocator-selection-2026-09-05
```
