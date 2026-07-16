# ARCH-001: Execution, Ownership, and Determinism

Document role: concurrency and runtime architecture owner

Status: draft

Revision: 2026-07-16

Parent: ARCH-000

## 0. One-Line Definition

Lumin executes a dependency DAG with a single-owner Kahn scheduler and one explicit local Rayon pool, while workers consume immutable inputs and return owned outputs for deterministic reduction.

## 1. Why This Is a Foundation Contract

Parallelism changes ownership, error propagation, cache identity, result ordering, memory pressure, and observability. Adding it after a sequential engine is established would require redesigning every stage boundary.

The final concurrency model therefore exists in the first accepted vertical slice. A sequential run is the same engine with `jobs=1`, not a separate implementation.

## 2. Execution DAG

The engine constructs the complete stage graph for the selected profile before executing work.

```text
validate-root -> inventory
inventory -> extract-js-ts
inventory -> decompose-sfc -> extract-inline-js-ts
inventory -> extract-rust
extract-js-ts + decompose-sfc + extract-inline-js-ts -> finalize-sfc-facts
extract-js-ts + finalize-sfc-facts + extract-rust -> resolve-source-uses
resolve-source-uses -> build-symbol-graph
build-symbol-graph -> selected-analysis-stages -> persist-run
```

The stage node set is fixed before execution from the selected profile and compiled capability set. Inventory results never add or remove nodes; they supply deterministic input batches. A selected language stage receives an empty batch when no matching sources exist. A capability not compiled into the binary is represented as unavailable during profile construction rather than discovered by mutating the running DAG.

Every node declares:

- a stable task key;
- prerequisite task keys;
- capability owner;
- immutable input identities;
- expected output type;
- failure class;
- whether its output participates in canonical evidence.

Task nodes are enum-backed product operations, not dynamically named strings and not one trait implementation per tiny step.

The Kahn graph contains capability and reduction stages, not one node for every source file. High-cardinality file work runs as deterministic data-parallel batches inside its owning stage. Batching controls memory and scheduling only; it cannot omit semantic work or truncate findings.

### 2.1 SFC Finalization Stage

The stage topology is dialect-neutral. `decompose-sfc` emits model-owned SFC structure, template uses, embedded-source descriptors, and explicit dialect identity. `extract-inline-js-ts` lowers inline units through `lumin-js`, while `extract-js-ts` supplies facts for external source references. `finalize-sfc-facts`, implemented by `lumin-sfc` and routed by the engine, receives model-owned JS facts and dispatches dialect-specific binding inside the owner crate. The first slice executes the Vue binding path; recognized but unsupported dialects return explicit unavailable evidence from the same SFC stages. Neither `lumin-engine` nor `lumin-graph` implements framework policy.

The model represents:

- explicit SFC dialect identity and capability status;
- `EmbeddedSourceUnitId`, parent `SourceId`, parent byte-span mapping, parse mode, and content identity;
- inline embedded bytes owned only until extraction completes;
- `ExternalEmbeddedSourceRef(SourceId)` for `<script src>` rather than a copied source unit;
- logical SFC attachment identity separately from the physical source span owner.

An external script is read and parsed once for a given parse mode. Its JS facts and source-use edges remain owned by the physical `SourceId`; finalization emits an `SfcScriptAttachment` that links the parent SFC to those facts without cloning them under a second module identity. Inline facts remain owned by their `EmbeddedSourceUnitId`. A `lang` attribute that conflicts with the external file's supported parse mode is explicit unsupported evidence in the first slice, not permission to parse the same source under an implicit second mode.

## 3. Kahn Scheduler Contract

The scheduler owns all mutable DAG state:

- in-degree counters;
- ready task ordering;
- task lifecycle state;
- cancellation state;
- completed output registration.

Execution proceeds as follows:

