# PRODUCT-000: Lumin v2 Product Contract

Document role: product source of truth

Status: reopened for the narrow REVIEW-004 follow-up

Revision: 2026-08-24

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

Grounded findings remain canonical evidence regardless of source role, framework convention, or remediation disposition. Policy may attach a stable `ReviewOnly` reason, but it cannot create a hidden `Muted` or `Suppressed` truth class, remove the finding from canonical counts or an unfiltered default finding query, alter its finding identity, or erase its gate signal. Only an explicit caller-supplied query filter may narrow a collection, and the normalized filter remains visible in that query's scope and continuation identity.

### 2.4 Failure Semantics

Expected repository facts are data, not process failures. Examples include unresolved imports, external packages, non-source assets, generated virtual modules, unsupported framework syntax, and parse failures isolated to individual files.

Lumin hard-stops only when continuing would make the run contract dishonest, including:

- malformed or unsupported request schemas;
- a declared repository path escaping its root;
- a caller targeting the reserved state namespace, or a foreign/redirected/mismatched `.lumin` namespace;
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

The default interaction is evidence pull, not artifact push. An agent starts from a small overview, retains its concrete run ID, requests findings pinned to that run, and drills into selected finding IDs. Every bounded response reports scope, normalized filters, the unfiltered scope total, matched total, returned count, truncation state, and continuation cursor.

### 2.8 Write Gate

Pre-write opens a durable transaction and returns one gate ID. Post-write requires that ID and compares against the same baseline. The agent must not resend the intent or locate invocation-specific files.

Concurrent transactions may proceed only when their exclusive write leases do not overlap and no transaction writes another active gate's semantic inputs. Mixed-language work is one user transaction with internally owned language lanes.

An authorizing baseline or close observation includes every exact semantic input actually consumed by its capability owners. Discovery is two-phase: an owner first returns a path-level demand without reading that input, Lumin conflict-checks and reserves it, inventory captures its exact identity/bytes, and only then may the owner consume it and report an exact consulted identity. Cache validation follows the same reservation-before-consumption order. Lumin reruns affected analysis until no demand remains and cannot authorize from a partial read set.

Cache reuse cannot change this safety contract. Cached demand metadata is keyed only by exact prerequisites and semantic owner-task/profile parameters already supplied and cannot reveal a downstream demand derived from uncaptured bytes. A warm execution replays the owner-authored outcome state, facts or opaque/failure payload, diagnostics, limitations, gate-neutral signals, and consulted semantic inputs together after every demanded identity is reserved and captured; otherwise it performs grounded work again or reports incomplete evidence. Request-specific gate signals are recomputed by the owning capability from the validated outcome and current typed `GateProjectionContext`. Cold and warm execution of the same exact inputs must produce the same capability state and canonical semantic dump, not merely the same decision.

In one shared worktree, Lumin authorizes observable repository state transitions, not unverifiable operating-system process authorship. A gate may analyze concurrently, but close-out reconciles every intervening terminal gate transition in store order. An unexplained change or a still-active intervening write cannot be approved as this gate's delta. Retention cannot remove an exact transition capsule while any active gate may still need it for reconciliation.

Every gate result has one decision: `Allow`, `AllowWithWarnings`, `Deny`, `Incomplete`, or `Stale`. Only the first two authorize the requested lifecycle step. Machine-readable output and process exit behavior are stable product contracts.

A nonauthorizing pre-write creates a queryable rejected record but no active lease. A nonauthorizing post-write appends an attempted revision and leaves the existing gate active. Authorization is bound to the exact final worktree/config observation returned with the decision. A result that could not seal an observation returns a typed unsealed binding with its attempted domain and blocking inputs; it never fabricates a partial observation ID.

Every user-facing command that mutates gate, retention, or cache-cleanup state carries a caller-retained operation ID. This includes gate open/close/abandon, durable retention plan/pin/unpin/confirmation mutations, and active-cache eviction. Retrying the same operation ID and request returns the same committed result instead of duplicating state; reusing it for different input is malformed. A result-delivery failure does not erase an already committed result, which remains recoverable by operation ID.

