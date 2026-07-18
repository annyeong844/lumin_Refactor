# Phase 0 Numeric Target Contract

## Authority Boundary

This packet approves Phase 1 target numbers. It does not claim that an
unimplemented product has achieved them. Legacy output is timing evidence only;
legacy `MUTED`, caps, classifications, and artifact shape are never product
truth.

The blocking Phase 1 benchmark uses the generated
`phase1-scale-findings.v1` corpus. Its expected finding tuples are authored by
`source/generate-scale-corpus.py`, not copied from legacy or product output.
Every measured sample is invalid unless the public binary returns all 256
grounded findings through an unfiltered query with `filters: {}`,
`scopeTotal == total == 256`, the exact 128/64/64 disposition breakdown, no
limitations, the authored source-role reasons, and the same stable finding IDs
in cold, warm, `jobs=1`, and default-jobs runs. The authored truth owns semantic
finding tuples rather than copying opaque ID bytes from an implementation; the
product must provide a one-to-one tuple-to-ID mapping and preserve those IDs in
every compared run.

## Approved Targets

All byte values are exact. All times measure process start through complete
machine-response flush and process exit.

| Dimension | Phase 1 target |
| --- | ---: |
| Cold full audit p50 | `<= 30,000 ms` |
| Warm unchanged audit p50 | `<= 8,000 ms` |
| Cold pre-write p50 | `<= 6,000 ms` |
| Warm pre-write p50 | `<= 4,000 ms` |
| Post-write, one changed file p50 | `<= 4,000 ms` |
| Post-write, 32-file wave p50 | `<= 8,000 ms` |
| Peak product-process RSS | `<= 536,870,912 bytes` |
| Rayon worker stack | exactly `4,194,304 bytes` |
| Default jobs | `max(1, min(8, available_parallelism))` |
| Default-jobs cold full-audit p50 on hosts exposing at least 4 workers | `<= 75%` of `jobs=1` p50 |
| Each stripped, uncompressed release `lumin` executable | `<= 12,582,912 bytes` |

## Measurement Rules

1. Each time target uses the median of three valid repetitions on each blocking
   environment. Peak RSS uses the maximum observed value across every valid
   repetition and mode.
2. Cold means a fresh repository copy, fresh `.lumin` namespace, and new
   process. It does not claim an OS-cold page cache. The harness records the OS
   cache state without flushing machine-global caches.
3. A warm repetition first completes an unmeasured seed run, then measures the
   unchanged repository with the same valid state namespace in a new process.
4. The one-file wave changes only
   `packages/pkg-00/src/live/live-000.ts`. The 32-file wave changes
   `live-000.ts` through `live-003.ts` in each of the eight packages. Changes
   alter numeric values without changing imports, exports, source roles, or the
   authored finding tuple set.
5. The product is invoked directly. Runtime Node, Cargo, semantic fallback,
   child analyzers, hidden worker caps, arbitrary timeouts, sampling, result
   caps, implicit finding filters, and scope reduction invalidate the sample.
6. `available_parallelism` is the quota-aware Rust runtime observation and is
   recorded with requested/actual jobs and the exact stack size. A host exposing
   fewer than four workers still runs determinism and absolute budgets, but does
   not authorize the scaling-ratio row.
7. RSS is the maximum resident set of the one native product process. A product
   analysis child process is a contract failure rather than memory excluded from
   the total.
8. Blocking environments are native Windows on NTFS, WSL2 on ext4, and the
   declared release-compatible native Linux CI host. Each reports CPU, logical
   processors, memory, OS/kernel, filesystem, package identity, and binary
   digest. WSL `/mnt/<drive>` remains report-only.
9. A target miss is a Phase 1 slice failure or a reviewed contract amendment.
   CI cannot relax a number after seeing the result.

## Selection Rationale

- The exact legacy full audit over 2,038 files and 400,155 lines took about
  199 seconds cold and 59 seconds warm on Windows, and 141 seconds cold and 27
  seconds warm on WSL2 ext4. Those runs retained the old artifact warehouse and
  nine muted rows, so they establish cost only.
- The exact packaged legacy lifecycle over 564 files took 3.46 seconds cold,
  3.07-3.62 seconds warm, and 2.87 seconds post-write with Cargo unavailable.
  The stronger Phase 1 contract therefore receives explicit margin at 6/4/4
  seconds and an 8-second 32-file wave.
- The OXC probe passed 1 MiB stacks but failed 256 and 512 KiB. Four MiB passed
  every stack and jobs run and leaves a fourfold margin over the observed
  minimum.
- The OXC jobs matrix improved from `jobs=1` to `jobs=8` by more than 50% on
  both Windows and WSL2. Eight is the bounded default; the 25% whole-product
  improvement requirement leaves room for serial graph and persistence work.
- The largest selected static-packaging probe executable is 1,901,280 bytes.
  The 12 MiB product limit leaves more than six times that standalone surface
  while still preventing dependency and binary-size drift.
- The 512 MiB RSS limit is deliberately far below the legacy Node cold peak but
  far above the bounded parser and store probes. It requires the ownership and
  allocator-lifetime design to pay off without inventing truncation.