1. Validate that every dependency exists.
2. Compute in-degrees.
3. Place all zero-degree tasks in a stable ordered ready set.
4. Dispatch the current compatible ready batch to the local pool.
5. Receive owned task results.
6. Register results in stable task-key order.
7. Decrement dependent in-degrees.
8. Repeat until all tasks finish or a hard-stop invalidates the run.
9. If uncompleted nodes remain with no ready nodes, report an internal DAG cycle and persist no completed run.

Workers never decrement in-degrees or mutate the ready set. Kahn scheduling prevents dependency races and detects cycles; Rust ownership and reduction rules prevent memory races.

## 4. Rayon Contract

`lumin-engine` creates exactly one local pool with `rayon::ThreadPoolBuilder` and executes the run under that pool.

Required properties:

- no use of Rayon's process-global pool;
- no nested pool creation by capability crates;
- explicit worker count;
- explicit worker stack policy selected and documented through corpus measurement;
- stable worker naming for diagnostics;
- panic propagation converted into an internal hard-stop diagnostic;
- the chosen worker count and stack policy recorded in run metrics.

The CLI exposes `--jobs`. Its default is derived from the runtime's available parallelism and is artifact-visible. Hidden environment-specific thread caps are forbidden.

Capability crates may use Rayon parallel iterators only while installed in the engine-owned pool. They do not own thread policy.

## 5. Worker Ownership

### 5.1 Source Workers

Inventory enumerates normalized `SourceEntry` values without retaining the entire corpus bytes. An engine-owned file-worker adapter asks `lumin-inventory` to read one entry and create a `SourceSnapshot` from the exact worktree bytes, then queries `lumin-store` through a backend-neutral cache reader. It drops hit bytes or calls the owning language extractor for miss bytes. Language capability crates never read arbitrary files or open the cache or persistence backend themselves.

For parser-backed languages, each worker owns:

- its parser allocator;
- its AST;
- temporary traversal state;
- per-file interning or scratch buffers;
- the resulting owned `FileFacts` value.

AST and allocator-backed references never cross the worker boundary. Workers lower all retained information into project-owned values before returning.

Cache misses return fact records and cache-write candidates as owned outputs. One deterministic cache writer commits compatible entries after reduction; cache writes never mutate canonical run evidence.

### 5.2 Shared Inputs

Large immutable byte buffers may use `Arc<[u8]>` when multiple real consumers require the same bytes. `Arc` cloning is allowed only for intentional immutable sharing. Shared mutable parser, graph, or evidence state is forbidden.

### 5.3 Worker Outputs

Workers return owned result enums such as:

```text
Complete(FileFacts)
Incomplete(FileFacts, diagnostics, LimitationScope)
Unsupported(OpaqueFact, LimitationScope)
Failed(FileFailure)
```

`OpaqueFact` is a model value that an evidence owner may later cite. An empty fact set is not a substitute for a failed or unsupported file.

## 6. Deterministic Reduction

Parallel workers do not write into a central `HashMap`, graph, database, or artifact.

Every fan-in point has a single reducer that:

1. receives all required owned outputs;
2. sorts by stable semantic keys;
3. assigns stable IDs;
4. deduplicates according to the owning contract;
5. materializes the next immutable stage snapshot.

Stable ordering keys include normalized repository path, source span, symbol identity, use kind, and finding rule. Wall-clock completion order is never an ordering key.

Maps that affect persisted output use deterministic ordering or are sorted before persistence. Generated timestamps are metadata and do not participate in cache or semantic identities.

Determinism compares a canonical semantic dump, not physical store bytes. The dump includes sorted source identities, facts, findings, capability states, limitations, diagnostics, semantic policy versions, and stable IDs. It excludes run IDs, timestamps, worker policy, timings, cache and RSS metrics, publication pointers, physical page layout, store size, and store hash. Runtime metrics remain canonical run records in a non-semantic partition.

Cross-run finding IDs derive from rule ID and version plus repository-relative semantic source, symbol, and evidence identity. Run-local ordinals are not finding IDs. A hash collision is an internal hard-stop unless the owning ID format provides deterministic collision disambiguation.

## 7. Stage Parallelism

Parallel work is used where ownership is naturally independent:

