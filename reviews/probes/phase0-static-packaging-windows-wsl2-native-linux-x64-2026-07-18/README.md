# Phase 0 Standalone Static-Packaging Feasibility Probe

Status: **NEW-STATIC-PACKAGING-01 closure evidence regenerated; independent adversarial review pending**

Source manifest SHA-256:
`38c1a75d06edb12bb2798d93bc1ce788325ca33c6bc12dabd4ef10df943b677c`

The packet-level `SHA256SUMS` binds 73 files and excludes only itself to avoid a
self-referential digest.

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

The seal hashes each detached artifact, directly parses its PE/ELF machine and
linkage from those bytes, copies those exact bytes into a fresh seal-owned directory,
executes that copy, and rehashes it after execution. The run reports the source
manifest and frozen architecture identities compiled into the executable. Verification
requires the detached artifacts again and repeats inspection and execution; evidence
alone is nonauthorizing.

It cannot pass product package behavior, native path/root DTO round trips, packaged
skills, process reopen/recovery/pagination, determinism, or achieved performance
budgets. Those remain Phase 1 acceptance.

See [source/PROBE-CONTRACT.md](./source/PROBE-CONTRACT.md) for the frozen oracle and
hard stops.

## Current Results

| Scope | Artifact | Result |
| --- | --- | --- |
| Windows 11 x64 / NTFS | PE32+ MSVC, 1,412,608 bytes | **PASS** |
| WSL2 Ubuntu 24.04 / ext4 | ELF64 GNU, 1,794,736 bytes | **PASS** |
| WSL2 Ubuntu 24.04 / ext4 | static ELF64 musl, 1,897,184 bytes | **PASS** |
| native Ubuntu 24.04 / ext4 | ELF64 GNU, 1,794,800 bytes | **PASS** |
| native Ubuntu 24.04 / ext4 | static ELF64 musl, 1,901,280 bytes | **PASS** |

Every completed artifact emitted OXC statement count `2`, Rayon sum `4950`, and redb
value `42`. Cargo metadata contains exact direct dependency versions and one known
`rayon-core@1.13.0:rayon-core` `links` sentinel. The pinned Rayon Core build script
states that this sentinel links no native library and exists only to prevent two
Rayon Core versions; any additional `links` declaration is a hard stop.

Every scope also records rejection of a source-identity-tampered artifact, an
unrelated native executable, and pre-existing run output. Both Linux scopes additionally
record direct rejection of the dynamic GNU artifact when labeled as musl. The WSL
runner and an explicit adversarial rerun reject `/bin/true` as GNU with
`run-json-invalid` and as musl with `dynamic-musl-artifact`.

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

The checked-in `runner/workflow.yml` and `runner/run-native.sh` ran the same source on
GitHub's native `ubuntu-24.04` runner:

- workflow run: `29634512936`;
- exact runner commit: `b7560b443d973540020bd2de984a99b69c35d14e`;
- artifact ID: `8426637860`;
- artifact ZIP SHA-256:
  `2f238899ccccbb43a1c345eab3746f68da56a86208ef0d46fa11e36853cbb971`.

`runner/verify-native-download.py` independently rehashes the 21 manifest members and
the two downloaded ELF artifacts, validates their format and run oracle, and rejects
dynamic musl linkage. It also binds the run-v2 source/architecture identities and the
inspection/execution records. Its `72/72` result is retained in
`runner/native-independent-checks.json`. The workflow and artifact are Phase 0 probe
infrastructure, not product CI or a production package.

All three environments executed the same source manifest. The evidence packet is now
complete but does not close the Phase 0 gate until an independent adversarial reviewer
binds the exact candidate and reports PASS or an explicitly accepted risk.
