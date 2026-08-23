# ARCH-002: Evidence Store, Query Protocol, and Write Gate

Document role: evidence delivery and lifecycle architecture owner

Status: reopened for the narrow REVIEW-004 follow-up

Revision: 2026-08-24

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
  repository.json
  latest.json
  lifecycle.lock
  lifecycle.store
  lifecycle-migration.json  # immutable root of a v12 migration journal, if one exists
  lifecycle-migration.revision-<sequence>.json
  lifecycle-migration.artifact-<slot>.store  # journal-bound target/retired-source slots
  attempts/
    namespace.anchor         # immutable managed-parent bootstrap
    <attempt-id>/
      attempt.json
  runs/
    namespace.anchor         # immutable managed-parent bootstrap
    <run-id>/
      run.json
      evidence.store
  trash/
    namespace.anchor         # immutable managed-parent bootstrap
    <prune-plan-id>/
      ... noncanonical payloads awaiting idempotent reclamation ...
    cache-evictions/         # immutable nested quarantine binding
      namespace.anchor       # immutable nested-binding bootstrap
      <invocation-id>.<ordinal>.<manifest-sha256>  # authorized detached tree
  cache/
    namespace.anchor         # immutable managed-parent bootstrap
    ... disposable exact-input cache ...
```

Users and agents do not edit these files directly.

### 2.0 Reserved State Namespace

`.lumin` is a product-owned reserved namespace, including every physical alias and descendant. It is never authored source, a scan target, or a legal gate write. The namespace has three global immutable bootstrap values: `StateDirectoryIdentity`, `LifecycleLockIdentity`, and a cryptographically random `StateNamespaceNonce`. It also has the closed top-level managed-parent kinds `Attempts`, `Runs`, `Trash`, and `Cache`. Each kind owns one immutable `ManagedStateParentBinding { kind, directory_physical_identity, anchor_physical_identity, parent_nonce }`; `namespace.anchor` is a create-new/no-follow regular one-link object inside that exact parent. The `Trash` binding additionally owns the literal nested `cache-evictions` child through one immutable `CacheEvictionParentBinding { directory_physical_identity, anchor_physical_identity, parent_nonce }`. That nested directory and its own `namespace.anchor` are canonical namespace bootstrap, not a fifth top-level managed-parent kind or disposable payload. Physical identities include the platform filesystem/volume and stable object identity, while global and parent nonces distinguish reincarnation even if an object identifier is reused. The four top-level parents, five anchors, and nested quarantine directory are never renamed, replaced, migrated, linked, trashed, or retention-eligible. Cache payload descendants and authenticated quarantine payload children are disposable; their bound parents and anchors are not. Repository open performs a no-follow namespace admission before reading or creating canonical state:

1. Open the canonical repository root by stable directory handle and open or create its direct `.lumin` child without following a symlink, junction, mount-style reparse point, or later alias. Capture the state-directory physical identity. The state directory, all four top-level managed parents, and the nested quarantine parent must be real directories on the same state filesystem/volume required by their atomic-rename protocols.
2. Open `repository.json` and `lifecycle.lock` relative to that exact state-directory handle. `lifecycle.lock` is a create-new/no-follow regular object with one link, an immutable global bootstrap header, and a captured `LifecycleLockIdentity`; it is never renamed, replaced, migrated, linked, trashed, or retention-eligible. Its header binds the state schema, `RepositoryId`, canonical/root physical identities, state-directory identity, lock identity, and namespace nonce.
3. For admission inspection, open each exact top-level managed-parent entry relative to the held state-directory handle, then open its `namespace.anchor` relative to the held parent handle. Open the literal `trash/cache-evictions` child and its anchor relative to the held trash and quarantine handles. Require real same-volume directories and regular one-link anchors. Each immutable anchor header binds the global bootstrap values plus its parent kind, held directory physical identity, held anchor physical identity, and random parent nonce; the nested anchor additionally binds its owning `Trash` parent identity and nonce. `repository.json` and the current `lifecycle.store` header bind the same global values, the exact four-kind `ManagedStateParentBinding` set in kind order, and the exact `CacheEvictionParentBinding`; missing, duplicate, or extra bindings are invalid. Close these admission-only parent/anchor handles before lock acquisition; they authorize no read or mutation.
4. Immediately before acquiring either side of `lifecycle.lock`, re-open the root `.lumin` and state-directory lock entries without following and prove that they still name the held global objects. After acquisition, open/re-open every top-level managed-parent, nested quarantine parent, and anchor entry, prove that all entries name the held objects, and validate the marker, anchors, and current store header through those handles. Repeat the complete global/parent/quarantine proof before and after each physical mutation or canonical commit and before lock release. Only the post-acquisition complete proof may create a shared transaction token or exclusive `CatalogPublicationGuard`.
5. After canonical marker/store publication, every attempt publication, run publication/recovery, retention move/reclamation, cache mutation, lifecycle commit, pointer replacement, and migration replacement uses only store-owned APIs carrying the complete validated binding token. Immediately before and after its physical mutation or canonical commit, it repeats the global, four-parent, and nested-quarantine entry-to-held-handle proof. A symlink, junction, reparse point, mount crossing, extra anchor/lock link, parent or anchor replacement, marker/anchor change, missing/extra binding, or identity/nonce disagreement is a store-integrity hard-stop.

Caller input whose lexical path is `.lumin`, descends from it, or physically aliases any verified state object is malformed before operation allocation: exit `2`, no operation/gate/lease. External mutation of canonical state discovered after admission is not an unplanned source edit; it is a typed integrity hard-stop and cannot append a successful gate revision. Inventory always excludes the reserved namespace by lexical and physical identity.

Initial namespace publication has one recovery order: create/open the real `.lumin` directory and capture its identity; create-new the regular one-link `lifecycle.lock`; allocate the namespace nonce, write/flush its global bootstrap header, and capture its identity; acquire its exclusive side and revalidate both global directory entries; create each real top-level managed parent and its regular one-link anchor, allocate the parent nonce, write/flush the anchor header, and capture the directory/anchor identities; create the literal nested quarantine parent and its regular one-link anchor relative to the held trash handle, allocate its nonce, write/flush the nested anchor, and capture both identities; flush the nested quarantine, trash, every other top-level parent, and `.lumin`; publish `repository.json` with the global binding, exact four-parent set, and exact nested quarantine binding by flushed temporary file plus atomic rename and parent flush; then create `lifecycle.store` with the same complete binding. The pre-marker exclusive bootstrap token authorizes only this initialization/recovery sequence and is not a `CatalogPublicationGuard`; no ordinary state success can publish until the full post-acquisition proof can yield that guard. A crash before the marker leaves no canonical repository state; recovery may remove only exact global-, parent-, and nested-quarantine-nonce-bound initialization objects whose headers/identities agree with the interrupted bootstrap and then restart. A crash after the marker leaves that matching complete binding authoritative and resumes the matching store idempotently. Any other pre-marker entry, replacement lock/parent/anchor, copied directory, or post-marker identity/schema/nonce disagreement is foreign state, not an initialization remnant.

Simple lock-file, state-directory, top-level or nested-quarantine parent, or anchor replacement cannot form a second accepted state domain: a new object cannot satisfy the old marker/store physical identity and parent nonce set, and copied headers retain the old directory/anchor binding. A process already holding a displaced object must fail the next complete binding check, roll back any open backend transaction, publish no pointer or success, and return an integrity hard-stop. A physical write that raced between the pre- and post-mutation checks is not a canonical committed result: the repository remains integrity-stopped and no Lumin process may accept either parent domain until recovery proves one matching marker/lock/store/anchor binding. Architecture v1 does not claim protection against an actor that can forge and replace every bound canonical record; such a rewrite is deliberate store tampering, not a second valid Lumin state domain.

The immutable namespace-binding schema is distinct from the replaceable `lifecycle.store` schema. The immutable `lifecycle.lock` header carries only the global bootstrap fields named in step 2 and never carries a managed-parent or `CacheEvictionParentBinding`. A `repository.json` marker or `lifecycle.store` header that lacks the exact nested binding is `IncompatibleStateSchema`; repository open performs no lazy quarantine creation, adoption, or partial header upgrade. A lock header is incompatible only when its own global bootstrap schema or values are missing or invalid, not because it omits parent bindings. Architecture v1 initializes only the complete marker/store binding above. Any future in-place namespace-binding migration must separately replace the current immutable-lock/bootstrap contract through an independently frozen amendment; Section 12 backend migration cannot invent this binding.

### 2.1 `attempt.json`

Audit start is ordered and crash-recoverable. One repository-catalog transaction allocates the attempt ID/sequence and durable process-liveness lease. Lumin then writes and flushes a `Running` `attempt.json`, advances `latestAttempt` under the exclusive catalog-publication guard defined below, and begins scanning only after both publications are durable. A sequence allocated before its envelope may become a legal gap; it is never reused. A terminal audit atomically replaces the running envelope once with an immutable terminal envelope containing repository/build/request identity, timestamps, success or failure class, concise diagnostics, and optional completed run ID. A hard-stop or unstable snapshot therefore remains addressable even when no run exists.

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
- logical source identities and spans, plus non-semantic physical-alias/payload-snapshot relationships needed to explain reuse and conflicts;
- findings;
- evidence links;
- diagnostics and limitations;
- relationships between findings, symbols, files, and packages;
- metrics;
- projection metadata.

Runtime metrics are canonical run records but belong to a non-semantic partition. Determinism compares the ARCH-001 canonical semantic dump rather than physical store bytes or the complete metric-bearing store.

Only `lumin-store` knows the physical schema or backend API. No public product contract exposes SQL, tables, or backend-specific keys.

Run publication is a crash-consistent multi-object commit protocol, not one fictional atomic filesystem operation. Every operation that publishes/repairs either latest pointer or confirms retention against latest targets acquires the exclusive side of the marker-bound immutable `lifecycle.lock` as the `CatalogPublicationGuard`. The store API yields that token only after the Section 2.0 entry-to-handle proof and repeats the proof around the guarded mutation. This guard is cross-process and spans the final current-generation/linkage validation, read of the existing pointer document, field-wise monotonic-key merge, temporary write, atomic replacement, required flush, and final binding check. Every active cache-cleanup recovery or physical-work interval and every migration interval uses the same exclusive side and therefore cannot overlap publication, retention confirmation, another cleanup interval, or migration. The exact durable `interrupted` observation point defined in Section 2.5 is between cleanup intervals: no cleanup lease or physical mutation is active, the guard is released so read-only show can acquire the shared side, and the operation's continuous active-cache mutation reservation blocks another cache writer or operation from advancing the unfinished plan. A pathname-only open, shared lock, or backend single-writer assumption is not sufficient.

`latestAttempt` and `latestCompleted` are merged independently. `latestAttempt` uses the total monotonic key `(attempt_sequence, envelope_phase)`, where `Running < Terminal`; a terminal envelope therefore advances the same attempt sequence, an identical phase/result is idempotent, and two different terminal results for one sequence are an integrity hard-stop. `latestCompleted` uses the originating successful attempt sequence; the same sequence/run is idempotent and the same sequence naming another run is an integrity hard-stop. A candidate replaces only a greater key while preserving the other field. Thus a later failed attempt may advance `latestAttempt` while an older successful completion still advances `latestCompleted`, but neither field can regress or strand a terminal attempt behind its own `Running` projection.

Publication then follows:

1. Build `evidence.store` and `run.json` in a private staging directory on the same filesystem as `.lumin/runs`.
2. Close the store, validate its schema and semantic identity, record size/hash in `run.json`, durably flush required files, then durably flush the staging directory using the platform-supported equivalent.
3. Atomically rename the validated staging directory to its immutable run directory and durably flush the `runs` parent.
4. Publish the terminal `attempt.json` by same-directory temporary write, durable flush, atomic replacement, and parent flush.
5. Acquire `CatalogPublicationGuard`, open and validate the current generation/linkage, close the transaction-scoped backend handle, reread the current `latest.json`, apply the field-wise monotonic-key merge, write and flush a complete replacement when either field advances, atomically replace it, and flush `.lumin` before releasing the guard. If retention won the guard and the candidate target is now `Pruning`/`Pruned`, publication does not create a pointer and returns the typed nonpublication/integrity result owned by that lifecycle state; if publication wins, retention confirmation observes changed latest/catalog state and remains `Prepared` with `Stale`.

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

The publication probe forces sequence 10 and 11 publishers to read/complete in reverse order, forces one attempt's `Running -> Terminal` update at the same sequence, and races publication against retention confirmation. The only valid final pointer fields are their independent monotonic maxima; no lost update, stranded `Running`, dangling target, or unreported retention win is permitted.

### 2.4 `lifecycle.store`

The repository-wide `lifecycle.store` contains its `StoreGeneration`, audit attempt sequence/process-lease catalog metadata, gate/retention/cache-cleanup operation-id records, cache-eviction authorization manifests, provisional admission and semantic-read-extension reservations, declared intents, logical path leases, baseline fingerprints and facts, advisory findings, immutable worktree transitions/capsules/active-gate references, close-out deltas, retention plans/tombstones, and lifecycle history for every gate in that repository. Attempt evidence remains in `attempt.json`; the catalog metadata exists only for allocation and recovery.

One transactional store is required so overlap detection and lease creation commit atomically across concurrent Lumin processes. Completed gate records become immutable by application contract. Active gates are not temporary transport records and are never silently removed while open.

The repository lock order is fixed: root handle -> marker-bound state-directory handle -> marker-bound `lifecycle.lock` -> held four-kind top-level managed-parent/anchor set plus nested quarantine binding -> transaction-scoped backend handle/transaction. The pre-acquire global proof never retains a managed-parent handle across lock acquisition; initialization creates parents only after it owns the exclusive lock. Ordinary transactions acquire the shared side and then create a complete validated binding token before opening the backend. Publication, latest recovery, latest-sensitive retention confirmation, cache cleanup, and migration enter through the already-held exclusive `CatalogPublicationGuard` token and must not reacquire the shared side. No code acquires `lifecycle.lock` while holding a parent/backend handle or keeps a backend handle after releasing the lock. Publication and cleanup may close their backend transactions and perform their named physical mutations while retaining the exclusive guard; retention closes its confirmation transaction before later trash moves. All retain the relevant held parent handles and repeat the complete Section 2.0 proof before and after physical mutation. There is no scan lock in this order.

### 2.5 Cache

Cache payload content below the immutable `cache/namespace.anchor` is disposable and noncanonical. Evicting active payload descendants may affect performance but cannot change the meaning of a completed run or gate. The cache parent and anchor and the nested quarantine parent and anchor are canonical namespace bootstrap; cleanup never creates, deletes, replaces, or rotates them. Cache corruption below the active anchor becomes a visible miss. Quarantine payload children are noncanonical bytes, but their authorization records are canonical integrity state.

Every quarantine payload child must match `<invocation-id>.<ordinal>.<manifest-sha256>`. The invocation ID is exactly 32 lowercase hexadecimal digits from a fresh 128-bit value, the ordinal is exactly 16 lowercase hexadecimal digits, and the manifest digest is exactly 64 lowercase hexadecimal digits. The digest is SHA-256 over a length-prefixed `cache-eviction-manifest.v1` frame containing canonical owner-type byte encodings of the top-level kind, stable physical identity, link count, regular-file length and content SHA-256 when applicable, and every descendant row ordered by its byte-complete component sequence relative to the top-level root. The frame excludes the active-cache top-level name, quarantine child name, invocation ID, ordinal, and invocation-local Linux mount ID so it remains recomputable after move and reboot. Each traversal separately proves through held handles that every observed child remains on the current parent mount/volume before hashing it.

Name grammar and an unkeyed digest prove only self-consistency and never authorize admission. Each quarantine payload child must have exactly one matching row in the store-owned `CacheEvictionAuthorization` table. That row binds the repository ID, caller-retained `OperationId`, request digest, invocation ID, ordinal, original active-cache component key, destination name, complete expected manifest, and manifest digest; its owning `CacheCleanupOperationRecord` must exist and agree. A committed cleanup result retains the row for as long as the named child remains; lifecycle-store migration preserves and validates the complete authorization/child bijection, and ordinary retention cannot prune either side. A child with no row, multiple rows, a cross-repository row, an invalid migrated closure, or any name/tree/identity/digest disagreement is foreign reserved state and hard-stops before active-cache inspection or success. An interrupted operation's authorized-but-unvalidated child is recoverable only by that same operation ID; a different operation cannot adopt it.

`CacheEvictionAuthorizationSetId` is SHA-256 over a length-prefixed `cache-eviction-authorization-set.v1` frame of the validated rows ordered by byte-complete destination component key. Each framed row contains its repository ID, operation ID, request digest, invocation ID, ordinal, source component key, destination component key, manifest digest, and `Validated` state. Backend key order, physical store generation, display text, and the repeated full manifest bytes are excluded; migration must reproduce the same logical set ID after separately validating every preserved manifest and child.

The public cleanup surface is:

```text
lumin cache clean --operation-id <operation-id> [--format json]
```

It requires exactly one split-form `--operation-id <operation-id>` and accepts at most one split-form `--format json`, in either order, with no equals-form flag, other option, or positional argument. JSON is the only format in Architecture v1. The mutation request digest binds the repository ID, command kind, and cleanup protocol version; the format flag is delivery-only. Conflicting reuse of an operation ID is malformed exit `2` before mutation. A successful invocation emits exactly one `lumin.cache-cleanup.v2` object whose only fields, in canonical order, are `schemaVersion`, `operationId`, `requestDigest`, and `status`, with `status: "clean"`, followed by the normal single transport newline and with empty stderr. Exit `0` is permitted only for the one committed operation result after the active cache is anchor-only, every planned entry is validated and durable, the authenticated quarantine is exact, and the complete Section 2.0 proof passes.

Integrity, persistence, or output-delivery failure exits `1`. A failure discovered before transport leaves stdout empty and uses the ordinary `lumin:` stderr diagnostic. A stdout stream failure may have transferred a nonauthoritative JSON prefix, but no incomplete value is a delivered success object. `BrokenPipe` exits `1` with empty stderr. Any other stdout write or flush failure exits `1` and, when stderr remains writable, emits exactly `lumin: cannot write stdout\n`; simultaneous stderr failure cannot turn the result into success. No storage transaction, catalog-publication guard, or operation-liveness lease is held during transport. A post-commit process death or delivery failure is recovered through `lumin operation show <operation-id>` or an identical retry with the same operation ID; the retry returns the one stored `lumin.cache-cleanup.v2` result and performs no second eviction. `clean` is the immutable result of that operation's final cache observation, not a claim that later cache writers cannot add payloads; cleaning a later active set requires a new operation ID.

The cleanup variant of `lumin operation show` is one bounded `lumin.cache-cleanup-operation.v2` object. Its only fields, in canonical order, are `schemaVersion`, `operationId`, `kind: "cache-clean"`, `requestDigest`, `status`, `interruptionCount`, `authorizedCount`, `validatedCount`, `result`, and `lastDeliveryStatus`. `status` is `pending`, `interrupted`, or `committed`; a new execution starts `pending` with `interruptionCount: 0`. `authorizedCount` is the immutable plan-row count and `validatedCount` is its durable completed prefix; `result` is `null` until commit and then the exact stored `lumin.cache-cleanup.v2` object. `lastDeliveryStatus` is `not-attempted`, `unknown`, `succeeded`, or `failed`: `not-attempted` means that no delivery sequence has ever been allocated, while `unknown` means that the greatest allocated attempt has no durable completion and may have delivered none, some, or all of its bytes. Counts are scalar summaries, so this recovery query never embeds unbounded manifests or entry lists. The amended binary always emits v2 for cleanup-operation projections and offers no v1 negotiation or fallback. The frozen v1 schema retains only `not-attempted`, `succeeded`, and `failed`; a v1-only client therefore sees the distinct v2 `schemaVersion` and must reject it as unsupported before interpreting `lastDeliveryStatus`, never receive `unknown` under a v1 envelope. The v2 projection is backed only by the private `lumin-cache-cleanup-operation.v2` record schema. Lifecycle migration from private v1 records is deterministic and fail-closed: a `pending` or `interrupted` row with no result retains zero allocated/completed delivery sequences; a committed row whose legacy status is `not-attempted` becomes synthetic allocated sequence 1 with no completion; and a committed legacy `succeeded` or `failed` row retains that result as completed historical sequence 1 but also receives a synthetic allocated sequence 2 with no completion. Every migrated committed row therefore projects `unknown`. The unfinished greatest sequence conservatively represents a legacy retry that may already be transporting after all store and liveness locks were released but that private v1 had no pre-transport allocation record with which migration could prove its absence. Any other legacy status/result/lease shape is `IncompatibleStateSchema` before generation replacement. This is an explicit store migration, never a lazy query projection; after replacement an old binary cannot record a v1 completion into v13, and the next current delivery allocates the successor of the migrated maximum—sequence 2 after legacy `not-attempted`, or sequence 3 after legacy `succeeded`/`failed`. Only that current greatest attempt's completion can change the migrated public projection from `unknown`. The canonical operation record separately retains a strictly increasing greatest allocated delivery sequence, completion evidence keyed by sequence, and the greatest completed sequence, but none of those scalars is public. Before transport, one short exclusive transaction allocates and commits the next sequence and atomically makes the public projection `unknown`; it then releases every storage transaction, publication guard, and liveness lease. Completion records that sequence and its `succeeded` or `failed` result in another short transaction. The public status becomes that result only when the completed sequence is still the greatest allocated sequence. While a greater allocated sequence remains unfinished, completion of any older sequence leaves the projection `unknown`; allocating a newer sequence after a completed attempt likewise changes it immediately back to `unknown`. An equal sequence with the same result is idempotent, an equal sequence with a different result is an integrity hard-stop, and a lower late completion appends only its private evidence without overwriting the greatest sequence or public delivery status. Thus process death before completion cannot be projected as `not-attempted`, and concurrent identical retries have one backend-independent winner ordered before transport rather than by transaction scheduling after transport.

`lumin operation show` is strictly read-only: it neither performs a platform liveness check nor changes an operation, execution attempt, reservation, delivery record, or interruption count. Creating a cleanup operation atomically installs one canonical active-cache mutation reservation owned by that operation before the first active-cache manifest read. The reservation remains continuously present through every `pending` lease, process death, the `interrupted` observation, and fresh-lease reattachment; only the transaction that commits the immutable cleanup result releases it. Every store-owned active-cache writer and every different cleanup operation must check this reservation in the same transaction that admits its mutation and reject the unfinished owner before touching payload bytes. The operating-system lock may disappear with a dead process, but the canonical reservation does not.

Immediately after a cleanup child dies, show therefore returns the last canonical `pending` projection until an identical mutating retry begins recovery, while active-cache writers remain blocked by that existing reservation. Under the exclusive `CatalogPublicationGuard`, that retry applies the Section 2.3 platform process-liveness proof to the exact stored execution-attempt lease; timeout or PID text alone is never proof. If the lease is still live, the retry joins that execution without a second mutation. If it is dead, one compare-and-swap transaction records that exact execution-attempt ID as interrupted, sets status to `interrupted`, and increments `interruptionCount` exactly once without replacing or releasing the active-cache reservation. The retry then closes the transaction and releases the guard at the exact recovery barrier, allowing public show to observe `interrupted` through the shared side without permitting cache mutation. To resume, the same or another identical retry reacquires the exclusive guard and uses a second compare-and-swap transaction to allocate a fresh execution-attempt lease and return status to `pending`; the same reservation remains owned throughout, and only then may physical reconciliation resume. A retry that finds the already-recorded `interrupted` state claims it without incrementing again. A death after the fresh `pending` lease requires a new exact liveness proof and contributes one later increment. Concurrent retries serialize on the attempt ID: one performs or claims each transition and the others join the resulting live attempt or return the committed result. Repeated shows never affect any count or state.

Cleanup acquires the exclusive marker-bound lifecycle lock and complete Section 2.0 handle set. The ordinary operation-ID state machine reserves or reopens one `CacheCleanupOperationRecord`: identical ID and digest joins live work, recovers a proven interrupted execution, or returns its committed result; a different ID cannot adopt an unfinished cleanup. Creation of that record and its continuous active-cache mutation reservation is one transaction, and no active payload inspection occurs first. Before creating a new record, cleanup requires every existing quarantine child to match one `Validated` authorization row and computes one ordered `CacheEvictionAuthorizationSetId` plus count over those rows; any `Authorized` row owned by another operation returns a recovery-required failure naming that operation rather than becoming prior state. It manifests every active top-level payload without following redirects or crossing the current parent mount/volume. Unsupported entry kinds, redirects, multiply linked regular files, or topology disagreement hard-stop before authorization or movement.

For every active top-level tree, cleanup first flushes regular-file data and metadata and then descendant directories bottom-up through no-follow held handles, flushes the top-level object, and recomputes the complete manifest. Unsupported directory flushing or any before/after manifest disagreement is a persistence or integrity hard-stop before a move. One lifecycle-store transaction then durably commits the operation ID/request digest, fresh invocation ID, initial authorization-set ID/count, complete deterministic move plan, and one new `Authorized` table row per plan entry before any filesystem rename. The operation record references the canonical table rather than copying every historical quarantine row, so repeated cleanup does not create a quadratic history. These records are authorization and recovery metadata, not evidence that a physical move already occurred.

In manifest order, cleanup revalidates one authorized source, then atomically moves the current winner without replacement from the held cache parent to its record-bound destination under the held quarantine parent. It reopens the destination relative to the held quarantine handle and requires exact top-level identity, full descendant manifest, and digest agreement with the authorization row. It then flushes the moved tree bottom-up again, remanifests it, durably flushes the held active-cache, quarantine, and owning trash directories, repeats the complete namespace proof, and only then transactionally changes that row from `Authorized` to `Validated`. A top-level, nested, or digest mismatch preserves the moved winner in quarantine, leaves prior validated moves quarantined and later entries active, emits no clean result, and remains attached to the same operation for fail-closed recovery.

A same-operation retry reconciles each nonvalidated row before doing new work. Exactly one of two states is recoverable: the authorized source alone still exists and matches, so the move may resume; or the authorized destination alone exists and matches, so retry performs the complete bottom-up tree flush, cache/quarantine/trash parent flush, remanifest, namespace proof, and `Validated` transaction before continuing. Both names, neither name, a substitute, or any manifest/identity disagreement hard-stops. A visible rename or quarantine parent entry is never treated as durable merely because enumeration found it, and a new operation ID cannot convert interrupted physical state into admitted prior quarantine.

After all planned rows are `Validated`, cleanup proves that the active cache contains only its bound anchor and that quarantine contains exactly its immutable anchor, the initially authenticated entries, and this operation's authorized destinations. Even for an empty active cache or a recovered final move, it revalidates and durably flushes the cache, quarantine, and owning trash directories, repeats the complete namespace proof, and then commits the `lumin.cache-cleanup.v2` result with the operation record in one lifecycle-store transaction. A death before that transaction is recovered only with the same operation ID; a death after it returns the stored result. Quarantine authentication records are provenance and recovery authority for physical state, never semantic run/gate truth.

Physical reclamation of cache-eviction entries is not part of `lumin cache clean`, its success state, or its delivery recovery, and ordinary retention reclamation ignores this disjoint family. Cleanup never calls `unlink`, `rmdir`, recursive removal, or an equivalent final disposition on a quarantine payload. A future store-owned reclamation protocol may dispose of one only through an operating-system primitive whose final disposition is bound to the already opened physical object, or through an enforceable isolation boundary that prevents replacement through disposition; the matching canonical authorization row may change only in the same recoverable protocol. A secret or random name, advisory lock, second pathname validation, sleep, or scheduler timing is not such authority. If no supported primitive exists, the noncanonical payload and its canonical authorization remain.

The required proof uses exact public-child barriers and store-owned durability hooks rather than sleeps. It covers: a syntactically valid self-hashed quarantine tree with no authorization record; top-level and nested-child substitution before move; death after authorization but before rename; death after rename visibility but before tree or parent flush; death after all physical flushes but before `Validated`; death after every row validates but before result commit; dirty regular-file and nested-directory payloads; empty-cache cleanup with prior authenticated quarantine; and both stdout failure classes. Delivery barriers stop after sequence allocation but before output and after a partial or complete stdout value but before completion recording. They force both completion orders: first a lower sequence completes while a greater allocation remains unfinished, so show stays `unknown` until the greater completion selects its result; then a greater sequence completes while the lower completion is held, after which the lower completion commits late. The latter must leave the already selected status and complete public v2 operation projection byte-for-byte unchanged; the private final ledger must equal its pre-release state plus exactly that lower sequence/result, with the greatest allocated/completed sequences and every other durable field unchanged. An exact prior-schema fixture also proves each private-v1 mapping above and rejects every invalid legacy shape before generation replacement. Every recovery uses the same operation ID, asserts `operation show`, remaining relative order, the exact authorization/entry states, parent and anchor identities, and the final durable snapshot. A different operation ID cannot adopt a pending move. Standard, determinism, store fault, and Windows/Linux package lanes together prove response bytes, authentication, flush order, process restart, and unchanged completed run/gate evidence.

The target `lifecycle.store` header schema advances to `lumin-lifecycle-store-header.v13` with the private cleanup-operation record schema. Ordinary repository open and query accept only v13 and never deserialize or project private-v1 cleanup rows; a valid v12 header is `IncompatibleStateSchema` with the exact `lumin store migrate` recovery instruction below. Only `lumin store migrate` has the exclusive generation-fenced reader that may admit `lumin-lifecycle-store-header.v12`, authenticate its complete logical snapshot, and apply the exact transformation above.

A cache lookup may replay one owner-authored `CachedOwnerStep` keyed by the exact snapshots and semantic owner-task/profile parameters already supplied in that iteration. `NeedsInputs` metadata may depend only on those keyed prerequisites, is not a semantic hit, and cannot reveal a downstream demand derived from uncaptured bytes. During gate analysis every demanded path is conflict-checked and reserved before inventory captures it; only then may the next cold/cached step run. An accepted finished envelope replays the exact owner outcome/capability state, facts or opaque/failure payload, diagnostics, limitations, gate-neutral signals, and consulted inputs. Request-specific signals and lifecycle deltas are recomputed by the owning capability from that validated outcome and current model-owned `GateProjectionContext`.

### 2.6 Storage Backend Decision Gate

Architecture v1 does not select a persistence engine by familiarity. `lumin-store` first defines a backend-neutral contract for:

- immutable run publication and read-only reopening;
- indexed bounded queries with stable cursors;
- one atomic cross-process gate lease transaction;
- crash recovery, migrations, and corruption-visible failure;
- Windows NTFS, Linux ext4, and Linux musl release operation.

The architecture review benchmarks at least one pure-Rust embedded candidate, initially `redb`, against bundled SQLite. The comparison records clean and incremental build time, release binary size, transitive and unsafe surface, cold and warm store latency, peak memory, store size, multi-process contention behavior, and crash recovery.

`redb`'s first probe is two independent writer processes contending for the same lifecycle store. If open/lock/retry behavior cannot preserve atomic lease admission without a daemon or a second truth owner, the candidate is rejected before performance comparison.

Correctness probes inject process death at every row of the publication, retention, and lifecycle-migration crash tables. They cover unadoptable orphan runs, dangling pointers, corrupt hashes, stale-writer sequence regression, old-generation late writers, retention tombstone/trash recovery, and unsupported durable-flush behavior on each required platform. Probe evidence preserves source and fixture hashes, toolchain/target, exact commands, expected invariants, crash point, and raw result under `reviews/probes/<probe-id>/`; build output is removed, but reproducibility evidence is not.

The first failing correctness requirement rejects a candidate before performance ranking. Architecture v1 records one accepted backend and rationale; production does not ship dual backends or a runtime fallback.

The 2026-07-17 selection gate passed for exact `redb 4.1.0` and bundled SQLite through exact `rusqlite 0.39.0`. Both candidates passed the frozen Windows x64/NTFS, WSL2 ext4 GNU/musl, and native non-WSL Linux ext4 GNU/musl correctness matrices. Architecture v1 selects exact `redb 4.1.0` as the sole production backend. Across the five measured target/mode comparisons, redb won every durable-admission p50 and release-binary comparison, used 12-13 fewer transitive packages, and bundled no native C source. SQLite won four bounded-query p50 comparisons, every peak-RSS comparison, and store size; those costs remain inputs to numeric-budget approval rather than being hidden. The exact evidence bindings, contrary metrics, and rationale are recorded in [`phase0-store-backend-selection-2026-07-17`](../reviews/probes/phase0-store-backend-selection-2026-07-17/). Bundled SQLite remains probe-only evidence and is not a product dependency, migration target, selector option, or runtime fallback.

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
- one model-owned `FindingDisposition` that is `ReviewCandidate` or `ReviewOnly` with a stable reason and never controls finding existence;
- a concise claim;
- evidence references;
- relevant scan scope and limitations;
- source fingerprints for referenced spans;
- optional remediation and verification hints.

Counts are computed from canonical rows or owned canonical aggregate rows. A bounded top-N projection or remediation disposition cannot become the count owner. Canonical evidence has no `Muted` or `Suppressed` finding class.

## 4. Query Protocol

The primary interface is:

```text
lumin overview [--run <run-id>]
lumin findings --run <run-id> [filters] [--cursor <cursor>]
lumin explain --run <run-id> <finding-id> [--evidence-cursor <cursor>] [--relations-cursor <cursor>]
lumin related --run <run-id> <finding-id> [--cursor <cursor>]
lumin files --run <run-id> <repo-path> [--cursor <cursor>]
lumin capabilities [--run <run-id>] [--cursor <cursor>]
```

These commands are the query subset of the canonical ARCH-000 command table. Skills use `--format json`; human-readable output is a projection of the same DTO. Machine output writes one versioned JSON value to stdout and diagnostics to stderr.

`overview` without `--run` selects and returns a concrete immutable run scope after showing any newer failed attempt. Every follow-up run-evidence command requires that returned run ID; it never follows a moving latest pointer. `capabilities` without a run reports the current binary's compiled capabilities, while `capabilities --run <run-id>` reports the states recorded by that run.

Every collection response uses one envelope:

```json
{
  "scope": {"kind": "run", "id": "run_..."},
  "filters": {},
  "ordering": "findings.v1",
  "scopeTotal": 812,
  "total": 812,
  "returned": 20,
  "truncated": true,
  "nextCursor": "...",
  "items": []
}
```

Rules:

- no hidden `take(N)`;
- normalized `filters`, unfiltered `scopeTotal`, matched `total`, `returned`, and `truncated` are mandatory;
- an omitted CLI filter normalizes to `{}`; `lumin findings` and `lumin gate findings` with `{}` return every canonical finding, including `ReviewOnly`, with no implicit role, framework, severity, or remediation filter;
- cursors are opaque and bound to protocol schema, immutable scope identity or gate revision, normalized filters, collection path, ordering ID/version, page-size policy, and last semantic key;
- every encoded cursor carries a versioned, domain-separated SHA-256 content binding over its exact inner payload, and each paging owner accepts an existing anchor only when its one-based resume offset is a nonterminal multiple of the bound page size; changing an issued payload without its matching binding or selecting a nonboundary row is malformed;
- this deterministic content binding is neither a MAC nor an authentication or authorization credential: a local caller that reimplements the open codec may construct a cursor at another actual legal boundary, which is an equivalent read continuation because the caller may already read or discard preceding pages; no secret, global state, repository access for current-binary queries, or read-time mutation is introduced;
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

Severity sorts by its explicit `lumin-evidence` rank and confidence by the model-owned `ConfidenceRank`, never localized or display labels. Their selected ordering is part of `findings.v1`; changing it requires a new ordering ID.

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
- finding disposition may change remediation wording but cannot omit a canonical finding, change its ordering key, or erase its gate signal; only an explicit caller filter may narrow a projection;
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
<NUL-delimited native path records> | lumin pre-write --operation-id op_... --paths0-from -
```

