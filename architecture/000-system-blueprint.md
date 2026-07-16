# ARCH-000: Lumin v2 System Blueprint

Document role: final architecture blueprint and review packet

Status: draft

Revision: 2026-07-16

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

- losslessly encoded, normalized repository-relative paths;
- logical source, physical file, payload snapshot, symbol, use, span, and package identifiers;
- source snapshots and fingerprints;
- language-neutral module and symbol facts;
- typed resolution outcomes;
- path-level semantic-input demands, exact consulted-semantic-input sets, and owner step/outcome values;
- owner diagnostic, opaque-fact, and file-failure payload values lowered before capability return;
- closed limitation reasons and their model-owned scopes;
- build, analysis-contract, analysis-input, repository, attempt, run, gate, operation, gate-baseline-observation, gate-close-observation, and embedded-source identity value types;
- dependency-light gate signals and their originating owner/rule identities;
- the typed gate-projection context routed to capability owners after cold/cache validation;
- dependency-light confidence and grounding rank values used by owner facts and deltas;
- completeness and opacity states.

`RepoPath` is a component sequence, never a Unicode `String`. `repo-path.v1` stores a portable component as its exact UTF-8 bytes without normalization, a non-UTF-8 Unix component as exact bytes, and a non-scalar Windows component as exact WTF-16 code units. Its versioned length-prefixed binary encoding is the sole lexical key for logical source IDs, ordering, hashes, leases, cache keys, and cursor anchors; observed physical identity and parent-specific comparison behavior remain separate facts. The absolute repository root uses the same lossless native atoms plus platform root/volume and physical identity in `RepositoryRootIdentity`; it is never squeezed into a relative `RepoPath` or display string. Protocol display text cannot be converted back into identity.

`LogicalSourceId`, `PhysicalFileIdentity`, and `PayloadSnapshotId` are distinct. Inventory creates one logical source for each admitted lexical `RepoPath` and source kind, so package ownership, controlling configuration, scan role, resolver profile, findings, and gate reads remain path-contextual. Several logical sources may name one physical file. Physical identity establishes containment, alias conflicts, cycle prevention, and safe read reuse only; it never selects a representative logical source. A payload snapshot identifies exact captured bytes plus the physical observation used to validate them. Owners may reuse one payload/parse result for compatible parse modes, but they lower a separate logical-source fact envelope and resolve every source use in that logical source's context.

It must not depend on parsers, filesystems, persistence engines, CLI frameworks, or artifact formats. It is not a miscellaneous helper crate.

### 4.2 `lumin-evidence`

Owns canonical run evidence:

- run, capability, finding, diagnostic, metric, and limitation records;
- canonical records and ordering semantics for model-owned confidence and grounding ranks;
- gate effects, decisions, lifecycle state, and lifecycle evidence;
- the closed, versioned gate-signal-to-effect policy;
- stable evidence relationships;
- gate findings shared by audit and write-gate workflows.

It depends only on `lumin-model` and dependency-light value libraries. It does not persist or render evidence.

### 4.3 `lumin-inventory`

Owns repository observation:

- root validation and scan scope;
- ignore and exclusion policy application;
- workspace and nearest-manifest ownership;
- explicit entry declarations and their normalized source identities;
- semantic configuration snapshots;
- extraction payload snapshots, final freshness identities, and source-set fingerprints;
- generated, test-like, vendored, and out-of-scope classification.

It emits project-owned model types. No downstream crate reads arbitrary source files behind its back.

### 4.4 Language Crates

`lumin-js` owns OXC-based JS/TS parsing and lowering. OXC allocator, AST, spans, and syntax types never leave the crate.

`lumin-sfc` owns Vue, Svelte, Astro, and related container decomposition. It emits embedded source units, component/resource references, and opaque framework evidence. It does not call the JS parser directly; the engine routes emitted embedded units to `lumin-js`.

The SFC boundary is dialect-extensible rather than Vue-shaped. Common project-owned facts carry explicit dialect identity, per-dialect capability status, and decomposition/finalization outcomes, while dialect-specific parsing and binding policy stays private to `lumin-sfc`. A complete Vue capability cannot turn unavailable Svelte or Astro evidence into aggregate SFC completeness. This extension seam is not a public plugin trait and does not require one crate per dialect.

The first slice implements Vue as the first production dialect. Adding Svelte, Astro, or another dialect adds owner-local behavior and corpus truth without adding engine stages or framework policy to `lumin-engine`, `lumin-resolve`, or `lumin-graph`. Any required common-model expansion is an explicit owner-contract revision, never an implicit fallback.

`lumin-rust` owns Rust syntax and optional compiler-oracle integration. Rust parser and compiler ecosystem types never leave the crate.

