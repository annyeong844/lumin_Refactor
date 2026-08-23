# REVIEW-004: Cache Cleanup Protocol Amendment

Document role: focused Architecture v1 amendment and independent-review record

Status: post-merge follow-up candidate awaits independent PASS

Date: 2026-08-24

Owners: PRODUCT-000 Sections 2.8 and 2.9, ARCH-000 Section 8, ARCH-002 Sections 2, 2.5, and 11, SLICE-001 Sections 6, 7, 9, 11, 14, and 15

## Trigger

The merged cache-cleanup increment mapped `reserved-state-namespace` while its owner
documents specified only that cache payload descendants were disposable and the cache
anchor must survive. The implementation itself supplied the command grammar, response
schema, exit behavior, and result-delivery behavior. That reversed the required truth
direction: the acceptance test could prove its own implementation-authored protocol.

An independent review also supplied a concrete deletion counterexample. After a payload
was validated and its handle dropped, a concurrent actor could replace its pathname with
`namespace.anchor` or an unvalidated tree before pathname-based removal. Cleanup could
therefore destroy the immutable anchor or traverse bytes that had never passed admission,
then detect the integrity failure only after damage.

These are architecture findings, not test-only omissions. They reopen only ARCH-000's
cache-cleanup command registration and the cache-cleanup portion of the frozen
state-namespace contract.

## First Review Result

Independent review bound exact candidate
`8bb25bf61d3840d8a8e902ac826f710cf1ab4a17` and returned `REOPEN`. It found that the
single top-level barrier did not exercise recursive child claims, the traceability row
routed the standard/determinism cleanup fixture only through an inapplicable store-crash
lane, the cleanup-owned destination name recreated the final validation-to-unlink race,
and the Slice adapter/AC wording still required operation IDs for every mutation. The
replacement decision removed online physical deletion.

## Second Review Result

Independent review bound exact candidate
`b9c26e6c3f81dec267b4db0056a74f025bc53ae6` and returned `REOPEN`. It found that an
empty-cache retry could report clean without admitting malformed pre-existing quarantine,
process-death recovery had no public child-process acceptance fixture, first creation of
`trash/cache-evictions` was not explicitly durable in its trash parent, and stdout stream
failure left stderr behavior platform-dependent.

## Third Review Result

Independent review bound exact candidate
`9be3fe613bac12162eabd32c6a6ca59b648be7a0` and returned `REOPEN`. It found that a
process death after a racing substitute was moved but before comparison discarded the
only expected manifest, allowing retry to admit the substitute as prior quarantine. It
also found that ARCH-000's canonical command set did not authorize `lumin cache clean`.
The next candidate added the command to ARCH-000 and carried a recomputable manifest
digest in each quarantine child name.

## Fourth Review Result

Independent review bound exact candidate
`c6af1a6a1c511ad36bd6453daf1e660ca6ebe5b9` and returned `REOPEN`. It found three
remaining authority and durability gaps. First, a public name grammar plus an unkeyed
digest proves self-consistency, not that Lumin authorized the moved object; an external
actor could manufacture an admissible quarantine tree. Second, retry could observe a
visible interrupted rename and report clean without durably flushing the recovered cache,
quarantine, and owning trash directory entries. Third, parent-directory flushes did not
make dirty regular-file contents or descendant directory entries durable, so a successful
manifest-bearing move could become permanently unverifiable after power loss. The prior
no-operation-ID decision is withdrawn. The replacement decision makes one canonical
operation record the pre-move authority and binds success to bottom-up tree durability.

## Fifth Review Result

Independent review bound exact candidate
`8dc487ae3bab8a7705a92e57ba240586523c2f50` and returned `REOPEN`. It found three
remaining consistency and recovery gaps. First, the newly added cleanup/publication race
raised the store-crash registry to eleven rows while PLAN-001 still allowed a ten-row
exit. Second, the lock header was correctly defined and initialized as global bootstrap
only, but a later sentence incorrectly required it to carry the nested quarantine
binding, making every fresh repository incompatible on reopen. Third, the cleanup
projection exposed `interrupted` and `interruptionCount` without assigning the exact
liveness proof, transition owner, transaction boundary, or read-only `operation show`
behavior. The replacement decision counts the eleventh crash obligation, keeps parent
bindings in marker/store headers only, and makes same-ID mutating retry the sole owner of
an idempotent pending/interrupted/pending recovery transition.

## Sixth Review Result

