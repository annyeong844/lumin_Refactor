# Phase 0 Static-Packaging Independent Review Request

Status: **ready for an external independent verdict; author-side output is not PASS evidence**

## Exact Subject

- Repository: `annyeong844/lumin_Refactor`
- Exact evidence candidate: `e0a2810b46f6091895b5e9f7dd4454e8854fee0e`
- Candidate parent: `47f8fa9693bb8ecae2cff3ff72e54b0d259676d0`
- Gate base before probe work: `84fe32d2e57ec10399964999a4a5a60563944a2b`
- Commit message: `Normalize static packaging packet manifest`
- Candidate tree: `bed2f4dde295bcb0b0b9eaf78d20877468d236fd`
- Packet manifest: 65 entries; SHA-256
  `b1c685a535a5c5e36011de7b59fd89d8400a29d0b5d06db874b629c6e8180ed7`
- Source manifest: 9 entries; SHA-256
  `dd30eeda67caf9e354838a9ec7974cdd3dc118a9136c2556fcfe56c9f441db45`
- Frozen architecture candidate: `9a0dbe5c89463892c001e864c4f18eeab9e0eaed`
- Frozen architecture manifest SHA-256:
  `e2ca379a8a659f2febbc4e277c89db67bb02035a6b10467cf78a5663f21cd99a`

The candidate adds only a Phase 0 non-product probe packet under
`reviews/probes/phase0-static-packaging-windows-wsl2-native-linux-x64-2026-07-18/`.
It does not add a Rust workspace, production crate, public `lumin` command, DTO, skill,
gate/query/process behavior, repository scanner, or product scaffold.

## Evidence Claimed

The same source manifest and exact Rust `1.96.0` linked exact redb `4.1.0`, OXC
`0.126.0`, and Rayon `1.11.0` on five target/scope pairs:

| Scope | Artifact | Manifest | Claimed result |
| --- | --- | --- | --- |
| Windows 11 x64 / NTFS | PE32+ MSVC | 11 entries, `0058fc94…6117` | PASS |
| WSL2 Ubuntu 24.04 / ext4 | ELF64 GNU + static musl | 18 entries, `adbf0749…5bf2` | PASS |
| native Ubuntu 24.04 / ext4 | ELF64 GNU + static musl | 18 entries, `3f43d654…2523` | PASS |

Every run must report OXC statement count `2`, Rayon sum `4950`, and redb value `42`.
The musl artifacts must have no interpreter or dynamic `NEEDED` entry. Cargo's exact
`links` surface is one pinned Rayon Core uniqueness sentinel and no other declaration.

The transport ZIP also carries the Windows and WSL binaries plus the exact native
GitHub Actions artifact ZIP. These detached binaries are review evidence, not Git
packet members or product packages.

## Independence Boundary

This request, `verify_candidate.py`, `author-preflight.json`, and the probe's own seal
scripts were produced by the authoring session. They are consistency aids only. The
external reviewer must open the complete-history bundle independently, address the
full immutable candidate SHA, rehash exact Git blobs and detached artifacts, inspect
the source and oracle, and derive the verdict without treating any author PASS as
approval evidence.

## Required Independent Review

1. Bind the exact candidate, parent, tree, subject, two-commit gate range, and unchanged
   16-file architecture packet.
2. Rebuild the 65-entry packet manifest from exact candidate Git blobs.
3. Rebuild the 9-entry source manifest and all three evidence manifests; reject unsafe,
   duplicate, missing, extra, or mismatched paths.
4. Inspect the harness and runners and decide whether they remain inside the Phase 0
   standalone toolchain/linker/artifact/dependency boundary without emulating product
   behavior or creating a scaffold.
5. Verify exact Rust/dependency resolution, the Rayon Core `links` sentinel
   interpretation, and absence of any unexpected Cargo `links` declaration.
6. Rehash and inspect all five detached binaries; verify PE32+/ELF64 x86-64 identity,
   exact sizes, run outputs, and static musl linkage.
7. Bind native evidence to workflow run `29629760482`, runner commit
   `721984d52e75d2385948767ce8ade6f190babaf2`, artifact ID `8425075747`, and artifact
   ZIP SHA-256 `073ef5907944f8b79df8eab07d135826365f143c4d590ee3d59d7f57d5926454`.
8. Confirm the packet makes no product-package, path/root DTO, packaged-skill,
   public-process, determinism, achieved-budget, or runtime-without-Cargo claim.
9. Report any H/R/product regression or new finding and preserve `AR-BACKEND-01` and
   `AR-MEASURE-01` unless exact counter-evidence reopens them.

Use [review-template.md](./review-template.md). On PASS, the static-packaging gate
closes, but overall Phase 0 and Phase 1 remain blocked by clean pinned-upstream
provenance reproduction and numeric Phase 1 target approval.