Retention is a public, crash-consistent lifecycle operation. It cannot expose a record as deleted before its canonical indexes and payload ownership agree, cannot remove a protected latest/pinned/active-transition reference closure, and has one recoverable outcome at every deletion boundary. A known record in `Pruning` or `Pruned` state remains distinguishable from a never-existing ID through public queries, and independent pins cannot remove one another's protection. Lifecycle-store migration admits mutations in exactly one generation: backend handles are transaction-scoped, source/target exchange is generation-fenced, and every migration crash boundary has one recovery rule. An incompatible but explicitly supported prior lifecycle-store schema is upgraded only through the public `lumin store migrate` command; absent state is rejected without initialization. Ordinary repository-state commands fail closed with that recovery route and never decode private old-schema user records on open. Intent preparation uses a handle-owned automatically disposed object, so death before no-replace publication cannot leave a named pending or partial intent. Canonical v12 commits an append-only authorization for the exact revision-zero object before that root may be published; a self-consistent file cannot authorize itself. Every published successor is a distinct immutable object in one contiguous append-only journal and no update replaces or removes its predecessor. Typed binding events distinguish pending target publication, visible publication, a proven superseded-unpublished identity, exchange, immutable retired source, and the canonical mutable target, so a missing historical pending target never masquerades as a missing current artifact. The journal binds the canonical source and private target by exact pre/post-exchange names, byte/logical and physical identities, and publication-time one-link state. Migration exchanges those objects with Linux `renameat2(RENAME_EXCHANGE)` or Windows handle-relative `SetFileInformationByHandle(FileRenameInfo, ReplaceIfExists = false)` moves, never physically disposes a published artifact, and retains the exact v12 source plus terminal journal as permanent provenance. The v13 target's complete migration-time payload is authenticated through terminalization; subsequent normal mutations are admitted by immutable provenance/header facts plus current-v13 integrity, never by comparison with that historical payload hash. The product does not claim that an advisory lock can prevent a noncooperating same-UID hard-link operation; instead, an observed substitution or extra link fails closed while every object remains. An unbound, substituted, or no-journal object is foreign and no command may adopt or delete it. A successful migration command is safely repeatable after process or output-delivery failure and reports only the fully validated current target schema; packaged Codex and Claude adapters recover only through that public command and DTO.

### 2.9 Path and State Integrity

Repository path identity is lossless and byte-complete under one exact checked-in codec artifact. Lumin does not require a native path to be printable Unicode and never uses escaped display text, lossy conversion, Unicode normalization, or backend collation as a logical source ID, ordering key, cursor anchor, cache key, or gate lease. Native command input, `RepoPathDto`, and `RepositoryRootDto` round-trip the same canonical path/root bytes through required canonical Base64 and reject noncanonical encodings or disagreeing readable projections. A logical source is a lexical module context; physical-file identity may establish aliases/conflicts and payload snapshots may reuse bytes, but neither may merge package ownership, controlling configuration, scan role, resolution, findings, or query identity across two logical paths. Declaring a write to one existing alias expands the effective lease and reanalysis domain to every admitted logical alias of that physical object; topology changes outside the leased endpoints cannot authorize close.

`.lumin` and every physical alias/descendant are a product-owned reserved state namespace, not authored repository content. Lumin creates one immutable lifecycle-lock object, one immutable anchor for each top-level managed `attempts`, `runs`, `trash`, and `cache` parent, and one separately bound nested quarantine parent/anchor at `trash/cache-evictions`. The repository marker and lifecycle-store header bind the state-directory/lock identities, namespace nonce, the exact kind/directory/anchor/parent-nonce tuple for every top-level parent, and the nested quarantine binding. Every ordinary shared/exclusive acquisition and state mutation proves those directory entries still name the bound objects; pre-marker bootstrap authority cannot publish ordinary state success. Cache and authenticated quarantine payloads are disposable, but their bound parents and anchors are not silently replaceable. Foreign, copied, redirected, replaced, multiply linked, mismatched, or externally mutated state fails closed; a caller cannot scan or lease that namespace as a planned write.