Independent review bound exact candidate
`0db2bf15d2861157952a106123d995c18b358de7` and reported no major issues. The
normalized REVIEW-004 verdict is `PASS`: no remaining P1/P2 gap was found in the
eleven-row crash ledger, global-only lock bootstrap, marker/store nested binding,
read-only projection, or guarded pending/interrupted/pending recovery protocol. This
result section and the Workboard status are record-only follow-up; they do not change the
reviewed PRODUCT-000, ARCH-000, ARCH-002, SLICE-001, or PLAN-001 owner bytes. Owner merge
is still required before Rust implementation begins.

## Seventh Review Result

Independent review bound exact candidate
`1abd3d7e58e0e9ef127d7a3b3cd80ca18aaa7928` and returned `REOPEN`. It found that a
delivery attempt could commit its private sequence, write some or all stdout, and die
before recording completion while the public projection still claimed
`lastDeliveryStatus: "not-attempted"`. The same review found four owner-consistency gaps:
the Workboard still called ARCH-002 frozen and copied merge history into its registry,
the old sixth-review `PASS` appeared to cover the expanded checklist, and the formal
Slice corpus truth, AC 17/36, and traceability rows did not require either reverse-order
delivery completion or continuous cache-writer rejection. The replacement decision adds
the public `unknown` state for every allocated but unfinished greatest delivery attempt,
makes the registry reflect the reopened owner without a chronicle, scopes the old `PASS`
to its exact candidate, and maps both counterexamples plus the new uncertainty boundary
into executable acceptance.

## Post-Merge Follow-up Review

The approved owner bytes merged in `e988a85`. Delayed independent review then supplied
concrete counterexamples that were not covered by the sixth review. Concurrent
identical retries could complete delivery metadata in backend commit order because no
pre-transport sequence chose the `lastDeliveryStatus` winner. A cleanup process could
also die while `pending` before the later retry installed the documented recovery
reservation, leaving a window in which a normal active-cache writer could invalidate
the immutable plan. The first follow-up candidate then left an allocated delivery whose
completion was missing indistinguishable from a transport that had never started.
REVIEW-004 is therefore narrowly reopened. The replacement decision allocates a durable
delivery sequence before each transport, projects the greatest unfinished attempt as
`unknown`, and lets only completion of the greatest allocated sequence select
`succeeded` or `failed`. It also installs one continuous active-cache
mutation reservation with operation creation and retains it through
`pending -> interrupted -> pending` until result commit. No other command, namespace,
quarantine, manifest, or response field changes. The exact follow-up candidate requires
an independent `PASS` before its Rust implementation may resume.

## Eighth Review Result

Independent review bound exact candidate
`b927cda1acf891c50528e50cae6a723537effb9c` and returned `REOPEN`. It found three
remaining contract and routing gaps. First, acceptance forced a lower completion while
a greater delivery attempt remained unfinished but did not force the converse ordering,
so a late lower completion could still overwrite the greatest attempt's final status.
Second, adding `unknown` to `lumin.cache-cleanup-operation.v1` changed a frozen public
enum without changing its schema identity. Third, the Workboard reopened ARCH-002 while
its Phase Ledger still represented all of Phase 0 as frozen. The replacement decision
forces both completion orders and asserts the unchanged durable snapshot after the late
lower completion, advances the cleanup-operation projection to v2 with no v1 fallback,
and marks only the REVIEW-004 portion of Phase 0 narrowly reopened.

## Ninth Review Result

Independent review bound exact candidate
`ef6932da7c4968c7aa899c85de84f161d700cd20` and returned `REOPEN`. It found two
remaining migration and proof contradictions. First, a committed private-v1 cleanup
record with legacy `not-attempted` may represent a process that wrote stdout and died
before recording delivery, so direct v2 projection would falsely claim that no sequence
was allocated. Second, the greater-first proof required the complete durable operation
snapshot to remain byte-identical after a held lower completion even though the contract
also required that lower completion to append private evidence. The replacement decision
maps validated private-v1 records to exact synthetic v2 sequence state, with ambiguous
committed `not-attempted` becoming `unknown`, and distinguishes the byte-identical public
v2 projection from the private ledger's one permitted lower-completion append.

## Tenth Review Result

Independent review bound exact candidate
`a0119ba72cf79d13ddeb54641e487efadb205c2f` and returned `REOPEN`. The candidate made
v12 an explicit incompatibility boundary and referred users to a migration command, but
neither the canonical command set nor the public slice defined such a command's grammar,
response, failure behavior, or delivery recovery. The replacement decision registers
`lumin store migrate` in ARCH-000 and defines it as the sole public v12 reader, with one
target-only byte-stable response, exact ordinary-command recovery instruction, exclusive
generation-fenced recovery, and no second replacement on an already-current retry.

