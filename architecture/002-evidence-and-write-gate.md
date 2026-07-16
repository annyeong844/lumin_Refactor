# ARCH-002: Evidence Store, Query Protocol, and Write Gate

Document role: evidence delivery and lifecycle architecture owner

Status: draft

Revision: 2026-07-16

Parent: ARCH-000

## 0. One-Line Definition

Lumin persists one canonical evidence model and exposes bounded pull queries, while pre-write and post-write share a durable transaction identified by one gate ID rather than exchanging JSON files.

## 1. Evidence Delivery Position

The default product must not emit a warehouse of large artifacts and instruct an agent to read them. Large artifact push has three structural failures:

- the same count, status, and path are recomputed by multiple projections;
- agents spend context discovering which file matters;
- a failed producer can disappear as a missing artifact and be mistaken for zero findings.

Lumin v2 uses one canonical evidence store. JSON, Markdown, SARIF, review packs, and legacy files are generated projections.

## 2. Storage Layout

The internal workspace layout is:

```text
.lumin/
  latest.json
  lifecycle.store
  attempts/
    <attempt-id>/
      attempt.json
  runs/
    <run-id>/
      run.json
      evidence.store
  trash/
    <prune-plan-id>/
      ... noncanonical payloads awaiting idempotent reclamation ...
  cache/
    ... disposable exact-input cache ...
```

Users and agents do not edit these files directly.

### 2.1 `attempt.json`

Audit start is ordered and crash-recoverable. One repository-catalog transaction allocates the attempt ID/sequence and durable process-liveness lease. Lumin then writes and flushes a `Running` `attempt.json`, advances `latestAttempt` with the sequence-checked replacement protocol, and begins scanning only after both publications are durable. A sequence allocated before its envelope may become a legal gap; it is never reused. A terminal audit atomically replaces the running envelope once with an immutable terminal envelope containing repository/build/request identity, timestamps, success or failure class, concise diagnostics, and optional completed run ID. A hard-stop or unstable snapshot therefore remains addressable even when no run exists.

### 2.2 `run.json`

`run.json` exists only for a published run and contains:

- originating attempt ID and sequence, run ID, repository identity, Lumin build identity, and envelope schema version;
- `AnalysisContractId` for software semantic compatibility and `AnalysisInputId` for the exact repository/profile observation;
- publication state;
- publication-time snapshot status;
- the evidence store format, schema, identity, size, and hash.

Scan scope, capability states, findings, counts, blind zones, metrics, and suggested queries are read from `evidence.store`. The envelope is publication metadata, not a second evidence summary.

### 2.3 `evidence.store`

The immutable run store contains normalized:

- capabilities;
- source identities and spans;
- findings;
- evidence links;
- diagnostics and limitations;
- relationships between findings, symbols, files, and packages;
- metrics;
- projection metadata.

Runtime metrics are canonical run records but belong to a non-semantic partition. Determinism compares the ARCH-001 canonical semantic dump rather than physical store bytes or the complete metric-bearing store.

Only `lumin-store` knows the physical schema or backend API. No public product contract exposes SQL, tables, or backend-specific keys.

Run publication is a crash-consistent multi-object commit protocol, not one fictional atomic filesystem operation:

1. Build `evidence.store` and `run.json` in a private staging directory on the same filesystem as `.lumin/runs`.
2. Close the store, validate its schema and semantic identity, record size/hash in `run.json`, durably flush required files, then durably flush the staging directory using the platform-supported equivalent.
3. Atomically rename the validated staging directory to its immutable run directory and durably flush the `runs` parent.
4. Publish the terminal `attempt.json` by same-directory temporary write, durable flush, atomic replacement, and parent flush.
5. Under the latest-pointer store lock, compare attempt sequences, write and flush a complete replacement `latest.json`, atomically replace it, and flush `.lumin`. A stale process cannot move either pointer backward.

A failed terminal attempt cannot link an evidence store as its completed run. A crash may leave a validated run directory before terminal linkage; recovery treats it only as the orphan case below. Recovery validates target existence, run envelope, store hash/schema, attempt linkage, and sequence before trusting a pointer. A dangling or corrupt pointer is rejected and reported; Lumin does not silently reinterpret an older run as latest. Staging and pointer-temp remnants are noncanonical and may be removed only after validation.

Every crash point has one outcome:

| Crash point | Canonical recovery |
| --- | --- |
| before attempt catalog allocation | no attempt exists |
| after catalog allocation and before durable `Running` envelope | release the dead process lease, preserve a legal sequence gap, and publish no invented attempt |
| after durable `Running` envelope and before `latestAttempt` replacement | publish `Interrupted` and advance the non-regressing attempt pointer |
| after `latestAttempt=Running` publication and before run-directory rename | publish `Interrupted`; no run exists |
| after valid run-directory rename and before terminal attempt publication | preserve the directory as an unpointed orphan, publish `Interrupted`, and never adopt that orphan as a successful run |
| after terminal attempt publication and before latest-pointer replacement | the terminal attempt and linked run are authoritative; recovery advances only non-regressing pointers |
| during latest temporary write or atomic replacement | accept only the complete old or complete new pointer document; partial JSON is noncanonical |

The durable process lease identifies the process/operation that may finish a `Running` attempt. Recovery authority begins only after platform liveness checks prove that lease released. An orphan without a terminal success attempt is inspectable recovery evidence and retention input, not a completed run or an automatic success candidate.

### 2.4 `lifecycle.store`

The repository-wide `lifecycle.store` contains audit attempt sequence/process-lease catalog metadata plus operation-id records, provisional admission and semantic-read-extension reservations, declared intents, logical path leases, baseline fingerprints and facts, advisory findings, immutable worktree transitions, close-out deltas, retention plans/tombstones, and lifecycle history for every gate in that repository. Attempt evidence remains in `attempt.json`; the catalog metadata exists only for allocation and recovery.

One transactional store is required so overlap detection and lease creation commit atomically across concurrent Lumin processes. Completed gate records become immutable by application contract. Active gates are not temporary transport records and are never silently removed while open.

### 2.5 Cache

Cache content is disposable and noncanonical. Deleting it may affect performance but cannot change the meaning of a completed run or gate. Cache corruption becomes a visible miss.

### 2.6 Storage Backend Decision Gate

Architecture v1 does not select a persistence engine by familiarity. `lumin-store` first defines a backend-neutral contract for:

