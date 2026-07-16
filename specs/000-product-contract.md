# PRODUCT-000: Lumin v2 Product Contract

Document role: product source of truth

Status: draft

Revision: 2026-07-16

Scope: final Lumin v2 product, independent of implementation phase

## 0. One-Line Definition

Lumin gives AI coding agents grounded repository evidence, a safe transactional write gate, and explicit uncertainty without requiring users or agents to read an artifact warehouse.

## 1. Problem

The legacy product grew bottom-up across Node producers, Rust helpers, runtime bridges, generated source mirrors, and platform binaries. A single semantic change can cross all of those owners. Large JSON artifacts duplicate counts and statuses, normal resolution misses can abort unrelated analysis, and runtime fallback can hide incompatible or stale helpers.

Lumin v2 exists to preserve the product identity while replacing that ownership model.

## 2. Product Contract

### 2.1 Identity

Lumin remains:

- a Codex skill;
- a Claude Code skill;
- a native repository-audit CLI;
- a pre-write/post-write safety gate;
- an evidence source for AI judgment, not a substitute for judgment.

The skills are product surfaces. They must not contain a second implementation of analysis semantics.

### 2.2 Core Workflows

Lumin must support four workflows:

1. Audit a repository and persist a versioned run.
2. Query bounded evidence relevant to a user question.
3. Open a pre-write transaction for planned changes.
4. Validate and close that exact transaction after changes.

Users and agents must not have to construct, retain, or delete request JSON files for these workflows.

### 2.3 Evidence Honesty

Every absence claim must identify:

- the run and scan scope;
- the capability that owns the claim;
- whether that capability completed;
- relevant opaque or unsupported surfaces;
- whether the evidence was validated against the current worktree;
- whether the returned result was truncated.

Missing, stale, degraded, or failed evidence must never be rendered as zero findings.

### 2.4 Failure Semantics

Expected repository facts are data, not process failures. Examples include unresolved imports, external packages, non-source assets, generated virtual modules, unsupported framework syntax, and parse failures isolated to individual files.

Lumin hard-stops only when continuing would make the run contract dishonest, including:

- malformed or unsupported request schemas;
- a declared repository path escaping its root;
- corrupt canonical evidence storage;
- an impossible internal invariant;
- a required capability failing without an artifact-visible incomplete result.

Fallback must never silently change evidence ownership or semantics.

### 2.5 Distribution

Supported users must run Lumin without installing Cargo, Node analysis dependencies, or native parser bindings.

The product ships verified prebuilt binaries for its declared platform matrix. A missing or incompatible required binary is a visible hard failure, not a request to compile during an audit.

### 2.6 Determinism

The same repository snapshot, configuration, and Lumin version must produce the same canonical semantic findings and evidence identities regardless of worker count or task completion order. Runtime metrics, publication metadata, and physical store layout are not semantic evidence.

### 2.7 AI Consumption

The default interaction is evidence pull, not artifact push. An agent starts from a small overview, retains its concrete run ID, requests findings pinned to that run, and drills into selected finding IDs. Every bounded response reports scope, total, returned count, truncation state, and continuation cursor.

### 2.8 Write Gate

Pre-write opens a durable transaction and returns one gate ID. Post-write requires that ID and compares against the same baseline. The agent must not resend the intent or locate invocation-specific files.

Concurrent transactions may proceed only when their exclusive write leases do not overlap and no transaction writes another active gate's semantic inputs. Mixed-language work is one user transaction with internally owned language lanes.

An authorizing baseline or close observation includes every exact semantic input actually consumed by its capability owners. Discovery is two-phase: an owner first returns a path-level demand without reading that input, Lumin conflict-checks and reserves it, inventory captures its exact identity/bytes, and only then may the owner consume it and report an exact consulted identity. Cache validation follows the same reservation-before-consumption order. Lumin reruns affected analysis until no demand remains and cannot authorize from a partial read set.

Cache reuse cannot change this safety contract. Cached demand metadata is keyed only by exact prerequisites and semantic owner-task/profile parameters already supplied and cannot reveal a downstream demand derived from uncaptured bytes. A warm execution replays the owner-authored outcome state, facts or opaque/failure payload, diagnostics, limitations, gate-neutral signals, and consulted semantic inputs together after every demanded identity is reserved and captured; otherwise it performs grounded work again or reports incomplete evidence. Request-specific gate signals are recomputed by the owning capability from the validated outcome and current typed `GateProjectionContext`. Cold and warm execution of the same exact inputs must produce the same capability state and canonical semantic dump, not merely the same decision.

