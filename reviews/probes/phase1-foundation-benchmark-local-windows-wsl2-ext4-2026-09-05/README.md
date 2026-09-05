# Phase 1 Foundation Benchmark: Local Windows and WSL2 ext4

Status: **author measurement candidate; P1-60 remains open**

## Scope

This packet retains complete local benchmark reports for two blocking
environments named by SLICE-001:

- packaged Windows x64 on NTFS; and
- packaged static Linux x64 under WSL2 on ext4.

Both reports were produced by `lumin-xtask benchmark foundation` from packages
with the same build ID. Each report contains the full seven-mode matrix with
three repetitions per mode, not a reduced smoke test.

## Result

| Observation | Windows NTFS | WSL2 ext4 |
| --- | ---: | ---: |
| report status | PASS | PASS |
| samples | 21 | 21 |
| cold-audit scaling ratio | 0.466564 | 0.733024 |
| peak product-process RSS | 68,857,856 bytes | 137,637,888 bytes |
| packaged executable | 10,478,080 bytes | 11,913,968 bytes |
| worker stack | 4,194,304 bytes | 4,194,304 bytes |
| target misses | 0 | 0 |

The reports agree on the exact 780-file, 7,461,511-byte fixture, its authored
truth digest, the 256-entry finding-ID map, and its semantic digest. Every
product process exited successfully, spawned no analysis child, and reproduced
that same semantic digest for default jobs and `jobs=1`.

## Authority boundary

These retained local observations do not close P1-60. Native Linux CI still
owns the release-host benchmark verdict and clean-checkout package evidence.
The report's `PASS` describes the measured numeric budget comparisons. Its
`actualJobs` values are derived from the requested/default policy, and
`workerStackBytes` is checked against the source policy; neither is runtime
telemetry from the product. `stageTimingsNanoseconds` measures fixture, setup,
product-process, and truth-validation steps, not engine-owner stages. The
runtime observability required by ARCH-001 Sections 4 and 12 therefore remains
open under [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
The report-only WSL `/mnt/<drive>` run is separately blocked by the documented
no-replace namespace capability gap in
`../phase1-wsl2-mnt-rename-noreplace-2026-09-05/`; this packet neither waives
that requirement nor changes the frozen architecture.

## Verification

From the repository root:

```text
python reviews/probes/phase1-foundation-benchmark-local-windows-wsl2-ext4-2026-09-05/source/verify.py \
  reviews/probes/phase1-foundation-benchmark-local-windows-wsl2-ext4-2026-09-05
```