The caller creates and retains a repository-scoped `OperationId` before invoking a mutating lifecycle command. Reusing it with different canonical input is malformed and cannot mutate state. With the same request digest, a retry returns a committed result immediately, joins a still-live execution without starting another mutation, or re-acquires and re-executes an operation proven interrupted before any gate-lifecycle or durable-path-lease mutation. The mutating command owner, never a read-only query, performs the platform liveness proof and idempotent execution-attempt transition under its required lifecycle guard. Provisional reservation and operation-attempt records may already exist; they are recovery metadata, not authorization. No arbitrary timeout converts a live execution into an interrupted one.

A caller-declared path outside the canonical root is malformed request input: exit `2`, no operation record, no gate ID, and no lease. Valid pre-write then uses one protected handoff:

1. The controller quiesces its participating editors for the declared domain and already known semantic inputs.
2. Inventory resolves each declared existing path to its physical identity/conflict key without consuming source bytes. Inferred manifest/lockfile writes receive the same treatment.
3. One lifecycle-store transaction binds the operation ID/request digest, checks those initial keys and the current catalog, and creates a short-lived provisional reservation for the declared/new paths, existing physical conflict keys, and candidate semantic-read sets. This reservation blocks conflicting compliant opens but is not an `Active` gate lease and does not authorize edits.
4. Inventory enumerates the Section 8 `PhysicalAliasWriteClosure` for every reserved existing object. Before capturing an additional alias, one lifecycle-store transaction checks and extends the reservation to its logical path and the same physical key. An active/provisional conflict, unobservable group, or root-escaping alias stops with typed incomplete evidence; no arbitrary representative is leased.
5. Inventory captures the exact declared/closure-expanded observation domain, alias topology, candidate semantic reads, source set, configuration, content identities, and catalog revision.
6. Owners analyze only supplied snapshots and return ARCH-001 `OwnerStep` values. `NeedsInputs` contains path-level demands only: neither the owner nor a cache validator has read or accepted those demanded bytes. `Finished` contains the complete outcome, gate-neutral signals, and exact supplied identities actually consumed.
7. For every new demand, one lifecycle-store transaction normalizes and checks the path against active/provisional writes and extends the reservation before any capture or consumption. A conflict, unbounded demand, or unobservable required input stops the closure branch with typed `Incomplete` evidence and records the attempted domain, last complete read set when one exists, and the conflicting or unbounded demands. When all additions are admissible, inventory captures their exact identities/bytes; only then may the next prerequisite-keyed cached step run or an affected cold owner resume from its owned continuation. If that cached next step misses, the owner starts once with all snapshots supplied so far. No path already consumed in this execution is reread or reparsed. Steps 6-7 repeat until every owner is `Finished` and no demand remains.
8. A successful closure transactionally removes any reserved input that no finished owner consumed, seals the exact finished semantic-read set, and derives `ObservationBinding::Sealed(Baseline(GateBaselineObservationId))`. Inventory then rediscovers and rehashes the complete sealed path/identity and alias-topology sets; drift yields a sealed `Stale` result. A failed closure derives `ObservationBinding::Unsealed` from the typed failure data and creates no baseline observation ID or fabricated partial hash.
9. One final lifecycle-store transaction rechecks the operation digest, reservation, catalog and transition revisions, alias closure, and every applicable conflict; a sealed branch also rechecks the sealed reads. It maps typed signals through the canonical effect policy, allocates the gate ID, and atomically commits either `Active` plus its durable path lease and sealed read set or queryable `Rejected` without a lease. Only `Allow` or `AllowWithWarnings` with a sealed binding may become `Active`. The provisional reservation is removed in the same transaction.
10. The controller may resume editors after delivery succeeds or, if delivery fails, after it recovers the committed operation result. The returned decision carries the exact `ObservationBinding` accepted in step 9 and exposes the declared paths plus every automatically leased logical alias.

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

