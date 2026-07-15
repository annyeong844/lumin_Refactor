# ARCH-002: Evidence Store, Query Protocol, and Write Gate

Document role: evidence delivery and lifecycle architecture owner

Status: draft

Revision: 2026-07-15

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
  runs/
    <run-id>/
      run.json
      evidence.store
  cache/
    ... disposable exact-input cache ...
```

Users and agents do not edit these files directly.

### 2.1 `run.json`

`run.json` is a small, stable run envelope containing:

- run ID, repository identity, Lumin build identity, and envelope schema version;
- publication state;
- the evidence store format, schema, identity, size, and hash;
- enough failure detail to explain why no valid evidence store was published.

Scan scope, capability states, findings, counts, blind zones, metrics, and suggested queries are read from `evidence.store`. The envelope is publication metadata, not a second evidence summary.

### 2.2 `evidence.store`

The immutable run store contains normalized:

- capabilities;
- source identities and spans;
- findings;
- evidence links;
- diagnostics and limitations;
- relationships between findings, symbols, files, and packages;
- metrics;
- projection metadata.

Only `lumin-store` knows the physical schema or backend API. No public product contract exposes SQL, tables, or backend-specific keys.

The engine builds a run in a private temporary location, validates it, closes the writer, and atomically publishes the completed run directory. A failed run may publish a separate failure envelope, but it cannot publish an evidence store marked complete.

### 2.3 `gates.store`

The repository-wide gate store contains declared intents, logical path leases, baseline fingerprints and facts, advisory findings, close-out deltas, and lifecycle history for every gate in that repository.

One transactional store is required so overlap detection and lease creation commit atomically across concurrent Lumin processes. Completed gate records become immutable by application contract. Active gates are not temporary transport records and are never silently removed while open.

### 2.4 Cache

Cache content is disposable and noncanonical. Deleting it may affect performance but cannot change the meaning of a completed run or gate. Cache corruption becomes a visible miss.

### 2.5 Storage Backend Decision Gate

Architecture v1 does not select a persistence engine by familiarity. `lumin-store` first defines a backend-neutral contract for:

- immutable run publication and read-only reopening;
- indexed bounded queries with stable cursors;
- one atomic cross-process gate lease transaction;
- crash recovery, migrations, and corruption-visible failure;
- Windows NTFS, Linux ext4, and Linux musl release operation.

The architecture review benchmarks at least one pure-Rust embedded candidate, initially `redb`, against bundled SQLite. The comparison records clean and incremental build time, release binary size, transitive and unsafe surface, cold and warm store latency, peak memory, store size, multi-process contention behavior, and crash recovery.

`redb`'s first probe is two independent writer processes contending for the same gate store. If open/lock/retry behavior cannot preserve atomic lease admission without a daemon or a second truth owner, the candidate is rejected before performance comparison.

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
lumin overview [--run <id>]
lumin findings [filters] [--cursor <cursor>]
lumin explain <finding-id>
lumin related <finding-id>
lumin files <repo-path>
lumin capabilities
```

Every collection response uses one envelope:

```json
{
  "runId": "run_...",
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
- cursors are opaque and tied to run identity and query ordering;
- stable default ordering is severity, confidence, rule, path, span, and finding ID;
- a query against a changed source span reports fingerprint drift;
- an unavailable capability is returned as unavailable, never as an empty item set;
- stdout is bounded; exhaustive export is an explicit command.

## 5. Agent Consumption

Codex and Claude Code follow the same short workflow:

1. Run `lumin overview`.
2. Select a relevant area and run `lumin findings`.
3. Inspect chosen IDs with `lumin explain`.
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
lumin export sarif
lumin export review-pack --area structure
lumin export markdown --finding <id>
lumin export legacy --artifact symbols
```

Projection rules:

- all values come from canonical evidence;
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
  --path src/api.ts \
  --path src/App.vue \
  --symbol createUser \
  --dependency zod
