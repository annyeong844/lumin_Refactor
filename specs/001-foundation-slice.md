# SLICE-001: Native JS/TS/Vue Evidence and Write Gate

Document role: first implementation specification

Status: draft, blocked by Architecture v1 review

Revision: 2026-07-15

Parents: PRODUCT-000, ARCH-000, ARCH-001, ARCH-002

## 0. One-Line Definition

The first slice ships a production-grade native path from Codex/Claude invocation through parallel JS/TS/Vue analysis, export-level dead evidence, bounded queries, and a durable pre/post transaction on Windows and Linux prebuilt binaries.

## 1. Why This Slice Is First

This slice crosses every permanent system boundary and directly attacks the legacy product's highest-impact failures:

- Node producer orchestration and repeated parsing;
- SFC imports classified as non-source assets and aborting the graph;
- public re-export protection applied to an entire file;
- reachable files excluded from export-level dead analysis;
- `import.meta.glob` and dynamic-use precision drift;
- artifact warehouses and duplicated counts;
- JSON write-gate intent transport;
- runtime Cargo compilation and WSL platform confusion.

Completing this slice proves the architecture. It is not permission to bypass final boundaries temporarily.

## 2. Implementation Scope

SLICE-001 creates only crates that contain real slice behavior:

- `lumin-model`;
- `lumin-evidence`;
- `lumin-inventory`;
- `lumin-js`;
- `lumin-sfc` with complete Vue ownership for the declared corpus;
- `lumin-resolve`;
- `lumin-graph`;
- `lumin-dead`;
- `lumin-store`;
- `lumin-engine`;
- `lumin-protocol`;
- `lumin-cli`.

The Rust, clone, structure, and discipline analysis crates are not created in this slice. Their capabilities appear as unavailable in queries until a later accepted slice creates real implementations.

## 3. Supported Source Contract

### 3.1 JavaScript and TypeScript

The slice must preserve evidence for:

- ESM named, default, namespace, side-effect, and type-only imports;
- direct exports, alias exports, default exports, and re-exports;
- namespace member access and broad namespace escape;
- literal dynamic imports, member-precise dynamic imports, and nonliteral opacity;
- `import.meta.glob` relative patterns with explicit unsupported evidence for unsupported patterns;
- CommonJS `require`, exact exports, namespace use, and computed-property broad evidence;
- `.js`, `.jsx`, `.mjs`, `.cjs`, `.ts`, `.tsx`, `.mts`, `.cts`, and declaration inputs under the declared scan policy;
- extension and compiled-output fallback order proven by corpus tests;
- parse failures as scoped incomplete evidence.

Unsupported syntax may reduce only the owning file or capability's completeness. It cannot become an empty successful file.

### 3.2 Vue SFC

For Vue files, `lumin-sfc` owns:

- SFC block decomposition;
- inline and `src` script units;
- script language selection;
- component import and template-use facts needed by graph evidence;
- style and other non-source resource references as non-source assets;
- comments and inactive template regions required by the declared corpus;
- generated and unresolved SFC references as typed evidence.

An import such as `import App from "./App.vue"` resolves to the Vue source module when present. A missing `.vue` target becomes unresolved evidence. Neither case is routed through an exception labeled `non-source-asset-specifier`.

Svelte, Astro, and other SFC dialects remain explicitly unavailable in this slice. The generic graph cannot claim they were analyzed.

## 4. Resolution Contract

Resolution is performed against the immutable source inventory and workspace configuration observed for the run.

The slice covers:

- relative exact and extensionless source imports;
- JS-output-to-TS-source mappings with canonical probe order;
- package external classification;
- workspace package ownership;
- nearest `package.json` dependency ownership;
- tsconfig/baseUrl and exact or wildcard aliases included in the accepted corpus;
- non-source assets before declaration-sidecar fallbacks;
- Next.js route-group characters as ordinary valid path segments;
- generated virtual and unresolved outcomes without graph abortion.

Every source use receives one `ResolutionOutcome`. A skipped record without a typed reason is a contract failure.

