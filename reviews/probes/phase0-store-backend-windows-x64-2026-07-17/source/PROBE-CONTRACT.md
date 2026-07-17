# Store Backend Probe Contract

## Frozen source

- Repository: `annyeong844/lumin_Refactor`
- Architecture commit: `65e60216891bb3d826a4778f84cb8aaa377abe92`
- Candidate manifest SHA-256:
  `66925583362be22257dd7357072d176cd3f1ae3d4f99874a53f6689b7cf535e0`
- Owner section: `architecture/002-evidence-and-write-gate.md`, Sections 2.0-2.6

This harness does not infer product behavior from either backend. Every oracle below
comes from the frozen backend-neutral contract.

## Atomic admission

### Conflicting writers

Two independent child processes race to admit the same logical path. Exactly one must
commit, the other must report that exact winner, and a fresh transaction must read the
same canonical holder.

### Disjoint writers

Two independent children race on distinct logical paths. Both must commit and a fresh
transaction must observe both exact holders. Backend serialization is allowed; data
loss, false conflict, a daemon, or a second truth owner is not.

### Process death before commit

A child inserts a lease in an uncommitted transaction, publishes a test checkpoint,
and is forcibly terminated. A fresh process must reopen the store and admit the path.

### Process death after durable commit

A child durably commits, publishes a test checkpoint while its handle remains open,
and is forcibly terminated. A fresh process must reopen, report conflict with the
committed holder, and preserve it as canonical.

## Backend contract and fault matrix

Each backend must pass all of these cases before its performance numbers are ranked:

- bounded indexed traversal of 1,000 ordered records in pages of 37, with exact
  no-gap/no-duplicate reconstruction;
- visible hard failure after deliberate canonical database-header corruption;
- process death at all eight publication boundaries, including orphan-run and pointer
  recovery;
- reverse sequence 10/11 publication and same-sequence `Running -> Terminal` races;
- both forced orders of publication versus retention confirmation under one exclusive
  catalog guard: publication-first makes retention stale, retention-first blocks the
  pointer;
- process death at all six retention boundaries, plus both-canonical-and-trash and
  neither-canonical-nor-trash integrity hard-stops;
- process death at all five migration boundaries and rejection of an old-generation
  late writer;
- state-directory, lifecycle-lock, all four managed-parent, and all four anchor
  replacement races, including run publication and trash moves immediately before
  physical mutation, after physical mutation, and before canonical commit;
- lifecycle-lock and anchor content mutation and extra-hard-link races.

Namespace cases capture stable filesystem/volume and object identity through no-follow
handles. Mutable link count is a live integrity observation, not part of persistent
object identity: lifecycle-lock and anchor files must remain one-link objects, while a
directory's Unix `st_nlink` may legitimately change as child directories are created or
moved. Every case records one of two noninterchangeable outcomes:
`injected-and-detected`, where the child revalidates to a typed hard-stop, or
`kernel-prevented-before-displacement`, where Windows refuses a state/managed-parent
directory rename while the complete bound handle set is held. Kernel prevention is
accepted only for the named directory-replacement cases, includes the raw OS error,
terminates the waiting child, and must leave the canonical store uncommitted. It is
Windows evidence, not a substitute for an injected-and-detected result on a platform
that permits displacement.

## Measurement method

The workload is deterministic: 10,000 ordered records with 256 payload bytes each.
The benchmark records:

- database initialization and one durable bulk insert;
- the first query after closing and reopening the process-local database handle;
- 100 further close/reopen bounded-query samples with p50/p95/p99 and mean;
- 200 independent durable admission transactions with p50/p95/p99 and mean;
- process peak working set, canonical store bytes, and feature-specific executable
  bytes.

The first reopen sample is not called an OS-cold measurement because this harness does
not flush the operating-system page cache. Build evidence separately records clean and
incremental feature-specific build time, dependency count/tree, binary bytes, selected
dependency Rust `unsafe` keyword-line count, and bundled native-source files/bytes.
The keyword count is a comparison surface, not a safety audit.

## Rejection and evidence scope

Any correctness invariant failure rejects that backend before performance ranking. A
harness watchdog expiration is a correctness failure (`Wedged`), not a product timeout
policy. Each platform result is partial Phase 0 evidence only. Windows x64 and WSL2
ext4/musl observations do not replace native Linux filesystem/package evidence and
cannot satisfy native-path, OXC, or numeric-budget gates or select a production backend
by themselves.

Each executable embeds the exact probe-source byte hashes used to compile it. The
packager invokes each measured binary's `identity` command, recomputes that embedded
source manifest, requires admission/fault/benchmark/current-tree agreement, and binds
every report to the on-disk SHA-256 and byte length of the measured release binary.
Runtime source-tree reads are a drift check, not executable provenance.

## Backend configurations

- `redb 4.1.0`: one database handle per repository-lock transaction; durable commits;
  `DatabaseAlreadyOpen` is retried by reopening after the competing process closes.
- `rusqlite 0.39.0` with bundled SQLite: WAL initialized once, `synchronous=FULL`,
  `foreign_keys=ON`, and an immediate transaction for atomic admission. Busy handling
  waits only for a competing transaction; the parent watchdog remains the test
  liveness oracle.
