# Phase 0 OXC Memory and Worker-Stack Probe Contract

Architecture identity:

- commit `65e60216891bb3d826a4778f84cb8aaa377abe92`;
- 16-file manifest `66925583362be22257dd7357072d176cd3f1ae3d4f99874a53f6689b7cf535e0`.

## Scope

This probe measures OXC parser feasibility only. It does not implement `lumin-js`, the
engine scheduler, cache behavior, semantic facts, or any public command. Results cannot
select a product memory guard, worker stack, worker-count default, or numeric budget.

## Named Corpus

The realistic corpus is the tracked JavaScript and TypeScript source set from
`https://github.com/annyeong844/lumin_lab.git` commit
`35290cb683a37e83bc8c915d1d0f9ca0f3f96fd0`. Dirty working-tree bytes are never read.
The exporter accepts only regular `.js`, `.jsx`, `.mjs`, `.cjs`, `.ts`, and `.tsx`
blobs selected by exact-commit `git ls-tree`, reads their object bytes through
`git cat-file --batch` without checkout or archive EOL conversion, records every
SHA-256 and byte count, and adds four independently generated stress fixtures:

- 512 nested JavaScript parenthesized expressions;
- 256 nested TypeScript object types;
- 256 nested TSX elements;
- 4,096 top-level TypeScript declarations.

Stress fixtures are stack probes, not a product maximum or supported-depth promise.

## Worker and Allocator Invariants

Each file task reads and hashes one source payload, constructs one worker-local OXC
`Allocator`, parses one AST, lowers only scalar owned facts, records allocator
`used_bytes` and `capacity`, then drops the parser result and allocator before returning.
No source bytes, OXC AST, span, diagnostic, or allocator-backed reference enters the
returned value. Workers never mutate a shared graph or result vector; one sorted merge
computes the digest.

Parse diagnostics and unrecoverable parser outcomes remain counted by file. They are
not converted to clean files. A source/hash mismatch, duplicate/noncanonical manifest,
unsupported file kind, missing file, extra source file, or cross-wave digest mismatch is
a hard failure.

## Matrix

The stack sweep executes one worker and one wave in isolated child processes with exact
candidate sizes `256 KiB`, `512 KiB`, `1 MiB`, `2 MiB`, `4 MiB`, and `8 MiB`. Process
failure remains evidence. At least the `4 MiB` and `8 MiB` candidates must complete;
this is a probe validity check, not product policy.

The jobs sweep uses an explicit `4 MiB` stack and three waves at `1, 2, 4, ...` workers
through the host's `available_parallelism`, including a non-power-of-two final value.
A separate one-worker, eight-wave allocator-lifetime run records post-wave current RSS.
There is no wall-time cap, file cap, source truncation, retry, or hidden worker cap.

Every successful run must produce the same sorted semantic digest across waves, stack
candidates, worker counts, Windows, and WSL2. Reports include corpus file/byte counts,
requested and actual workers, stack bytes, read/parse/lower time, allocator totals and
per-file maxima, current RSS after each wave, process peak RSS, parse diagnostics,
platform, filesystem class, source identity, corpus identity, and executable identity.

## Evidence Limits

Windows NTFS and WSL2 ext4 are partial Phase 0 evidence. WSL2 does not replace native
Linux CI/release-host evidence. Peak RSS is process-wide and allocator release can be
masked by the operating system or global allocator. The matrix demonstrates observed
feasibility on the named corpus; it does not approve product budgets or prove every
future source shape.
