# ARCH-000: Lumin v2 System Blueprint

Document role: final architecture blueprint and review packet

Status: draft

Revision: 2026-07-15

Parent: PRODUCT-000

## 0. One-Line Definition

Lumin v2 is one native, deterministic analysis engine whose compile-time crate DAG separates source acquisition, language facts, resolution, graph construction, analysis, evidence persistence, and product transport.

## 1. Architectural Position

This is an architecture-first rewrite, not a line-by-line Rust migration. The legacy repository is retained as:

- a behavior inventory;
- a known-defect registry;
- a compatibility corpus;
- a differential benchmark;
- a source of individually proven algorithms.

Legacy module boundaries, producer boundaries, bridge contracts, and generated source mirrors are not migration targets.

The entire destination architecture is designed before implementation. Implementation then proceeds through production-grade vertical slices. No horizontal empty-crate scaffold is allowed.

## 2. Structural Principles

1. One production engine owns analysis semantics.
2. Crate boundaries enforce dependency direction at compile time.
3. Each fact has one canonical owner.
4. Parser, persistence, CLI, and framework library types do not cross their owner crate.
5. Expected repository uncertainty is represented by typed outcomes.
6. Parallel workers produce owned values; reducers own shared state.
7. Skills are thin product adapters, not alternate engines.
8. JSON, Markdown, and SARIF are projections, not internal transport.
9. A new crate must enforce a meaningful forbidden dependency or isolate a substantial dependency set.
10. A new capability enters the existing DAG; it does not create a parallel pipeline.

## 3. Repository Shape

The final workspace is hierarchical by responsibility:

```text
crates/
  foundation/
    model/                 lumin-model
    evidence/              lumin-evidence
  source/
    inventory/             lumin-inventory
  languages/
    js/                    lumin-js
    sfc/                   lumin-sfc
    rust/                  lumin-rust
  graph/
    resolve/               lumin-resolve
    symbols/               lumin-graph
  analyses/
    dead-code/             lumin-dead
    clones/                lumin-clones
    structure/             lumin-structure
    discipline/            lumin-discipline
  application/
    store/                 lumin-store
    engine/                lumin-engine
    protocol/              lumin-protocol
    cli/                   lumin-cli
tools/
  xtask/                   lumin-xtask (development only)
skills/
  codex/
  claude-code/
specs/
architecture/
corpus/
reviews/
```

This tree describes the final destination. Crates are created only when an accepted vertical slice contains real behavior for them.

## 4. Crate Ownership

### 4.1 `lumin-model`

Owns the dependency-light domain vocabulary:

- normalized repository-relative paths;
- source, symbol, use, span, and package identifiers;
- source snapshots and fingerprints;
- language-neutral module and symbol facts;
- typed resolution outcomes;
- build, analysis-contract, repository, attempt, run, gate, and embedded-source identity value types;
- completeness and opacity states.

It must not depend on parsers, filesystems, persistence engines, CLI frameworks, or artifact formats. It is not a miscellaneous helper crate.

### 4.2 `lumin-evidence`

Owns canonical run evidence:

- run, capability, finding, diagnostic, metric, and limitation records;
- confidence and grounding states;
- gate decisions and lifecycle evidence;
- stable evidence relationships;
- gate findings shared by audit and write-gate workflows.

It depends only on `lumin-model` and dependency-light value libraries. It does not persist or render evidence.

### 4.3 `lumin-inventory`

Owns repository observation:

- root validation and scan scope;
- ignore and exclusion policy application;
- workspace and nearest-manifest ownership;
- semantic configuration snapshots;
- extraction payload snapshots, final freshness identities, and source-set fingerprints;
- generated, test-like, vendored, and out-of-scope classification.

It emits project-owned model types. No downstream crate reads arbitrary source files behind its back.

### 4.4 Language Crates

`lumin-js` owns OXC-based JS/TS parsing and lowering. OXC allocator, AST, spans, and syntax types never leave the crate.

`lumin-sfc` owns Vue, Svelte, Astro, and related container decomposition. It emits embedded source units, component/resource references, and opaque framework evidence. It does not call the JS parser directly; the engine routes emitted embedded units to `lumin-js`.

`lumin-rust` owns Rust syntax and optional compiler-oracle integration. Rust parser and compiler ecosystem types never leave the crate.

Any future compiler oracle is an explicit opt-in external capability with visible toolchain requirements and unavailable semantics. It is not a hidden default dependency or runtime fallback.

Every language crate lowers into `lumin-model` facts. A parse failure is a file outcome, not permission to invent an empty successful file.

### 4.5 `lumin-resolve`

Owns source-use resolution over an immutable inventory index. It accepts normalized requests and returns exactly one typed outcome:

```rust
enum ResolutionOutcome {
    Internal(SourceId),
    External(PackageId),
    NonSourceAsset(RepoPath),
    GeneratedVirtual(VirtualSourceId),
    Unresolved(UnresolvedReason),
    Unsupported(UnsupportedReason),
}
```

`UnresolvedReason` and `UnsupportedReason` are dependency-light model values. Evidence owners may cite them later, but the model does not depend on the evidence crate.