- immutable run publication and read-only reopening;
- indexed bounded queries with stable cursors;
- one atomic cross-process gate lease transaction;
- crash recovery, migrations, and corruption-visible failure;
- Windows NTFS, Linux ext4, and Linux musl release operation.

The architecture review benchmarks at least one pure-Rust embedded candidate, initially `redb`, against bundled SQLite. The comparison records clean and incremental build time, release binary size, transitive and unsafe surface, cold and warm store latency, peak memory, store size, multi-process contention behavior, and crash recovery.

`redb`'s first probe is two independent writer processes contending for the same lifecycle store. If open/lock/retry behavior cannot preserve atomic lease admission without a daemon or a second truth owner, the candidate is rejected before performance comparison.

Correctness probes inject process death at every row of the publication and retention crash tables. They cover unadoptable orphan runs, dangling pointers, corrupt hashes, stale-writer sequence regression, interrupted gate migration, retention tombstone/trash recovery, and unsupported durable-flush behavior on each required platform. Probe evidence preserves source and fixture hashes, toolchain/target, exact commands, expected invariants, crash point, and raw result under `reviews/probes/<probe-id>/`; build output is removed, but reproducibility evidence is not.

The first failing correctness requirement rejects a candidate before performance ranking. Architecture v1 records one accepted backend and rationale; production does not ship dual backends or a runtime fallback.

## 3. Canonical Evidence Model

```text
Run
  Capability
  Source
  Finding
    Evidence
    SourceSpan
    Confidence
    Limitation
    RelatedFinding
  Diagnostic
  Metric
```

Every finding has:

- a stable finding ID derived from semantic identity, not output order;
- one rule and owner capability;
- severity and confidence as separate values;
- a concise claim;
- evidence references;
- relevant scan scope and limitations;
- source fingerprints for referenced spans;
- optional remediation and verification hints.

Counts are computed from canonical rows or owned canonical aggregate rows. A bounded top-N projection cannot become the count owner.

## 4. Query Protocol

The primary interface is:

```text
lumin overview [--run <run-id>]
lumin findings --run <run-id> [filters] [--cursor <cursor>]
lumin explain --run <run-id> <finding-id> [--evidence-cursor <cursor>] [--relations-cursor <cursor>]
lumin related --run <run-id> <finding-id> [--cursor <cursor>]
lumin files --run <run-id> <repo-path> [--cursor <cursor>]
lumin capabilities [--run <run-id>]
```

These commands are the query subset of the canonical ARCH-000 command table. Skills use `--format json`; human-readable output is a projection of the same DTO. Machine output writes one versioned JSON value to stdout and diagnostics to stderr.

`overview` without `--run` selects and returns a concrete immutable run scope after showing any newer failed attempt. Every follow-up run-evidence command requires that returned run ID; it never follows a moving latest pointer. `capabilities` without a run reports the current binary's compiled capabilities, while `capabilities --run <run-id>` reports the states recorded by that run.

Every collection response uses one envelope:

```json
{
  "scope": {"kind": "run", "id": "run_..."},
  "ordering": "findings.v1",
  "total": 812,
  "returned": 20,
  "truncated": true,
  "nextCursor": "...",
  "items": []
}
```

Rules:

- no hidden `take(N)`;
- `total`, `returned`, and `truncated` are mandatory;
- cursors are opaque and bound to protocol schema, immutable scope identity or gate revision, normalized filters, collection path, ordering ID/version, page-size policy, and last semantic key;
- every collection uses its owner-defined ordering below; there is no backend-order or generic-finding-order fallback;
- a current-worktree absence query reports `SnapshotStatus`; drift or unverifiable freshness cannot render a clean claim;
- an unavailable capability is returned as unavailable, never as an empty item set;
- every nested collection, including evidence and relations inside one `explain` result, uses the same bounded page contract;
- stdout is bounded; exhaustive export is an explicit command.

Canonical collection orderings are:

| Collection path | Ordering ID | Canonical key |
| --- | --- | --- |
| run/gate findings | `findings.v1` | severity descending, confidence descending, rule ID, normalized repository path, span start/end, finding ID |
| finding evidence | `evidence.v1` | evidence kind, source identity, span start/end, stable evidence ID |
| finding relations / `related` | `relations.v1` | relation kind, target semantic finding ID, stable relation semantic ID |
| file findings | `file-findings.v1` | normalized repository path, span start/end, finding ID |
| run catalog | `runs.v1` | attempt sequence descending, run ID |
| active gate catalog | `active-gates.v1` | opened transition/catalog sequence, gate ID |
| capabilities | `capabilities.v1` | capability ID |
| retention-plan items | `retention-plan-items.v1` | record-kind rank, owning sequence, stable record ID |

The closed `retention-plan-items.v1` record-kind rank is `attempt=0`, `run=1`, `gate=2`, `gate-revision=3`, `finding=4`, `evidence=5`, `operation=6`, `transition=7`, `pin-or-reference=8`, `orphan-payload=9`, and `tombstone=10`; adding a kind requires a new ordering ID. `owning sequence` is the nearest attempt, run, or gate-revision sequence that owns the item. Every canonical relation row has a stable semantic relation ID derived by `lumin-evidence` from source semantic identity, relation kind, target semantic identity, and grounding evidence identity. Identical relation tuples canonicalize to one row.

Every ordering key is total: its final stable ID uniquely identifies one canonical row after owner deduplication. Textual keys use their canonical model encoding and ascending byte order unless a direction is stated. Adding or changing a collection ordering changes its ordering ID and protocol contract.

Severity and confidence sort by explicit `lumin-evidence` rank values, not localized or display labels. Those ranks are part of `findings.v1`; changing them requires a new ordering ID.

Run collections use `{"kind":"run","id":"run_..."}` scope. Gate collections use `{"kind":"gate-attempt","gateId":"gate_...","revision":7}` scope. Repository catalogs use `{"kind":"repository","id":"repo_...","revision":42}`; a mutation makes an unresumable old view `Stale` rather than silently continuing against new rows. Current-binary capability collections use `{"kind":"binary","buildId":"..."}`, while run capabilities use run scope. An immutable prepared retention plan uses `{"kind":"retention-plan","planId":"plan_...","contentIdentity":"..."}`; unrelated repository mutations do not invalidate its pages, while confirmation separately revalidates current catalog state. Every gate advisory or close attempt increments the revision and persists an immutable revision record. A cursor for an older active-gate revision remains valid only against that revision; it cannot silently advance to newer gate evidence. Invalid, cross-scope, or tampered cursors fail as malformed requests rather than restarting at page one.

