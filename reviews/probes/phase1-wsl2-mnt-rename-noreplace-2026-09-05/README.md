# WSL `/mnt` No-Replace Namespace Probe

## Scope

This packet records one Phase 1 design blocker; it is not performance PASS
evidence. SLICE-001 requires the report-only WSL `/mnt/<drive>` benchmark to run
the same public lifecycle matrix. ARCH-002 simultaneously requires run
publication to atomically move a validated staging directory without replacing
or disposing any destination winner.

The observed WSL2 drvfs mount supplies neither Linux
`renameat2(RENAME_NOREPLACE)` nor a directory hard-link primitive from which an
equivalent no-replace move can be built. Falling back to `renameat2(..., 0)`
would permit replacement and would weaken the frozen namespace contract, so the
implementation deliberately does not do that.

## Reproduction

From WSL, with this checkout visible under `/mnt/c`:

```text
python3 source/probe.py \
  --parent /mnt/d \
  --output /tmp/wsl2-mnt-drvfs.json
cmp /tmp/wsl2-mnt-drvfs.json evidence/wsl2-mnt-drvfs.json
python3 source/verify.py .
```

The retained run used WSL2 kernel
`6.6.87.2-microsoft-standard-WSL2`, Python `3.12.3`, and the `/mnt/d` `9p`
mount whose super options identify `aname=drvfs`.

## Result

- no-replace rename of an unoccupied regular-file destination: `EINVAL`;
- no-replace rename of an unoccupied directory destination: `EINVAL`;
- flags-zero directory rename: succeeds, but is not a no-replace primitive;
- regular-file hard link: succeeds;
- directory hard link: fails.

The source survived and destination remained absent after both rejected
no-replace calls. The complete machine-readable observation is
`evidence/wsl2-mnt-drvfs.json` and is authenticated by
`evidence/SHA256SUMS`.

## Required Review

P1-60 remains open. The owner contract must choose one implementable outcome
before `/mnt` can supply the mandated same-metrics diagnostic:

1. define a separately reviewed crash-consistent publication protocol available
   on drvfs without weakening foreign-winner preservation; or
2. amend the report-only diagnostic to record an explicit unsupported namespace
   capability result rather than claiming that the full lifecycle matrix ran.

Neither choice is made by this probe, and neither blocking Windows, WSL ext4,
nor native-Linux budget is relaxed.