Framework crates choose the semantic kind of an edge; the resolver determines its target outcome. The resolver never throws merely because a legitimate target is absent.

`lumin-resolve` also lowers inventory-owned package metadata into model-owned `PackageSurfaceDeclaration` facts. The graph consumes those declarations but never interprets `package.json` fields itself.

### 4.6 `lumin-graph`

Owns deterministic symbol and source-use graph construction. It consumes already extracted and resolved facts. It does not parse files, probe the filesystem, interpret package manifests, or know Vue/Next/Nuxt conventions.

The graph distinguishes:

- exact identity fan-in;
- type-space fan-in;
- broad or opaque consumption;
- module reachability;
- public surface membership;
- unresolved and unsupported evidence.

Public protection is identity-scoped. A public export cannot protect unrelated sibling exports solely because they share a file.

### 4.7 Analysis Crates

Each analysis consumes immutable model or graph snapshots and emits `EvidenceBatch` values.

- `lumin-dead`: export-level and definition-level liveness classification.
- `lumin-clones`: block, function, shape, and near-clone analysis.
- `lumin-structure`: topology, cycles, boundaries, cohesion, and call-graph projections.
- `lumin-discipline`: escapes, catches, unsafe signals, and checklist facts.

No analysis crate writes artifacts or mutates the canonical graph. Analysis-specific policy stays with its owner instead of accumulating in the engine.

### 4.8 `lumin-store`

Owns immutable run persistence, exact-input cache persistence, and write-gate transactions. The selected persistence engine, physical cache schema, migrations, and locking primitives are private. Capability owners define semantic cache keys; only `lumin-store` calls the backend API or commits storage.

### 4.9 `lumin-engine`

Owns orchestration:

- profile-to-task DAG construction;
- the local Rayon pool and Kahn scheduler;
- stage barriers and deterministic reduction;
- capability lifecycle and cancellation;
- audit and write-gate application services;
- persistence coordination.

It contains no parser implementation, analysis policy, persistence backend calls, Markdown rendering, or framework convention logic.

### 4.10 `lumin-protocol`

Owns versioned external representations:

- CLI request and response DTOs;
- bounded query envelopes;
- optional JSON, Markdown, SARIF, and legacy projections;
- compatibility version negotiation.

It converts between domain values and wire values. Protocol types do not flow back into analysis crates.

### 4.11 `lumin-cli`

Owns argument parsing, process exit policy, stdout/stderr discipline, and invocation of engine services. It contains no analysis semantics and does not select a fallback implementation.

### 4.12 Identity and Version Authority

Type ownership and value authority are distinct:

| Fact | Type owner | Value authority |
| --- | --- | --- |
| `BuildIdentity` | `lumin-model` | `lumin-cli` constructs it once from compile-time release metadata and passes it inward. |
| `AnalysisContractId` | `lumin-model` | `lumin-engine` derives it from ordered capability semantic versions. |
| `RepositoryId` | `lumin-model` | `lumin-inventory` derives it from the canonical root and repository identity inputs. |
| `AttemptId`, `RunId`, `GateId` | `lumin-model` | `lumin-store` allocates and persists them. |
| `GateDecision` and lifecycle state | `lumin-evidence` | The engine gate application service derives them from canonical evidence. |
| `EvidenceQuery` and `PageAnchor` | `lumin-evidence` | The engine query service validates filters and derives deterministic continuation anchors; `lumin-protocol` encodes and decodes opaque cursors. |
| External protocol version and DTO schema | `lumin-protocol` | `lumin-protocol`. |
| Run envelope, evidence-store, gate-store, and cache schema versions | `lumin-store` | `lumin-store`. |
| Extractor, resolver, graph, and rule semantic versions | project-owned model values | The owning capability crate. |

No crate duplicates a value because it owns a representation. Store and protocol receive model or evidence values through their allowed dependency direction.

## 5. Compile-Time Dependency DAG

```text
lumin-cli ------> lumin-engine
    |                  |
    +----------> lumin-protocol
                       |
lumin-engine ----------+----> inventory / languages / resolve / graph / analyses
    |                                     |          |         |        |
    +-------------> lumin-store           +----------+---------+--------+
                         |                                      |
                         v                                      v
                  lumin-evidence ------------------------> lumin-model

lumin-protocol ------> lumin-evidence
lumin-protocol ------> lumin-model
```

The diagram is conceptual; the enforceable edge list is:

- `lumin-model`: external value dependencies only.
- `lumin-evidence` -> `lumin-model`.
- `lumin-inventory` -> `lumin-model`.
- each language crate -> `lumin-model`.
- `lumin-resolve` -> `lumin-model`.
- `lumin-graph` -> `lumin-model`.
- each analysis crate -> `lumin-model`, `lumin-evidence`, and only the graph products it actually consumes.
- `lumin-store` -> `lumin-model`, `lumin-evidence`.
- `lumin-protocol` -> `lumin-model`, `lumin-evidence`.
- `lumin-engine` -> `lumin-model`, `lumin-evidence`, all capability crates it orchestrates, and `lumin-store`.
- `lumin-cli` -> `lumin-engine`, `lumin-protocol`, `lumin-model`.

