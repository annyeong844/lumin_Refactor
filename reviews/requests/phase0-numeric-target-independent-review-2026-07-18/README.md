# Phase 0 Numeric-Target Independent Review Request

Status: **ready for an external exact-tree verdict; author checks are not PASS authority**

## Exact Subject

- Repository: `annyeong844/lumin_Refactor`
- Candidate: `a410605ff9f5512cadd1cb105d346444044398ce`
- Parent: `5125b4b2ade1c98ff9fc667a7363a659c6799564`
- Tree: `529a35b9ce677d2a241dd059f81769f8a427a182`
- Subject: `Select Phase 1 numeric targets`
- 16-file B-07 manifest SHA-256:
  `6b23f500e633f28611c27bedb9998fb7ff44a69af6682d58b8554e7b5b33d86c`
- 42-entry numeric packet manifest SHA-256:
  `a749a5d80600295fe765edbccf8ed23be170a9211151186923e2f3040349c2f4`
- Generated 780-file corpus content SHA-256:
  `9e51b070a934c027e6d2d9a4610fac764592ecb8f05bf41a2ab6f5eb46158d3e`
- Later ledger-only commit: `bee8a2ab685862825c9fcd0efd9ef147715b94c3`
- Author consistency checks: `345/345`; captured Windows output SHA-256
  `f152f92f389809930cd5c95d5b310301e40ee06ea14f52ec906f15aa15d6d065`;
  checked-in LF-normalized copy SHA-256
  `c1a316e84a34bb058b8ceeeb1000b632b7ae7bbdde08e611d24c7c41fb632358`

The candidate changes three existing owner/control files and adds one standalone
numeric-selection packet:

```text
WORKBOARD.md
architecture/001-execution-and-ownership.md
specs/001-foundation-slice.md
reviews/probes/phase0-numeric-target-selection-2026-07-18/**
```

It contains no product implementation, workspace, crate, package, skill adapter,
fixture for product acceptance, or production scaffold.

## Candidate Contract

The blocking generated corpus has 780 files, 7,461,511 bytes, and 256 authored
grounded finding tuples:

```text
128 ReviewCandidate
64 generated ReviewOnly
64 vendored ReviewOnly
```

Every valid measured run must use `filters: {}`, return
`scopeTotal == total == 256`, report no limitations, preserve the authored
source-role reasons, and map every semantic tuple one-to-one to the same stable
finding ID across cold, warm, `jobs=1`, and default-jobs runs.

The selected targets are:

| Dimension | Target |
| --- | ---: |
| Cold full audit p50 | `<= 30,000 ms` |
| Warm unchanged audit p50 | `<= 8,000 ms` |
| Cold pre-write p50 | `<= 6,000 ms` |
| Warm pre-write p50 | `<= 4,000 ms` |
| Post-write, one changed file p50 | `<= 4,000 ms` |
| Post-write, exact 32-file wave p50 | `<= 8,000 ms` |
| Peak product-process RSS | `<= 536,870,912 bytes` |
| Rayon worker stack | exactly `4,194,304 bytes` |
| Default jobs | `max(1, min(8, available_parallelism))` |
| Default-jobs cold p50, host exposes at least 4 workers | `<= 75%` of `jobs=1` p50 |
| Each stripped uncompressed release binary | `<= 12,582,912 bytes` |

Time is the median of three valid repetitions per blocking environment. RSS is
the maximum across all valid repetitions and modes. Cold means a fresh repository
copy, `.lumin` namespace, and process, not an OS-cold page cache.

## Evidence Boundary

- Exact legacy commit `35290cb683a37e83bc8c915d1d0f9ca0f3f96fd0`
  supplies timing and RSS context only. Its `MUTED` output is never product truth.
- The authored generator owns expected semantic tuples. It does not copy product
  finding IDs or legacy classifications.
- Existing OXC evidence supplies bounded allocator/stack/jobs feasibility.
- Existing static-packaging evidence supplies executable-size context.
- This packet approves targets only. Phase 1 must build the product and achieve
  them through the public binary on every blocking environment.
- Files under packet `evidence/` retain exact host bytes and line endings under
  the repository `-text` rule. Their integrity is hash-based.

## Required Independent Review

1. Bind the exact candidate, parent, tree, subject, diff scope, all 16 B-07
   blobs, the 42-entry packet manifest, and every packet member.
2. Regenerate the scale corpus in a new empty directory and verify its 780-file
   inventory, byte count, content digest, 256 unique authored tuples, role and
   disposition breakdown, root entry, and mutation waves.
3. Decide whether every target is explicit, measurable, demanding but feasible,
   and supported by the named evidence without treating legacy output as truth.
4. Try to authorize a sample after omitting `ReviewOnly`, applying an implicit
   filter, changing `scopeTotal`/`total`, changing a reason or tuple-to-ID map,
   returning a limitation, reducing scope, sampling, timing out, capping output,
   or spawning a hidden analyzer. Each route must invalidate the sample.
5. Review cold/warm lifecycle, three-repetition p50, maximum RSS, exact one-file
   and 32-file waves, direct-process timing, process exit/flush, host identity,
   and `/mnt/<drive>` report-only semantics for ambiguity or gaming paths.
6. Review the exact 4 MiB stack, bounded default-jobs formula, 75% scaling row,
   512 MiB RSS, and 12 MiB binary target against OXC/static/legacy evidence.
7. Check owner/Cargo feasibility and confirm no hidden worker/stack cap, product
   scaffold, runtime Node/Cargo dependency, or Phase 1 achieved-product claim was
   introduced.
8. Re-run B-07, H-01 through H-12, R3/R4, NEW-H10-01, NEW-H11-01,
   NEW-PHASE-GATE-01/F-12, NEW-FALSE-NEGATIVE-01, prior evidence gates, Product
   AC 22/22, Slice AC/trace 38/38, and accepted-risk regression scope.
9. Decide whether approval closes `AR-MEASURE-01`, whether Phase 0 can freeze,
   and whether Phase 1 may begin after the external verdict is recorded.
10. Report `PASS`, `REOPEN`, or a new finding. Do not use this README, the author
    verifier, or `author-checks.json` as evidence that the target choice is sound.

## Consistency Check

From a clone containing the candidate and request commit:

```text
python reviews/requests/phase0-numeric-target-independent-review-2026-07-18/verify_candidate.py --repo .
```

The script checks immutable Git bytes, packet framing, generated corpus
reproduction, and contract anchors. It does not approve the target values.
