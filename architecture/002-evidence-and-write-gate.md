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
  gates.store
  attempts/
    <attempt-id>/
      attempt.json
  runs/
    <run-id>/
      run.json
      evidence.store
  cache/
    ... disposable exact-input cache ...
```

Users and agents do not edit these files directly.

### 2.1 `attempt.json`

At audit start, the store allocates an attempt ID/sequence, records a logical process lease, and advances `latestAttempt` to `Running` without holding a store lock for the scan. A terminal audit publishes one immutable attempt envelope containing repository/build/request identity, timestamps, success or failure class, concise diagnostics, and optional completed run ID. A hard-stop or unstable snapshot therefore remains addressable even when no run exists.

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
| before attempt sequence allocation | no attempt exists |
| after `Running` attempt/process-lease publication and before run-directory rename | publish `Interrupted`; no run exists |
| after valid run-directory rename and before terminal attempt publication | preserve the directory as an unpointed orphan, publish `Interrupted`, and never adopt that orphan as a successful run |
| after terminal attempt publication and before latest-pointer replacement | the terminal attempt and linked run are authoritative; recovery advances only non-regressing pointers |
| during latest temporary write or atomic replacement | accept only the complete old or complete new pointer document; partial JSON is noncanonical |

The durable process lease identifies the process/operation that may finish a `Running` attempt. Recovery authority begins only after platform liveness checks prove that lease released. An orphan without a terminal success attempt is inspectable recovery evidence and retention input, not a completed run or an automatic success candidate.

### 2.4 `gates.store`

The repository-wide gate store contains operation-id records, provisional admission reservations, declared intents, logical path leases, baseline fingerprints and facts, advisory findings, immutable worktree transitions, close-out deltas, and lifecycle history for every gate in that repository.

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

`redb`'s first probe is two independent writer processes contending for the same gate store. If open/lock/retry behavior cannot preserve atomic lease admission without a daemon or a second truth owner, the candidate is rejected before performance comparison.

Correctness probes inject process death at every row of the crash table and every retention pointer step. They cover unadoptable orphan runs, dangling pointers, corrupt hashes, stale-writer sequence regression, interrupted gate migration, and unsupported durable-flush behavior on each required platform. Probe evidence preserves source and fixture hashes, toolchain/target, exact commands, expected invariants, crash point, and raw result under `reviews/probes/<probe-id>/`; build output is removed, but reproducibility evidence is not.

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
- cursors are opaque and bound to protocol schema, immutable scope identity or gate revision, normalized filters, collection path, ordering, page-size policy, and last semantic key;
- stable default ordering is severity, confidence, rule, path, span, and finding ID;
- a current-worktree absence query reports `SnapshotStatus`; drift or unverifiable freshness cannot render a clean claim;
- an unavailable capability is returned as unavailable, never as an empty item set;
- every nested collection, including evidence and relations inside one `explain` result, uses the same bounded page contract;
- stdout is bounded; exhaustive export is an explicit command.

Run collections use `{"kind":"run","id":"run_..."}` scope. Gate collections use `{"kind":"gate-attempt","gateId":"gate_...","revision":7}` scope. Repository catalogs use `{"kind":"repository","id":"repo_...","revision":42}`; a mutation makes an unresumable old view `Stale` rather than silently continuing against new rows. Current-binary capability collections use `{"kind":"binary","buildId":"..."}`, while run capabilities use run scope. Every gate advisory or close attempt increments the revision and persists an immutable revision record. A cursor for an older active-gate revision remains valid only against that revision; it cannot silently advance to newer gate evidence. Invalid, cross-scope, or tampered cursors fail as malformed requests rather than restarting at page one.

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
  --path src/api.ts \
  --path src/App.vue \
  --symbol-at src/api.ts createUser \
  --dependency-at src/api.ts zod
```

For a large path set:

```text
<NUL-delimited paths> | lumin pre-write --operation-id op_... --paths0-from -
```

The caller creates and retains a repository-scoped `OperationId` before invoking a mutating gate command. Repeating the same operation ID with the same canonical request digest returns its existing committed result. Reusing it with different input is malformed and cannot mutate state.

A caller-declared path outside the canonical root is malformed request input: exit `2`, no operation record, no gate ID, and no lease. Valid pre-write then uses one protected handoff:

1. The controller quiesces its participating editors for the declared domain and semantic inputs.
2. One gate-store transaction binds the operation ID/request digest, checks the current catalog, and creates a short-lived provisional reservation for the normalized leased-write and semantic-read sets. This reservation blocks conflicting compliant opens but is not an `Active` gate lease and does not authorize edits.
3. Inventory captures the exact declared/leased observation domain, semantic reads, source set, configuration, content identities, and catalog revision as a candidate `GateBaselineObservationId`.
4. Owners analyze only those bytes, then inventory rediscovers and rehashes the same sets. Drift yields `Stale`; an unobservable required input yields `Incomplete`.
5. One final gate-store transaction rechecks the operation digest, reservation, catalog revision, and conflicts; maps typed signals through the canonical effect policy; allocates the gate ID; and atomically commits either `Active` plus its durable path lease or `Rejected` without a lease. The provisional reservation is removed in the same transaction.
6. The controller may resume editors after delivery succeeds or, if delivery fails, after it recovers the committed operation result. The returned decision names the exact `GateBaselineObservationId` accepted in step 5.

If the process dies while a provisional reservation exists, process-lease recovery marks the operation interrupted and removes the reservation; it never promotes an unreturned operation to an active gate. A hard-stop before the final transaction produces no gate decision. A delivery failure after commit does not undo or invalidate the durable result; the caller recovers it by operation ID.

The agent does not create an intent JSON file.

### 7.2 Intent Semantics

Required input is the planned write set. Optional enrichments are:

- symbols or names being created, moved, or changed;
- dependencies expected to be added or newly consumed;
- capability-owned type escapes explicitly allowed by the change when that capability is available;
- refactor source locations;
- a short human-readable label.

Omitted optional lanes mean no exception was planned. Agents do not send empty arrays or zero declarations.

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

A gate records:

- gate ID and lifecycle schema;
- opening operation ID and canonical request digest;
- canonical repository root and repository identity;
- base VCS revision when available;
- opening Lumin build identity;
- `AnalysisContractId`;
- opening `AnalysisInputId`;
- lifecycle state and monotonic revision;
- normalized declared write set;
- exclusive leased write set;
- semantic read set;
- internally partitioned language lanes;
- baseline source-set and content fingerprints;
- baseline `GateBaselineObservationId` and opening gate-catalog revision;
- baseline findings needed by the declared intent;
- advisory decision and evidence.

The sets have distinct meanings:

- `declared_write_set`: paths or directory scopes the caller says it will change;
- `leased_write_set`: normalized existing/new paths plus inferred manifest and lockfile writes that no other active gate may read or write;
- `semantic_read_set`: manifests, lockfiles, tsconfig/workspace configuration, public-entry metadata, and affected source facts whose change invalidates the baseline.

Read/read overlap is allowed. Write/write and write/read overlap with another active gate are admission conflicts.

Mixed JS, SFC, and Rust work remains one user transaction. The engine fans it into language-owned task lanes and joins the result before returning the advisory.

### 7.4 Close

After edits:

```text
lumin post-write <gate-id> --operation-id op_...
```

Post-write reloads the exact active transaction. The agent does not resend intent, baseline, paths, or an advisory filename. The operation ID is bound to the gate ID and opening revision; retry returns the same committed close-attempt revision.

Close-out verifies:

- repository and `AnalysisContractId` compatibility;
- baseline `AnalysisInputId` and current gate-observation freshness;
- planned and actual changed paths;
- unexpected new, removed, or modified source files;
- symbol and other capability-owned deltas available in the active slice, including shape or escape evidence only when their owners are registered;
- dependency ownership and manifest deltas;
- capability regressions and newly opaque evidence;
- generated-artifact effects within declared scope.

Post-write also recomputes the actual write set after reconciling immutable intervening transitions, checks the remaining delta against every other active gate's leased and semantic-read sets, and revalidates this gate's semantic read set. A changed semantic input yields `Stale`; an unexplained or unauthorized transition yields `Deny`; a changed path still owned by an active gate has no terminal transition to reconcile and yields `Incomplete`. None authorizes close-out.

Close uses one exact observation protocol:

1. The controller quiesces its participating editors, then captures the current source/config path sets, exact identities, source snapshots, and transition-catalog revision.
2. Reconcile every post-baseline terminal transition under Section 8.1. A transition touching this gate's semantic reads yields `Stale`; a changed path covered only by another active gate yields `Incomplete`; an identity mismatch or unexplained path yields `Deny`.
3. Remove only exactly chained, disjoint terminal transitions from the raw baseline/current diff. The remainder is this gate's actual-write set and, with its semantic reads and catalog revision, forms `GateCloseObservationId A`.
4. Analyze only A's captured bytes and derived affected facts.
5. Rediscover the same path sets and rehash their exact inputs. Any difference from A yields `Stale`; an unobservable required input yields `Incomplete`.
6. In the close transaction, recheck the operation digest, gate revision/lifecycle, transition-catalog revision, every other active gate, and the reconciliation chain.
7. Persist the immutable operation result and close-attempt revision and, only for an authorizing decision, append the terminal worktree transition, close the gate, and release its durable logical path lease atomically.