CI reads `cargo metadata` and rejects workspace dependency edges not listed in the canonical edge policy.

### 5.1 Development-Tool DAG

`tools/xtask` is one development-only crate with `architecture-check`, `corpus`, and `package-check` subcommands. It may inspect `cargo metadata`, repository policy files, fixtures, and public binary behavior. Production crates never depend on it, it is not linked into `lumin`, and it does not import private analysis internals to manufacture expected results.

The architecture check combines `cargo metadata`, scoped Clippy disallowed-method/type policy, owner-path source checks, and compile/public-API boundary fixtures. It rejects global Rayon entry points, runtime Node/Cargo launch sites, source-file reads outside `lumin-inventory`, backend API use outside `lumin-store`, OXC imports outside `lumin-js`, and configured third-party types in public project APIs. Corpus and package checks execute the public binary.

## 6. Forbidden Dependencies

The following are architecture violations:

- foundation crates depending on application crates;
- analysis crates depending on CLI, protocol, store, or parser crates;
- graph crates accessing the filesystem or parser types;
- language crates depending on each other;
- SFC code embedded in the generic resolver or graph;
- persistence backend APIs or types outside `lumin-store`;
- `serde_json::Value` as cross-crate domain transport;
- a `common`, `shared`, or `utils` crate without a named domain responsibility;
- source copies generated into skill packages;
- runtime feature probes duplicated across wrappers and binaries;
- skills importing engine internals;
- production crates depending on `lumin-xtask` or corpus fixtures.

## 7. End-to-End Data Flow

```text
Repository root
  -> SourceInventory
  -> SourceSnapshot / EmbeddedSourceUnit
  -> FileFacts
  -> ResolutionOutcome
  -> SymbolGraph
  -> EvidenceBatch
  -> Canonical Run Store
  -> bounded query / SARIF / optional legacy projection
```

Each arrow crosses through project-owned types. Large ASTs and mutable parser state remain inside the worker that created them.

## 8. Product Surfaces

The one `lumin` binary exposes this canonical command set:

```text
lumin audit
lumin overview
lumin findings
lumin explain
lumin related
lumin files
lumin capabilities
lumin pre-write
lumin post-write <gate-id>
lumin gate
lumin export
lumin help-agent
```

`lumin-protocol` owns command DTOs and machine formats; `lumin-cli` owns parsing and exit mapping. Other documents may narrow a slice's available subset but cannot add commands. Codex and Claude Code skills teach this small surface without embedding schemas, classification logic, platform binary selection policy, or internal capability lists.

## 9. Build and Distribution

- The workspace produces one user-facing `lumin` executable.
- Windows x64 and Linux x64 musl are required from the first accepted slice.
- Platform helpers are release products built in CI from the canonical workspace.
- Skills package binaries and integrity metadata, not copied Rust source trees.
- Runtime compilation is not a supported recovery path.
- A binary reports the protocol version owned by `lumin-protocol` and the `BuildIdentity` value supplied at process construction.
- Package validation executes behavioral contract probes against every shipped binary.

Additional platforms require an explicit product-contract amendment and corpus execution.

## 10. Growth Rules

A new crate proposal must answer:

1. Which forbidden dependency does the crate boundary enforce?
2. Which substantial dependency or compilation unit does it isolate?
3. What project-owned API crosses the boundary?
4. Why is a private module insufficient?
5. What existing owner becomes smaller or clearer?

If those questions do not have concrete answers, use a private module in the current owner.

No implementation file should normally exceed 500 lines excluding tests. A file approaching 800 lines requires an ownership review before more behavior is added. This is a review trigger, not permission to split incoherent fragments into a helper zoo.

## 11. Architecture Acceptance Criteria

1. Every final capability has exactly one owner crate.
2. The dependency graph is acyclic and machine-checked against the canonical edge policy.
3. No parser or persistence-engine type crosses its owner boundary.
4. No stage exchanges analysis data through JSON.
5. SFC resolution misses are representable without exceptions or graph abortion.
6. Exact public API protection is symbol-scoped.
7. One engine and one protocol version serve CLI, Codex, and Claude Code.
8. Packaging contains no copied source fallback.
9. A vertical slice can be added without creating an empty future crate.
10. Independent reviewers can identify where each product fact is created, transformed, persisted, and queried.
11. Identity values and schema versions follow the authority table without reverse dependencies.
12. Development verification runs through `lumin-xtask` without entering the production dependency DAG.

## 12. Review Questions

Architecture reviewers must challenge:

- whether `lumin-model` or `lumin-engine` can become a new mega-crate;
- whether an analysis policy has leaked into orchestration;
- whether SFC ownership is sufficiently isolated without duplicating JS parsing;
- whether the selected persistence engine satisfies the ARCH-002 decision gate without leaking through `lumin-store`;
- whether the dependency edge policy is enforceable rather than aspirational;
- whether any compatibility requirement would force a second production truth owner;
- whether the first vertical slice proves the architecture instead of bypassing it.