Symbol and dependency intent is path-scoped. The path identifies the consuming source or package context; Lumin resolves its nearest owner manifest. A dependency addition adds the inferred owner manifest and lockfile to the leased write set. Command-line path flags are read through native `args_os` semantics. `--paths0-from` uses NUL-separated raw path bytes on Unix and canonical WTF-8 records on Windows; both decode into `repo-path.v1`, preserve legal newline-containing paths, and round-trip through the machine DTO without using display text.

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
- normalized candidate leased-write set, including the explicit `PhysicalAliasWriteClosure` expansion and physical conflict keys;
- internally partitioned language lanes;
- baseline `ObservationBinding` and opening gate-catalog revision;
- available baseline or attempted-domain findings needed by the declared intent;
- advisory decision and evidence.

A sealed opening additionally records its opening `AnalysisInputId`, sealed semantic-read set, source-set/content fingerprints, complete `PhysicalAliasWriteClosure` topology, and `GateBaselineObservationId`. Only such an opening may become `Active` and promote its candidate leased-write set to an exclusive durable path lease. An unsealed `Rejected` record has no authoritative opening `AnalysisInputId`, sealed read set, baseline fingerprint set, observation ID, or durable lease; its binding stores the attempted domain, optional last complete read set, and conflicting, unbounded, or unobservable inputs instead.

