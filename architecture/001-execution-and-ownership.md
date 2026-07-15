# ARCH-001: Execution, Ownership, and Determinism

Document role: concurrency and runtime architecture owner

Status: draft

Revision: 2026-07-15

Parent: ARCH-000

## 0. One-Line Definition

Lumin executes a dependency DAG with a single-owner Kahn scheduler and one explicit local Rayon pool, while workers consume immutable inputs and return owned outputs for deterministic reduction.

## 1. Why This Is a Foundation Contract

Parallelism changes ownership, error propagation, cache identity, result ordering, memory pressure, and observability. Adding it after a sequential engine is established would require redesigning every stage boundary.

The final concurrency model therefore exists in the first accepted vertical slice. A sequential run is the same engine with `jobs=1`, not a separate implementation.

## 2. Execution DAG

The engine constructs the complete stage graph for the selected profile before executing work.

```text
validate-root
  -> inventory
      -> extract-js/ts files -----------+
      -> decompose-sfc files -> extract embedded js/ts
      -> extract-rust files ------------+
                                          -> resolve source uses
                                              -> build symbol graph
                                                  -> dead analysis --------+
                                                  -> structure analysis ---+-> persist run
                                                  -> clone analysis -------+
                                                  -> discipline analysis --+
```

The concrete graph varies by profile and observed languages, but every node declares:

- a stable task key;
- prerequisite task keys;
- capability owner;
- immutable input identities;
- expected output type;
- failure class;
- whether its output participates in canonical evidence.

Task nodes are enum-backed product operations, not dynamically named strings and not one trait implementation per tiny step.

The Kahn graph contains capability and reduction stages, not one node for every source file. High-cardinality file work runs as deterministic data-parallel batches inside its owning stage. Batching controls memory and scheduling only; it cannot omit semantic work or truncate findings.

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

Inventory enumerates normalized `SourceEntry` values without retaining the entire corpus bytes. A file worker reads one entry, creates a `SourceSnapshot` from the exact worktree bytes, computes its identity, checks exact-input reuse, and either drops hit bytes or parses miss bytes within that worker pipeline.

For parser-backed languages, each worker owns:

- its parser allocator;
- its AST;
- temporary traversal state;
- per-file interning or scratch buffers;
- the resulting owned `FileFacts` value.

AST and allocator-backed references never cross the worker boundary. Workers lower all retained information into project-owned values before returning.

### 5.2 Shared Inputs

Large immutable byte buffers may use `Arc<[u8]>` when multiple real consumers require the same bytes. `Arc` cloning is allowed only for intentional immutable sharing. Shared mutable parser, graph, or evidence state is forbidden.

### 5.3 Worker Outputs

Workers return owned result enums such as:

```text
Complete(FileFacts)
Incomplete(FileFacts, diagnostics)
Unsupported(OpaqueFact)
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

Already running workers may finish and release resources, but their outputs are not promoted into a completed run after a hard-stop. Cancellation is cooperative and artifact-visible; elapsed wall-time caps are not a correctness mechanism.

No task may swallow a panic, channel closure, parse failure, or persistence error and replace it with default data.

## 9. Filesystem and I/O Rules

- Inventory enumerates the source set once without retaining all source bytes.
- Each analyzed worktree file is read once per cold run by its worker pipeline.
- Every parser consumes the same bytes used to compute that snapshot's content identity.
- Downstream stages consume facts, not source files.
- Result transport occurs after analysis locks and leases are released.
- Canonical persistence uses one writer and an atomic publish step.
- WSL `/mnt/<drive>` performance is measured separately from WSL ext4 and native Windows; Rayon is not presented as a cure for cross-filesystem latency.

## 10. Incremental Identity

Incremental reuse is an optimization over exact inputs, never a source of truth.

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

1. `jobs=1` and each supported parallel worker count produce byte-identical canonical evidence, excluding explicitly non-semantic runtime metadata.
2. Randomized task completion order does not change IDs, counts, ordering, or classifications.
3. A scheduler cycle fails before capability execution.
4. Parser AST types cannot be named from downstream crates.
5. No global Rayon pool is used.
6. No worker mutates canonical graph or store state.
7. The exact bytes used for a cache identity are the bytes parsed.
8. A missing SFC target produces typed unresolved evidence while unrelated files complete.
9. A hard-stop cannot publish a run marked complete.
10. Cold and warm corpus benchmarks report stage timings and peak memory on native Windows, WSL ext4, and the declared Linux CI platform.