In one shared worktree, Lumin authorizes observable repository state transitions, not unverifiable operating-system process authorship. A gate may analyze concurrently, but close-out reconciles every intervening terminal gate transition in store order. An unexplained change or a still-active intervening write cannot be approved as this gate's delta. Retention cannot remove an exact transition capsule while any active gate may still need it for reconciliation.

Every gate result has one decision: `Allow`, `AllowWithWarnings`, `Deny`, `Incomplete`, or `Stale`. Only the first two authorize the requested lifecycle step. Machine-readable output and process exit behavior are stable product contracts.

A nonauthorizing pre-write creates a queryable rejected record but no active lease. A nonauthorizing post-write appends an attempted revision and leaves the existing gate active. Authorization is bound to the exact final worktree/config observation returned with the decision. A result that could not seal an observation returns a typed unsealed binding with its attempted domain and blocking inputs; it never fabricates a partial observation ID.

Every user-facing command that mutates gate or retention lifecycle state carries a caller-retained operation ID. This includes gate open/close/abandon and durable retention plan, pin, unpin, and confirmation mutations. Retrying the same operation ID and request returns the same committed result instead of duplicating state; reusing it for different input is malformed. A result-delivery failure does not erase an already committed result, which remains recoverable by operation ID.

Retention is a public, crash-consistent lifecycle operation. It cannot expose a record as deleted before its canonical indexes and payload ownership agree, cannot remove a protected latest/pinned/active-transition reference closure, and has one recoverable outcome at every deletion boundary. A known record in `Pruning` or `Pruned` state remains distinguishable from a never-existing ID through public queries, and independent pins cannot remove one another's protection. Lifecycle-store migration admits mutations in exactly one generation: backend handles are transaction-scoped, replacement is generation-fenced, and every migration crash boundary has one recovery rule.

## 3. Non-Goals

Lumin v2 does not:

- ask an embedded language model to interpret arbitrary natural-language change requests;
- make unsupported evidence look complete;
- preserve legacy internal file layouts as architecture;
- run two production analysis engines and choose between them at runtime;
- create one crate per type, policy, or single-use helper;
- require agents to read every raw finding or generated projection;
- claim that Rust or parallelism alone fixes semantic false positives;
- make every analysis parallel when a deterministic single-owner reduction is clearer.

## 4. Product Acceptance Criteria

1. The default audit path is one native process and contains no Node analysis stage.
2. Windows and Linux users can execute supported releases without Cargo.
3. `jobs=1` and `jobs=N` produce identical canonical evidence for the same snapshot.
4. A required capability failure is visible in the overview and cannot be interpreted as a clean result.
5. Agents can complete audit, finding inspection, pre-write, and post-write without creating JSON files.
6. A completed gate can be inspected by gate ID after the creating process exits and a new process opens the repository store.
7. Query truncation is explicit, resumable, and pinned to one immutable scope or gate revision; every collection, including current-binary and run-scoped capabilities, exposes its continuation surface.
8. Framework-specific misses cannot abort unrelated language or repository analysis.
9. A public re-export protects only the exported identity, not every sibling export in the same file.
10. Legacy JSON and SARIF are optional projections from canonical evidence, not independent truth owners.
11. Codex and Claude Code skills invoke the same native product contract.
12. Every accepted slice includes real corpus fixtures, platform verification, and measured performance evidence.
13. The latest failed attempt cannot be hidden behind an older completed run.
14. Post-write cannot infer or auto-select a gate ID.
15. A write that invalidates another gate's semantic baseline, cannot be reconciled to an immutable intervening gate transition, or changes the final close observation is rejected, incomplete, or visibly stale before approval; lifecycle policy uses a total owner-defined relation over identity, targets, affected domain, confidence, grounding, and evidence before assigning an effect.
16. Retrying any committed gate or retention lifecycle mutation by operation ID returns the same durable result and never creates a duplicate revision, plan, pin change, or deletion.
17. An authorizing gate result is derived only after every newly demanded input is conflict-checked and reserved before capture/consumption, cold and warm paths produce the same complete owner outcome and semantic inputs, no demand remains, and the sealed read set passes final conflict and freshness validation; a nonauthorizing unsealed result names no fabricated observation ID.
18. Public retention commands and lookups preserve one crash-recoverable, queryable state at every deletion boundary and cannot break latest, independent pin, active-gate transition, operation, attempt, run, gate, or lifecycle-generation referential integrity.

## 5. Verification Contract

Each active vertical-slice specification must map every acceptance criterion it claims to:

- a behavior test;
- a corpus case when repository semantics are involved;
- a verification command;
- an artifact or query result that proves completion.

Architecture review may mark a criterion not yet implemented, but runtime output may not mark it complete until those proofs exist.