Direct run/gate lookup resolves one public state before reading payload collections:

```text
RecordLookup =
  Live
  | Pruning { plan_id, recoverable_state }
  | Pruned { plan_id, tombstone_identity, physical_reclamation_pending }
  | NeverExisted
  | Corrupt
```

`Live` may return the requested bounded payload. `Pruning` and `Pruned` return a typed tombstone envelope with exit `0`, never an empty findings collection or plain not-found; the same state is projected by plan and operation queries. `NeverExisted` is a typed not-found with exit `2`. `Corrupt` is an integrity hard-stop with exit `1`.

## 5. Agent Consumption

Codex and Claude Code follow the same short workflow:

1. Run `lumin overview` and retain its concrete run ID.
2. Select a relevant area and run `lumin findings --run <run-id>`.
3. Inspect chosen IDs with `lumin explain --run <run-id>`.
4. Request related evidence only when needed.

Skills do not list internal artifact filenames or duplicate rule catalogs. If command syntax is needed, the agent asks the installed binary:

```text
lumin help-agent
lumin <command> --help
```

The binary owns current command syntax. Skill text owns workflow and interpretation discipline.

## 6. Projections

Projections are explicit:

```text
lumin export sarif --run <run-id>
lumin export review-pack --run <run-id> --area structure
lumin export markdown --run <run-id> --finding <id>
lumin export legacy --run <run-id> --artifact symbols
```

Projection rules:

- all values come from canonical evidence;
- every run-derived projection names one immutable run and cannot follow a moving latest pointer;
- projection limits do not alter canonical totals;
- omitted data is counted and identified;
- legacy exports are compatibility products with an explicit retirement status;
- projection failures do not mutate canonical evidence;
- CI policy decides whether SARIF levels block a merge; SARIF generation alone is not a gate.

## 7. Write-Gate Command Contract

### 7.1 Open

An agent opens a gate with repeated typed flags:

```text
lumin pre-write \
  --operation-id op_... \
  --include 'src/**' \
  --exclude 'src/legacy/**' \
  --role-at 'test/**' test \
  --entry src/main.ts \
  --resolution-profile bundler \
  --path src/api.ts \
  --path src/App.vue \
  --symbol-at src/api.ts createUser \
  --dependency-at src/api.ts zod
```

For a large path set:

```text
<NUL-delimited paths> | lumin pre-write --operation-id op_... --paths0-from -
```

The caller creates and retains a repository-scoped `OperationId` before invoking a mutating lifecycle command. Reusing it with different canonical input is malformed and cannot mutate state. With the same request digest, a retry returns a committed result immediately, joins a still-live execution without starting another mutation, or re-acquires and re-executes an operation proven interrupted before any gate-lifecycle or durable-path-lease mutation. Provisional reservation and operation-attempt records may already exist; they are recovery metadata, not authorization. No arbitrary timeout converts a live execution into an interrupted one.

A caller-declared path outside the canonical root is malformed request input: exit `2`, no operation record, no gate ID, and no lease. Valid pre-write then uses one protected handoff:

1. The controller quiesces its participating editors for the declared domain and known semantic inputs.
2. One lifecycle-store transaction binds the operation ID/request digest, checks the current catalog, and creates a short-lived provisional reservation for the normalized leased-write and candidate semantic-read sets. This reservation blocks conflicting compliant opens but is not an `Active` gate lease and does not authorize edits.
3. Inventory captures the exact declared/leased observation domain, candidate semantic reads, source set, configuration, content identities, and catalog revision.
4. Owners analyze only those bytes and return complete cold or validated-cache `CachedCapabilityOutput` values, including `ConsultedSemanticInputs`, facts, signals, and limitations. The engine unions every reported input with the candidate set.
5. If one complete iteration adds no input, closure has reached its fixed point. If the set grows, one lifecycle-store transaction checks every added read against active/provisional writes. A conflict, unbounded input, or unobservable required input stops the closure branch with typed `Incomplete` evidence and records the attempted domain, last complete read set when one exists, and conflicting or unbounded inputs; none is omitted to force convergence. When all additions are admissible, the same transaction extends the reservation, inventory captures their exact identities, and every affected owner reruns. Steps 4-5 repeat.
6. A successful closure seals the semantic-read set and derives `ObservationBinding::Sealed(Baseline(GateBaselineObservationId))`. Inventory then rediscovers and rehashes the complete sealed path/identity sets; drift yields a sealed `Stale` result. A failed closure derives `ObservationBinding::Unsealed` from the typed failure data and creates no baseline observation ID or fabricated partial hash.
7. One final lifecycle-store transaction rechecks the operation digest, reservation, catalog and transition revisions, and every applicable conflict; a sealed branch also rechecks the sealed reads. It maps typed signals through the canonical effect policy, allocates the gate ID, and atomically commits either `Active` plus its durable path lease and sealed read set or queryable `Rejected` without a lease. Only `Allow` or `AllowWithWarnings` with a sealed binding may become `Active`. The provisional reservation is removed in the same transaction.
8. The controller may resume editors after delivery succeeds or, if delivery fails, after it recovers the committed operation result. The returned decision carries the exact `ObservationBinding` accepted in step 7.

If the process dies before the final transaction while a provisional reservation exists, process-lease recovery marks that execution attempt interrupted and removes the reservation. The same operation ID may then execute again because no gate-lifecycle or durable-path-lease mutation committed. A hard-stop before the final transaction produces no gate decision. If the final transaction committed, the gate result remains authoritative even when the process dies before delivery; the caller recovers it by operation ID.

The agent does not create an intent JSON file.

### 7.2 Intent Semantics

Required input is the planned write set. Optional enrichments are:

- symbols or names being created, moved, or changed;
- dependencies expected to be added or newly consumed;
- capability-owned type escapes explicitly allowed by the change when that capability is available;
- refactor source locations;
- a short human-readable label.

Omitted optional lanes mean no exception was planned. Agents do not send empty arrays or zero declarations.

Typed analysis inputs such as the scan include/exclude/role tier, explicit entries, and a supported resolution-profile override are baseline parameters rather than natural-language intent. Pre-write stores the normalized caller-supplied override tier, its configuration sources, and the resulting effective values in the operation digest, `AnalysisInputId`, and semantic-read closure. Post-write reuses the caller override tier and rejects replacement flags as malformed. Effective values derived from a repository config are recomputed only when that config change is this gate's self-writable delta; external config drift remains stale.