The public cache-cleanup operation may evict only disposable payload descendants from the active cache. Before any move, one durable store-owned operation record binds the caller's operation ID and request digest to the ordered initial quarantine authorization-set identity/count, deterministic active-cache manifest, source/destination names, physical tree identities, and per-entry `Authorized` state. A quarantine child is admissible only through exactly one matching authorization row; a well-formed self-hashed foreign tree is still reserved-state corruption, and repeated operations reference rather than copy historical rows. Cleanup flushes every regular file and descendant directory bottom-up and remanifests before authorization, atomically moves without replacement, then repeats the tree flush/remanifest and durably flushes the cache, quarantine, and owning trash directories before marking that entry `Validated`. A same-operation retry may resume an exact authorized source or validate an exact moved destination; another operation ID cannot adopt interrupted state. Success commits one recoverable operation result only after every entry is validated, the active cache is anchor-only, the authenticated quarantine is exact, all affected parents are durable, and the complete namespace proof passes. Cleanup never physically deletes a quarantined object. A payload or nested-child substitution is preserved and fails closed; physical reclamation remains outside this command and requires final disposition bound to the opened object or an enforceable isolation boundary. Process death and delivery failure recover through the same operation ID and `operation show`, never by treating a visible directory entry or unkeyed digest as authorization.

Latest pointer publication is one cross-process serialized compare/merge/replace operation. Concurrent attempts merge `latestAttempt` by `(sequence, Running < Terminal)` and `latestCompleted` by successful originating sequence, while retention confirmation uses the same guard; atomic file replacement alone is not treated as lost-update protection.

### 2.10 Resolver Configuration Honesty