## Eleventh Review Result

Independent review bound exact candidate
`f28cc45debe801f33f8efaafbe40e062e6d4e983` and returned `REOPEN`. It found three
remaining recovery and distribution gaps. First, even a committed private-v1 row whose
stored delivery status is `succeeded` or `failed` may have a newer same-result retry
already transporting after all store and liveness locks were released; mapping that
legacy status to the greatest completed v2 sequence falsely made delivery final. Second,
the acceptance trace assigned the new public migration command only to the store-crash
development lane, so neither shipped Windows nor Linux binary had to contain or execute
it. Third, a crash after intent removal but before private-source cleanup leaves valid
v13 plus a store-owned source artifact, yet ordinary-command routing named only v12 and
live-intent states. The replacement decision gives every committed private-v1 row an
unfinished synthetic greatest sequence and therefore public `unknown`, assigns migration
admission/success/recovery/retry probes to both packaged platforms, and defines the sole
bounded no-intent source envelope/header that routes back to `lumin store migrate`, which
alone authenticates its private-v1 payload, while all foreign remnants integrity
hard-stop before deletion.

## Twelfth Review Result

Independent review bound exact candidate
`b9a1e679c5ba0059eecc31ca0d2e02498c10f9ea` and returned `REOPEN`. It found two
remaining provenance and acceptance gaps. First, a valid envelope/header with a corrupt
private-v1 payload was routed to migration, but no public fixture proved that an ordinary
command returns the migration-required diagnostic without opening that payload before
`lumin store migrate` alone detects corruption. Second, after intent removal a
byte-identical copy of the genuine v12 source could satisfy every proposed remnant check;
without surviving store-owned provenance, migration could delete externally introduced
reserved state. The replacement protocol keeps the digest-binding `MigrationIntent`
durable until all authorized private artifacts are deleted and their absence is flushed,
then removes the intent as the final mutation. A no-intent private artifact is therefore
always foreign. Public children now prove the header-valid/payload-corrupt live-intent
split and the no-intent byte-identical-copy hard-stop without deletion.

## Thirteenth Review Result

Independent review bound exact candidate
`361172f03d9e969440f385586648355c20b27842` and returned `REOPEN`. It found two
remaining physical-authority and publication-crash gaps. First, the durable intent bound
private names and logical digests but not physical identity or link count, so a logically
identical source/target substitution before cleanup could be mistaken for the authorized
object and deleted. Second, death while preparing or renaming
`lifecycle-migration.json` could leave an incomplete canonical file or named pending file
without a durable intent, contradicting both the crash table and the rule that no-intent
reserved state is never deleted. The replacement protocol self-binds each canonical
intent revision, durably binds every private artifact's identity and one-link state before
its name becomes visible, and permits only final disposition bound to the opened object.
Intent preparation uses a handle-owned automatically disposed object with no admissible
pending name, so every pre-flush death yields either no intent or one complete canonical
intent. Store-crash and both packaged platforms now own the exact publication,
substitution, multiple-link, and held-object-disposition proof.

## Fourteenth Review Result

Independent review bound exact candidate
`4f51863247fa8c13c69df2e5ad6d6d104464bb4d` and returned `REOPEN`. It found one
remaining physical-authorization race and four acceptance/ownership gaps. First, an
external actor could add a hard link after the one-link check but before disposition;
entry identity alone did not make link-count validation atomic. Second, source/target
substitution fixtures omitted the self-bound canonical intent before revision update and
terminal removal. Third, canonical v12 `lifecycle.store` itself was not physically bound
at pathname replacement, so a foreign winner could be deleted. Fourth, neither packaged
skill adapter was assigned the public migration recovery workflow. Fifth, the stated
absent-store refusal had no public fixture.

The replacement protocol binds canonical source and private target identities and uses an
identity-preserving exchange/replace-with-backup primitive or isolated crash-recoverable
no-replace moves rather than pathname overwrite. Every intent
mutation, exchange, and disposition requires an operating-system isolation authority that
fences both entry substitution and new hard links from final validation through reopen or
absence proof and parent flush; lack of that authority rejects migration before mutation.
Exact public-child barriers now cover canonical-source and intent substitution, the
post-check hard-link attempt, and absent-store immutability. Both shipped platforms must
execute the real isolation/exchange/disposition primitives, while both packaged Codex and
Claude adapters must invoke only the public migration command and consume its DTO.

## Fifteenth Review Result