Every close revision records its `ObservationBinding`. A sealed close also records the current `AnalysisInputId`, exact protected close-read set, closure-expanded actual-write set, final alias topology, current fingerprints, and own/intervening transition chain. An unsealed close records no current `AnalysisInputId` or complete current fingerprint set, retains the prior active revision's sealed read protection, and stores only its typed attempted-domain data. No conditional field is populated with a partial value to satisfy storage shape.

The sets have distinct meanings:

- `declared_write_set`: paths or directory scopes the caller says it will change;
- `leased_write_set`: normalized existing/new paths, every admitted logical alias in each existing physical write closure, and inferred manifest/lockfile writes that no other active gate may read or write;
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

1. The controller quiesces its participating editors, then captures the current source/config path sets, exact identities, source snapshots, opening semantic reads, baseline/current physical-alias topology, and transition-catalog revision.
2. Reconcile every post-baseline terminal transition under Section 8.1. A transition touching a protected opening semantic read outside this gate's leased and actual write sets yields `Stale`; a changed path covered only by another active gate yields `Incomplete`; an identity mismatch or unexplained path yields `Deny`.
3. Remove only exactly chained, disjoint terminal transitions from the raw baseline/current diff. Expand one explained physical payload change across every baseline-leased logical alias, classify alias entry creation/removal/retargeting as topology writes, and reject any changed endpoint outside this gate's path/directory leases. The remainder is the preliminary actual-write set.
4. Analyze only the supplied snapshots and derived affected facts. Each owner returns an ARCH-001 `OwnerStep`: `NeedsInputs` names unconsumed path-level demands, while `Finished` names only exact supplied identities actually consumed.
5. Before inventory reads or hashes any newly demanded input, one lifecycle-store transaction checks it against active/provisional writes and establishes an operation-scoped semantic-read-extension reservation. A still-active writer, unbounded demand, or unobservable required input stops the closure branch with typed `Incomplete` evidence and an unsealed attempted-domain record. When every demand is admissible, inventory captures the added exact identities/bytes; only then may the next prerequisite-keyed cached step run or an affected cold owner resume from its owned continuation. If that cached step misses, the owner starts once with all snapshots supplied so far. The transition catalog and preliminary delta refresh with that capture, and no payload already consumed in this execution is reread or reparsed. Terminal transitions completed before it are analyzed at their exact current identities; a later transition changes the catalog and fails final validation. Steps 4-5 repeat until every owner is `Finished` and no demand remains.
6. A successful closure transactionally removes any unused extension reservation, seals the exact finished close-read set, recomputes the exact closure-expanded actual-write set and final alias topology, derives the current close `AnalysisInputId`, and creates `ObservationBinding::Sealed(Close(GateCloseObservationId A))` from those sets, their content/physical identities, and the accepted transition/catalog revision. Every admitted alias receives a separate logical-context analysis. A failed closure creates `ObservationBinding::Unsealed` with no A and retains the prior protected read set.
7. Only a sealed branch rediscovers the complete path/alias sets and rehashes their exact inputs. Any difference from A yields a sealed `Stale` result.
8. In the close transaction, recheck the operation digest, gate revision/lifecycle, transition-catalog revision, every physical alias closure, every other active gate or reservation, and reconciliation chain; a sealed branch also rechecks its exact read set and A, while an unsealed branch rechecks its attempted-domain conflict identities without pretending they form a complete observation.
9. Persist the immutable operation result, `ObservationBinding`, and close-attempt revision. A sealed revision records its current `AnalysisInputId`; an unsealed revision records none. A conflict-free sealed `Deny` or `Incomplete` whose snapshot is still `Current` may replace the active revision's protected read set; an unsealed result or any sealed `Stale` historical observation leaves the prior protection unchanged. Every retry still computes lifecycle delta against the immutable opening semantic baseline, never the prior failed close. Only `Allow` or `AllowWithWarnings` with a sealed binding appends the terminal worktree transition, closes the gate, and releases its durable logical path lease atomically. Every operation-scoped extension reservation is removed in this transaction.

