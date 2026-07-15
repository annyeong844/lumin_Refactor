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

The development-only `lumin-xtask` crate contains architecture, corpus, determinism, and package verification commands. It is not a product capability or runtime dependency.

The Rust, clone, structure, and discipline analysis crates are not created in this slice. Shape and type-escape intent lanes therefore remain unavailable; requesting either returns visible unavailable/incomplete evidence rather than a temporary implementation in `lumin-js` or `lumin-engine`.

## 3. Supported Source Contract

### 3.1 Inventory and Scan Policy

| Input class | Normative first-slice behavior |
| --- | --- |
| Source extensions | Include `.js`, `.jsx`, `.mjs`, `.cjs`, `.ts`, `.tsx`, `.mts`, `.cts`, `.d.ts`, `.d.mts`, `.d.cts`, and `.vue` under the canonical root. |
| Ignore policy | Apply explicit excludes and repository ignore files. Always exclude `.git`, `.lumin`, and dependency-owned `node_modules`; do not prune an authored directory merely because its basename is `target`, `build`, or `coverage`. |
| Generated/vendor | Classify separately. In-scope uses may contribute liveness, but generated or vendored definitions are not default dead-removal candidates. |
| Tests | Inventory and classify test-like files. Full audit counts their fan-in separately; production liveness does not treat test-only consumers as production consumers. |
| Declarations | Parse declaration files for type-space facts only. A declaration cannot satisfy a runtime value edge or become a value dead-removal candidate. |
| Symlink/junction | Do not recursively traverse directory links by default. An explicitly included root-contained target is deduplicated by physical file identity; an outside-root target is rejected and reported. |
| Semantic inputs | Snapshot applicable ignore files, package manifests, lockfiles, tsconfig files, workspace metadata, and explicit entry configuration even when they are not source files. |

The scan profile and every exclusion are persisted. An omitted or unobservable path is a scope limitation, not evidence that the path contains no consumers.

### 3.2 JavaScript and TypeScript

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

Unsupported syntax is recorded by its owning file or capability and cannot become an empty successful file. Its downstream absence impact follows the explicit limitation scopes in Section 5.2.

### 3.3 Vue SFC

For Vue files, `lumin-sfc` owns:

- SFC block decomposition;
- inline and `src` script units;
- script language selection;
- component import and template-use facts needed by graph evidence;
- style and other non-source resource references as non-source assets;
- comments and inactive template regions required by the declared corpus;
- generated and unresolved SFC references as typed evidence.

After JS extraction, the ARCH-001 `finalize-sfc-facts` stage returns model-owned script facts to `lumin-sfc` for Vue-specific template/import binding. Inline scripts retain parent span mappings. `<script src>` references an existing inventory `SourceId`; it does not create a copied source or a second parse. A conflicting external `lang`/extension mode is unsupported evidence in this slice.

An import such as `import App from "./App.vue"` resolves to the Vue source module when present. A missing `.vue` target becomes unresolved evidence. Neither case is routed through an exception labeled `non-source-asset-specifier`.

Svelte, Astro, and other SFC dialects remain explicitly unavailable in this slice. The generic graph cannot claim they were analyzed.

## 4. Resolution Contract

Resolution is performed against the immutable source inventory and semantic configuration snapshot. The first slice models the declared subset below rather than claiming complete TypeScript resolver parity. Resolution first derives host runtime candidates for the selected mode, then applies TypeScript source substitution to each candidate before advancing to the next host candidate.

| Specifier or host candidate | Ordered first-slice probes |
| --- | --- |
| Explicit TypeScript or Vue source path | Exact path only for `.ts`, `.tsx`, `.mts`, `.cts`, or `.vue`. Explicit declaration paths are exact and type-space only. JavaScript runtime extensions use the substitution rows below even when written explicitly. |
| Runtime `.js` candidate | Value space: `.ts`, `.tsx`, `.js`, `.jsx`. Type space inserts `.d.ts` after `.tsx`. |
| Runtime `.jsx` candidate | Value space: `.tsx`, `.jsx`. Type space inserts `.d.ts` after `.tsx`. |
| Runtime `.mjs` candidate | Value space: `.mts`, `.mjs`. Type space inserts `.d.mts` after `.mts`. |
| Runtime `.cjs` candidate | Value space: `.cts`, `.cjs`. Type space inserts `.d.cts` after `.cts`. |
| Extensionless path in a permitting mode | Derive the host `.js` candidate and apply its substitution row. Do not invent extensionless `.mts`, `.cts`, `.mjs`, or `.cjs` candidates. |
| Directory in a permitting mode | Resolve its supported `package.json` entry under the package-field rules, then derive an `index.js` host candidate and apply the `.js` substitution row. |
| Unsupported explicit extension | Return `NonSourceAsset` or typed `Unsupported` evidence. Do not substitute a declaration sidecar. |