Independent review bound exact candidate
`c05dc1d5011f7c2d476584e3cc1d3dda70d43938` and returned `REOPEN`. It found three
remaining feasibility and recovery contradictions. First, the proposed
`MigrationNamespaceIsolation` required an unprivileged Linux process to exclude a
noncooperating same-UID `linkat` across the filesystem, while the Linux package contract
also required migration to succeed; that authority does not exist. Second, replacing an
occupied `lifecycle-migration.json` during an intent revision could leave an unbound
staging object or erase the only durable authority at a crash boundary not covered by the
table. Third, the generic missing-canonical admission rule rejected the exact
source-retired/canonical-absent exchange state that the recovery protocol required.

The replacement protocol performs no physical cleanup. Linux uses
`renameat2(RENAME_EXCHANGE)` and Windows uses handle-relative
`SetFileInformationByHandle(FileRenameInfo, ReplaceIfExists = false)` moves; every
published object survives the exchange. The
v12 source remains permanently bound by a terminal append-only migration journal, so a
racing hard link can trigger fail-closed validation but can never make Lumin delete a
foreign object. Revision zero and every successor are distinct immutable self-bound files
published no-replace at unique names and chained to the exact predecessor; no revision
update overwrites or removes authority. The migration command alone admits the exact
journal-proven canonical-absent placement, while an unexplained missing store remains an
immutable integrity hard-stop. Exact public-child barriers cover every initial/successor
publication boundary, the canonical-absent split, and the real platform hard-link race.

## Sixteenth Review Result

Independent review bound exact candidate
`90eeb46584f03b209800aa99f5fe0a5c70c14004` and returned `REOPEN`. It found four
remaining provenance and recovery gaps. First, the terminal binding required every later
v13 admission to match the migration-time target logical hash, so the first legitimate
gate/cache/retention mutation would make the next command reject its own store. Second,
the output-layout acceptance still permitted migration artifacts only while migration was
live even though the protocol permanently retained the terminal journal and v12 source.
Third, a target-binding revision could survive while its unpublished target died, but the
cumulative binding model gave no typed way to supersede that absent historical object.
Fourth, revision zero was self-bound without any authority outside its own payload, so a
foreign actor could manufacture an apparently resumable root.

The replacement decision commits an append-only root authorization inside canonical v12
before revision-zero publication, then finalizes that root against the post-authorization
source. That authorization also fixes an immutable migrated-v13 target-header
`MigrationProvenanceAnchor` to the root and source; its framing excludes the later target
digest from the root core, so the cross-binding is acyclic. The journal folds a closed
role-specific event machine: only a proven-dead absent
pending target may become `SupersededUnpublished`; a visible target becomes `Published`,
and published identities cannot be superseded. Terminalization authenticates the complete
migration-time target once and transitions it to `CanonicalMutable`; later admission checks
the exact immutable target-header anchor and ordinary current-v13 integrity instead of the
historical payload digest. Native-v13 and migrated-v13 output layouts now have separate
exact truths, with only the latter requiring the authorized terminal journal and retired
source.

## Decision

The owner amendments define the cleanup command and the sole public store-upgrade route:

```text
lumin cache clean --operation-id <operation-id> [--format json]
lumin store migrate [--format json]
```