An optional lane exists only when its canonical capability owner is registered in the active product slice. Requesting an unavailable lane returns `Incomplete`; the engine and language crates do not implement temporary substitutes.

Symbol and dependency intent is path-scoped. The path identifies the consuming source or package context; Lumin resolves its nearest owner manifest. A dependency addition adds the inferred owner manifest and lockfile to the leased write set. NUL-delimited input preserves legal newline-containing paths.

Lumin infers from planned paths:

- language and framework lanes;
- nearest workspace and package owners;
- dependency owner manifests;
- scan scope;
- affected source neighborhoods available from the current index.

Natural-language interpretation remains the coding agent's responsibility. Lumin receives compact typed intent.

### 7.3 Gate Identity

Every gate decision carries one closed binding:

```text
ObservationBinding =
  Sealed(Baseline(GateBaselineObservationId) | Close(GateCloseObservationId))
  | Unsealed {
      reason,
      attempted_domain,
      last_complete_read_set,
      conflicting_or_unbounded_inputs
    }
```

`last_complete_read_set` is optional and never presented as the complete observation. `Allow` and `AllowWithWarnings` require `Sealed`; `Deny`, `Incomplete`, and `Stale` may carry `Sealed` when a complete observation exists or `Unsealed` when closure/freshness could not establish one. No partial domain receives an observation ID.

A gate or rejected gate attempt always records:

- gate ID and lifecycle schema;
- opening operation ID and canonical request digest;
- canonical repository root and repository identity;
- base VCS revision when available;
- opening Lumin build identity;
- `AnalysisContractId`;
- the immutable normalized caller-supplied invocation override tier and its configuration sources;
- lifecycle state and monotonic revision;
- normalized declared write set;
- normalized candidate leased-write set;
- internally partitioned language lanes;
- baseline `ObservationBinding` and opening gate-catalog revision;
- available baseline or attempted-domain findings needed by the declared intent;
- advisory decision and evidence.

A sealed opening additionally records its opening `AnalysisInputId`, sealed semantic-read set, source-set and content fingerprints, and `GateBaselineObservationId`. Only such an opening may become `Active` and promote its candidate leased-write set to an exclusive durable path lease. An unsealed `Rejected` record has no authoritative opening `AnalysisInputId`, sealed read set, baseline fingerprint set, observation ID, or durable lease; its binding stores the attempted domain, optional last complete read set, and conflicting, unbounded, or unobservable inputs instead.

Every close revision records its `ObservationBinding`. A sealed close also records the current `AnalysisInputId`, exact protected close-read set, current fingerprints, and own/intervening transition chain. An unsealed close records no current `AnalysisInputId` or complete current fingerprint set, retains the prior active revision's sealed read protection, and stores only its typed attempted-domain data. No conditional field is populated with a partial value to satisfy storage shape.

The sets have distinct meanings:

- `declared_write_set`: paths or directory scopes the caller says it will change;
- `leased_write_set`: normalized existing/new paths plus inferred manifest and lockfile writes that no other active gate may read or write;
- `semantic_read_set`: the fixed-point set of manifests, lockfiles, tsconfig/workspace configuration, explicit/public-entry metadata, and affected source facts actually consulted by owners; each stored revision names the exact sealed set it protects.

Read/read overlap is allowed. Write/write and write/read overlap with another active gate are admission conflicts.

Mixed JS, SFC, and Rust work remains one user transaction. The engine fans it into language-owned task lanes and joins the result before returning the advisory.

### 7.4 Close

After edits:

```text
lumin post-write <gate-id> --operation-id op_...
```

Post-write reloads the exact active transaction. The agent does not resend intent, baseline, paths, or an advisory filename. On operation admission, the operation ID/request digest binds to the gate ID and then-current active revision. Retry returns that operation's same committed close-attempt revision. After a nonauthorizing close increments the gate revision, a later close attempt requires a new operation ID bound to the new current revision; two operation IDs cannot mutate the same gate revision concurrently.

Close-out does not compare the opening `AnalysisInputId` to a current whole-value ID for equality. It verifies:

- repository and `AnalysisContractId` compatibility;
- exact compatibility of the caller-supplied opening override tier (profile, entries, and scan flags), protected opening reads outside this gate's own write delta, and a sealed branch's current close `AnalysisInputId` whose effective config values and source-set changes are explained only by this gate or reconciled terminal transitions;
- planned and actual changed paths;
- unexpected new, removed, or modified source files;
- symbol and other capability-owned deltas available in the active slice, including shape or escape evidence only when their owners are registered;
- dependency ownership and manifest deltas;
- capability regressions and newly opaque evidence;
- generated-artifact effects within declared scope.

At close, opening semantic reads have two classes. A path is self-writable only when it belongs to this gate's leased-write set and exact preliminary/final actual-write set. Its changed bytes are recaptured, owner analysis and semantic-read closure rerun, and any config-derived effective profile, entry, or scan value is recomputed under the unchanged caller override tier. The change participates in the current `AnalysisInputId` and lifecycle delta; it is not stale merely because the path was also read. Every other opening semantic read remains protected at its exact identity. Another active gate cannot write a self-writable path because admission and final conflict checks compare leases and reads.

Post-write recomputes the actual write set after reconciling immutable intervening transitions, checks the remaining delta against every other active gate's leased and semantic-read sets, and revalidates both classes. External or unexplained drift of a protected read yields `Stale`; an unexplained or unauthorized transition yields `Deny`; a changed path still owned by an active gate has no terminal transition to reconcile and yields `Incomplete`. None authorizes close-out.

Close uses one exact observation protocol:

1. The controller quiesces its participating editors, then captures the current source/config path sets, exact identities, source snapshots, opening semantic reads, and transition-catalog revision.
2. Reconcile every post-baseline terminal transition under Section 8.1. A transition touching a protected opening semantic read outside this gate's leased and actual write sets yields `Stale`; a changed path covered only by another active gate yields `Incomplete`; an identity mismatch or unexplained path yields `Deny`.
3. Remove only exactly chained, disjoint terminal transitions from the raw baseline/current diff. The remainder is the preliminary actual-write set.
4. Analyze the captured bytes and derived affected facts. Every owner returns `ConsultedSemanticInputs`; the engine unions them with the opening reads to form the candidate close read set.
5. If one complete iteration adds no input, closure has reached its fixed point. If the set grows, one lifecycle-store transaction checks each added read against active/provisional writes. A still-active writer, unbounded input, or unobservable required input stops the closure branch with typed `Incomplete` evidence and an unsealed attempted-domain record. When all additions are admissible, the transaction establishes an operation-scoped semantic-read-extension reservation, inventory captures the added exact identities, refreshes the transition catalog and preliminary delta, and reruns every affected owner. Terminal transitions completed before the refreshed capture are analyzed at their exact current identities; a later transition changes the catalog and fails final validation. Steps 4-5 repeat.
6. A successful closure seals the close semantic-read set, recomputes the exact actual-write set, derives the current close `AnalysisInputId`, and creates `ObservationBinding::Sealed(Close(GateCloseObservationId A))` from those sets, their content identities, and the accepted transition/catalog revision. A failed closure creates `ObservationBinding::Unsealed` with no A and retains the prior protected read set.
7. Only a sealed branch rediscovers the complete path sets and rehashes their exact inputs. Any difference from A yields a sealed `Stale` result.
8. In the close transaction, recheck the operation digest, gate revision/lifecycle, transition-catalog revision, every other active gate or reservation, and reconciliation chain; a sealed branch also rechecks its exact read set and A, while an unsealed branch rechecks its attempted-domain conflict identities without pretending they form a complete observation.
9. Persist the immutable operation result, `ObservationBinding`, and close-attempt revision. A sealed revision records its current `AnalysisInputId`; an unsealed revision records none. A nonauthorizing attempt with a conflict-free sealed read set updates the active gate's protected semantic reads for its new revision; an unsealed attempt leaves the prior protected set. Only `Allow` or `AllowWithWarnings` with a sealed binding appends the terminal worktree transition, closes the gate, and releases its durable logical path lease atomically. Every operation-scoped extension reservation is removed in this transaction.

The controller may resume participating editors after delivery succeeds or, if delivery fails, after it recovers the committed operation result. An authorizing result is bound to the sealed returned `GateCloseObservationId`; a nonauthorizing result returns its sealed or typed unsealed binding. Neither is a claim that an unlocked worktree can never change after the final observation. A later edit requires a new gate transaction.

If the close process dies before its final transaction, liveness recovery removes only that operation's semantic-read-extension reservation and leaves the gate at its prior durable revision/read set. The same operation ID may retry because no close revision committed. A death after final commit preserves the committed revision and is recovered through `lumin operation show`.

Actual delta derives from the baseline and current inventory identity maps. A rename is canonical only when one baseline path and one current path share the same unique persistent filesystem identity; otherwise even identical content is reported conservatively as remove plus add. VCS status may accelerate candidate discovery but is never the truth owner. Both rename endpoints require leases in either representation.

Only `Allow` and `AllowWithWarnings` commit the terminal close result, worktree transition, and logical lease release in one store transaction. `Deny`, `Incomplete`, and `Stale` append an immutable close-attempt revision and keep the gate active until a later successful close or explicit abandon; a conflict-free sealed close read set becomes that active revision's protected semantic-read set. Result transport occurs after storage transaction locks, scan locks, and operation-liveness leases are released. The durable logical path lease and protected semantic reads of an `Active` gate remain repository state until a later validated revision, close, or abandon.

## 8. Concurrent Agents and Path Leases

Path leases are logical transaction records, not OS file locks held by a long-running process.

Path identity follows the observed root filesystem:

- existing paths resolve every symlink or junction prefix, remain inside the canonical root, and record repository spelling plus each existing prefix's platform file identity and observed comparison behavior;
- a new path resolves its nearest existing parent and compares each unresolved component under that parent's observed case behavior, refreshing policy as parents are created;
- physical identity wins for existing aliases; root-wide case policy is only a fallback when a parent-specific observation is unavailable, and Linux byte-distinct names are never collapsed by generic Unicode normalization;
- directory leases conflict with descendants, and a rename requires both source and destination leases;
- an alias that reaches the same existing file conflicts even when its spelling differs.

On `pre-write`:

1. Normalize the declared, leased, and semantic-read sets.
2. Compare the new leased set with every active leased and semantic-read set.
3. Compare the new semantic-read set with every active leased set.
4. Reject conflicts with the gate IDs, paths, and read/write relationship.
5. Protect capture with the operation-scoped provisional reservation from Section 7.1.
6. Promote the reservation to an `Active` gate lease only with the exact accepted baseline; a rejected or interrupted operation releases it.

Steps 2-5 repeat whenever semantic-read closure discovers another input. A newly discovered read that intersects an active or provisional write yields `Incomplete`; it is never omitted from the observation to preserve apparent concurrency.

Directory declarations expand to their observed source paths at open time and retain a directory-level lease for new-file detection.

Parallel agents with nonconflicting write/read sets may proceed. Their closes are serializable through the transition ledger below, not assumed independent merely because analysis overlapped. Workers in one coordinated wave should share one gate. An abandoned gate requires an explicit command:

```text
lumin gate abandon <gate-id> --operation-id op_... --reason "..."
```

No age-based cleanup may silently release an active write contract.

Abandon validates the operation digest and exact active gate revision, then commits `Abandoned`, the reason, lease release, and operation result in one lifecycle-store transaction. A retry returns that terminal revision; a different operation against an already terminal gate cannot create another lifecycle revision.

### 8.1 Shared-Worktree Transition Ledger

Lumin does not claim which OS process or editor wrote a byte. It proves whether the observed worktree state is covered by an authorizing gate transition.

Every authorizing close appends one immutable, monotonically sequenced `WorktreeTransition` containing the gate/revision, baseline and close observation IDs, leased writes, sealed close semantic reads, and exact before/after identities. Every opening baseline records the current transition sequence.

At close, changes after that sequence are partitioned as follows:

- this gate's declared/leased transition is analyzed as its actual delta;
- another gate's terminal transition may be reconciled only when its exact before/after identity chain reaches the current bytes and its paths are disjoint from this gate's leased writes and sealed opening semantic reads;
- a terminal transition intersecting a sealed opening semantic read makes the baseline `Stale`;
- a path first discovered as a close-time semantic read may consume a terminal transition only by recapturing and analyzing that transition's exact current identity before the close read set is sealed; a later transition changes the catalog and invalidates the observation;
- a changed path or candidate semantic read covered by another still-`Active` gate has no terminal identity to reconcile and makes close `Incomplete` with an attribution-pending finding;
- a missing transition, broken identity chain, or other unexplained changed path is an unplanned-transition signal and makes close `Deny`.