Any future compiler oracle is an explicit opt-in external capability with visible toolchain requirements and unavailable semantics. It is not a hidden default dependency or runtime fallback.

Every language crate lowers into `lumin-model` facts. A parse failure is a file outcome, not permission to invent an empty successful file.

### 4.5 `lumin-resolve`

Owns source-use resolution over an immutable inventory index. It accepts normalized requests and returns exactly one typed outcome:

```rust
enum ResolutionOutcome {
    Internal(LogicalSourceId),
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

`lumin-resolve` owns resolution-profile selection. A typed invocation override wins and supersedes only profile selection. Without that override, the effective value in an importer's nearest controlling `tsconfig` selects the profile when supported; an explicit unsupported value is incomplete rather than skipped; and an importer with no explicit value uses the named product default, `bundler`. Unreadable controlling configuration remains incomplete even under an override because non-profile resolver inputs may be unknown. Embedded SFC script source uses are importers under this same rule; template-to-component binding consumes resolved script bindings and is not a second resolver lane. The resolver records the selected profile, source, and reason as model facts. Configuration choices participate in `AnalysisInputId`, while the mapping/default policy version participates in `AnalysisContractId`.

`lumin-resolve` also owns the closed [`resolver-config-semantics.v1` artifact](../specs/resolver-config-semantics.v1.json), exact file SHA-256 `50a4c59b4ff594ad1ef99062d030f1c1fae4159d6aa1fdf697111f4b64c92f48`. It pins `typescript@6.0.0-beta`, the extracted 122-key compiler-option set and shape digest, Node `v24.14.1` package semantics, supported condition keys, every tsconfig/package field and nested shape, neutral rationale, unsupported limitation, and exports grammar. Unknown `compilerOptions`/tsconfig keys, unknown package condition keys, unknown shapes beneath a registered field, and future fields absent from this exact artifact are unsupported rather than presumed neutral. Unknown top-level `package.json` fields are neutral only under the artifact's explicit package-top-level rule because the product resolver never consults them. The artifact byte identity and compiled-table digest participate in `AnalysisContractId`; every observed field/value identity participates in `AnalysisInputId` and semantic-read closure. Architecture-check rejects any compiled table, baseline digest, reason, shape, or condition set that differs from the checked-in artifact.

`lumin-inventory` reads each configuration payload once and lowers strict package JSON or tsconfig JSONC into model-owned `ConfigDocument`/`ConfigValue` values with source order and spans. Duplicate keys are malformed. `lumin-resolve` consumes that project-owned tree and applies the registry; parser nodes, `serde_json::Value`, and a second unparsed read never cross the crate boundary.

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

Owns immutable run persistence, exact-input cache persistence, write-gate transactions, no-follow admission of the reserved `.lumin` namespace, and the exclusive catalog-publication guard shared by latest publication/recovery, retention confirmation, and migration. The selected persistence engine, physical cache schema, migrations, directory-handle operations, and locking primitives are private. Capability owners define semantic cache keys; only `lumin-store` calls the backend API or commits storage.

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
| `AnalysisContractId` | `lumin-model` | `lumin-engine` derives it only from the ordered software semantic versions of the profile, inventory/path/scan contracts, extractors, resolver/config registry, graph, and selected rules. |
| `AnalysisInputId` | `lumin-model` | `lumin-engine` derives it from repository identity, profile parameters, effective explicit entries, exact canonical path/source-set identity, scan policy, and consulted repository configuration identities. |
| `RepoPath`, `RepositoryRootIdentity`, their canonical bytes, and path comparison facts | `lumin-model` | `lumin-inventory` lowers native OS paths/roots losslessly and observes physical identity/comparison behavior; no other crate reconstructs identity from display text. |
| `LogicalSourceId`, `PhysicalFileIdentity`, and `PayloadSnapshotId` | `lumin-model` | `lumin-inventory` allocates one logical identity per admitted lexical source context, records physical aliases separately, and derives a payload identity only from exact captured bytes plus their validated physical observation. |
| `RepositoryId` | `lumin-model` | `lumin-inventory` derives it from the canonical root and repository identity inputs. |
| `ConfigDocument` and `ConfigValue` | `lumin-model` | `lumin-inventory` parses each exact config snapshot once into ordered project-owned values/spans; semantic owners consume that tree without receiving parser/backend types. |
| `AttemptId`, `RunId`, `GateId` | `lumin-model` | `lumin-store` allocates and persists them. |
| `OperationId` | `lumin-model` | The caller creates it before a mutating gate or retention lifecycle command; `lumin-store` binds it to one repository-scoped request digest and committed result. |
| `RetentionPlanId`, its canonical content identity, and `PinId` | `lumin-model` | `lumin-store` derives/allocates them in the transaction that commits the immutable prepared plan or independent run reference; each is scoped to one `RepositoryId`. |
| `SemanticInputDemandSet`, `ConsultedSemanticInputs`, and `OwnerStep<T,C>` | `lumin-model` | A capability owner emits path-level demands without consuming their bytes and may carry one capability-owned, fully owned continuation `C`; `lumin-engine` reserves demands, `lumin-inventory` captures exact snapshots, and only a finished owner step reports the exact supplied identities it consumed. |
| `OwnerDiagnostic`, `OpaqueFact`, and `FileFailure` | `lumin-model` | Capability owners lower parser/resolver-specific state into these dependency-light values; `lumin-evidence` creates canonical diagnostic/evidence records from them without changing their meaning. |
| `OwnerOutcome<T>`, `CachedOwnerStep<T>`, and `CachedCapabilityOutput<T>` | `lumin-model` | The capability owner creates each exact-supplied-input demand step and the complete finished state/payload/diagnostic/limitation/gate-neutral envelope; `lumin-store` persists them and the engine replays one prerequisite-keyed step at a time. |
| `GateBaselineObservationId` | `lumin-model` | `lumin-engine` derives it from the exact declared/leased observation domain, semantic reads, content identities, and gate-catalog revision accepted at open. |
| `GateCloseObservationId` | `lumin-model` | `lumin-engine` derives it from the exact actual-write and semantic-read sets, content identities, and transition/catalog revision accepted at close. |
| `ObservationBinding` | `lumin-model` | `lumin-engine` emits `Sealed` only from a complete baseline/close observation and emits typed `Unsealed` for nonauthorizing closure failures without inventing an ID. |
| `WorktreeTransition`, `TransitionCapsule`, and `ActiveGateTransitionRef` | `lumin-evidence` | `lumin-engine` derives the capsule from sealed terminal observations; `lumin-store` sequences it and atomically creates/releases references for active gates that may need the reconciliation proof. |
| `ConfidenceRank` and `GroundingRank` | `lumin-model` | The owning capability assigns them under its versioned semantic contract; `lumin-evidence` persists them and owns their canonical record/query projection without redefining the rank. |
| `DeltaKey`, `DeltaDimensionChange`, and `GateDeltaClassification` | `lumin-model` | For post-write, the owning capability compares normalized baseline/current facts under the total relation defined by the active slice before any adverse lifecycle signal is mapped. Pre-write may emit only the named advisory signal for a complete existing fact or a required-evidence signal for missing authorization evidence. |
| `GateSignal` | `lumin-model` | Capability owners emit signals from facts/deltas; the engine gate service emits named transaction-invariant signals from typed store/inventory outcomes, while its capability registry emits only named compiled-profile availability facts/signals. |
| `GateProjectionContext` | `lumin-model` | `lumin-engine` derives it from normalized intent/affected scope and the exact opening/current gate identities; capability owners consume it only after validating their owner outcome, and it never participates in a repository-input-only cache payload. |
| `GateEffect`, `GateDecision`, and lifecycle state | `lumin-evidence` | `lumin-evidence::gate_policy` owns the closed signal-to-effect table and policy version; the engine only invokes that mapping and applies the canonical reducer/transition tables. |
| `EvidenceQuery`, `CollectionOrderingId`, and `PageAnchor` | `lumin-evidence` | The engine query service validates filters, selects the owner-defined collection ordering version, and derives deterministic continuation anchors; `lumin-protocol` encodes and decodes opaque cursors. |
| `RepoPathDto` and escaped path display | `lumin-protocol` | The protocol always carries canonical `repo-path.v1` bytes and may add readable UTF-8/display projections; decoding validates canonical bytes instead of trusting the projection. |
| External protocol version and DTO schema | `lumin-protocol` | `lumin-protocol`. |
| Run envelope, evidence-store, lifecycle-store, and cache schema versions | `lumin-store` | `lumin-store`. |
| State-namespace schema/marker, `StateDirectoryIdentity`, `LifecycleLockIdentity`, `StateNamespaceNonce`, and `CatalogPublicationGuard` | `lumin-store` project API/private values | `lumin-store` creates the namespace/lock once, binds both physical identities and the nonce into the marker and store header, and yields the guard only after entry-to-handle revalidation. |
| `StoreGeneration` and `MigrationIntent` | `lumin-store` project API | `lumin-store` allocates the next generation under the exclusive repository migration lock; every transaction is fenced to the generation of the backend handle it opened. |
| Extractor, resolver, graph, and rule semantic versions | project-owned model values | The owning capability crate. |

No crate duplicates a value because it owns a representation. Store and protocol receive model or evidence values through their allowed dependency direction.

The physical gate-policy authority is fixed:

| Signal family | Fact/signal value owner | Effect-policy owner | Permitted edge |
| --- | --- | --- | --- |
| parse, SFC, and opacity | owning language crate | `lumin-evidence::gate_policy` | language -> model; evidence -> model |
| resolution | `lumin-resolve` | `lumin-evidence::gate_policy` | resolve -> model; evidence -> model |
| package/dependency ownership and observation drift | `lumin-inventory` | `lumin-evidence::gate_policy` | inventory -> model; evidence -> model |
| graph/dead evidence | owning graph or analysis crate | `lumin-evidence::gate_policy` | graph -> model or analysis -> model/evidence |
| compiled capability availability/profile | `lumin-engine` capability registry | `lumin-evidence::gate_policy` | engine -> model/evidence; registry emits availability only and owns no substitute analysis |
| lease, containment, unplanned-transition, and lifecycle invariants | engine gate service from typed store/inventory outcomes | `lumin-evidence::gate_policy` | engine -> model/evidence/store |

Capability crates never construct `GateEffect`, and `lumin-engine` never chooses an effect. Adding a signal or changing its effect requires the fact owner contract, the closed gate-policy table/version, and the architecture edge check to change together.

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

The architecture check combines `cargo metadata`, scoped Clippy disallowed-method/type policy, owner-path source checks, exhaustive owner matches, and compile/public-API boundary fixtures. It rejects global Rayon entry points, runtime Node/Cargo launch sites, source-file reads outside `lumin-inventory`, backend API use or backend handles outside `lumin-store`, backend handles that escape one repository-lock transaction, OXC imports outside `lumin-js`, configured third-party types in public project APIs or owner continuations, continuations containing borrowed parser/allocator state or open handles, owners that consume unsupplied/unreserved semantic inputs, gate projections that access I/O or emit late demands, cache envelopes missing owner outcome or diagnostic state, unavailable-capability signals outside the engine registry, limitation variants without static scope/absence/relevance ownership, semantic fact fields absent from their owner's key/dimension/metadata registry, non-total post-write delta mappings, adverse lifecycle effects that bypass typed delta classification, string/lossy path identity, physical-file deduplication that erases a logical source context, raw `.lumin` filesystem access outside store-owned no-follow helpers, a lock/namespace marker or store header missing bound physical identities, latest compare/replace or retention confirmation outside the exclusive catalog guard, a resolver compiled table or baseline digest differing from the exact `resolver-config-semantics.v1` artifact, unsupported affecting fields that fail to emit incomplete evidence, and any product `ScanLock` correctness primitive. Corpus and package checks execute the public binary.

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
lumin operation
lumin gate
lumin runs
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
11. Identity values and schema versions follow the authority table without reverse dependencies; software compatibility and repository input freshness use different IDs.
12. Development verification runs through `lumin-xtask` without entering the production dependency DAG.
13. A new SFC dialect enters through the existing `lumin-sfc` stages without leaking framework policy into the engine, resolver, graph, or a second pipeline.
14. Cache replay, observation binding, gate delta classification, retention plans, pins, and collection ordering use the authority table without duplicate policy in engine, store, protocol, or CLI.
15. Every semantic input is demanded before reservation/capture/consumption, every unavailable capability signal has the named registry owner, and every post-write fact relation has one exhaustive delta classification.
16. Active-gate transition references and generation-fenced transaction handles prevent retention or lifecycle-store migration from invalidating a live transaction's proof.
17. Every native repository path has one lossless `repo-path.v1` identity and wire round trip; logical source identity survives physical aliasing, while physical identity and payload snapshots may deduplicate only conflicts and byte/parse work.
18. Every first-slice resolver-affecting configuration field/shape is owned by the exact checked-in registry artifact or emits scoped incomplete evidence; the compiled table, baselines, and artifact digest must match it byte-for-byte.
19. Only `lumin-store` opens the reserved `.lumin` namespace or acquires `CatalogPublicationGuard`; bound state-directory/lock identities prevent replacement split brain, and latest/retention/migration code cannot bypass revalidation or the exclusive guard.
20. Architecture v1 contains no `ScanLock` product type or correctness claim; scheduler coordination cannot substitute for snapshot, reservation, or lifecycle-store owners.

## 12. Review Questions

Architecture reviewers must challenge:

- whether `lumin-model` or `lumin-engine` can become a new mega-crate;
- whether an analysis policy has leaked into orchestration;
- whether SFC ownership is sufficiently isolated and dialect-extensible without duplicating JS parsing or leaking framework policy;
- whether the selected persistence engine satisfies the ARCH-002 decision gate without leaking through `lumin-store`;
- whether the dependency edge policy is enforceable rather than aspirational;
- whether any compatibility requirement would force a second production truth owner;
- whether the first vertical slice proves the architecture instead of bypassing it.