Exactly one split-form operation ID is required. At most one split-form `--format json`
is accepted; equals forms, other options, and positional arguments are malformed. The
successful response is `lumin.cache-cleanup.v2` with only `schemaVersion`, `operationId`,
`requestDigest`, and `status: "clean"` in canonical order. Malformed input exits `2`.
Integrity, persistence, and delivery failures exit `1`; a pre-transport failure leaves
stdout empty. `BrokenPipe` leaves stderr empty, while another stdout write/flush failure
emits exactly `lumin: cannot write stdout\n` when stderr remains writable. No storage
transaction, publication guard, or operation-liveness lease remains held during output.
Delivery recovery uses the same operation ID or the bounded
`lumin.cache-cleanup-operation.v2` projection from `lumin operation show`, never a new
whole-command mutation. That projection exposes only its operation identity/kind/digest,
status and interruption count, authorized/validated counts, stored result, and last
delivery status; it never embeds unbounded manifests. Show is strictly read-only and
does not prove process liveness or change a record. The amended binary always emits the
v2 projection and provides no v1 negotiation or fallback. Frozen public v1 never
contains `unknown`; a v1-only client observes the distinct v2 `schemaVersion` and rejects
it as unsupported before interpreting the delivery enum. The private record schema also
advances to `lumin-cache-cleanup-operation.v2`. Explicit lifecycle migration retains
zero delivery sequences for valid pending/interrupted private-v1 rows, maps a committed
legacy `not-attempted` row to synthetic allocated sequence 1 without completion and thus
public `unknown`, and maps committed `succeeded`/`failed` to historical completed
sequence 1 plus synthetic allocated sequence 2 without completion. Every committed
legacy row therefore initially projects `unknown`, because private v1 cannot prove that
no newer lock-free transport is in flight. Any other legacy shape is
`IncompatibleStateSchema` before generation replacement; no query performs lazy
conversion, and the next current delivery allocates sequence 2 or 3 respectively. The
target lifecycle-store header
advances from `lumin-lifecycle-store-header.v12` to v13 with the private record schema.
Ordinary repository-state commands accept only v13 and never migrate on open. An
admissible v12 header, matching unfinished v12-to-v13 journal, or exact journal-proven
canonical-absent intermediate returns the `lumin store migrate` recovery instruction
without recovery or private-v1 logical decoding. Any bound source/target without that
journal is foreign and every command hard-stops without adoption or deletion. The
migration command accepts at most one split-form
`--format json`, has no operation ID, and is the one exclusive generation-fenced
private-v1 reader. Revision zero and every successor are prepared on same-volume
handle-owned objects and published no-replace at unique final names. Before root
publication, canonical v12 commits the greatest append-only `MigrationRootAuthorization`
for that exact candidate identity/core and source/transformed-target user-logical facts;
the root is finalized against the post-authorization source, so it cannot authorize
itself. The authorization fixes a `MigrationProvenanceAnchor` in the migrated v13 header
that cross-binds the root/source without including mutable target payload bytes. Each
immutable successor names the exact predecessor, and no update replaces or
removes it. The chain binds the opened canonical v12 source and held private v13 target by
role, pre/post-exchange name, generation, schema, complete migration-time byte/logical
SHA-256, physical identity, and publication-time one-link state. Its closed event fold
distinguishes observed source, pending/published/superseded target, exchange, retained
immutable source, and canonical mutable target. Only a proven-dead absent pending target
may be superseded before a fresh binding is allocated; a published target never may.
Migration uses
Linux `renameat2(RENAME_EXCHANGE)` or Windows handle-relative
`SetFileInformationByHandle(FileRenameInfo, ReplaceIfExists = false)` moves and never
disposes a published artifact. It validates the final v13 dump,
appends a terminal revision, and permanently retains that chain and the now-private v12
source. A racing hard link therefore causes fail-closed validation without requiring an
impossible Linux exclusion or deleting any object. Already-valid native v13 with no
journal and migrated v13 with an exact terminal journal/source are validating no-ops with
no generation advance. After terminalization, normal v13 mutations may change the target
bytes/logical dump; later admission validates its terminal physical identity, exact
immutable target-header anchor, and current-v13 integrity rather than the migration-time
target digest. A header-valid but
payload-corrupt source under a live journal
therefore receives the
ordinary migration-required diagnostic first and hard-stops only when migration opens
it; a byte-identical private v12 source without a matching journal hard-stops every command
without deletion. Concurrent invocations serialize so at most one exchange advances
the generation. Malformed arguments exit `2`; schema, identity, integrity, generation,
durability, and pre-transport failures exit `1` with empty stdout. Its only successful
response exits `0` and is the canonical `lumin.lifecycle-store-migration.v1` object with
only `schemaVersion`,
`storeSchema`, and `status` in that order, naming v13 and `status: "ready"`. Because it
omits source schema, generation, and a changed bit, initial
success, post-exchange or output-delivery recovery, and every current-v13 retry are
byte-identical. Locks and backend handles are released before output; a delivery failure
uses the same `BrokenPipe`/stdout diagnostic contract as cleanup, leaves v13 authoritative,
and retry performs no second exchange. A never-initialized repository returns exact
`lumin: lifecycle store is not initialized\n` with empty stdout and no state creation. It
cannot chain another old schema, downgrade, or guess a migration, and performs no
new gate, retention, or cache-cleanup operation beyond the frozen record transformation.
Before every cleanup-result transport, a short
transaction allocates one increasing delivery-attempt sequence, atomically projects
`lastDeliveryStatus: "unknown"`, and then releases every guard and transaction.
`not-attempted` is valid only before the first allocation. Completion may project
`succeeded` or `failed` only when its sequence is still the greatest allocated sequence;
while a greater attempt is unfinished, a lower completion leaves `unknown`. Equal
matching completion is idempotent, equal disagreement is integrity failure, and a lower
late completion appends its private sequence/result evidence but cannot change the
greatest allocated/completed sequences or public projection.
Immediately after child death the canonical state remains `pending`, and the active-cache
mutation reservation installed with operation creation remains continuously held. Under
the exclusive catalog guard, only an identical mutating retry proves the exact execution
lease dead, atomically records that execution ID as `interrupted`, and increments the
count once without replacing or releasing the reservation. It then releases the guard at
an exact public-show barrier. A second guarded transaction attaches a fresh lease and
returns to `pending` under the same reservation before physical reconciliation. Repeated
show or recovery of the same interrupted attempt does not increment again. A replayed
`clean` is the immutable result of that operation's final observation. Cache payloads
added later require a new operation ID and are never silently consumed by replay.