Every configuration field or nested shape that can affect supported source/workspace ownership or resolution is owned by exactly one checked-in inventory or resolver registry artifact and is either modeled, explicitly known neutral with a reviewed reason under pinned pnpm/TypeScript/Node baselines, or reported as owner-scoped unsupported evidence before ownership or target probing. Their field partitions, artifact bytes, and generated-table digests are part of the analysis contract. An unknown, mismatched, or unsupported affecting field cannot silently fall through to package workspaces, a simpler resolver/public surface, or an invocation override and cannot support an absence claim. Profile-specific package-feature applicability is selected before field-shape validation: a disabled feature is recorded but not consulted and emits no affecting limitation, while an enabled unsupported feature fails closed. `tsconfig.extends` lexical dispatch, relative exact/`.json` selection, exact workspace-package identity matching, and unsupported subpath/external handling are artifact-owned before any target demand. Workspace-package config entry selection, declaration-entry precedence, package-target lowering, valid-but-unsupported pnpm value forms, and duplicate package identity are likewise artifact-owned semantics rather than generic package-manager or Node assumptions.

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
7. Query filtering and truncation are explicit, resumable, and pinned to one immutable scope or gate revision; every collection reports its normalized filters and unfiltered scope total, and every collection, including current-binary and run-scoped capabilities, exposes its continuation surface.
8. Framework-specific misses cannot abort unrelated language or repository analysis.
9. A public re-export protects only the exported identity, not every sibling export in the same file; source-role, framework, or remediation classification cannot hide a grounded finding from canonical totals or an unfiltered default finding query.
10. Legacy JSON and SARIF are optional projections from canonical evidence, not independent truth owners.
11. Codex and Claude Code skills invoke the same native product contract.
12. Every accepted slice includes real corpus fixtures, platform verification, and measured performance evidence.
13. The latest failed attempt cannot be hidden behind an older completed run.
14. Post-write cannot infer or auto-select a gate ID.
15. A write that invalidates another gate's semantic baseline, cannot be reconciled to an immutable intervening gate transition, or changes the final close observation is rejected, incomplete, or visibly stale before approval; lifecycle policy uses a total owner-defined relation over identity, targets, affected domain, confidence, grounding, and evidence before assigning an effect.
16. Retrying any committed gate, retention, or cache-cleanup mutation by operation ID returns the same durable result and never creates a duplicate revision, plan, pin change, deletion, authorization row, or cache move.
17. An authorizing gate result is derived only after every newly demanded input is conflict-checked and reserved before capture/consumption, cold and warm paths produce the same complete owner outcome and semantic inputs, no demand remains, and the sealed read set passes final conflict and freshness validation; a nonauthorizing unsealed result names no fabricated observation ID.
18. Public retention commands and lookups preserve one crash-recoverable, queryable state at every deletion boundary and cannot break latest, independent pin, active-gate transition, operation, attempt, run, gate, or lifecycle-generation referential integrity.
19. Concurrent latest publication, recovery, retention confirmation, cache cleanup, and migration serialize through one bound exclusive domain and cannot regress either pointer, lose an independent pointer-field update, strand terminal state behind same-sequence `Running`, publish a pruned target, or duplicate a cache move. Only `lumin store migrate` admits an explicitly supported prior store schema; ordinary repository-state commands never mutate it on open, and absent state is never initialized by migration. Intent publication leaves no crash-surviving pending name; canonical v12 authorizes the exact root before publication, and every successor is a unique immutable revision chained to its retained predecessor. The closed target-binding state machine either authenticates a visible publication or permanently supersedes a proven-dead unpublished binding before allocating another. Source/target replacement is a generation-fenced no-disposition exchange rather than pathname overwrite, and the terminal journal plus retired source remain canonical provenance because supported Linux cannot fence a noncooperating same-UID hard-link creation through deletion. The exact journal-proven canonical-absent intermediate is migration-recoverable; every unexplained missing store or unbound/substituted/no-journal object is foreign. After terminalization, legitimate v13 commits may change the canonical store payload; later admission validates immutable provenance/header facts and the current v13 referential truth rather than the migration-time logical dump. Migration/output recovery returns the same validated target-only response without a second exchange.
20. Every admitted native repository path/root has one byte-complete canonical identity, strict decoder, canonical `RepoPathDto`/`RepositoryRootDto`, and golden machine round trip; distinct Linux byte names and Windows native names cannot merge, and a write to one physical alias expands leases, actual-write attribution, and reanalysis to every admitted logical context without merging them.
21. `.lumin` is a no-follow reserved namespace bound to one repository/root, state-directory, immutable lock-object, namespace nonce, exact four-kind top-level managed-parent binding set, and exact nested cache-quarantine binding; replacement split brain, copied/swapped parents, foreign/redirected state, caller writes, unauthorized quarantine, validation-to-eviction substitution, dirty or unflushed manifested trees, and unverified moved state fail before scan evidence, gate authorization, or cleanup success. Cleanup durably authenticates each move before rename, validates and flushes the complete tree and parent chain before result commit, recovers only through the matching operation record, preserves every bound parent/anchor, and performs no pathname-based physical deletion.
22. Every inventory/workspace- or resolver-affecting configuration field/shape is present under one owner in the exact reviewed registry artifacts; profile-disabled fields are not consulted, enabled unsupported or malformed fields emit the correct semantic-family limitation before ownership or target probing, `extends` dispatch and relative/package selection are exact, workspace config/package entry and target-lowering order is deterministic, valid unsupported pnpm forms reach their named limitation, duplicate package identity fails closed, and artifact/owner-partition/generated-table drift is a contract failure.

## 5. Verification Contract

Each active vertical-slice specification must map every acceptance criterion it claims to:

- a behavior test;
- a corpus case when repository semantics are involved;
- a verification command;
- an artifact or query result that proves completion.

Architecture review may mark a criterion not yet implemented, but runtime output may not mark it complete until those proofs exist.