The controller may resume participating editors after delivery succeeds or, if delivery fails, after it recovers the committed operation result. An authorizing result is bound to the sealed returned `GateCloseObservationId`; a nonauthorizing result returns its sealed or typed unsealed binding. Neither is a claim that an unlocked worktree can never change after the final observation. A later edit requires a new gate transaction.

If the close process dies before its final transaction, liveness recovery removes only that operation's semantic-read-extension reservation and leaves the gate at its prior durable revision/read set. The same operation ID may retry because no close revision committed. A death after final commit preserves the committed revision and is recovered through `lumin operation show`.

Actual delta derives from the baseline and current inventory identity maps. A rename is canonical only when one baseline path and one current path share the same unique persistent filesystem identity; otherwise even identical content is reported conservatively as remove plus add. VCS status may accelerate candidate discovery but is never the truth owner. Both rename endpoints require leases in either representation.

Only `Allow` and `AllowWithWarnings` commit the terminal close result, worktree transition, and logical lease release in one store transaction. `Deny`, `Incomplete`, and `Stale` append an immutable close-attempt revision and keep the gate active until a later successful close or explicit abandon; only a conflict-free sealed current `Deny`/`Incomplete` read set may replace that active revision's protection. A sealed stale snapshot is preserved as historical evidence but never becomes current protection. Result transport occurs after storage transaction locks, any `CatalogPublicationGuard`, and operation-liveness leases are released. The durable logical path lease and protected semantic reads of an `Active` gate remain repository state until a later validated revision, close, or abandon. Architecture v1 has no `scan lock`; reservations, snapshots, final validation, and lifecycle transactions own the safety guarantees.

## 8. Concurrent Agents and Path Leases

Path leases are logical transaction records, not OS file locks held by a long-running process.

Path identity follows the observed root filesystem:

- existing paths resolve every symlink or junction prefix, remain inside the canonical root, and record repository spelling plus each existing prefix's platform file identity and observed comparison behavior;
- a new path resolves its nearest existing parent and compares each unresolved component under that parent's observed case behavior, refreshing policy as parents are created;
- physical identity wins for existing alias-conflict and containment checks, never for logical source deduplication; root-wide case policy is only a fallback when a parent-specific observation is unavailable, and Linux byte-distinct names are never collapsed by generic Unicode normalization;
- directory leases conflict with descendants, and a rename requires both source and destination leases;
- an alias that reaches the same existing file conflicts even when its spelling differs.

The lexical representation is lossless and backend-neutral. The exact checked-in [`repo-path-semantics.v1` artifact](../specs/repo-path-semantics.v1.json), file SHA-256 `ee686f81164ff40b281483afaae591793964cc576afaca0ce7b5b51a6798b4a6`, not this prose summary, owns every tag byte, width, endian rule, root-prefix form, canonical rejection, Base64 rule, WTF-8 conversion, and golden vector. Its exact bytes and generated-code digest participate in `AnalysisContractId`:

- `RepoPath` is a normalized relative component sequence. A portable component stores exact UTF-8 bytes with no NFC/NFD/case normalization; a non-UTF-8 Unix component stores exact native bytes; a Windows component not representable as Unicode scalar text stores exact WTF-16 code units. `.`/`..`, root prefixes, embedded separators, and NUL are rejected before identity creation.
- `RepositoryRootIdentity` encodes the absolute native root with the same atoms plus platform prefix/volume and observed physical directory identity. `RepositoryId`, state admission, and root-equality checks use it; a root that is not printable Unicode remains supported and cannot collide with another root through display conversion.
- `repo-path.v1` uses artifact-owned `LUMRPATH`, big-endian version/count/length fields, and exact component-kind payloads. These canonical bytes, not display text or backend collation, are used for `LogicalSourceId`, stable finding identity, ordering, hashing, gate sets, cache keys, and cursor anchors. Portable UTF-8 paths have the same canonical bytes on Windows and Unix; native-only components remain explicitly platform-kind-tagged.
- `repository-root.v1` separately uses artifact-owned platform/address-prefix and physical-identity records for Unix roots, Windows drive/UNC/volume-GUID roots, and rejects device namespace roots.
- Every JSON/machine path uses `RepoPathDto { encoding: "repo-path.v1", canonicalBase64, display, utf8? }`, and every JSON/machine root uses `RepositoryRootDto { encoding: "repository-root.v1", canonicalBase64, display, readableAddress? }`. Root `canonicalBase64` includes the complete physical identity; `readableAddress` excludes it and is permitted only for a portable artifact-defined Unix/drive/UNC/volume-GUID address. Base64 is padded RFC 4648 standard form and must decode then re-encode byte-for-byte. Decoding validates canonical form, rejects a disagreeing readable projection, and rejects parallel structured root fields. Human output may show `display` or the optional readable projection but never accepts either as an identity round trip.
- Git-wildmatch operates on slash-separated `RepoPathMatchBytes`: exact UTF-8 bytes for portable components, raw bytes for Unix native components, and canonical WTF-8 for Windows native components. Ignore-file patterns retain their file bytes; invocation/JSON patterns are UTF-8. No Unicode normalization occurs, and wildcard matching cannot merge two distinct canonical paths.
- Existing physical identity and observed parent comparison behavior remain separate from lexical bytes. They may establish alias conflicts, prevent directory cycles, and allow one validated payload read, but never rewrite canonical spelling, choose a representative source, erase a package/config/role context, or cause Linux byte-distinct names to share a logical key.

Inventory persists three separate identities:

- `LogicalSourceId`: one admitted lexical `RepoPath` plus source kind; this owns package/config/scan-role context, findings, source-use resolution, and query paths;
- `PhysicalFileIdentity`: the observed filesystem object used for root containment, aliases, write conflicts, and snapshot validation;
- `PayloadSnapshotId`: exact captured bytes bound to one validated physical observation, used only to reuse reads and compatible path-independent parse work.

Two logical paths to one symlink target or hard-linked file remain two logical sources. They may share a payload snapshot and parse product, but each receives a separate owner fact envelope and resolver pass. Gate path sets retain lexical identities and physical alias groups: either logical alias conflicts with a write to the same physical object, while evidence and stable finding IDs remain attributable to the exact logical path. Only duplicate discovery of the same normalized lexical path is collapsed.

`PhysicalAliasWriteClosure` closes the write meaning for that group:

1. Inventory first reserves each declared existing object's physical conflict key, then enumerates every currently admitted root-contained logical alias and extends the reservation before capturing each member. The effective leased set contains every member plus the physical key; the caller's declared set remains unchanged for explanation.
2. The pre-write response exposes each automatic expansion and its physical-group identity before edits are authorized. Every expanded logical source is analyzed in its own package/config/role/resolver context even when payload and compatible parse work are reused.
3. An alias whose identity or containment is unobservable, a known alias outside the canonical root, or a physical alias group that cannot be bounded yields typed `Incomplete`; Lumin does not lease an arbitrary representative path.
4. Close recomputes the group and treats a byte change through a declared member as explaining the same payload change at every baseline-leased alias, while retaining each alias in the actual-write/reanalysis domain.
5. Creating, removing, or retargeting a symlink/hard-link/case alias is a topology write. Every changed directory entry and physical-group endpoint must already be covered by a path or directory lease. Otherwise close is `Deny`; if final identity/containment cannot be observed it is `Incomplete` or `Stale` under the existing freshness rule.
6. A newly admitted alias that is fully covered by those leases joins the final closure and is analyzed in its logical context before authorization. A removed alias remains in the transition evidence with its exact prior identity.

On `pre-write`:

1. Normalize the declared and semantic-read sets and resolve initial existing physical conflict keys without consuming source bytes.
2. Protect those keys with the operation-scoped provisional reservation from Section 7.1, then enumerate and reservation-extend every `PhysicalAliasWriteClosure` member before capture.
3. Compare the closure-expanded leased set with every active leased and semantic-read set.
4. Compare the semantic-read set with every active leased set.
5. Reject conflicts with the gate IDs, logical paths, physical groups, and read/write relationship.
6. Promote the complete reservation to an `Active` gate lease only with the exact accepted baseline; a rejected or interrupted operation releases it.

The same reservation-before-capture comparisons repeat for every alias expansion and semantic-read demand. A newly discovered read that intersects an active or provisional write yields `Incomplete`; it is never omitted from the observation to preserve apparent concurrency.

Directory declarations expand to their observed source paths at open time and retain a directory-level lease for new-file detection.

Parallel agents with nonconflicting write/read sets may proceed. Their closes are serializable through the transition ledger below, not assumed independent merely because analysis overlapped. Workers in one coordinated wave should share one gate. An abandoned gate requires an explicit command:

```text
lumin gate abandon <gate-id> --operation-id op_... --reason "..."
```

No age-based cleanup may silently release an active write contract.

Abandon validates the operation digest and exact active gate revision, then commits `Abandoned`, the reason, lease release, and operation result in one lifecycle-store transaction. A retry returns that terminal revision; a different operation against an already terminal gate cannot create another lifecycle revision.

### 8.1 Shared-Worktree Transition Ledger

Lumin does not claim which OS process or editor wrote a byte. It proves whether the observed worktree state is covered by an authorizing gate transition.

Every authorizing close appends one immutable, monotonically sequenced `WorktreeTransition`. Its immutable `TransitionCapsule` is the transition's sole reconciliation payload, not a second truth owner, and contains the gate/revision, baseline and close observation IDs, leased writes, sealed close semantic reads, and exact before/after identities needed for later reconciliation. Every opening baseline records the current transition sequence.

In the same terminal-close transaction, `lumin-store` creates an `ActiveGateTransitionRef` from every other active gate whose opening sequence precedes the new transition to that capsule. The reference does not claim that the transition is compatible; it keeps the exact proof available until that active gate can classify it. Closing or abandoning an active gate atomically releases all references it owns, while a successful close also creates references from the remaining eligible active gates to its new capsule. A referenced capsule is retention-ineligible even when its originating gate is terminal. A later gate opened after the transition needs no reference because its sealed baseline already observes the resulting bytes.

At close, changes after that sequence are partitioned as follows:

- this gate's declared/leased transition is analyzed as its actual delta;
- another gate's terminal transition may be reconciled only when its exact before/after identity chain reaches the current bytes and its paths are disjoint from this gate's leased writes and sealed opening semantic reads;
- a terminal transition intersecting a sealed opening semantic read makes the baseline `Stale`;
- a path first discovered as a close-time semantic read may consume a terminal transition only by recapturing and analyzing that transition's exact current identity before the close read set is sealed; a later transition changes the catalog and invalidates the observation;
- a changed path or candidate semantic read covered by another still-`Active` gate has no terminal identity to reconcile and makes close `Incomplete` with an attribution-pending finding;
- a missing transition, broken identity chain, or other unexplained changed path is an unplanned-transition signal and makes close `Deny`.

The store serializes terminal transitions, active-gate transition references, and close revisions. Thus disjoint gates may analyze concurrently, but a close that observes another in-flight edit waits through an `Incomplete` retry until that edit becomes a terminal transition. If a different process produced bytes later authorized by another gate, Lumin reports only that the final state transition was authorized; it never fabricates process provenance.

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

`lumin operation show` returns the canonical command kind, request digest, mutation status, target IDs/revisions, cleanup authorization/validation counts when applicable, committed result, and last delivery status. It is a read-only recovery projection for every gate, retention, or cache-cleanup mutation when a caller retained its operation ID but did not receive stdout; it never proves liveness, marks an interruption, claims an execution, or resumes work. Delivery attempts may append transport metadata; they never create another lifecycle revision, plan, pin change, deletion, cache move, or authorization row.

