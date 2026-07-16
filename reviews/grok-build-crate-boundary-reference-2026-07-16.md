# Grok Build Crate-Boundary Reference

Document role: non-canonical comparative research note

Status: recorded for post-freeze implementation checks; does not amend Architecture v1

Observed source: local source archive `C:\Users\endof\Downloads\repo\suyeonevo\grok-build-main`

Observation date: 2026-07-16

## 1. Scope and Evidence Limit

The observed source is a Cargo workspace archive without Git metadata. Its exact upstream revision
could not be established. `cargo metadata --no-deps --format-version 1`, run with the installed
stable toolchain, reported 79 packages and 278 local dependency edges.

This note records architecture lessons only. No source, generated API, manifest fragment, or public
type was copied into Lumin. The current Lumin Architecture v1 candidate and independent-verification
manifest remain unchanged.

## 2. Patterns Worth Keeping

### Dependency-light contract ownership

`xai-grok-workspace-types` is a real leaf boundary: it owns wire values while excluding async runtime,
I/O, and implementation dependencies. This supports Lumin's existing decision that `lumin-model` and
`lumin-evidence` remain dependency-light and that parser, store, filesystem, and CLI types stay private
to their owners.

### Host-owned execution

`xai-agent-lifecycle` gives contributors data-only hook inputs and injected capabilities while keeping
loop control in the host. This supports Lumin's existing capability contract: owners emit facts,
consulted inputs, evidence batches, and gate signals; `lumin-engine` alone schedules and reduces them.

### API separate from implementation

`xai-grok-tools-api` allows protocol consumers to avoid the tools implementation crate. Lumin already
uses the same principle through `lumin-protocol`, but a separate `*-api`, `*-types`, or `*-client` crate
must still require a real second consumer or process boundary. Naming alone is not justification.

### Query in place instead of cloning state

`xai-codebase-graph` documents manager-owned direct queries that avoid cloning a full index. This
supports Lumin's canonical-store and bounded pull-query model. It does not justify adding a second
manager, scheduler, cache owner, or query truth.

### Platform-heavy implementation isolation

`xai-fast-worktree` keeps Git, CoW, platform, and optional SQLite mechanics inside one feature owner.
This supports keeping parser, persistence, and platform dependencies inside their named Lumin crates.
It is a future reference for an isolated-worktree mode, not a reason to replace Architecture v1's
selected shared-worktree transition reconciliation.

### Windows path enforcement

The observed workspace uses Clippy `disallowed-methods` to reject raw `std`, `Path`, and Tokio
`canonicalize` calls because Windows verbatim paths can escape into external-tool inputs and equality
keys. During Phase 1, Lumin should select one inventory-owned path-identity implementation and make
architecture-check plus scoped Clippy reject bypass calls. The observable path, containment, and
fail-closed behavior remains owned by ARCH-002; this note does not select a third-party helper.

## 3. Patterns Deliberately Rejected

- Crate count is not an architecture target. The observed graph contains large integration hubs,
  including one package with 47 local dependencies and another with 30.
- A crate called `config-types` describes itself as a leaf while depending on configuration and MCP
  implementation crates. Lumin judges boundaries from the Cargo DAG and public types, not names.
- A generic `shared` crate combines clipboard, session, stderr, image, and UI configuration concerns.
  Lumin keeps the ARCH-000 ban on unnamed `common`, `shared`, and `utils` crates.
- The composition-root binary source is 2,893 lines and starts with broad dead/unreachable/unused lint
  allowances. Lumin's `lumin-cli` must remain a thin process adapter and must not absorb application
  helpers, analysis policy, persistence calls, or framework behavior.
- Several extracted crates exist partly to improve parallel compilation and retain broad lint
  allowances. Build partitioning alone does not prove a domain boundary.
- The leaf wire crate documents placeholder shapes. Lumin keeps its prohibition on empty future
  crates, placeholder owners, and speculative public contracts.

## 4. Lumin Enforcement Consequences

These are implementation checks derived from existing owners, not new product contracts:

1. `lumin-xtask architecture-check` verifies the canonical Cargo edge allowlist and rejects public
   third-party type leakage.
2. The CLI crate is checked as a composition adapter: argument parsing, DTO conversion, engine-service
   invocation, exit mapping, and output discipline only.
3. Filesystem identity and containment calls route through the inventory owner; scoped Clippy rejects
   direct production bypasses once the Phase 1 helper is selected and corpus-tested on Windows/Linux.
4. No `*-api`, `*-client`, `*-types`, `shared`, `common`, or `utils` crate is introduced without real
   slice behavior and an ARCH-000 owner/edge amendment.
5. High fan-out is reviewed through explicit dependency edges and ownership, not an arbitrary numeric
   crate or dependency cap.

## 5. Decision

Use Grok Build as a comparative corpus and pattern library, not as Lumin's blueprint. Its strongest
boundaries corroborate Architecture v1; its integration hubs, misleading leaf names, broad lint
allows, and oversized composition root explain why Lumin must keep fewer, stricter owner crates.

