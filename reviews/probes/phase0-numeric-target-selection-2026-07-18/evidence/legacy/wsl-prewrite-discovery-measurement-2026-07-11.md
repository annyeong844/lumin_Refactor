# WSL pre-write discovery measurement (2026-07-11)

## Scope

Measured the packaged Linux `lumin-audit-core` against the 564-file maintainer
repository on a WSL `/mnt/c` checkout. Result files and temporary comparison
caches were written under `/tmp` unless the cache-location comparison required
the mounted checkout.

## Observations

| Mode | Elapsed | User CPU | System CPU |
| --- | ---: | ---: | ---: |
| no incremental cache | 11.21s | 1.00s | 1.47s |
| cold strict cache | 9.31s | 1.44s | 2.32s |
| warm strict cache, 564/564 facts reused | 8.67s | 0.18s | 0.65s |

The warm run removed OXC parsing but retained most wall time. Host Git clean
identity cost approximately 0.7s across root, status, and stage queries. The
remaining dominant work is repository filesystem discovery on DrvFS plus final
projection/result serialization.

Host Git visible tracked/untracked inventory matched the checked walker exactly
for this repository (564 files) and completed in 0.34s. It was not selected as
the product fix because Git-only discovery can omit ignored authored source in
other repositories. Enumerating every ignored path took 7.66s and emitted a
3.9 MiB path stream, so adding that query would erase the benefit.

## Decision

Keep the checked filesystem scan semantics. Parallelize independent,
already-sorted directory subtree walks on a local Rayon pool. Worker results
must be merged in input order at each directory and then sorted/deduplicated so
concurrency cannot change artifacts or error selection. Do not add a
repository-size cap, elapsed timeout, Git-only source scope, or stat/mtime
absence claim.

## Result

The checked recursive directory-job implementation was measured with the same
564-file request and a locally built static Linux audit-core:

| Mode | Before | After |
| --- | ---: | ---: |
| no incremental cache, repeated warm filesystem | 7.48-9.03s | 4.06-4.59s |
| cold strict cache | 9.31s | 6.58s |
| warm strict cache, all facts reused | 8.67s | 5.17-5.22s |

The old and new no-cache results had identical `files`, `symbols`, and
`topology` projections. The v45 cache attempted to avoid mounted-worktree reads
by using clean Git blob identities and bytes. That optimization was retired in
v46 because Git filters, LFS, and working-tree encodings can make repository
blob bytes differ from the file the user is reviewing.

The regenerated packaged skill was then dogfooded on the same WSL checkout with
`LUMIN_AUDIT_CORE_NO_AUTO_BUILD=1`, proving that no Cargo/source fallback was
used. Lifecycle-only pre-write completed in 6.56s and the paired post-write
completed in 5.01s with `No silent new any`.

## Follow-up phase profile

Temporary in-process timing showed that the remaining wall time was not final
projection or result serialization:

| Phase | Observed |
| --- | ---: |
| repository discovery | 3.27-5.32s |
| extraction/cache | 0.81-1.00s |
| projection | 0.04-0.07s |
| result JSON write | < 0.01s |

Increasing the local discovery pool from 8 through 48 workers did not produce
a stable winner. The bottleneck remained DrvFS traversal, especially source-
empty `docs`, `experiments`, `tools`, and generated skill subtrees. A direct
probe of the already-packaged Windows x64 Rust helper completed the same
no-cache request in 1.77s. Its `files`, `symbols`, `topology`, `summary`, and
type-escape inventory matched the Linux result canonically. The next slice
therefore routes only this WSL-mounted pre-write evidence command through the
current-contract Windows helper instead of adding more Linux walker threads or
weakening scan scope.

The implemented source bridge was then dogfooded with Cargo auto-build disabled:

| Lifecycle route | Elapsed |
| --- | ---: |
| cold pre-write | 2.97s |
| warm pre-write, 564/564 facts reused | 3.45s |
| paired post-write | 3.31s |

The returned evidence restored `root`, inventory root, cache root, and cache
file paths to WSL spelling. Temporary host result directories were removed
after each command, and the paired post-write completed without a silent-new-
any delta.

## Worktree-byte correction

The v46 cache reads every scoped file from the current worktree, computes
SHA-256 over those exact bytes, and passes the same buffer to OXC on a miss.
It does not consult Git status, index entries, or blob content for identity.
The WSL bridge keeps this affordable by running the evidence command through
the exact-contract Windows release helper, where the reads use native NTFS.

Dogfood with Cargo auto-build disabled and both packaged-platform contracts
held at v46 produced:

| Lifecycle route | Elapsed |
| --- | ---: |
| cold pre-write | 2.10s |
| warm pre-write, 564/564 facts reused | 1.79s |
| paired post-write, 564/564 facts reused | 1.73s |

All three passes reported `identityMode: "sha256"`,
`contentHashFiles: 564`, and `gitBlobFiles: 0`. The post-write result reported
`No silent new any in the scan range.`

The package builder now rebuilds the current-platform helper with Cargo's
release profile. A regenerated skill was then run with Cargo absent from
`PATH`, auto-build disabled, and no binary overrides. Its packaged Linux and
Windows helpers alone completed cold pre-write in 3.46s and repeated warm
pre-write in 3.07-3.62s; packaged post-write completed in 2.87s with no silent
new any. The remaining gap from the source measurement is the
cost of launching lifecycle-control calls from the packaged Linux binary on
the mounted `/mnt/c` tree; the repository discovery/read/hash pass itself still
uses the packaged Windows release helper.