- file reads after inventory enumeration;
- per-file parsing and extraction;
- SFC decomposition by file;
- resolution by immutable use batches;
- clone fingerprinting and candidate buckets;
- independent analyses after graph finalization;
- independent query projection pages.

Single-owner execution is retained where it protects clarity or determinism:

- DAG state mutation;
- canonical path and identity assignment;
- graph final reduction;
- canonical evidence commit;
- active write-gate lease mutation;
- process exit policy.

No performance claim may be based solely on increasing threads. Algorithms must still have justified complexity, and candidate limiting must occur before expensive pairwise scoring when semantics permit it.

## 8. Failure and Cancellation

Failures are classified by their owner:

- `file-incomplete`: the run continues and records scoped limitations;
- `capability-incomplete`: independent capabilities may continue, but absence claims from this capability are disabled;
- `unsupported`: the run continues with opaque evidence;
- `hard-stop`: the scheduler stops dispatching dependent work and refuses to publish a completed run.

Every incomplete or opaque result carries a model-owned limitation scope: `File`, `Module`, `ExplicitTargets`, `Package`, or `Workspace`. Analysis owners must intersect that scope with candidate evidence before making an absence claim. A vertical slice defines the normative scope for each supported failure and opacity class; reducers cannot invent or widen it silently.

Each active slice owns a closed registry for every incomplete, unsupported, and opaque reason it can emit. Every reason maps exhaustively to its fact owner, limitation scope, optional target derivation, downstream absence effect, and gate signal. Capability crates convert their private reason enums through exhaustive matches; architecture verification fails when a reason can be emitted without a mapping.

Already running workers may finish and release resources, but their outputs are not promoted into a completed run after a hard-stop. Cancellation is cooperative and artifact-visible; elapsed wall-time caps are not a correctness mechanism.

No task may swallow a panic, channel closure, parse failure, or persistence error and replace it with default data.

## 9. Filesystem and I/O Rules

- Inventory enumerates the source set once without retaining all source bytes.
- Each analyzed worktree payload is read once for extraction per cold run; a separate final hash-only freshness pass is permitted.
- Every parser consumes the same bytes used to compute that snapshot's content identity.
- Downstream stages consume facts, not source files.
- Result transport occurs after storage transaction locks, scan locks, and operation-liveness leases are released. An `Active` gate's durable logical path lease is repository state, not a held runtime lock, and remains until close or abandon.
- Canonical persistence uses one writer and the ARCH-002 crash-consistent publication protocol.
- WSL `/mnt/<drive>` performance is measured separately from WSL ext4 and native Windows; Rayon is not presented as a cure for cross-filesystem latency.

### 9.1 Snapshot and Freshness Contract

`SnapshotStatus` is a model-owned value evaluated against an explicit observation:

- `Current`: the complete source set, semantic configuration set, and exact content identities were compared and match;
- `Drifted`: a compared path set or content identity differs;
- `Unverifiable`: the required comparison was not or could not be completed;
- `UnstableDuringScan`: the source or semantic configuration snapshot changed between capture and publication validation.

Before publishing a completed run, inventory repeats source/config set discovery and an exact hash-only identity pass. `UnstableDuringScan` publishes an attempt failure but no completed run. Lumin does not retry under an arbitrary wall-time or attempt cap.

Any query that presents a current-worktree absence claim performs the required freshness comparison. A historical query may skip that work only by reporting `Unverifiable` for current-worktree freshness. Stored source fingerprints still describe exactly which bytes the historical evidence analyzed.

### 9.2 Semantic-Read Closure

Every capability owner that reads a source or configuration identity returns model-owned `ConsultedSemanticInputs` with its facts and signals. The engine does not infer this set from display diagnostics, and an owner cannot read an unreported path behind the inventory boundary.

Gate analysis closes semantic inputs by monotonic fixed point:

1. start from the declared and owner-inferred candidate read set;
2. capture exact identities and run the affected owners;
3. union every returned `ConsultedSemanticInputs` value;
4. when the union grows, reserve the new reads against active writers, capture their exact identities, and rerun every owner whose result can change;
5. seal the set only when a complete iteration adds no input.

