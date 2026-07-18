# Phase 0 Standalone Static-Packaging Feasibility Probe

Status: **Windows NTFS and WSL2 ext4 PASS; native Linux ext4 execution pending**

Source manifest SHA-256:
`dd30eeda67caf9e354838a9ec7974cdd3dc118a9136c2556fcfe56c9f441db45`

## Scope

This standalone harness tests whether the frozen heavy production dependency and link
surface can compile, link, and start on:

- Windows x64 MSVC on NTFS;
- WSL2 Ubuntu 24.04 on ext4, GNU and musl targets;
- native non-WSL Ubuntu 24.04 on ext4, GNU and musl targets.

It links exact `redb 4.1.0`, OXC `0.126.0`, and Rayon `1.11.0` under exact Rust
`1.96.0`. The executable performs only constant-input dependency smoke checks. It is
not named `lumin`, exposes no product command or DTO, reads no repository, packages no
skill, and implements no gate/query/process behavior.

## Claim Boundary

The probe may establish only:

- target toolchain and linker viability;
- PE/ELF x64 artifact viability;
- exact dependency-lock viability;
- the exact Cargo `links` surface, limited to Rayon Core's non-native uniqueness
  sentinel with no unexpected declaration;
- static musl linkage without `PT_INTERP` or dynamic `NEEDED` entries;
- successful startup of the linked dependency islands.

It cannot pass product package behavior, native path/root DTO round trips, packaged
skills, process reopen/recovery/pagination, determinism, or achieved performance
budgets. Those remain Phase 1 acceptance.

See [source/PROBE-CONTRACT.md](./source/PROBE-CONTRACT.md) for the frozen oracle and
hard stops.

## Current Results

| Scope | Artifact | Result |
| --- | --- | --- |
| Windows 11 x64 / NTFS | PE32+ MSVC, 1,411,584 bytes | **PASS** |
| WSL2 Ubuntu 24.04 / ext4 | ELF64 GNU, 1,795,120 bytes | **PASS** |
| WSL2 Ubuntu 24.04 / ext4 | static ELF64 musl, 1,897,184 bytes | **PASS** |
| native Ubuntu 24.04 / ext4 | GNU and static musl | **PENDING** |

Every completed artifact emitted OXC statement count `2`, Rayon sum `4950`, and redb
value `42`. Cargo metadata contains exact direct dependency versions and one known
`rayon-core@1.13.0:rayon-core` `links` sentinel. The pinned Rayon Core build script
states that this sentinel links no native library and exists only to prevent two
Rayon Core versions; any additional `links` declaration is a hard stop.

Raw evidence is sealed independently under:

- `evidence/windows-ntfs/SHA256SUMS`;
- `evidence/wsl2-ext4/SHA256SUMS`;
- `evidence/native-linux-ext4/SHA256SUMS` after the native runner completes.

## Reproduction

Windows PowerShell from the repository root:

```powershell
& reviews/probes/phase0-static-packaging-windows-wsl2-native-linux-x64-2026-07-18/source/scripts/run-windows.ps1
```

WSL2 from an exact source-manifest copy on `/home` ext4:

```bash
bash source/scripts/run-linux.sh \
  --scope wsl2-ext4 \
  --evidence evidence-wsl2
```

The checked-in `runner/workflow.yml` and `runner/run-native.sh` run the same source on
GitHub's native `ubuntu-24.04` runner. They are copied to a temporary runner branch;
the workflow and uploaded artifact are not product CI or a production package.

Evidence is accepted only when all three environments execute the same source
manifest. Until the native packet passes, this Phase 0 gate remains pending.