The controller may resume participating editors after delivery succeeds or, if delivery fails, after it recovers the committed operation result. An authorizing result is bound to the returned `GateCloseObservationId`; it is not a claim that an unlocked worktree can never change after the final observation. A later edit requires a new gate transaction.

Actual delta derives from the baseline and current inventory identity maps. A rename is canonical only when one baseline path and one current path share the same unique persistent filesystem identity; otherwise even identical content is reported conservatively as remove plus add. VCS status may accelerate candidate discovery but is never the truth owner. Both rename endpoints require leases in either representation.

Only `Allow` and `AllowWithWarnings` commit the terminal close result, worktree transition, and logical lease release in one store transaction. `Deny`, `Incomplete`, and `Stale` append an immutable close-attempt revision but keep the gate active until a later successful close or explicit abandon. Result transport occurs after storage transaction locks, scan locks, and operation-liveness leases are released. The durable logical path lease of an `Active` gate remains repository state until close or abandon.

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

Directory declarations expand to their observed source paths at open time and retain a directory-level lease for new-file detection.

Parallel agents with nonconflicting write/read sets may proceed. Their closes are serializable through the transition ledger below, not assumed independent merely because analysis overlapped. Workers in one coordinated wave should share one gate. An abandoned gate requires an explicit command:

```text
lumin gate abandon <gate-id> --reason "..."
```

No age-based cleanup may silently release an active write contract.

### 8.1 Shared-Worktree Transition Ledger

Lumin does not claim which OS process or editor wrote a byte. It proves whether the observed worktree state is covered by an authorizing gate transition.

Every authorizing close appends one immutable, monotonically sequenced `WorktreeTransition` containing the gate/revision, baseline and close observation IDs, leased paths, and exact before/after identities. Every opening baseline records the current transition sequence.

At close, changes after that sequence are partitioned as follows:

- this gate's declared/leased transition is analyzed as its actual delta;
- another gate's terminal transition may be reconciled only when its exact before/after identity chain reaches the current bytes and its paths are disjoint from this gate's leased writes and semantic reads;
- a terminal transition intersecting this gate's semantic reads makes the baseline `Stale`;
- a changed path covered by another still-`Active` gate has no terminal identity to reconcile and makes close `Incomplete` with an attribution-pending finding;
- a missing transition, broken identity chain, or other unexplained changed path is an unplanned-transition signal and makes close `Deny`.

The store serializes terminal transitions and close revisions. Thus disjoint gates may analyze concurrently, but a close that observes another in-flight edit waits through an `Incomplete` retry until that edit becomes a terminal transition. If a different process produced bytes later authorized by another gate, Lumin reports only that the final state transition was authorized; it never fabricates process provenance.

## 9. Gate Queries

```text
lumin gate show <gate-id> [--revision <revision>]
lumin gate findings <gate-id> --revision <revision> [--cursor <cursor>]
lumin gate explain <gate-id> --revision <revision> <finding-id> [--evidence-cursor <cursor>] [--relations-cursor <cursor>]
lumin gate list --active [--cursor <cursor>]
lumin gate operation show <operation-id>
lumin gate abandon <gate-id> --reason <text>
lumin gate prune --terminal-before <timestamp> [--cursor <cursor>]
lumin gate prune --confirm <plan-id>
```

`lumin post-write` always requires an explicit gate ID. The CLI never infers or auto-selects a transaction.

`lumin gate operation show` returns the canonical request digest, mutation status, gate/revision, decision, and last delivery status. It is the recovery path when a caller retained its operation ID but did not receive stdout. Delivery attempts may append transport metadata; they never create another lifecycle revision.

### 9.1 Decision and Exit Contract

| Decision or failure | Meaning | Exit |
| --- | --- | --- |
| `Allow` | The requested lifecycle step is authorized. | `0` |
| `AllowWithWarnings` | Authorized with queryable cautions. | `0` |
| malformed invocation or request | No valid operation was started. | `2` |
| `Deny` | Checked evidence rejects the requested step. | `3` |
| `Incomplete` | Required evidence could not complete; no clean claim is possible. | `4` |
| `Stale` | The baseline or current-worktree relationship is invalid. | `5` |
| internal, persistence, or pre-commit encoding hard-stop | No trustworthy result was committed. | `1` |
| result-delivery failure after commit | A trustworthy result exists in the store but was not delivered; recover it by operation ID. | `1` |