```

For a large path set:

```text
<newline-delimited paths> | lumin pre-write --paths-from -
```

The binary returns a gate ID, decision, bounded finding summary, and optional next queries.

The agent does not create an intent JSON file.

### 7.2 Intent Semantics

Required input is the planned write set. Optional enrichments are:

- symbols or names being created, moved, or changed;
- dependencies expected to be added or newly consumed;
- type escapes explicitly allowed by the change;
- refactor source locations;
- a short human-readable label.

Omitted optional lanes mean no exception was planned. Agents do not send empty arrays or zero declarations.

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
- canonical repository root and repository identity;
- base VCS revision when available;
- Lumin analysis-contract identity;
- normalized planned write set;
- internally partitioned language lanes;
- baseline source-set and content fingerprints;
- baseline findings needed by the declared intent;
- logical path leases;
- advisory decision and evidence.

Mixed JS, SFC, and Rust work remains one user transaction. The engine fans it into language-owned task lanes and joins the result before returning the advisory.

### 7.4 Close

After edits:

```text
lumin post-write <gate-id>
```

Post-write reloads the exact transaction. The agent does not resend intent, baseline, paths, or an advisory filename.

Close-out verifies:

- repository and analysis-contract compatibility;
- planned and actual changed paths;
- unexpected new, removed, or modified source files;
- symbol and shape deltas relevant to intent;
- dependency ownership and manifest deltas;
- newly introduced escapes;
- capability regressions and newly opaque evidence;
- generated-artifact effects within declared scope.

The close-out result and logical lease release commit in one store transaction. Result transport occurs after storage locks are released.

## 8. Concurrent Agents and Path Leases

Path leases are logical transaction records, not OS file locks held by a long-running process.

On `pre-write`:

1. Normalize the declared write set.
2. Compare it with every active gate in the same repository.
3. Reject an overlapping write set with the conflicting gate IDs and paths.
4. Atomically persist non-overlapping leases and the new baseline in `gates.store`.

Directory declarations expand to their observed source paths at open time and retain a directory-level lease for new-file detection.

Parallel agents with disjoint paths may proceed. Workers in one coordinated wave should share one gate. An abandoned gate requires an explicit command:

```text
lumin gate abandon <gate-id> --reason "..."
```

No age-based cleanup may silently release an active write contract.

## 9. Gate Queries

```text
lumin gate show <gate-id>
lumin gate findings <gate-id>
lumin gate explain <gate-id> <finding-id>
lumin gate list --active
lumin gate abandon <gate-id> --reason <text>
```

If `lumin post-write` is called without an ID, it may proceed only when exactly one compatible active gate exists. Otherwise it fails closed and lists the candidate IDs.

## 10. Gate Performance Model

Pre-write is not a disguised full audit.

It:

1. opens the exact-input index;
2. refreshes repository inventory needed for new-file and source-set detection;
3. reparses planned and affected files whose identities changed;
4. queries reusable names, shapes, dependency ownership, and escape evidence;
5. persists the baseline transaction;
6. returns a bounded advisory.

Post-write:

1. identifies actual source deltas against that baseline;
2. reparses changed files and affected graph neighborhoods;
3. computes intent-specific deltas;
4. persists and returns the close-out decision.

A caller may explicitly request a broader audit, but the write gate does not silently launch one. Cold and warm timings are reported separately.

## 11. Durability and Retention

- Completed runs and gates are immutable.
- `latest.json` is a pointer, not copied evidence.
- Active gates survive process exit.
- Cache has an independent cleanup policy.
- Retention commands report exactly which immutable run or gate records will be removed.
- A durable finding referenced by a review or CI result is addressable by run and finding ID.
- No user workflow requires manual deletion of generated intent transport.

## 12. Security and Integrity

- Repository roots and planned paths are canonicalized before storage.
- A path escaping the declared root is rejected.
- Store writes use validated typed operations; raw backend queries do not cross `lumin-store`.
- Store locks live under the repository-owned `.lumin` directory, not a shared global temp path.
- Published run envelopes contain evidence-store hashes.
- A gate cannot close under a different repository identity.
- Incompatible lifecycle schemas fail closed with a concise recovery instruction.

## 13. Acceptance Criteria

1. A complete default audit creates only the small run envelope and canonical evidence store.
2. An agent can answer a focused finding question without opening either file directly.
3. Every bounded response supports explicit continuation.
4. Projection limits cannot change canonical counts.
5. A failed required capability is prominent in `overview`.
6. Pre-write and post-write require no intent JSON or temporary transport file.
7. Post-write needs only the gate ID and repository context.
8. Overlapping active write sets are rejected atomically.
9. Disjoint transactions can run concurrently and close independently.
10. Mixed-language changes remain one user-visible gate with language-owned internal lanes.
11. A stale or incompatible gate cannot be interpreted as passed.
12. Dependency checks use the nearest owner manifest for the planned paths.
13. Completed gate evidence remains queryable after process exit.
14. No storage or scan lock is held while stdout or a result projection is transported.
15. Architecture v1 selects exactly one store backend only after the correctness probes and measured comparison pass.