An identifier whose text begins with `--` remains a valid mutation option value. For a positional lookup, the caller places the end-of-options marker before it, for example `lumin operation show -- --retry-token`; without that marker, a leading option is rejected rather than silently consumed as an identifier.

Pre-write, post-write, gate abandon, run pin/unpin, prune-plan creation, prune confirmation, and cache cleanup all require an operation ID before they mutate durable state. The same operation state machine applies to each command: identical ID plus digest joins live work, retries only a protocol-proven interruption, or returns the one committed result; conflicting reuse is malformed. Read-only list/show/page commands do not require an operation ID.

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

Gate policy never infers severity from display text. For lifecycle comparison, each capability owner canonicalizes baseline and current facts by a model-owned `DeltaKey` that excludes mutable comparison dimensions, then applies one total relation:

```text
GateDeltaClassification =
  Introduced
  | Unchanged
  | Regressed { changes: NonEmpty<DeltaDimensionChange> }
  | Improved { changes: NonEmpty<DeltaDimensionChange> }
  | ChangedIncomparable {
      regressions,
      improvements,
      incomparable_changes
    }
  | Resolved
  | BaselineUnavailable

DeltaDimensionChange =
  TargetAdded(identity)
  | TargetRemoved(identity)
  | AffectedIdentityAdded(identity)
  | AffectedIdentityRemoved(identity)
  | ConfidenceRaised(from, to)
  | ConfidenceLowered(from, to)
  | GroundingRaised(from, to)
  | GroundingLowered(from, to)
  | EvidenceIdentityChanged(from, to)
  | OwnerPayloadRegressed(field_id, from, to)
  | OwnerPayloadImproved(field_id, from, to)
  | OwnerPayloadChanged(field_id, from, to)
```

Absent-to-present is `Introduced`, present-to-absent is `Resolved`, and exact semantic equality is `Unchanged`. For target and affected-domain sets, additions are regressions and removals are improvements; limitation scopes first derive their exact affected-domain set rather than relying on enum-name order. Every slice defines explicit confidence and grounding ranks, where a rank loss is regression and a gain is improvement. Changed evidence identity and any owner payload field without a declared direction are incomparable. Only regressive dimensions produce `Regressed`, only improving dimensions produce `Improved`, and any mixture or incomparable dimension produces `ChangedIncomparable`. Thus overlapping sets such as `{a,b} -> {b,c}`, narrowed-but-present limitations, confidence loss, owner-payload replacement, and evidence replacement all have exactly one classification. Every persisted owner fact field is declared as part of `DeltaKey`, one closed comparison dimension, or non-semantic metadata; the architecture check rejects an unregistered semantic field. Duplicate rows for one `DeltaKey` are canonicalized before comparison, and adding an owner dimension requires an exhaustive relation and signal mapping.

Static limitation registries own scope and absence impact, not lifecycle effect. Owners turn the total classification and its dimension changes into distinct typed `GateSignal` values under the active slice: adverse introduced/regressed/mixed changes, unchanged advisory facts, improvements/resolutions, and unavailable required comparison cannot share an implicit path. Pre-write may emit advisory signals from complete existing facts or incompleteness when required evidence is unavailable, but it does not invent a post-write delta. The engine gate service emits only named transaction-invariant signals from typed inventory/store outcomes; the engine capability registry separately emits only compiled-profile availability facts/signals and never substitutes analysis. The closed, versioned `lumin-evidence::gate_policy` table maps signals to effects:

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

A caller may explicitly request a broader audit, but the write gate does not silently launch one. Cold and warm timings are reported separately. Warm reuse validates and replays the full ARCH-001 owner outcome/capability state, diagnostics, payload, limitations, gate-neutral signals, and consulted reads. The owning capability then recomputes request-specific signals from the current `GateProjectionContext`. Cold/warm execution over one exact observation must reach the same binding, decision, and canonical semantic dump.

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

Plan creation allocates a model-owned `RetentionPlanId` and persists one immutable `Prepared` plan in the same transaction; the ID is scoped to the repository and collision-checked by `lumin-store`. It deletes nothing. A run plan contains the exact attempt, completed-run, orphan-payload, every independent pin/reference, byte count, exclusions, repository catalog revision, and content identity. A gate plan contains the exact terminal gate, revisions, findings/evidence, operation records, terminal `WorktreeTransition`/`TransitionCapsule`, and any `ActiveGateTransitionRef` that excludes that closure from deletion. The content identity derives from the canonically ordered logical plan payload under the lifecycle schema, never backend row/page order. The plan plus its creation/confirmation operation records are not members of their own deletable closure; they become the minimal retained tombstone. `plan show` pages the immutable retention-plan scope and never creates a replacement plan from repeated filters. Unrelated repository mutations do not invalidate paging; confirmation separately revalidates the current catalog. Gate retention uses the `lumin gate prune` commands above; no second cleanup owner exists.

Confirmation accepts only the exact plan ID plus a new operation ID. Before deletion begins, it acquires `CatalogPublicationGuard`; under that same exclusive guard one lifecycle-store transaction revalidates pin/lifecycle/catalog/latest/transition-reference state and every record identity, then commits `Pruning` or leaves the plan unchanged before releasing the guard. A changed input leaves the plan `Prepared` and returns `Stale`. Pinned or active records are never eligible. A terminal gate and its transition capsule are also ineligible while any `ActiveGateTransitionRef` points to that capsule; retention cannot turn a later active close from reconcilable into missing-transition `Deny`. The current `latestAttempt` target and `latestCompleted` target are never eligible, and each retains its linked attempt/run closure. The plan reports every exclusion reason. A concurrent publisher that wins first changes latest/catalog state and makes confirmation stale; confirmation that wins first makes a later publication revalidate the target's typed retention state before any pointer write.

Deletion is a crash-consistent state machine:

1. The successful confirmation transaction changes the plan and exact record closure to `Pruning(planId)`, stores expected canonical and same-filesystem trash identities plus the current source/trash `ManagedStateParentBinding` values, and binds the confirmation operation result-in-progress. Those records are no longer ordinary query results, but their tombstones remain inspectable.
2. After revalidating the complete parent set, run, attempt, and orphan filesystem payloads below but never including a `namespace.anchor` are atomically renamed into `.lumin/trash/<plan-id>/`; source/trash parent bindings are revalidated and both parent directories durably flushed before progress is accepted. Backend-resident gate/evidence rows move into a logical trash namespace through transactional tombstones; physical page reclamation is not canonical deletion truth.
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
- Terminal transition capsules and their full reconciliation payload remain linked and prune-ineligible until every active-gate transition reference is released by close or abandon.
- Cache has an independent cleanup policy.
- Retention commands report exactly which immutable run, attempt, orphan, gate, evidence, and operation records will be removed.
- A durable finding referenced by a review or CI result is addressable by run and finding ID.
- No user workflow requires manual deletion of generated intent transport.

Architecture v1 retains minimal plan/record/operation tombstones for the repository lifetime. They are not eligible for second-order pruning in the first slice; their count and bytes are measured separately in retention probes. Any future compaction requires an explicit lifecycle-schema and retention-contract amendment rather than silently erasing deletion history.

Attempt sequences are allocated atomically in the catalog; a sequence becomes a started attempt only when its `Running` envelope is durable. `CatalogPublicationGuard` serializes the complete guarded interval across concurrent publishers, recovery, retention confirmation, cache cleanup, and migration. `latestAttempt` identifies the highest `(started sequence, envelope phase)` and its success/failure status, while `latestCompleted` identifies the highest sequence that published a complete run. `overview` shows a newer failed attempt before presenting an older completed run.

If a process disappears before terminal publication, later store recovery follows the exact crash table in Section 2.3. In particular, a renamed run directory without a terminal success attempt remains an unpointed orphan beside an `Interrupted` attempt and is never adopted as success. Process leases are operation-liveness records, not durable gate path leases or result-transport locks.

Completed run directories are never migrated in place. Compatible old run schemas are read through versioned adapters or a disposable derived index. Unsupported schemas are reported as incompatible, not corrupt or empty.

The sole public migration surface for this schema step is:

```text
lumin store migrate [--format json]
```

The command accepts at most one split-form `--format json` and no positional argument,
equals-form flag, repeated flag, or other option; JSON is the default and only format.
Malformed input exits `2` before state admission. The current binary supports only the
exact `lumin-lifecycle-store-header.v12 -> lumin-lifecycle-store-header.v13` step. It
does not initialize an absent store, infer a chain from any other old schema, downgrade a
newer schema, or let another command migrate on open. Every ordinary repository-state
command that sees a valid v12 header, a matching unfinished v12-to-v13 journal, or the
exact journal-proven canonical-absent exchange intermediate exits `1` with empty stdout
and exactly `lumin: lifecycle store migration requires 'lumin store migrate'\n`
on stderr. An unrecognized, invalid, foreign, or newer header remains an
`IncompatibleStateSchema` or integrity hard-stop without this claim of migratability.
Any noncanonical migration source or target without the matching durable journal is
foreign state and hard-stops; no command adopts or deletes it.

A repository root with no initialized `.lumin` namespace exits `1` with empty stdout and
exactly `lumin: lifecycle store is not initialized\n` on stderr; the command creates no
directory, marker, lock, store, journal, or other byte. A present namespace whose canonical
store is missing or whose bootstrap is incomplete is ordinarily an immutable integrity
hard-stop. The sole exception is the source-retired exchange intermediate: only
`lumin store migrate` may admit it, and only when the maximal valid unfinished journal
binds both exact objects and their observed private placements while `lifecycle.store` is
absent. Other commands may perform only the bounded no-follow identity/envelope check
needed to select that diagnostic; they never decode either private logical payload or
change the state.

On valid v12, `lumin store migrate` acquires the exclusive marker-bound lock, executes or
recovers the exact generation-fenced transformation below, exchanges the bound source and
target identities without disposing either, appends a terminal journal revision, reopens
the canonical store, and validates the complete v13 logical dump before success. The
retired v12 source and the complete journal are immutable migration provenance and remain
in `.lumin`; this command, retention, and cleanup never reclaim them. On an already-current
valid v13 store with either no journal (a native v13 repository) or one exact terminal
journal and retained source (a migrated repository), it validates the applicable state
without exchanging the backend or advancing its generation.

The migration journal is an immutable, append-only chain. Revision zero is the self-bound
`lifecycle-migration.json`; successor `n` is the self-bound
`lifecycle-migration.revision-<16-digit-n>.json`. A `MigrationIntentRevision` contains its
strictly increasing revision and phase, the exact predecessor name, physical identity, and
canonical payload SHA-256, immutable source/target generations and schemas, and the full
cumulative artifact bindings. The only current authority is the unique maximal contiguous
chain from revision zero. A gap, fork, duplicate sequence, invalid predecessor, unknown
suffix, or noncanonical revision name is foreign state and is preserved on hard-stop.

The initial revision binds the already opened canonical v12 source before publication. A
bound slot is `MigrationArtifactBinding { role, pre_exchange_name, post_exchange_name,
generation, schema, byte_sha256, logical_sha256, physical_identity,
link_count_at_publication: 1 }`: the source moves from `lifecycle.store` to its bound
private retirement name, while the target moves from its distinct bound private target
name to `lifecycle.store`. `byte_sha256` lets current-v13 admission authenticate the
retained v12 object as opaque bytes without decoding private-v1 logical records. The target
binding is durable before the held target becomes visible. No published revision or bound
source/target artifact is ever unlinked, overwritten, or otherwise physically disposed by
migration.