## 5. Graph and Dead-Export Contract

The graph indexes every successfully lowered source file, including files reachable from entries and tests.

Dead classification is export-identity based:

- exact import fan-in is tracked per exported identity;
- type-space and value-space fan-in remain distinct;
- broad use is represented separately and cannot inflate exact scalar fan-in;
- module reachability does not suppress export-level analysis inside a reachable module;
- a package public surface protects only identities actually exported through that surface;
- public protection of one identity cannot protect unrelated siblings in the same file;
- side-effect imports preserve module reachability without marking every export exactly consumed;
- opaque dynamic or computed use limits absence claims with visible evidence;
- production and test consumers remain distinguishable.

The default query reports candidates, confidence, protection reasons, and limitations. It does not label every zero-fan-in symbol safe to delete.

## 6. Canonical Evidence and Query Contract

A successful run publishes:

```text
.lumin/runs/<run-id>/run.json
.lumin/runs/<run-id>/evidence.store
```

No legacy analysis JSON is emitted by default.

The slice implements:

```text
lumin audit
lumin overview
lumin findings --area dead-code
lumin explain <finding-id>
lumin related <finding-id>
lumin files <path>
lumin capabilities
lumin export sarif
```

All collection queries are bounded, deterministic, and cursor-resumable. Required capability failure appears in `overview` before ordinary findings.

## 7. Write-Gate Contract

The slice implements:

```text
lumin pre-write [typed intent flags]
lumin post-write <gate-id>
lumin gate show <gate-id>
lumin gate findings <gate-id>
lumin gate explain <gate-id> <finding-id>
lumin gate list --active
lumin gate abandon <gate-id> --reason <text>
```

Required behavior:

- no request JSON file;
- one durable gate ID returned by pre-write;
- baseline built from exact worktree bytes;
- language and nearest dependency owner inferred from planned paths;
- mixed JS/TS/Vue paths handled inside one gate;
- overlapping active path leases rejected;
- disjoint gates allowed concurrently;
- post-write detects unplanned changed and new source paths;
- post-write reports newly introduced type escapes and analysis opacity;
- post-write does not launch a full audit unless explicitly requested;
- all locks released before result transport;
- completed gate remains queryable.

A Rust path in this slice produces an explicit unsupported-language gate finding and cannot be silently routed to the JS owner.

## 8. Execution Contract

The slice uses the final ARCH-001 runtime:

- one local Rayon pool;
- Kahn scheduling over the actual task DAG;
- file-level parallel extraction;
- deterministic reducers;
- independent graph-dependent analysis tasks where applicable;
- one store writer;
- no global pool, nested pool, or shared mutable graph;
- no JSON between stages;
- exact-byte cache identity;
- `jobs=1` as the reference execution of the same engine.

There is no sequential compatibility engine.

## 9. Truth Corpus

The implementation creates repository fixtures with hand-authored expected truth, not expectations copied from the legacy output.

| Corpus case | Required truth |
| --- | --- |
| `plain-esm` | Exact named/default/type-only fan-in and side-effect reachability remain distinct. |
| `reachable-dead-sibling` | A live file can still contain a zero-fan-in dead export candidate. |
| `public-reexport-sibling` | One public re-export is protected; three unexported dead siblings remain candidates. |
| `vue-entry` | `main.js -> App.vue` resolves and the graph completes. |
| `vue-missing-target` | Missing `.vue` import becomes unresolved evidence without aborting other files. |
| `vue-non-source-asset` | Style/resource references do not resolve to declaration sidecars or source edges. |
| `next-route-group` | Paths such as `(doc)/layout.tsx` are accepted and resolved normally. |
| `dynamic-literal-member` | Literal dynamic member use preserves member precision. |
| `dynamic-nonliteral` | Nonliteral dynamic import creates opacity, not empty evidence. |
| `import-meta-glob` | Supported relative patterns expand deterministically; unsupported aliases remain visible. |
| `cjs-computed` | Computed destructuring or export access degrades to broad evidence. |
| `nearest-manifest` | Dependency checks use the owner manifest nearest each planned path. |
| `parallel-gates` | Disjoint gates coexist; overlapping write sets are rejected atomically. |
| `mixed-vue-gate` | JS and Vue changes share one user gate and keep owner-specific facts. |
| `required-capability-failure` | Overview warns that dead analysis is unavailable and never renders zero. |

