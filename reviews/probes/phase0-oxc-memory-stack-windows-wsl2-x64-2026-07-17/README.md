# Phase 0 OXC Memory and Worker-Stack Evidence

Status: **PASS for the named Windows NTFS and WSL2 ext4 feasibility matrix; not a
Phase 0 freeze approval**.

Architecture identity:

- commit `65e60216891bb3d826a4778f84cb8aaa377abe92`;
- 16-file manifest `66925583362be22257dd7357072d176cd3f1ae3d4f99874a53f6689b7cf535e0`.

This packet is a standalone Phase 0 probe. It does not create a Phase 1 product
crate, public API, scheduler, cache, or analysis implementation.

## Fixed Inputs

- OXC crates: exact `0.126.0`;
- Rust toolchain: exact `1.96.0`;
- realistic source: `annyeong844/lumin_lab` exact commit
  `35290cb683a37e83bc8c915d1d0f9ca0f3f96fd0`;
- realistic corpus: 705 Git blobs / 7,302,528 bytes;
- explicit stack fixtures: 4 files / 170,248 bytes;
- total corpus: 709 files / 7,472,776 bytes;
- corpus manifest SHA-256:
  `82baf59e6c3708e1f68c36d711bb47cbe081d468f3b554ac93cf7af36a85eb9b`;
- probe source manifest SHA-256:
  `e9b405f9816cbf6d0276b22d4dbb2f4c80b35a2839fb05704541401a11b2a024`.

The exporter reads exact blob bytes with `git ls-tree` and `git cat-file --batch`.
An initial Windows `git archive` trial was correctly rejected because checkout EOL
conversion changed the byte total. Dirty worktree bytes are never read.

## Result

Every successful child and every wave produced the same semantic digest:

```text
28aa44da0f371535feb01052cfd1c9298862c247110a76b5f5ae696d9f1b8721
```

| Observation | Windows x64 / NTFS | WSL2 x64 / ext4 |
| --- | ---: | ---: |
| Available parallelism | 12 | 12 |
| `256 KiB` stack | stack overflow | stack overflow |
| `512 KiB` stack | stack overflow | stack overflow |
| `1/2/4/8 MiB` stack | PASS | PASS |
| Jobs `1/2/4/8/12`, three waves | PASS | PASS |
| One-worker allocator lifetime, eight waves | PASS | PASS |
| Parse diagnostics / parser-panicked files | 0 / 0 | 0 / 0 |
| Maximum observed process peak RSS | 16,035,840 B | 16,367,616 B |
| Release binary | 1,384,448 B | 1,920,328 B |

The minimum observed passing stack was 1 MiB on both hosts. This is **not** a
selected product stack policy. The 4 MiB and 8 MiB passes are only the probe-validity
floor defined before execution.

The largest per-file OXC allocator observation was the synthetic 4,096-declaration
fixture: 3,948,520 used bytes and 8,371,648 capacity bytes. The per-wave capacity sum
of 85,391,168 bytes is the sum of sequential file-local allocators, not simultaneous
resident memory.

Jobs scaling observations:

| Jobs | Windows child / peak RSS | WSL2 child / peak RSS |
| ---: | ---: | ---: |
| 1 | 1.425 s / 11,042,816 B | 0.543 s / 9,445,376 B |
| 2 | 1.057 s / 12,029,952 B | 0.330 s / 10,092,544 B |
| 4 | 0.806 s / 13,332,480 B | 0.246 s / 11,927,552 B |
| 8 | 0.662 s / 14,286,848 B | 0.220 s / 13,819,904 B |
| 12 | 0.636 s / 16,035,840 B | 0.255 s / 16,367,616 B |

Eight-wave post-drop current RSS remained within 6,598,656-7,475,200 bytes on
Windows and 5,877,760-6,336,512 bytes on WSL2. This is a bounded observation over
eight waves, not a proof that every future corpus is leak-free.

## Evidence Integrity

- strict package manifest: [evidence/SHA256SUMS](./evidence/SHA256SUMS), SHA-256
  `bfba3524182822ebb9e7ec35c37ae08a1b03380fa0f961675499eef5031790be`;
- cross-platform summary: [evidence/summary.json](./evidence/summary.json), SHA-256
  `2f73daba1fa12b6a518962cab16400faeb275fca08296c02a9ec442f51c9c1c6`;
- raw child reports and logs: `evidence/windows/` and `evidence/wsl2/`;
- source and exact contract: [source/](./source/).

The strict packager rejected both exercised negative controls:

1. one byte appended to a raw jobs report;
2. a matrix semantic digest changed without changing child reports.

The observed rejection classes are recorded in
[evidence/negative-controls.json](./evidence/negative-controls.json).

Windows and WSL2 both passed `cargo fmt --check`, `cargo test --locked`, and
`cargo clippy --all-targets --locked -- -D warnings`. No command used a wall-time,
file-count, or worker-count cutoff.

## Decision Boundary

This packet supports OXC allocator-lifetime and explicit worker-stack feasibility on
the named corpus. It does not approve:

- a product worker stack or default worker count;
- Phase 1 time, RSS, or binary-size budgets;
- native Linux or musl release behavior;
- the Lumin scheduler, cache, facts, gate behavior, or packaged skills.

Those remaining execution gates keep the overall Phase 0 freeze **BLOCKED**.