Each successor revision is prepared on a same-volume handle-owned unpublished object. Its
physical identity is captured before serialization; the complete candidate binds itself
and its exact predecessor, is flushed, and is linked or moved no-replace directly to its
unique final revision name. Immediately before and after that publication, migration
reopens the predecessor and requires its bound identity and payload digest; the predecessor
is never replaced or removed. Death before
publication disposes only the still-unpublished handle. Death after publication but before
the parent flush yields either no successor or one complete successor; recovery validates
the contiguous chain and resumes. Thus revision publication needs no occupied-name
replacement, no named staging entry, and no unbound survivor. The same handle-owned,
no-replace sequence publishes a bound target at its unique name. A visible unbound name,
extra link observed at admission, or name/role/generation/schema/digest/physical-identity
disagreement is preserved and hard-stops.

Source/target exchange uses only implementable no-disposition primitives. Linux uses
`renameat2(RENAME_EXCHANGE)` for the two held same-filesystem entries when supported.
Windows uses two handle-relative
`SetFileInformationByHandle(FileRenameInfo, ReplaceIfExists = false)` moves: source to the
absent bound retirement name, then target to the still-absent canonical name. A platform
fallback may use the same crash-recorded no-replace sequence. Every turn reopens entries
no-follow from the held state-directory handle and compares their physical identities and
link counts before and after the primitive. These checks detect substitution and aliasing;
they do not pretend to exclude a noncooperating same-UID process from calling `linkat`.
Because migration never disposes any published artifact, a hard link created in that race
cannot make Lumin delete a now-foreign object. An observed extra link or post-move identity
disagreement prevents terminalization and leaves the journal plus every object for
inspection or exact retry after the external corruption is removed.

The binding names are platform-exact rather than falsely uniform. For Linux atomic
exchange, `source.post_exchange_name == target.pre_exchange_name`; that one private slot
changes from the v13 target identity to the retained v12 source identity. For the Windows
two-move protocol, source retirement and target staging are distinct bound names, and the
only canonical-absent placement has the source at the former and target at the latter.

The terminal journal revision binds the post-exchange canonical v13 target and retained
private v12 source. Successful migration and every later ordinary v13 admission validate
the terminal chain, both physical identities, current link-count policy, the target's full
logical identity, and the retired source's opaque byte hash. A concurrent mutation after a
validation is ordinary external state corruption and is caught on the next admission; no
advisory lock is described as mandatory authority over a noncooperating process. A future
reclamation protocol may remove the retained source or journal only after a separately
reviewed platform capability proves safe final disposition. Until then, preservation is
the fail-closed behavior.
Concurrent migration commands serialize on the same exclusive lock: at most one advances
the generation, and every follower validates v13 and emits the same response.
Any schema-shape, identity, referential, generation, durability, or I/O failure exits `1`,
leaves stdout empty, emits the ordinary `lumin:` diagnostic when stderr remains writable,
and follows the crash table below; no success is emitted while the journal is nonterminal
or its exact terminal provenance fails validation.

Success exits `0`, writes one `lumin.lifecycle-store-migration.v1` object whose only
fields in canonical order are `schemaVersion`, `storeSchema`, and `status`, with
`storeSchema: "lumin-lifecycle-store-header.v13"` and `status: "ready"`, then writes the
normal newline and nothing to stderr. The response deliberately omits source schema,
generation, and a changed/no-op bit, so the first successful migration, recovery after a
post-exchange process death, and every already-current retry return identical bytes.
This command has no operation ID or `operation show` variant: its sole target schema and
singleton append-only migration journal make the state transition intrinsically idempotent,
and it performs no new gate, retention, or cache-cleanup operation; its only record
transformation is the exact frozen v12-to-v13 schema mapping.
Every backend handle and exclusive guard is released before transport. A `BrokenPipe`
exits `1` with empty stderr; another stdout write/flush failure exits `1` and emits exactly
`lumin: cannot write stdout\n` when stderr is writable. Either failure leaves the validated
v13 store authoritative, and rerunning the same command returns the identical success
object without another exchange or generation advance.

Whole `lifecycle.store` migration is a generation-fenced copy-on-write protocol. The marker-bound, one-link `.lumin/lifecycle.lock`, exact four-kind top-level managed-parent/anchor set, and exact nested quarantine binding are outside the exchangeable backend and are never themselves exchanged. Every ordinary store transaction proves the complete namespace binding, acquires the shared side, opens the current backend only after acquiring it, validates the backend `StoreGeneration` plus global/parent/quarantine binding, closes the backend handle before releasing the lock, and retains at most a generation token across analysis. No backend handle crosses a transaction boundary. `lumin store migrate` acquires the exclusive side through the same complete proof, so exchange begins only after every old-generation handle has closed; a replaced lock, state directory, parent, nested quarantine, or anchor hard-stops before either generation can commit.

Migration follows one sequence:

1. Under the exclusive lock, open canonical v12 `lifecycle.store` no-follow, require one
   link, and derive SHA-256 over its complete bytes, complete logical dump, and the exact
   transformed-target logical dump. The source binding records that held object's
   canonical pre-exchange name and private post-exchange name. Prepare revision zero on a
   same-volume `IntentPublicationHandle` whose unpublished object is automatically removed
   on close. Linux uses `O_TMPFILE` plus no-replace `linkat`; Windows uses a delete-on-close
   handle plus a no-replace final-name publication with equivalent lifetime semantics.
   Revalidate the canonical source, write and flush the whole revision, publish that held
   object no-replace as `lifecycle-migration.json`, reopen its self-bound identity, and
   flush `.lumin`. There is no admissible named `.pending` intent. A live nonterminal
   journal blocks every mutation except migration recovery; existing durable operations,
   reservations, active gates, and process leases remain records to migrate rather than
   being expired.
2. Build and flush the generation `N+1` target through a handle-owned unpublished object.
   Create the next immutable journal revision binding that exact target identity and
   unique private name, flush it, publish it no-replace at its deterministic revision name,
   reopen it against revision zero, and flush `.lumin`. Then publish the same held target
   object no-replace at its bound name, reopen its identity, and flush `.lumin`. Preserve
   and validate attempt/catalog sequences, operation IDs/results, cache-cleanup
   authorization manifests, reservations, worktree transitions/capsules/references,
   retention plans/tombstones, pins, gate IDs/revisions, and history. A crash after the
   binding revision but before target publication leaves a typed missing-bound-target
   recovery state, not an unbound named file.
3. Compare the complete canonical logical dump and referential closure, revalidate the
   complete journal chain and every bound entry, then close every backend handle that is
   not required for exchange and durably flush the target and its parent. Failed validation
   leaves `N` authoritative and the journal recoverable; it cannot publish a partial target
   as canonical.
4. Reopen and revalidate the journal head, canonical source, and private target by exact
   physical identity, one-link state, schema, and digest. Use Linux
   `renameat2(RENAME_EXCHANGE)` or Windows handle-relative
   `SetFileInformationByHandle(FileRenameInfo, ReplaceIfExists = false)` no-replace moves
   so the target becomes `lifecycle.store`
   and the source becomes its distinct bound retirement name. No permitted turn disposes
   an object. A multi-move implementation accepts only the exact pre-exchange,
   source-retired/canonical-absent, or post-exchange placement proven by both identities.
   A pathname overwrite, remove-then-rename, or move that can discard a winner is
   forbidden. Reopen all affected names, verify identities and current link counts, and
   durably flush `.lumin`. A mismatch hard-stops with all objects retained; if an atomic
   exchange moved a racing substitute, that substitute remains at the other bound reserved
   name and is never cleanup authority.
5. Authenticate the canonical v13 target and now-private v12 source against the
   post-exchange bindings, then append and flush one terminal journal revision that records
   that placement. Reopen the full chain and both objects once more before success. The
   terminal journal and retired source remain immutable reserved provenance for the
   repository lifetime; neither is deleted, renamed, retention-eligible, nor a cleanup
   candidate.

The private cleanup-operation v1-to-v2 transformation in Section 2.5 is part of the lifecycle-store v12-to-v13 step 2 canonical logical copy. It maps only validated legacy shapes to the exact synthetic delivery state defined there, includes that state in the target logical dump, and validates the resulting v2 record before step 3. Every committed legacy status initially projects v2 `unknown`: `not-attempted` becomes one unfinished allocation, while `succeeded`/`failed` retains its historical completion below a distinct unfinished greatest allocation. An invalid or unrecognized legacy shape fails `IncompatibleStateSchema` while generation `N` remains authoritative.

A process that analyzed while holding generation token `N` must reacquire the shared lock and reopen the canonical store before its next transaction. If it observes `N+1`, it must revalidate its operation, gate revision, catalog/transition revision, and freshness against the new handle before continuing; it may reopen/retry under the same operation ID or return a typed generation-change failure, but it cannot commit through the stale token. A read transaction already holding the shared lock finishes and closes before migration can acquire exclusivity; new reads wait for live migration/recovery and never continue through an old handle during exchange. Logical query scopes and cursors survive migration only when their schema, scope identity, ordering version, and referenced logical records are unchanged; no cursor contains a physical backend generation.

Every migration crash point has one recovery rule:

| Crash point | Canonical recovery |
| --- | --- |
| while preparing any unpublished journal revision, before its unique final-name publication | the existing contiguous chain remains authoritative; closing or process death disposes the handle-owned candidate, no named staging entry exists, and migration resumes from the previous head |
| after a complete revision's no-replace publication but before its parent flush | recovery observes either the previous chain or that chain plus one complete self-bound successor; a partial revision, fork, gap, or named staging entry is foreign and no predecessor is removed |
| after a target-binding revision and before a validated target | `N` remains authoritative; an unpublished target dies with its handle, an exact bound missing name is superseded only by a later immutable revision, and a visible bound target is revalidated by physical identity before recovery resumes |
| after validated target and before the exchange protocol | `N` remains authoritative; recovery revalidates the journal head, canonical source, and private target before resuming |
| during exchange before the parent flush | migration alone admits an exact identity-proven pre-exchange placement (`N` canonical), source-retired intermediate (canonical absent, source at its retirement name, target still at its private name), or post-exchange placement (`N+1` canonical and `N` private); it completes the no-disposition protocol from that phase, while any other missing/invalid/substituted name is an integrity hard-stop |
| after durable exchange flush and before terminal-revision publication | `N+1` is authoritative only to migration recovery; ordinary commands still route to migration, which authenticates the retained source and canonical target and appends the terminal revision |
| after terminal-revision publication but before its parent flush | recovery observes either the nonterminal head and resumes terminalization or the complete terminal chain; both objects remain present and no cleanup or removal is attempted |
| after a durable terminal revision or migration output failure | current v13 admission validates the terminal chain, canonical target, and retained opaque v12 source; retry returns the same DTO without exchange, journal append, deletion, or generation advance |

The public fault proof includes a matching durable journal whose private source has a
valid no-follow envelope and v12 header but a corrupted private-v1 logical table. An
ordinary repository-state child must return the exact migration-required diagnostic
without opening that table or changing any byte. A following `lumin store migrate` child
must discover the corruption only under the exclusive migration reader, exit `1` with an
integrity diagnostic, and leave the canonical store, journal, artifact, and generation
unchanged. The proof also places a byte-identical valid v12 source at the private name
without a matching journal and requires every command, including migration, to hard-stop
without deleting or adopting it. Exact publication barriers kill public children before
the first byte, after a partial write, after candidate flush, after each unique final-name
publication, after reopen, and before/during the parent flush for revision zero, a target-
binding successor, and the terminal successor. Restart proves no staging name survives
and observes exactly the previous contiguous chain or that chain plus one complete next
revision; every published predecessor remains byte-for-byte present.