The admitted source/config inventory is finite, so closure uses set growth rather than an arbitrary iteration or wall-time cap. A dynamic or opaque input that cannot be bounded becomes a typed scoped limitation and prevents an authorizing gate decision; it is never omitted to force convergence. Baseline and close observation IDs are derived only after closure and include the sealed set. Final freshness and catalog validation still run after sealing.

## 10. Incremental Identity

Incremental reuse is an optimization over exact inputs, never a source of truth.

`AnalysisContractId` and `AnalysisInputId` are deliberately different:

- `AnalysisContractId` contains only ordered software semantic component versions and answers whether two evidence sets share one analysis meaning;
- `AnalysisInputId` contains repository identity, profile parameters, effective explicit entries, scan policy, source-set identity, and consulted semantic configuration identities and answers whether the same software contract observed the same repository inputs.

A configuration or source change creates a new `AnalysisInputId`, cache miss, or stale baseline under the same compatible `AnalysisContractId`. It never masquerades as binary semantic incompatibility. A software policy-version change creates a new `AnalysisContractId` even when repository bytes are unchanged.

Per-file fact identity includes:

- exact worktree content hash;
- language extractor version;
- relevant language policy version;
- parse mode and configuration identity.

Resolution and graph identity additionally include:

- the normalized source-set fingerprint;
- workspace and alias configuration identities;
- resolver policy version.

The bytes hashed are the bytes parsed. Artifact aliases, Git blob identities, or transformed repository objects cannot stand in for different worktree bytes.

An incompatible or corrupt cache entry becomes a visible miss. It never becomes empty evidence and never causes a fallback to another semantic owner.

## 11. Memory Rules

- Do not retain ASTs after lowering unless a documented capability consumes them within the same worker scope.
- Drop source bytes after all declared consumers complete unless an active query or gate baseline requires their fingerprint.
- Do not collect the entire corpus bytes before determining cache hits.
- Do not use `Arc<Mutex<Graph>>`, `Arc<Mutex<Evidence>>`, or equivalent shared hot-path mutation.
- Prefer worker-local vectors followed by deterministic merge.
- Record peak resident memory in corpus benchmarks.
- A memory guard may degrade only when its omitted scope and reason are part of canonical evidence; it cannot silently truncate.

## 12. Observability

Every run records at least:

- requested and actual worker count;
- worker stack policy;
- task count by owner;
- ready wait, execution, reduction, and persistence time by stage;
- files and bytes read;
- cache hits, misses, and incompatibilities;
- incomplete, unsupported, cancelled, and hard-stop counts;
- peak resident memory when supported;
- platform and filesystem class used by the benchmark harness.

Metrics describe execution but do not redefine semantic findings.

## 13. Acceptance Criteria

1. `jobs=1` and each supported parallel worker count produce byte-identical canonical semantic dumps.
2. Randomized task completion order does not change IDs, counts, ordering, or classifications.
3. A scheduler cycle fails before capability execution.
4. Parser AST types cannot be named from downstream crates.
5. No global Rayon pool is used.
6. No worker mutates canonical graph or store state.
7. The exact bytes used for a cache identity are the bytes parsed.
8. A missing SFC target produces typed unresolved evidence while unrelated files complete.
9. A hard-stop cannot publish a run marked complete.
10. Cold and warm corpus benchmarks report stage timings and peak memory on native Windows, WSL ext4, and the declared Linux CI platform.
11. The stage node set is fixed before inventory executes; language presence changes only input batches.
12. SFC finalization remains owned by `lumin-sfc`; first-slice Vue binding completes there, unsupported dialects remain visible, and an external script payload is not read or parsed twice for one mode.
13. Snapshot drift during a scan prevents completed-run publication, and later query drift is visible.
14. Repository input changes alter `AnalysisInputId` without altering `AnalysisContractId`; software semantic-version changes alter the contract ID.