The store serializes terminal transitions and close revisions. Thus disjoint gates may analyze concurrently, but a close that observes another in-flight edit waits through an `Incomplete` retry until that edit becomes a terminal transition. If a different process produced bytes later authorized by another gate, Lumin reports only that the final state transition was authorized; it never fabricates process provenance.

## 9. Gate Queries

```text
lumin gate show <gate-id> [--revision <revision>]
lumin gate findings <gate-id> --revision <revision> [--cursor <cursor>]
lumin gate explain <gate-id> --revision <revision> <finding-id> [--evidence-cursor <cursor>] [--relations-cursor <cursor>]
lumin gate list --active [--cursor <cursor>]
lumin operation show <operation-id>
lumin gate abandon <gate-id> --operation-id <operation-id> --reason <text>
lumin gate prune plan --terminal-before <timestamp> --operation-id <operation-id>
lumin gate prune plan show <plan-id> [--cursor <cursor>]
lumin gate prune confirm <plan-id> --operation-id <operation-id>
```

`lumin post-write` always requires an explicit gate ID. The CLI never infers or auto-selects a transaction.

`lumin operation show` returns the canonical command kind, request digest, mutation status, target IDs/revisions, committed result, and last delivery status. It is the recovery path for every gate or retention lifecycle mutation when a caller retained its operation ID but did not receive stdout. Delivery attempts may append transport metadata; they never create another lifecycle revision, plan, pin change, or deletion.

Pre-write, post-write, gate abandon, run pin/unpin, prune-plan creation, and prune confirmation all require an operation ID before they mutate durable state. The same operation state machine applies to each command: identical ID plus digest joins live work, retries only a proven pre-commit interruption, or returns the one committed result; conflicting reuse is malformed. Read-only list/show/page commands do not require an operation ID.

### 9.1 Decision and Exit Contract

| Decision or failure | Meaning | Exit |
| --- | --- | --- |
| `Allow` | The requested lifecycle step is authorized. | `0` |
| `AllowWithWarnings` | Authorized with queryable cautions. | `0` |
| malformed invocation or request | No valid operation was started. | `2` |
| typed query `NeverExisted` | The lookup completed and no live record or retained tombstone ever had that ID. | `2` |
| `Deny` | Checked evidence rejects the requested step. | `3` |
| `Incomplete` | Required evidence could not complete; no clean claim is possible. | `4` |
| `Stale` | The baseline or current-worktree relationship is invalid. | `5` |
| internal, persistence, or pre-commit encoding hard-stop | No trustworthy result was committed. | `1` |
| result-delivery failure after commit | A trustworthy result exists in the store but was not delivered; recover it by operation ID. | `1` |

Ordinary audit findings are data and do not make a successful audit process fail. Skills read the decision field from `--format json`; exit codes remain stable for shells and controllers.

### 9.2 Gate Effects and Lifecycle

Gate policy never infers severity from display text. For lifecycle comparison, capability owners first emit model-owned `GateDeltaClassification` from normalized baseline/current facts: introduced, unchanged, expanded, resolved, or baseline unavailable. Static limitation registries own scope and absence impact, not lifecycle effect. Complete introduced/expanded adverse facts, unchanged advisory facts, and unavailable required comparison then become distinct typed `GateSignal` values; resolved facts emit no adverse signal. Pre-write may emit advisory signals from complete existing facts or incompleteness when required evidence is unavailable, but it does not invent a post-write delta. The engine emits only named transaction-invariant signals from typed inventory/store outcomes. The closed, versioned `lumin-evidence::gate_policy` table maps signals to effects:

| `GateEffect` | Meaning |
| --- | --- |
| `Stale` | The observation or baseline no longer describes the transaction being decided. |
| `Block` | Grounded current evidence violates a required gate invariant. |
| `Incomplete` | Required owner evidence did not complete, so authorization cannot be proven. |
| `Warn` | Grounded nonblocking caution that remains queryable. |

The engine cannot construct or choose `GateEffect`; it invokes the policy mapping, preserves every mapped effect, and reduces only by `Stale > Block > Incomplete > Warn > none`, producing `Stale`, `Deny`, `Incomplete`, `AllowWithWarnings`, or `Allow`. Internal/persistence/pre-commit hard-stops are not effects and produce no valid decision. Effect-policy versions participate in `AnalysisContractId`; signal facts and projections cannot mute or reclassify them.

Gate lifecycle is a separate closed state machine:

| Operation result | Lifecycle transition | Lease |
| --- | --- | --- |
| valid pre-write `Allow` or `AllowWithWarnings` | new record -> `Active` | acquired atomically with the accepted baseline |
| valid pre-write `Deny`, `Incomplete`, or `Stale` | new record -> `Rejected` | never acquired |
| active post-write `Allow` or `AllowWithWarnings` | `Active` -> `Closed` | released with the terminal close revision |
| active post-write `Deny`, `Incomplete`, or `Stale` | remains `Active` | retained; immutable close-attempt revision appended |
| explicit abandon | `Active` -> `Abandoned` | released with reason |

`Rejected`, `Closed`, and `Abandoned` are terminal. Only `Active` gates accept post-write. An operation-scoped provisional reservation is not a gate lifecycle state and cannot authorize edits. Final baseline validation, signal mapping/reduction, gate allocation, lifecycle transition, durable lease mutation, and provisional-reservation removal commit in one final lifecycle-store transaction; a rejected pre-write cannot block another agent afterward.

The lifecycle reducer also enforces the observation invariant: authorizing transitions require `ObservationBinding::Sealed`; nonauthorizing transitions persist whichever sealed or typed unsealed binding was actually established. It cannot synthesize an observation ID to satisfy a storage or DTO field.

## 10. Gate Performance Model

Pre-write is not a disguised full audit.

It:

1. opens the exact-input index;
2. refreshes repository inventory needed for new-file and source-set detection;
3. reparses planned and affected files whose identities changed;
4. queries only intent lanes owned by capabilities available in the active slice and marks every other requested lane unavailable;
5. persists the baseline transaction;
6. returns a bounded advisory.

Post-write:

1. identifies actual source deltas against that baseline;
2. reparses changed files and affected graph neighborhoods;
3. computes deltas only for available capability owners and preserves unavailable lanes;
4. persists and returns the close-out decision.