Ordinary audit findings are data and do not make a successful audit process fail. Skills read the decision field from `--format json`; exit codes remain stable for shells and controllers.

### 9.2 Gate Effects and Lifecycle

Gate policy never infers severity from display text. Capability owners emit model-owned typed `GateSignal` values from their facts; the engine emits only named transaction-invariant signals from typed inventory/store outcomes. The closed, versioned `lumin-evidence::gate_policy` table maps signals to effects:

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

`Rejected`, `Closed`, and `Abandoned` are terminal. Only `Active` gates accept post-write. An operation-scoped provisional reservation is not a gate lifecycle state and cannot authorize edits. Final baseline validation, signal mapping/reduction, gate allocation, lifecycle transition, durable lease mutation, and provisional-reservation removal commit in one final gate-store transaction; a rejected pre-write cannot block another agent afterward.

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

A caller may explicitly request a broader audit, but the write gate does not silently launch one. Cold and warm timings are reported separately.

When no compatible current index exists, pre-write rebuilds repository inventory plus only the planned/affected capability facts required by available owners. Any repository-wide absence lane that cannot be grounded by that focused rebuild is `Unverifiable` or `Incomplete`; it is never inferred from a missing cache and never triggers a hidden full audit.

## 11. Durability and Retention

Run retention is owned by the canonical `lumin runs` command:

```text
lumin runs list [--cursor <cursor>]
lumin runs pin <run-id> --reason <text>
lumin runs unpin <run-id>
lumin runs prune --before <timestamp> [--cursor <cursor>]
lumin runs prune --confirm <plan-id>
```

The first prune form creates an immutable, paged plan containing exact run/gate IDs, bytes, exclusions, repository catalog revision, and `planId`; it deletes nothing. Confirmation accepts only that plan ID, revalidates pin/lifecycle/catalog and latest-pointer state, fails stale on change, and then deletes exactly the planned eligible records. Pinned or active records are never eligible. The current `latestAttempt` target and the current `latestCompleted` target are also never eligible, and each protected pointer target retains its linked attempt/run records as a referential closure. The plan reports these exact exclusion reasons. Gate retention uses the `lumin gate prune` subcommand above; no second cleanup owner exists.

- Completed runs and gates are immutable.
- `latest.json` contains separate `latestAttempt` (attempt ID/sequence/status) and `latestCompleted` (run ID/originating sequence) pointers, not copied evidence.
- Active gates survive process exit.
- Operation records remain linked to their gate/revision for idempotent retry and delivery recovery for at least as long as that gate record.
- Cache has an independent cleanup policy.
- Retention commands report exactly which immutable run or gate records will be removed.
- A durable finding referenced by a review or CI result is addressable by run and finding ID.
- No user workflow requires manual deletion of generated intent transport.

Attempt sequences are allocated atomically at start. Concurrent completion cannot move either pointer backward: `latestAttempt` identifies the highest started sequence and its success/failure status, while `latestCompleted` identifies the highest sequence that published a complete run. `overview` shows a newer failed attempt before presenting an older completed run.

If a process disappears before terminal publication, later store recovery follows the exact crash table in Section 2.3. In particular, a renamed run directory without a terminal success attempt remains an unpointed orphan beside an `Interrupted` attempt and is never adopted as success. Process leases are operation-liveness records, not scan locks, durable gate path leases, or result-transport locks.

Completed run directories are never migrated in place. Compatible old run schemas are read through versioned adapters or a disposable derived index. Gate-store migration uses an exclusive repository store lock, copy-on-write replacement, and rollback-safe validation while preserving logical gate IDs and history. Unsupported schemas are reported as incompatible, not corrupt or empty.

## 12. Security and Integrity

- Repository roots and planned paths are canonicalized before storage.
- A caller-declared path that resolves outside the root is malformed input and creates no operation or gate record.
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

1. A complete default audit creates only the small attempt/run envelopes, latest pointer, and canonical evidence store.
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
27. Retrying a mutating command with the same operation ID/request returns the same gate or close revision; conflicting reuse is malformed.
28. Shared-worktree changes are approved only when this gate or an exact immutable intervening transition explains every observed identity change.
29. Retention cannot delete either latest-pointer target or break its attempt/run linkage.