ARCH-000 owns both command registrations. ARCH-002 owns their state transitions and recovery. Namespace
bootstrap creates and binds `trash/cache-evictions` and its immutable `namespace.anchor`
under the existing `Trash` parent. This is one nested `CacheEvictionParentBinding`, not a
fifth top-level managed-parent kind. The marker and store header bind it with the four
top-level parents, and every complete namespace proof revalidates it. Payload children
remain noncanonical; their authorization rows are canonical integrity and recovery state.
The namespace-binding schema is immutable and distinct from backend schema. The lock
header remains global-bootstrap-only and carries no parent binding. A repository whose
marker or store lacks the nested binding fails as `IncompatibleStateSchema`; absence of
that binding from the lock header is not an error. The marker/store binding is never
lazily upgraded or adopted through ordinary lifecycle-store migration.

Before any move, cleanup authenticates every existing quarantine child through exactly
one retained store-owned `CacheEvictionAuthorization` row. Name grammar and the manifest
digest are self-consistency checks only. A well-formed tree without a matching row, a
duplicate or cross-repository row, an invalid migrated closure, or any name/tree/identity
disagreement is foreign state. Ordinary retention and lifecycle migration may not orphan
either side of the authorization/child bijection.

Cleanup uses the ordinary operation-ID state machine. One durable
`CacheCleanupOperationRecord` binds repository and request digest, a fresh invocation ID,
the ordered initial authorization-set ID/count, and the complete deterministic move plan.
Creation of that record atomically installs the operation-owned active-cache mutation
reservation before active payload inspection. Every store-owned cache writer and every
different cleanup checks for that reservation in its mutation-admission transaction; the
reservation survives process death and both recovery states and is released only with
the committed result.
Each plan row binds the original active-cache component, destination name, complete
physical/tree manifest and digest, and an `Authorized` or `Validated` state through the
canonical authorization table. Historical rows are referenced rather than copied into
each operation. The complete record and every new `Authorized` row commit before the
first rename. A different operation ID cannot adopt unfinished physical state.

Before authorization, cleanup opens without following, flushes all regular-file data and
metadata, then flushes descendant directories bottom-up, flushes the top-level object,
and recomputes the manifest. Any unsupported flush or manifest change hard-stops before
movement. For each authorized row it revalidates the source, performs one same-volume
no-replace move through held parent handles, reopens the destination, compares the full
identity/tree manifest, flushes that moved tree bottom-up again, remanifests it, and
durably flushes the cache, quarantine, and owning trash directories. Only then may one
transaction change that row to `Validated`.

Same-operation recovery accepts exactly two nonvalidated states: the authorized source
alone still exists and matches, so the move may resume; or the authorized destination
alone exists and matches, so recovery performs the full tree and parent flush sequence
before marking it validated. Both names, neither name, a substitute, or any identity or
manifest disagreement hard-stops. A visible rename is never durability proof. After all
rows validate, cleanup proves the active cache is anchor-only, authenticates the exact
quarantine, flushes cache/quarantine/trash again even for an empty or recovered final
move, repeats the complete namespace proof, and commits one v2 result. Identical retry
returns that result without another move.

The move is the command's only payload mutation. Cleanup never unlinks, removes, or
recursively deletes quarantine payloads. Future physical reclamation requires an
identity-bound final-disposition primitive or an enforceable isolation boundary and must
update its matching authorization in the same recoverable protocol. With neither,
quarantine and authorization remain.

## Non-Goals

- Cache cleanup does not prune or reinterpret canonical run, finding, gate, or retention
  evidence.
- The command does not promise immediate disk-space reclamation and performs no physical
  deletion from quarantine.
- The amendment does not roll back already detached disposable payloads after a later
  integrity failure.
- The amendment does not authorize deleting or rotating the cache, trash, quarantine,
  or anchor bindings.
- Random names, unkeyed digests, pathname revalidation, advisory locks, sleeps, and
  scheduler luck are not authorization or durability evidence.

## Required Independent Review

The reviewer must bind one exact candidate commit and report `PASS`, `REOPEN`, or a new
finding for each item:

1. ARCH-000 authorizes both the exact cleanup operation-ID command and
   `lumin store migrate`; Product, ARCH-002, Slice, adapters, acceptance criteria, and
   traceability agree on their grammar, DTOs, exits, retry, and ownership without a
   conflicting command, lazy migration, or invented operation-ID exception. Both packaged
   adapters route the migration-required diagnostic through that public command and DTO
   rather than private state access or embedded migration logic.
2. The cleanup-result v2 field set/order and cleanup-operation v2 projection, request
   digest, exits, stdout/stderr rules, lock release, retry, and strictly read-only
   `operation show` recovery are complete. Frozen cleanup-operation v1 never emits
   `unknown`, v2 has no silent v1 fallback, and the private-v1 migration maps each valid
   pending/interrupted/committed delivery shape to the exact v2 sequence state while
   every committed row receives an unfinished greatest sequence and becomes `unknown`,
   including legacy `succeeded`/`failed` whose stored result remains only historical
   lower-sequence evidence. Pre-transport delivery
   allocation atomically exposes `unknown`, missing completion never appears
   `not-attempted`, neither delivery completion order can select the wrong winner, and
   exact dead-attempt proof, interruption counting, and pending/interrupted/pending
   transitions agree.
3. Namespace bootstrap durably binds the nested quarantine parent/anchor in marker/store
   while the lock remains global-bootstrap-only; replacement, mount, copied-state, or
   crash recovery cannot form a second binding, and a marker/store schema lacking it
   fails closed without lazy adoption or backend-only migration.
4. Every pre-existing or new quarantine child has one exact store-owned authorization;
   self-hashed foreign state, duplicate/missing rows, and generation disagreement fail.
5. The operation record, authorization-set ID/count, and complete `Authorized` plan commit
   before any rename, retain their child provenance without quadratic historical copies,
   and survive the named lifecycle-store v12-to-v13 migration as an exact row/child
   bijection. The migration logical dump includes the canonical synthetic delivery state,
   rejects every invalid legacy shape before exchange, and never exposes any committed
   private-v1 row as a known v2 delivery. Only the public migration command admits v12.
   Canonical v12 authorizes the exact revision-zero candidate before publication and fixes
   the migrated target header's immutable root/source anchor; a root cannot authenticate
   itself, and crash recovery cannot reuse a superseded authorization.
   The append-only journal physically binds canonical source and private target before
   their no-disposition exchange. Every successor is a distinct immutable file linked to
   the exact retained predecessor; no update replaces authority. Its typed event fold
   accepts a visible pending target only as that exact `Published` identity, supersedes only
   a proven-dead unpublished/absent identity, and never requires a superseded object later.
   The terminal chain and retired source remain permanent provenance because unprivileged
   Linux cannot fence a noncooperating same-UID hard-link creation through deletion.
   Revision preparation has no crash-surviving pending name; a substituted identity or
   observed second link hard-stops without cleanup. Its
   initial success, recovery, and already-current retry emit one byte-identical target-only
   response, while a v13 retry neither exchanges the store, appends a revision, deletes an
   artifact, nor advances its generation. Terminalization alone authenticates the target's
   complete migration-time payload; after a normal v13 mutation, fresh admission validates
   the exact immutable target-header anchor plus current-v13 integrity and does not compare
   against that historical payload digest. An unbound, substituted, or no-journal artifact
   is always foreign and is never deleted or advertised as migratable. Absent state fails
   without initialization.
6. Every regular file and directory is flushed bottom-up and remanifested before
   authorization and after movement; cache, quarantine, and trash entries are flushed
   before validation and result commit, including recovered and empty-cache runs.
7. Same-ID recovery distinguishes pending live execution, one idempotently recorded
   interrupted attempt, fresh pending lease, authorized source, moved destination,
   validated row, and committed result; one operation-owned cache-mutation reservation
   remains continuous until commit, and another ID, cache writer, or read-only show cannot
   adopt, bypass, or duplicate any transition.
8. Top-level and nested substitution barriers stop the exact turn, preserve the winning
   object and remaining order, and assert authorization states plus final snapshot.