A caller may explicitly request a broader audit, but the write gate does not silently launch one. Cold and warm timings are reported separately. Warm reuse replays and validates the full ARCH-001 `CachedCapabilityOutput`; it cannot omit consulted reads, signals, or limitations, and cold/warm execution over one exact observation must reach the same gate binding and decision.

When no compatible current index exists, pre-write rebuilds repository inventory plus only the planned/affected capability facts required by available owners. Any repository-wide absence lane that cannot be grounded by that focused rebuild is `Unverifiable` or `Incomplete`; it is never inferred from a missing cache and never triggers a hidden full audit.

## 11. Durability and Retention

Run retention is owned by the canonical `lumin runs` command:

```text
lumin runs list [--cursor <cursor>]
lumin runs pin <run-id> --operation-id <operation-id> --reason <text>
lumin runs unpin <pin-id> --operation-id <operation-id>
lumin runs prune plan --before <timestamp> --operation-id <operation-id>
lumin runs prune plan show <plan-id> [--cursor <cursor>]
lumin runs prune confirm <plan-id> --operation-id <operation-id>
```

Pin allocates and returns one repository-scoped `PinId`. Unpin accepts that exact pin ID, not merely the run ID. Each independent review/CI consumer therefore owns one reference, and a run becomes unpinned only after its last live pin is explicitly removed. Pin and unpin validate the exact run/reference plus operation digest and commit the change with the operation result in one lifecycle-store transaction. Delivery failure is recovered through `lumin operation show`; it never leaves an ambiguous second pin mutation or lets one consumer remove another's protection.

Plan creation allocates a model-owned `RetentionPlanId` and persists one immutable `Prepared` plan in the same transaction; the ID is scoped to the repository and collision-checked by `lumin-store`. It deletes nothing. A run plan contains the exact attempt, completed-run, orphan-payload, every independent pin/reference, byte count, exclusions, repository catalog revision, and content identity; a gate plan contains the exact terminal gate, revision, finding/evidence, and operation-record closure. The content identity derives from the canonically ordered logical plan payload under the lifecycle schema, never backend row/page order. The plan plus its creation/confirmation operation records are not members of their own deletable closure; they become the minimal retained tombstone. `plan show` pages the immutable retention-plan scope and never creates a replacement plan from repeated filters. Unrelated repository mutations do not invalidate paging; confirmation separately revalidates the current catalog. Gate retention uses the `lumin gate prune` commands above; no second cleanup owner exists.

Confirmation accepts only the exact plan ID plus a new operation ID. Before deletion begins, one lifecycle-store transaction revalidates pin/lifecycle/catalog/latest state and every record identity. A changed input leaves the plan `Prepared` and returns `Stale`. Pinned or active records are never eligible. The current `latestAttempt` target and `latestCompleted` target are never eligible, and each retains its linked attempt/run closure. The plan reports every exclusion reason.

Deletion is a crash-consistent state machine:

1. The successful confirmation transaction changes the plan and exact record closure to `Pruning(planId)`, stores expected canonical and same-filesystem trash identities, and binds the confirmation operation result-in-progress. Those records are no longer ordinary query results, but their tombstones remain inspectable.
2. Run, attempt, and orphan filesystem payloads are atomically renamed into `.lumin/trash/<plan-id>/` and required source/trash parent directories are durably flushed. Backend-resident gate/evidence rows move into a logical trash namespace through transactional tombstones; physical page reclamation is not canonical deletion truth.
3. After every planned payload is owned by trash, one lifecycle-store transaction removes canonical indexes and referential links, marks the records and plan `Pruned`, and commits the immutable confirmation operation result. Minimal tombstones retain plan, record identities, hashes, sequence, and completion state.
4. Trash files and unreachable backend pages are reclaimed idempotently after logical commit. A crash or cleanup failure here cannot make a pruned record queryable again and remains visible as pending physical reclamation.

Every retention crash point has one outcome:

| Crash point | Canonical recovery |
| --- | --- |
| before the `Prepared` plan commit | no plan exists; the same operation ID may retry |
| after `Prepared` and before confirmation commits `Pruning` | the immutable plan remains pageable and confirmable; no payload moved |
| after `Pruning` and before the first payload move | recovery resumes the same plan; records remain typed `Pruning` tombstones |
| during payload or logical-trash moves | exactly one validated canonical or trash identity must exist for each item; recovery resumes the remaining moves, while both-or-neither is an integrity hard-stop |
| after all moves and before the `Pruned` transaction | recovery validates trash ownership and completes the one catalog transaction |
| after `Pruned` and before or during physical reclamation | logical deletion is complete; recovery only resumes idempotent trash/page reclamation |

A retry with the confirmation operation ID joins or resumes this state machine and returns the same final result. Retention never rolls a `Pruning` record back into ordinary evidence and never interprets a missing payload as successful deletion.

Retention mutation output is separate from `GateDecision`:

```text
RetentionMutationResult =
  Prepared { plan_id, content_identity }
  | Pruning { plan_id, recoverable_state }
  | Pruned { plan_id, tombstone_identity, physical_reclamation_pending }
  | Stale { plan_id, changed_inputs }
```

Successful plan creation, resumable `Pruning`, and logical `Pruned` results exit `0`; callers inspect the typed status. `Stale` exits `5`, malformed/cross-repository input exits `2`, and integrity/persistence hard-stops exit `1`. `plan show`, `operation show`, and direct known-record lookup project the same canonical status. A payload query for a `Pruning` or `Pruned` target returns the Section 4 tombstone envelope rather than an empty collection or plain not-found. The public crash corpus checks these projections at every fault point.

- Completed runs and gates are immutable.
- `latest.json` contains separate `latestAttempt` (attempt ID/sequence/status) and `latestCompleted` (run ID/originating sequence) pointers, not copied evidence.
- Active gates survive process exit.
- Operation records remain linked to their gate/revision or retention plan/result for idempotent retry and delivery recovery for at least as long as that referential closure.
- Cache has an independent cleanup policy.
- Retention commands report exactly which immutable run, attempt, orphan, gate, evidence, and operation records will be removed.
- A durable finding referenced by a review or CI result is addressable by run and finding ID.
- No user workflow requires manual deletion of generated intent transport.