A declaration may satisfy type-space resolution but never proves that a runtime value target exists. When a value import also has declaration evidence, the resolver records that type companion separately from the value target.

Specifier and configuration policy is:

| Class | Contract |
| --- | --- |
| Resolution mode | Support `bundler`, legacy `node`, `node16`, and `nodenext`. Bundler/legacy-node and CJS lanes permit extensionless and directory fallback; Node16/NodeNext ESM lanes require an explicit relative extension and skip the extensionless and directory rows. Unsupported modes make resolution incomplete rather than selecting a fallback mode. |
| Relative | Resolve inside the canonical root with the probe order above. Route-group characters such as `(doc)` are ordinary path bytes. |
| Tsconfig | Use the importer's nearest config, root-contained relative/workspace-package `extends`, child override semantics, and the `baseUrl` of the config that declares each mapping. Cycles are incomplete configuration evidence. External-package extends and project-reference redirection are unsupported in this slice. |
| `paths` | Exact key before wildcard; wildcard keys permit one `*` and use longest literal prefix then declaration order. Probe mapped targets before `baseUrl` and package resolution. |
| Workspace package | Resolve `exports` exact key before one-star patterns. Within a condition object, the first supported matching branch in declaration order wins; the active set includes `types` for type space, `import` or `require` for the edge mode, `node`, and `default`. Unsupported condition shapes remain visible. |
| Package fields without `exports` | Type space probes `types`, then `typings`, then a declaration companion for the selected value target. Value space uses `module` then `main` in bundler mode and `main` in Node modes, followed by permitted directory fallback. A type field never proves runtime value liveness. |
| Bare external | Classify as `External` after workspace ownership lookup; never probe a similarly named relative file. |
| Absolute, URL, package `imports`, or unsupported alias | Return typed `Unsupported` or `Unresolved` evidence with the limitation scope below; never skip the record. |
| Generated virtual | Resolve only through an observed generated mapping; otherwise retain a typed virtual limitation. |

Every source use receives one `ResolutionOutcome`. A skipped record without a typed reason is a contract failure. Resolver policy version and every consulted configuration identity participate in cache and analysis-contract identity.

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

### 5.1 Entry, Public Surface, and Consumer Policy

| Fact | Contract |
| --- | --- |
| Entry root | Explicit CLI entries, package `exports` targets, then edge-appropriate `module`/`main` targets establish module reachability. No heuristic `src/index` entry is invented. |
| Public value surface | `exports` is authoritative when present; otherwise `module` then `main`. A directly exposed target protects its exported identities. A barrel protects only identities it actually re-exports, not every export in their source files. |
| Public type surface | Type conditions in `exports`, then `types`, protect type identities only. |
| Private package | `private: true` disables external-public protection from package fields; explicit entries still affect reachability and real workspace consumers still contribute fan-in. |
| Test consumer | Contributes test fan-in and protects `dead-in-test`, but leaves a production-zero identity eligible for `dead-in-production` review. |
| Side-effect/broad consumer | Preserves module liveness or marks target identities broad/unknown without incrementing exact identity fan-in. |
| Generated/vendor definition | May receive and contribute edges but is muted from default removal candidates with its classification reason. |

### 5.2 Uncertainty Propagation

An exact absence candidate is emitted only when no potential-consumer limitation intersects that identity. An intersecting limitation produces queryable incomplete liveness evidence, not a deletion candidate.

| Condition | Limitation scope |
| --- | --- |
| Recoverable parse with complete module-use extraction | `File`; extracted target facts remain usable, while unsupported local definitions stay limited to the file. |
| Unrecoverable parse or unknown module-use completeness | `Workspace`; the file could hide a consumer anywhere in the supported scan scope. |
| Nonliteral dynamic import | `ExplicitTargets` when a static path prefix bounds inventory matches; otherwise `Workspace`. |
| Unsupported `import.meta.glob` | `ExplicitTargets` for a literal static base; otherwise the importer's `Package`. |
| Computed CommonJS property on a resolved module | `Module` for that target and broad use across its value exports. |
| Opaque Vue template | Imported component candidates and observed global registrations as `ExplicitTargets`; `Package` when that set cannot be bounded. |
| Unresolved internal relative/configured alias | Resolver probe candidates as `ExplicitTargets`; `Workspace` when configuration opacity prevents a bounded domain. |
| Unknown generated virtual module | Observed generated-map targets as `ExplicitTargets`; otherwise the importer's `Package`. |

The limitation and its scope are canonical evidence. Reducers may narrow a scope only with additional grounded targets and may never silently drop it.

## 6. Canonical Evidence and Query Contract

A successful run publishes:

```text
.lumin/latest.json
.lumin/attempts/<attempt-id>/attempt.json
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
- write/write and write/semantic-read conflicts rejected;
- nonconflicting gates allowed concurrently;
- post-write detects unplanned changed, new, removed, and renamed paths;
- post-write checks dead-code, resolution, dependency-owner, and opacity deltas owned by this slice;
- shape and type-escape lanes remain visibly unavailable;
- post-write requires the explicit gate ID and checks actual writes against other active gates;
- post-write does not launch a full audit unless explicitly requested;
- all locks released before result transport;
- completed gate remains queryable.

A Rust path in this slice produces an explicit unsupported-language gate finding and cannot be silently routed to the JS owner.

## 8. Execution Contract

The slice uses the final ARCH-001 runtime:

- one local Rayon pool;
- Kahn scheduling over the actual task DAG;
- a profile-fixed stage set with empty batches for absent languages;
- file-level parallel extraction;
- `lumin-sfc` finalization after inline and external JS facts are available;
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
| `extension-probe-precedence` | Explicit TypeScript/Vue paths are exact; JavaScript runtime-output substitution precedes the runtime file; extensionless, declaration, and directory behavior follows Section 4. |
| `declaration-type-space` | Declaration facts satisfy type space only and cannot make a value export live. |
| `tsconfig-aliases` | Exact, wildcard, `baseUrl`, and supported `extends` precedence matches Section 4; unsupported config remains visible. |
| `workspace-package-exports` | Exact/pattern exports and edge-specific conditions resolve deterministically and define identity-scoped public surfaces. |
| `reachable-dead-sibling` | A live file can still contain a zero-fan-in dead export candidate. |
| `public-reexport-sibling` | One public re-export is protected; three unexported dead siblings remain candidates. |
| `vue-entry` | `main.js -> App.vue` resolves and the graph completes. |
| `vue-inline-script-setup` | Inline script facts bind template components through `finalize-sfc-facts` with parent spans. |
| `vue-external-script` | External script bytes are parsed once and attached without copied facts; conflicting mode is unsupported. |
| `vue-missing-target` | Missing `.vue` import becomes unresolved evidence without aborting other files. |
| `vue-non-source-asset` | Style/resource references do not resolve to declaration sidecars or source edges. |
| `next-route-group` | Paths such as `(doc)/layout.tsx` are accepted and resolved normally. |
| `dynamic-literal-member` | Literal dynamic member use preserves member precision. |
| `dynamic-nonliteral` | Nonliteral dynamic import creates opacity, not empty evidence. |
| `import-meta-glob` | Supported relative patterns expand deterministically; unsupported aliases remain visible. |
| `cjs-computed` | Computed destructuring or export access degrades to broad evidence. |
| `parse-failure-propagation` | Recoverable and unrecoverable parse limitations constrain only the scopes defined in Section 5.2. |
| `nearest-manifest` | Dependency checks use the owner manifest nearest each planned path. |
| `parallel-gates` | Read/read overlap coexists; write/write and write/read conflict atomically. |
| `gate-path-identity` | New paths, aliases, directory descendants, symlinks/junctions, case policy, and rename endpoints follow ARCH-002. |
| `gate-config-drift` | A changed semantic input makes the gate stale; an actual cross-gate write is denied. |
| `unplanned-edit` | Unplanned changed, new, removed, and renamed paths cannot receive an allow decision. |
| `mixed-vue-gate` | JS and Vue changes share one user gate and keep owner-specific facts. |
| `required-capability-failure` | Overview warns that dead analysis is unavailable and never renders zero. |
| `snapshot-and-latest` | Mid-scan drift blocks completion; failed or interrupted attempts remain visible beside the last completed run. |
| `bounded-nested-query` | Top-level and nested evidence pages expose totals, truncation, and stable continuation. |
| `path-escape-and-corrupt-store` | Root escape and corrupt canonical storage hard-stop without fallback or empty evidence. |

The corpus must include repositories synthesized from or minimized around real failure shapes, including Vue core-style package layouts and a Next.js route-group layout. A copied fixture records origin, license, source revision, and modifications in a local `PROVENANCE.md`; synthetic structure is preferred when copied code is unnecessary. Store-state fixtures are generated in a test temp root and do not require committing ignored `.lumin` output.

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

Performance approval has two non-circular phases.

**Phase 0 feasibility:** before this document becomes active, disposable harnesses measure store locking/backend behavior, OXC parser memory and stack needs, and Windows/Linux static packaging feasibility. They cannot expose product APIs or become a production scaffold. Commands, hardware, inputs, and results are retained in the architecture review record; disposable code is removed after the decision.

Architecture review then approves target budgets for:

- cold full slice audit;
- warm unchanged audit;
- cold pre-write;
- warm pre-write;
- post-write for one changed file;
- post-write for a representative multi-file wave;
- peak resident memory;
- `jobs=1` versus default jobs scaling.

Targets use named hardware/corpora, legacy baselines, and Phase 0 probes. They are goals rather than claims that an unimplemented product already achieved them.

**Phase 1 acceptance:** the completed public `lumin` binary is measured against every target below. A missed target is a slice failure or an explicitly reviewed contract revision; CI cannot invent or relax a number after seeing the result.

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
5. `jobs=1` and repeated default-job runs produce identical canonical semantic dumps and finding IDs; runtime metrics and physical store bytes are excluded.
6. Randomized worker completion tests preserve output identity.
7. No analyzed source payload is read or parsed more than once for extraction in a cold run; the separate final hash-only freshness pass is measured and does not reparse.
8. No runtime path executes Node or Cargo.
9. Windows and Linux packages pass the same behavioral fixture probes.
10. A user can perform pre-write and post-write using path-scoped typed flags, stable machine output, and one explicit gate ID.
11. Gates with nonconflicting read/write sets close independently; write/write and write/read conflicts fail before edits are authorized.
12. Query output is bounded and exhaustive results are reachable through cursors.
13. The default run emits no legacy artifact warehouse.
14. Required failures, snapshot freshness, and unsupported capabilities are prominent and queryable.
15. Strict workspace formatting, lint, unit, integration, corpus, dependency-edge, and package checks pass.
16. The public binary meets the approved Phase 1 performance and memory targets on every required environment.

## 15. Acceptance Traceability

| AC | Behavior test | Corpus/fixture | Command | Expected proof |
| --- | --- | --- | --- | --- |
| 1 | `foundation_corpus_contract` | all Section 9 rows | `lumin-xtask corpus foundation` | Every expected query value matches authored truth. |
| 2 | `framework_failures_are_scoped` | Vue and route-group rows | `lumin-xtask corpus foundation` | `overview` reports graph complete or scoped limitation, never process abort. |
| 3 | `public_surface_is_identity_scoped` | 20-module re-export matrix | `lumin-xtask corpus foundation` | 60 candidates and 20 protected identities. |
| 4 | `reachable_module_keeps_dead_exports` | `reachable-dead-sibling` | `lumin-xtask corpus foundation` | The unused sibling remains a candidate. |
| 5 | `semantic_dump_is_worker_invariant` | full foundation corpus | `lumin-xtask corpus foundation --determinism` | Canonical semantic dump and finding IDs match. |
| 6 | `scheduler_completion_order_is_irrelevant` | randomized stage-result fixture | `cargo test -p lumin-engine` | Repeated randomized completion yields one semantic dump. |
| 7 | `source_payload_is_extracted_once` | read-counter plus Vue external script | `lumin-xtask corpus foundation` | Read/parse counters distinguish extraction from final hash validation. |
| 8 | `runtime_has_no_source_fallback` | package runtime probe | `lumin-xtask package-check <target>` | Execution succeeds with Node and Cargo unavailable. |
| 9 | `packages_share_behavior_contract` | package fixture set | both package-check targets | Windows/Linux query values match. |
| 10 | `gate_round_trip_requires_id` | `mixed-vue-gate` | `lumin-xtask corpus foundation` | JSON decisions and explicit gate ID complete the round trip. |
| 11 | `gate_conflicts_are_serializable` | parallel/config/path identity rows | `lumin-xtask corpus foundation` | Read/read admits; write/write and write/read reject or become stale as specified. |
| 12 | `all_pages_are_reachable` | `bounded-nested-query` | `lumin-xtask corpus foundation` | Cursor traversal returns exactly `total` top-level and nested items. |
| 13 | `default_publication_is_bounded` | output-layout fixture | `lumin-xtask corpus foundation` | Only attempt/run envelopes, canonical store, and latest pointer are published. |
| 14 | `failure_and_freshness_are_visible` | failure, parse, snapshot, corrupt-store rows | `lumin-xtask corpus foundation` | `overview` exposes incomplete/stale/failed states and never zero. |
| 15 | `repository_policy_suite` | workspace and source policy | fmt, Clippy, workspace test, architecture-check | Every required quality command exits successfully. |
| 16 | `release_performance_matrix` | named benchmark corpora | `lumin-xtask benchmark foundation` | Each approved time/memory target is reported and met. |

## 16. Verification Commands

The implementation must provide stable repository commands equivalent to:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p lumin-xtask -- architecture-check
cargo run -p lumin-xtask -- corpus foundation
cargo run -p lumin-xtask -- corpus foundation --determinism
cargo run -p lumin-xtask -- benchmark foundation
cargo run -p lumin-xtask -- package-check windows-x64
cargo run -p lumin-xtask -- package-check linux-x64
```

The exact command wrappers may be finalized with the workspace, but CI and local development must invoke the same underlying checks.
