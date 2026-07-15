# Lumin v2 Repository Rules

## Start Here

1. Read `WORKBOARD.md`.
2. Read `SDD.md`.
3. Follow the Workboard routing table and read only the owner documents for the current change.
4. Respect the active phase. Architecture v1 must be reviewed and frozen before implementation begins.

Do not treat existing code, legacy output, or a passing test as permission to contradict a higher-level contract.

## Current Source of Truth

Use this order when facts disagree:

1. `specs/000-product-contract.md`
2. `architecture/000-system-blueprint.md`
3. The focused architecture owner
4. The active vertical-slice specification
5. This file

`SDD.md` owns the working method across every level.

## Architecture Discipline

- Implement through accepted production-grade vertical slices.
- Do not create an MVP, horizontal scaffold, empty future crate, placeholder module, or fallback engine.
- Every crate proposal must name the forbidden dependency it enforces and explain why a private module is insufficient.
- Keep third-party parser, database, CLI, and framework types inside their owner crate.
- Do not pass domain facts between stages as JSON or `serde_json::Value`.
- Do not create generic `common`, `shared`, or `utils` owners.
- Keep public crate APIs minimal; default to private modules and explicit exports.
- A helper remains private to its first owner until a second real owner with identical semantics exists.
- CI must validate actual Cargo dependency edges against the canonical architecture policy.

## Rust Rules

Every workspace crate inherits strict workspace lints. The initial lint contract must include:

- `unsafe_code = "forbid"` unless a separately reviewed crate and product requirement explicitly amend this rule;
- Clippy denies for `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`, `dbg_macro`, `await_holding_lock`, `redundant_clone`, `needless_collect`, `unnecessary_to_owned`, and uninlined format arguments;
- `#[allow(...)]` only beside a concrete reason that explains why the lint is wrong for that location.

Use project-owned enums instead of string states. Accept borrowed values in analysis helpers and materialize owned values at source, thread, persistence, and protocol boundaries. A nontrivial clone in an analysis hot path needs an ownership reason visible from the code.

Do not use a wall-time cap to bound Rust analysis. Complete, emit artifact-visible incomplete evidence, or hard-stop on a real contract failure.

## Concurrency Rules

- `lumin-engine` owns one explicit local Rayon pool.
- Never use Rayon's process-global pool or create nested pools in capability crates.
- The scheduler alone mutates DAG state.
- Workers consume immutable inputs and return owned outputs.
- Workers do not mutate canonical graphs, evidence, or databases.
- Do not place `Arc<Mutex>` around parser, graph, or evidence state.
- Every parallel fan-in ends in a deterministic single-owner reduction.
- `jobs=1` and `jobs=N` must produce identical semantic evidence.
- The bytes used for content identity must be the bytes parsed.

Read `architecture/001-execution-and-ownership.md` before changing any scheduler, Rayon, cache, memory, or task-lifecycle behavior.

## Evidence and Write-Gate Rules

- Canonical findings live in the evidence store, not in generated projections.
- Every count and status has one owner.
- Bounded queries report total, returned, truncation, and continuation.
- Missing or failed capability evidence is never rendered as zero.
- Pre-write opens a durable gate and post-write closes that gate by ID.
- Do not introduce intent JSON transport or require agents to clean temporary request files.
- Overlapping active write sets fail closed; disjoint gates may proceed.
- No storage or scan lock may remain held during result transport.

Read `architecture/002-evidence-and-write-gate.md` before changing persistence, queries, exports, pre-write, post-write, or concurrent-agent behavior.

## File and Module Health

- Target implementation modules below 500 lines excluding tests.
- A file approaching 800 lines requires an ownership review before more behavior is added.
- Split by cohesive responsibility, not by arbitrary line chunks.
- Keep behavior tests near their contract owner, preferably in dedicated test modules or integration suites.
- Do not add one-use helper methods merely to make a large function look shorter.
- Do not add a trait, newtype, builder, policy, or result wrapper unless it enforces a real invariant or physical boundary.

## Test Discipline

- Derive expected behavior from the active specification.
- Test public behavior, realistic edges, and hard-stop contracts.
- Use real minimized corpus shapes for repository semantics.
- Prefer whole-value equality and stable semantic output over field-by-field scaffolding assertions.
- Do not test prose wrapping, helper existence, internal filenames, or static strings unless they are the public protocol.
- Never weaken assertions, delete cases, add arbitrary caps, swallow errors, or skip tests to make implementation pass.
- Determinism, package execution, and dogfood are acceptance tests, not optional benchmarks.

## Dependencies and Distribution

- Add a dependency only to the crate that owns its use.
- Record why a new dependency is needed, which boundary contains it, and which transitive or unsafe surface it adds.
- Keep lockfiles committed and platform feature choices explicit.
- The user-facing product is one prebuilt native executable per supported platform.
- Runtime Cargo compilation and Node analysis dependencies are forbidden.
- Skills package verified binaries and workflow text, not copied Rust source or duplicated analysis contracts.
- Build every platform product from the same canonical source revision and probe it behaviorally before release.

## Git and Change Scope

- Commit the reviewed Architecture v1 baseline before implementation.
- Keep changes narrow and coherent to one active slice or architecture decision.
- Do not mix unrelated cleanup with behavior work.
- Do not copy legacy modules wholesale. Harvest only focused behavior whose contract is proven by the new corpus.
- Never modify or discard user work that is outside the active change.

## Review and Close-Out

Before declaring a change complete:

1. Re-read the active acceptance criteria.
2. Run the specification's verification commands.
3. Run formatting, strict Clippy, focused tests, affected workspace tests, architecture checks, corpus checks, determinism checks, and package probes required by the slice.
4. Confirm unsupported and incomplete scope is visible.
5. Confirm no generated build output, temporary transport, copied source mirror, or unrelated file is tracked.
6. Update the owning specification and Workboard status.
7. Report checked evidence, residual limitations, and exact commands run.

Architecture revisions require one design review and one independent adversarial review before freeze. AI review decisions follow the evidence-grounding rule in `SDD.md`.