Architecture v1 retains minimal plan/record/operation tombstones for the repository lifetime. They are not eligible for second-order pruning in the first slice; their count and bytes are measured separately in retention probes. Any future compaction requires an explicit lifecycle-schema and retention-contract amendment rather than silently erasing deletion history.

Attempt sequences are allocated atomically in the catalog; a sequence becomes a started attempt only when its `Running` envelope is durable. Concurrent completion cannot move either pointer backward: `latestAttempt` identifies the highest started sequence and its success/failure status, while `latestCompleted` identifies the highest sequence that published a complete run. `overview` shows a newer failed attempt before presenting an older completed run.

If a process disappears before terminal publication, later store recovery follows the exact crash table in Section 2.3. In particular, a renamed run directory without a terminal success attempt remains an unpointed orphan beside an `Interrupted` attempt and is never adopted as success. Process leases are operation-liveness records, not scan locks, durable gate path leases, or result-transport locks.

Completed run directories are never migrated in place. Compatible old run schemas are read through versioned adapters or a disposable derived index. Whole `lifecycle.store` migration uses an exclusive repository store lock, copy-on-write replacement, and rollback-safe validation while preserving attempt/catalog sequences, operation IDs/results, worktree transitions, retention plans/tombstones, pins/references, gate IDs/revisions, and history. Unsupported schemas are reported as incompatible, not corrupt or empty.

## 12. Security and Integrity

- Repository roots and planned paths are canonicalized before storage.
- A caller-declared path that resolves outside the root is malformed input and creates no operation or gate record.
- Any repository-owned configuration field declared as a repository path that lexically or physically resolves outside the root is malformed configuration and cannot publish a completed run or authorizing gate; a root-contained missing/excluded entry is typed incomplete evidence instead.
- Unsupported external configuration semantics are scoped incomplete evidence and never authorize an undeclared source/config read outside the root.
- If an admitted existing path's alias/symlink identity later resolves outside the root, baseline identity drift is `Stale`; if a planned new/final path is observed outside the root at close, the containment-invariant signal is `Block`/`Deny`.
- Each resolved existing prefix's comparison behavior and physical identity, plus the fallback root policy, are persisted with each gate.
- Store writes use validated typed operations; raw backend queries do not cross `lumin-store`.
- Store locks live under the repository-owned `.lumin` directory, not a shared global temp path.
- Published run envelopes contain evidence-store hashes.
- Run publication and latest-pointer replacement follow the crash-consistent, durable, sequence-checked protocol in Section 2.3.
- A gate cannot close under a different repository identity.
- An operation ID is repository-scoped and bound to one canonical request digest; conflicting reuse is malformed.
- Incompatible lifecycle schemas fail closed with a concise recovery instruction.

## 13. Acceptance Criteria

1. A complete default audit creates only the repository lifecycle store, small attempt/run envelopes, latest pointer, and canonical evidence store.
2. An agent can answer a focused finding question without opening either file directly.
3. Every bounded response supports explicit continuation.
4. Projection limits cannot change canonical counts.
5. A failed required capability is prominent in `overview`.
6. Pre-write and post-write require no intent JSON or temporary transport file.
7. Post-write needs only the explicit gate ID, a caller-retained operation ID, and repository context; it never resends intent or baseline.
8. Active write/write and write/read conflicts are rejected atomically.
9. Transactions with nonconflicting leased-write and semantic-read sets can analyze concurrently; closes serialize through exact intervening transitions, and an in-flight unexplained edit cannot be approved.
10. Mixed-language changes remain one user-visible gate with language-owned internal lanes.
11. A stale or incompatible gate cannot be interpreted as passed.
12. Dependency checks use the nearest owner manifest for the planned paths.
13. Completed gate evidence remains queryable after process exit.
14. No storage transaction lock, scan lock, or operation-liveness lease is held while stdout or a result projection is transported; an active gate's durable logical path lease remains.
15. Architecture v1 selects exactly one store backend only after the correctness probes and measured comparison pass.
16. `latestAttempt` exposes a newer failure while `latestCompleted` preserves the last complete run.
17. Post-write cannot run without an explicit gate ID.
18. Existing aliases, directory descendants, new paths, and both sides of a rename obey the path identity contract.
19. Gate decisions, machine output, and process exit codes follow the stable decision table.
20. Nested evidence and relation lists cannot bypass bounded query envelopes.
21. Store migration cannot rewrite completed logical evidence in place or erase active gate history.
22. Every run query is pinned to one immutable run, and every nested page can be requested explicitly without following latest.
23. `AnalysisContractId` compatibility cannot be invalidated merely by a different `AnalysisInputId`.
24. Pre-write rejection owns no lease; failed post-write remains active with an immutable attempted revision.
25. Post-write drift during analysis cannot authorize or release a gate.
26. Every publication crash point recovers according to Section 2.3 without a dangling pointer becoming clean evidence.
27. Retrying any mutating gate or retention lifecycle command with the same operation ID/request returns the same committed result; conflicting reuse is malformed.
28. Shared-worktree changes are approved only when this gate or an exact immutable intervening transition explains every observed identity change.
29. Retention cannot delete either latest-pointer target or break its attempt/run linkage.
30. Open and close observations are derived only after owner-reported semantic inputs reach a fixed point; every added read is reserved, captured, and conflict-checked before authorization.
31. Gate abandon, run pin/unpin, prune-plan creation, and prune confirmation recover committed results by operation ID after delivery failure.
32. Every retention deletion crash point recovers to exactly one `Prepared`, `Pruning`, or `Pruned` truth and never exposes missing payload as clean deletion.
33. Validated warm cache replay preserves the same owner-reported semantic inputs, signals, effects, and observation binding as cold execution.
34. A nonauthorizing closure failure persists typed `Unsealed` evidence and never invents a baseline or close observation ID.
35. Every public collection has one versioned canonical ordering, and an immutable retention-plan cursor survives unrelated repository mutations without crossing plan identity.
36. Known `Pruning` and `Pruned` records remain publicly distinguishable from never-existing IDs through plan, operation, and direct-record lookup.
37. Post-write reuses the exact caller-supplied opening scan/entry/profile override tier, rejects replacement parameters, and recomputes config-derived effective values only from validated self-writable inputs.
38. Lifecycle effects consume owner-produced introduced/unchanged/expanded/resolved/baseline-unavailable deltas rather than static limitation rows.
39. Independent `PinId` references protect one another, minimal tombstones remain auditable, and lifecycle-store migration preserves the complete logical catalog.
