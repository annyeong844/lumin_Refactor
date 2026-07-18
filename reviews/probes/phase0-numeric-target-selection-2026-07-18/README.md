# Phase 0 Numeric Target Selection

This packet selects numeric Phase 1 targets after the independent closure of
`NEW-FALSE-NEGATIVE-01`. It is a target-selection packet, not product code and
not achieved-product evidence.

## Inputs

- independently passed false-negative baseline candidate:
  `a1e07ed7b9e05181cd58bfba5f3846c1baab8a93`;
- exact legacy timing baseline: `35290cb683a37e83bc8c915d1d0f9ca0f3f96fd0`;
- checked-in OXC stack/jobs evidence under
  `phase0-oxc-memory-stack-windows-wsl2-x64-2026-07-17`;
- checked-in static-packaging evidence under
  `phase0-static-packaging-windows-wsl2-native-linux-x64-2026-07-18`;
- the historical exact legacy pre/post measurement in
  `docs/lab/wsl-prewrite-discovery-measurement-2026-07-11.md` at the legacy
  baseline commit.

The legacy full-audit captures are deliberately labeled timing-only because
their output contains the false-negative `MUTED` contract. They cannot satisfy
the authored truth condition in [TARGET-CONTRACT.md](TARGET-CONTRACT.md).

Files below `evidence/` retain the captured host bytes, including original line
endings, under the repository's `reviews/probes/**/evidence/** -text` rule.
Diff-whitespace checks apply to the packet prose and source paths; evidence
integrity is enforced by `SHA256SUMS` and `source/verify-packet.py`.

## Reproduction

Generate the scale corpus and compare its generated manifest/truth with the
retained evidence:

```text
python source/generate-scale-corpus.py \
  --output <empty-temp-dir>/corpus \
  --manifest <empty-temp-dir>/corpus-manifest.json \
  --truth <empty-temp-dir>/expected-truth.json
```

Capture timing-only legacy baselines from a clean exact checkout with no
arbitrary timeout:

```text
powershell -File source/capture-legacy-windows.ps1 \
  -RepoRoot <exact-legacy-checkout> -OutputRoot <new-output> -Mode cold

bash source/capture-legacy-wsl.sh \
  <exact-legacy-checkout> <new-output> cold
```

Repeat with `warm`. The capture scripts refuse a dirty or wrong-commit legacy
checkout and refuse to overwrite output.

## Claim Boundary

This packet approves only the exact numbers and measurement rules in
[TARGET-CONTRACT.md](TARGET-CONTRACT.md). Phase 1 must still build the public
binary, run all foundation truth corpus rows, prove package/skill/native-path
behavior, and achieve these targets on every blocking environment.