Separate live-journal barriers replace the canonical source and private target with
logically byte-identical one-link objects before exchange, and replace revision zero or
the current predecessor immediately before successor publication. Each mismatch must
hard-stop before the next operation, preserve the original and substitute objects plus
every journal revision, and leave all logical records unchanged. The proof also substitutes
canonical v12 `lifecycle.store` after logical validation but before exchange and requires
the same no-deletion outcome. A source-retired/canonical-absent fixture with both exact
journal-bound objects proves that ordinary commands return the migration-required route
and migration alone resumes; a superficially similar missing-canonical namespace without
those bindings remains an immutable integrity hard-stop.

An exact race barrier stops after the pre-exchange one-link validation while a competing
public child creates a hard link in another same-filesystem directory. The migration child
remains held until that link is visible, then resumes through its post-exchange and
pre-terminal validation. Linux does not pretend to block `linkat`: the fixture proves the
real `renameat2(RENAME_EXCHANGE)` path never unlinks either object, rejects terminal
success on the extra link, and retains the source, target, journal, and new link on
hard-stop. Windows proves the corresponding two-handle-relative-no-replace-move recovery
path. A never-initialized
repository runs `lumin store migrate`, receives the exact not-initialized diagnostic, and
remains entry-for-entry and byte-for-byte unchanged with no `.lumin` creation. Windows and
Linux package probes execute their actual handle-owned append-only publication and
no-disposition exchange primitives plus canonical-absent recovery and absent-store refusal
rather than accepting a development-only emulation. `lumin-xtask package-check skills`
proves that both packaged Codex and Claude adapters respond to the migration-required
diagnostic only by invoking the public `lumin store migrate` command, accepting the exact
target-only DTO, and retrying the original public command; neither adapter reads store
internals, synthesizes the DTO, or embeds migration logic.

## 12. Security and Integrity

- Repository roots and planned paths are losslessly lowered to `RepositoryRootIdentity`/`repo-path.v1`, then physically canonicalized for containment without rewriting their lexical bytes.
- `.lumin` and every lexical/physical alias or descendant are a reserved product state namespace admitted only by the no-follow Section 2.0 protocol; they cannot be scan entries or gate writes.
- The immutable lock bootstrap must match the global repository/root/state-directory/lock/namespace binding. `repository.json` and `lifecycle.store` must additionally carry the same exact kind-ordered `ManagedStateParentBinding` set, and every immutable parent-anchor header must cross-bind its own tuple to those global values. Foreign, copied, redirected, replaced, multiply linked, or externally mutated state hard-stops without adoption or source-delta fallback.
- A caller-declared path that resolves outside the root is malformed input and creates no operation or gate record.
- Any repository-owned configuration field declared as a repository path that lexically or physically resolves outside the root is malformed configuration and cannot publish a completed run or authorizing gate; a root-contained missing/excluded entry is typed incomplete evidence instead.
- Unsupported external configuration semantics are scoped incomplete evidence and never authorize an undeclared source/config read outside the root.
- If an admitted existing path's alias/symlink identity later resolves outside the root, baseline identity drift is `Stale`; if a planned new/final path is observed outside the root at close, the containment-invariant signal is `Block`/`Deny`.
- Each resolved existing prefix's comparison behavior and physical identity, plus the fallback root policy, are persisted with each gate.
- Store writes use validated typed operations; raw backend queries do not cross `lumin-store`.
- The immutable one-link store lock lives under the verified repository-owned `.lumin` directory, not a shared global temp path. Every acquisition proves that the root/state entries still name the marker-bound directory/lock objects. Latest publication/recovery, retention confirmation, cache cleanup, and migration use the exclusive side; ordinary transaction-scoped store access uses the shared side.
- Published run envelopes contain evidence-store hashes.
- Run publication and latest-pointer replacement follow the crash-consistent, durable, exclusively serialized sequence-merge protocol in Section 2.3.
- A gate cannot close under a different repository identity.
- An operation ID is repository-scoped and bound to one canonical request digest; conflicting reuse is malformed.
- Incompatible lifecycle schemas fail closed with a concise recovery instruction.

## 13. Acceptance Criteria

1. A complete default audit creates only the repository state marker, lifecycle lock/store, four immutable top-level managed-parent anchors, the immutable nested cache-quarantine anchor, small attempt/run envelopes, latest pointer, and canonical evidence store; the migration intent exists only while migration is live.
2. An agent can answer a focused finding question without opening either file directly.
3. Every bounded response, including binary- and run-scoped capabilities, supports explicit continuation.
4. Projection limits, source-role policy, and finding disposition cannot change canonical counts or hide a grounded finding from an unfiltered default query; only an explicit echoed caller filter may narrow a collection.
5. A failed required capability is prominent in `overview`.
6. Pre-write and post-write require no intent JSON or temporary transport file.
7. Post-write needs only the explicit gate ID, a caller-retained operation ID, and repository context; it never resends intent or baseline.
8. Active write/write and write/read conflicts are rejected atomically.
9. Transactions with nonconflicting leased-write and semantic-read sets can analyze concurrently; closes serialize through exact intervening transitions, and an in-flight unexplained edit cannot be approved.
10. Mixed-language changes remain one user-visible gate with language-owned internal lanes.
11. A stale or incompatible gate cannot be interpreted as passed.
12. Dependency checks use the nearest owner manifest for the planned paths.
13. Completed gate evidence remains queryable after process exit.
14. No storage transaction lock, `CatalogPublicationGuard`, or operation-liveness lease is held while stdout or a result projection is transported; an active gate's durable logical path lease remains, and no separate scan lock exists.
15. Architecture v1 selects exactly one store backend only after the correctness probes and measured comparison pass.
16. `latestAttempt` exposes a newer failure while `latestCompleted` preserves the last complete run.
17. Post-write cannot run without an explicit gate ID.
18. Existing aliases, directory descendants, new paths, and both sides of a rename obey the path identity contract; declaring one existing alias leases and reanalyzes the complete admitted physical-alias closure, while unleased topology changes cannot authorize close.
19. Gate decisions, machine output, and process exit codes follow the stable decision table.
20. Nested evidence and relation lists cannot bypass bounded query envelopes.
21. `lumin store migrate` is the only v12 admission route, refuses absent state without creating it, returns the same bounded `ready` response after migration or retry, and publishes no crash-surviving pending name. It binds canonical source and private target by byte/logical and physical identity, publishes each intent successor at a unique append-only name without replacing its predecessor, and exchanges source and target through an implementable no-disposition primitive. The exact terminal journal and retired v12 source remain immutable provenance; the exact journal-proven canonical-absent intermediate is migration-recoverable, while every unexplained missing store or unbound/substituted/no-journal artifact is foreign. Migration cannot rewrite completed logical evidence in place, erase active gate history, let an old-generation handle commit after exchange, or turn a racing hard link into deletion; it maps private cleanup-operation v1 records only through the exact fail-closed synthetic delivery state in Section 2.5 and is exercised through store-crash fixtures, both shipped platform packages, and both packaged skill adapters.
22. Every run query is pinned to one immutable run, and every nested page can be requested explicitly without following latest.
23. `AnalysisContractId` compatibility cannot be invalidated merely by a different `AnalysisInputId`.
24. Pre-write rejection owns no lease; failed post-write remains active with an immutable attempted revision.
25. Post-write drift during analysis cannot authorize or release a gate.
26. Every publication crash point recovers according to Section 2.3 without a dangling pointer becoming clean evidence.
27. Retrying any mutating gate, retention, or cache-cleanup command with the same operation ID/request returns the same committed result without duplicating a revision, plan, cache authorization, or physical move; conflicting reuse is malformed, and a cleanup delivery attempt without durable completion—including the synthetic greatest attempt of every migrated committed private-v1 row—is projected as `unknown`, never `not-attempted`, success, or failure.
28. Shared-worktree changes are approved only when this gate or an exact immutable intervening transition explains every observed identity change, and referenced transition capsules remain available while an active gate may need them.
29. Retention cannot delete either latest-pointer target or break its attempt/run linkage.
30. Open and close observations are derived only after owner-reported semantic inputs reach a fixed point; every added read is demanded, conflict-checked, and reserved before inventory captures it or an owner/cache validator consumes it.
31. Gate abandon, run pin/unpin, prune-plan creation/confirmation, and cache cleanup recover committed results by operation ID after delivery failure; lower delivery completion cannot mask a greater unfinished attempt, completion of the greatest allocated attempt deterministically establishes its public result, and a later lower completion appends private evidence without changing that public projection.
32. Every retention deletion crash point recovers to exactly one `Prepared`, `Pruning`, or `Pruned` truth and never exposes missing payload as clean deletion.
33. Validated warm cache replay preserves the same owner outcome/capability state, diagnostics, payload, limitations, semantic inputs, gate-neutral signals, request-specific effects, observation binding, and semantic dump as cold execution.
34. A nonauthorizing closure failure persists typed `Unsealed` evidence and never invents a baseline or close observation ID.
35. Every public collection has one versioned canonical ordering, and an immutable retention-plan cursor survives unrelated repository mutations without crossing plan identity.
36. Known `Pruning` and `Pruned` records remain publicly distinguishable from never-existing IDs through plan, operation, and direct-record lookup.
37. Post-write reuses the exact caller-supplied opening scan/entry/profile override tier, rejects replacement parameters, and recomputes config-derived effective values only from validated self-writable inputs.
38. Lifecycle effects consume one exhaustive owner-produced total delta relation over identity, targets, affected domain, confidence, grounding, and evidence rather than static limitation rows.
39. Independent `PinId` and active-gate transition references protect one another, minimal tombstones remain auditable, and the public generation-fenced lifecycle-store migration command preserves the complete logical catalog while ordinary repository-state commands never migrate on open.
40. Concurrent latest publishers, recovery, retention confirmation, cache cleanup, and migration serialize through one marker-bound exclusive guard; field-wise monotonic keys cannot regress, strand a terminal attempt behind `Running`, lose an independent pointer update, duplicate a cache move, or split across replacement lock objects.
41. Every repository path/root, machine DTO, NUL-stream input, stable ID, ordering key, cursor, and gate set reproduces the exact checked-in path-codec vectors and rejects noncanonical encodings; logical sources and alias-write closure survive physical aliases while payload reuse cannot erase package/config/role context.
42. `.lumin` and every alias/descendant are no-follow reserved state; state-directory/lifecycle-lock identities, the exact four-kind top-level managed-parent set, and the exact nested cache-quarantine directory/anchor/nonce binding are durably bound and revalidated. Foreign identity, copied/replaced state, redirected parents, caller writes, unauthorized self-consistent quarantine, substitution, or external mutation fails before scan evidence, gate success, or cleanup success. Cleanup commits store-owned authorization before any no-replace move, continuously blocks every other active-cache writer through `pending -> interrupted -> pending`, flushes and remanifests each complete tree plus cache/quarantine/trash directories before validation/result commit, recovers only through the matching operation ID, and never performs pathname-based physical deletion.
