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
  replacement races;
- lifecycle-lock and anchor content mutation and extra-hard-link races.

Namespace cases capture Windows volume/file identity through no-follow handles. The
child validates before the injected race and must revalidate to a typed hard-stop
before writing the test-only canonical-success marker.

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
policy. The Windows x64 result is partial Phase 0 evidence only; it cannot satisfy the
required Linux/musl, filesystem, package, native-path, OXC, or numeric-budget gates and
cannot by itself select a production backend.

## Backend configurations

- `redb 4.1.0`: one database handle per repository-lock transaction; durable commits;
  `DatabaseAlreadyOpen` is retried by reopening after the competing process closes.
- `rusqlite 0.39.0` with bundled SQLite: WAL initialized once, `synchronous=FULL`,
  `foreign_keys=ON`, and an immediate transaction for atomic admission. Busy handling
  waits only for a competing transaction; the parent watchdog remains the test
  liveness oracle.