9. Public process-death fixtures cover after authorization, rename visibility, physical
   durability, row validation, final result commit, delivery allocation, and partial and
   complete stdout before completion. Exact barriers force both delivery orders: lower
   completion while the greater attempt is unfinished, and greater completion followed
   by the held lower completion committing late. The latter asserts the complete public
   v2 projection remains byte-identical and the private ledger changes by exactly the
   lower sequence/result append, without changing either greatest sequence or any other
   durable field. No case uses scheduler timing; an exact guard race also proves cleanup
   cannot overlap publication, retention, or migration. Public migration children also
   prove the ordinary-command recovery diagnostic, exact deaths before/after the v12 root
   authorization and before/within initial and successor revision writing and after each
   unique final-name publication/before parent flush, every later migration crash boundary,
   pending-target absence supersession and exact-visible-target publication,
   no-disposition exchange,
   canonical-source/private-target and predecessor substitution, the exact real hard-link
   attempt after pre-exchange one-link validation, the journal-proven canonical-absent
   recovery split, the header-valid/payload-corrupt split between ordinary routing and
   migration-only authentication, the no-journal byte-identical-copy hard-stop,
   absent-store immutability, target-header-anchor substitution, output-delivery retry,
   identical current-v13 response, and
   no second exchange or journal append. After success, an ordinary current-v13 mutation
   changes the target payload and the next ordinary admission plus migration no-op must
   succeed without changing the immutable provenance.
10. Standard, determinism, store-crash, Windows/Linux package, and skill-adapter commands
    are assigned only to behavior they can execute and include `operation show` recovery.
    Both Windows and Linux package checks invoke the packaged `lumin store migrate` and
    exercise their actual handle-owned append-only publication plus no-disposition exchange
    primitives for admission rejection, absent-store refusal, root authorization,
    pending-target recovery, canonical-absent recovery, success, terminal-journal recovery,
    target-header-anchor validation, post-migration mutation admission, and byte-identical
    no-op retry;
    development/store-crash execution alone is insufficient.
    The skill check proves both packaged adapters invoke the public migration command,
    validate its exact DTO, and retry the original command without migration logic.
11. PRODUCT-000, ARCH-000, ARCH-002, SLICE-001 truth, acceptance, and traceability agree
    on the distinct native-v13 and migrated-v13 layouts, including the latter's exact
    terminal journal and retired source, without weakening any existing reserved-state or
    durability rule.
12. No implementation code or mapped-progress claim is accepted as independent truth.

The sixth-review `PASS` applies only to exact candidate
`0db2bf15d2861157952a106123d995c18b358de7` and the checklist it contained. It does not
cover this post-merge amendment or its expanded acceptance requirements. Rust
implementation and corpus completion for the follow-up remain blocked until the exact
amended owner candidate receives independent `PASS` and merges.

## Verification After Freeze

The post-merge follow-up behaviors may be implemented only after their exact amended
candidate receives owner approval and an independent `PASS`. Focused checks must then
cover nested-binding bootstrap/replacement,
operation admission/idempotency, foreign self-hashed quarantine, authorization-plan
durability, bottom-up flush order, every recovery boundary, exact CLI transport behavior,
allocated-but-unfinished delivery projection before output and after partial or complete
stdout, both exact delivery completion orders including the byte-identical public
projection and one exact private-ledger append after a late lower completion,
cleanup-operation v2 with explicit public-v1 incompatibility and the exact private-v1
synthetic migration in which every committed legacy row initially projects `unknown`,
the public `lumin store migrate` grammar/DTO/error route, canonical-v12 root authorization
and acyclic immutable target-header provenance anchor before revision-zero publication,
handle-owned initial and successor revision publication
at every write/publish/reopen/parent-flush boundary, exact predecessor chaining with no
replacement/removal, pending-target visible/absent recovery through the closed binding
event fold, canonical-source and private-target byte/logical/physical binding followed by
Linux `renameat2(RENAME_EXCHANGE)` or Windows
handle-relative `SetFileInformationByHandle(FileRenameInfo, ReplaceIfExists = false)`
moves, permanent terminal-journal and retired-source provenance, predecessor substitution
before successor publication, a real competing hard-link attempt after pre-exchange
validation with no published-object disposition, and exact journal-proven canonical-absent
recovery,
header-valid/payload-corrupt ordinary-routing versus migration-authentication proof,
no-journal byte-identical-source rejection without deletion, absent-store refusal with no
state creation, one ordinary post-migration v13 mutation followed by successful fresh
admission without historical target-payload comparison, the distinct native/migrated output
layouts, and byte-identical first/recovery/current response without a second generation
advance through store-crash and both packaged platform binaries,
continuous cache-writer rejection across a dead pending lease, both
substitution barriers, and unchanged run/gate evidence. The
public `reserved-state-namespace` row remains unmapped until standard and determinism lanes plus
Windows/Linux package checks execute those behaviors through the packaged CLI and the
skill package check proves operation-ID generation/recovery plus public migration routing
for both Codex and Claude adapters. Passing an internal store test alone is not acceptance
evidence.