The corpus must include repositories copied or minimized from real failure shapes, including Vue core-style package layouts and a Next.js route-group layout.

## 10. Differential Use of Legacy Tools

Legacy Lumin and Fallow may be run against corpus repositories to discover disagreements. They are not the expected-value owner.

Every disagreement is classified as:

- intentional parity;
- intentional Lumin v2 correction;
- unsupported and visible;
- unresolved specification question.

Code is harvested from the legacy product only when a focused behavior test proves the required contract and the code fits the new owner boundary. Whole modules and bridge layers are not copied.

## 11. Skills and Distribution

The slice ships:

- Windows x64 prebuilt `lumin`;
- Linux x64 musl prebuilt `lumin`;
- integrity metadata tied to build identity;
- one Codex skill;
- one Claude Code skill;
- behavioral package probes for both binaries.

Skills contain the concise audit/query/write-gate workflow. They do not package Rust source fallback, Node analysis dependencies, or duplicated command contracts.

Runtime execution with Cargo unavailable is part of package acceptance.

## 12. Performance Evidence

Before this document can move from draft to active, architecture review must approve numeric budgets for:

- cold full slice audit;
- warm unchanged audit;
- cold pre-write;
- warm pre-write;
- post-write for one changed file;
- post-write for a representative multi-file wave;
- peak resident memory;
- `jobs=1` versus default jobs scaling.

Budgets are established from checked benchmark runs on named hardware and corpora. They cannot be invented to satisfy CI after implementation.

Required benchmark environments are:

- native Windows on NTFS;
- WSL on ext4;
- WSL against `/mnt/<drive>` as a separately labeled diagnostic environment;
- Linux CI or a declared release-compatible Linux host.

Every benchmark reports source file count, total bytes, cache state, worker count, filesystem class, stage timings, and peak memory.

## 13. Non-Goals

SLICE-001 does not implement:

- Rust repository analysis;
- Svelte or Astro completeness;
- function, block, or shape clones;
- full topology and discipline review;
- natural-language intent parsing;
- a daemon or MCP transport;
- default legacy artifact emission;
- runtime source compilation;
- a second fallback analyzer.

These omissions must be visible through `lumin capabilities` and relevant overview limitations.

## 14. Acceptance Criteria

1. Every corpus row passes through the public `lumin` binary.
2. The Vue and Next.js regression corpora complete the symbol graph without a process abort.
3. The 20-module public-re-export corpus reports all 60 dead siblings and protects all 20 public identities.
4. A reachable file's unused export remains a candidate.
5. `jobs=1` and repeated default-job runs produce identical semantic store dumps and finding IDs.
6. Randomized worker completion tests preserve output identity.
7. No analyzed source file is read or parsed more than once in a cold run.
8. No runtime path executes Node or Cargo.
9. Windows and Linux packages pass the same behavioral fixture probes.
10. A user can perform pre-write and post-write using only paths, optional typed flags, and one gate ID.
11. Parallel disjoint gates close independently; overlapping gates fail before edits are authorized.
12. Query output is bounded and exhaustive results are reachable through cursors.
13. The default run emits no legacy artifact warehouse.
14. Required failures and unsupported capabilities are prominent and queryable.
15. Strict workspace formatting, lint, unit, integration, corpus, dependency-edge, and package checks pass.

## 15. Verification Commands

The implementation must provide stable repository commands equivalent to:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p lumin-architecture-check
cargo run -p lumin-corpus -- foundation
cargo run -p lumin-corpus -- foundation --determinism
cargo run -p lumin-package-check -- windows-x64
cargo run -p lumin-package-check -- linux-x64
```

The exact command wrappers may be finalized with the workspace, but CI and local development must invoke the same underlying checks.
