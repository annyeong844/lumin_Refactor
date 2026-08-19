# REVIEW-004: Cache Cleanup Protocol Amendment

Document role: focused Architecture v1 amendment and independent-review record

Status: owner merged; post-merge counterexample closure awaits independent PASS

Date: 2026-08-19

Owners: PRODUCT-000 Section 2.9, ARCH-000 Section 8, ARCH-002 Sections 2 and 2.5, SLICE-001 Sections 6, 9, 11, 14, and 15

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

## Post-Merge Follow-up Review

The approved owner bytes merged in `e988a85`. Delayed independent review then supplied
two concrete counterexamples that were not covered by the sixth review. Concurrent
identical retries could complete delivery metadata in backend commit order because no
pre-transport sequence chose the `lastDeliveryStatus` winner. A cleanup process could
also die while `pending` before the later retry installed the documented recovery
reservation, leaving a window in which a normal active-cache writer could invalidate
the immutable plan. REVIEW-004 is therefore narrowly reopened. The replacement decision
allocates a durable delivery sequence before each transport and lets only the greatest
completed sequence update the projection. It also installs one continuous active-cache
mutation reservation with operation creation and retains it through
`pending -> interrupted -> pending` until result commit. No other command, namespace,
quarantine, manifest, or response field changes. The exact follow-up candidate requires
an independent `PASS` before its Rust implementation may resume.

## Decision

The owner amendments define one public command:

```text
lumin cache clean --operation-id <operation-id> [--format json]
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
`lumin.cache-cleanup-operation.v1` projection from `lumin operation show`, never a new
whole-command mutation. That projection exposes only its operation identity/kind/digest,
status and interruption count, authorized/validated counts, stored result, and last
delivery status; it never embeds unbounded manifests. Show is strictly read-only and
does not prove process liveness or change a record. Before every transport, a short
transaction allocates one increasing delivery-attempt sequence and then releases every
guard and transaction. Completion may update `lastDeliveryStatus` only when its sequence
is greater than the greatest completed sequence; equal matching completion is idempotent,
equal disagreement is integrity failure, and a lower late completion is ignored.
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

ARCH-000 owns the command. ARCH-002 owns its state transition and recovery. Namespace
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

1. ARCH-000 authorizes the exact operation-ID command; Product, ARCH-002, Slice, adapters,
   acceptance criteria, and traceability expose no conflicting command or exception.
2. The v2 field set/order, request digest, exits, stdout/stderr rules, lock release, retry,
   and strictly read-only `operation show` recovery are complete; pre-transport delivery
   sequencing selects one deterministic last-status winner, and exact dead-attempt proof,
   interruption counting, and pending/interrupted/pending transitions agree.
3. Namespace bootstrap durably binds the nested quarantine parent/anchor in marker/store
   while the lock remains global-bootstrap-only; replacement, mount, copied-state, or
   crash recovery cannot form a second binding, and a marker/store schema lacking it
   fails closed without lazy adoption or backend-only migration.
4. Every pre-existing or new quarantine child has one exact store-owned authorization;
   self-hashed foreign state, duplicate/missing rows, and generation disagreement fail.
5. The operation record, authorization-set ID/count, and complete `Authorized` plan commit
   before any rename, retain their child provenance without quadratic historical copies,
   and survive lifecycle-store migration as an exact row/child bijection.
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
   durability, row validation, and final result commit without scheduler timing; an exact
   guard race also proves cleanup cannot overlap publication, retention, or migration.
10. Standard, determinism, store-crash, Windows/Linux package, and skill-adapter commands
    are assigned only to behavior they can execute and include `operation show` recovery.
11. PRODUCT-000, ARCH-000, ARCH-002, SLICE-001 truth, acceptance, and traceability agree
    without weakening any existing reserved-state or durability rule.
12. No implementation code or mapped-progress claim is accepted as independent truth.

The exact owner candidate above passed independent review. Rust implementation and corpus
completion must be based on those reviewed owner bytes and may begin only after owner
merge makes them authoritative on the base branch.

## Verification After Freeze

The two post-merge follow-up behaviors may be implemented only after their exact amended
candidate receives owner approval and an independent `PASS`. Focused checks must then
cover nested-binding bootstrap/replacement,
operation admission/idempotency, foreign self-hashed quarantine, authorization-plan
durability, bottom-up flush order, every recovery boundary, exact CLI transport behavior,
concurrent reverse-order delivery completion, continuous cache-writer rejection across a
dead pending lease, both substitution barriers, and unchanged run/gate evidence. The public
`reserved-state-namespace` row remains unmapped until standard and determinism lanes plus
Windows/Linux package checks execute those behaviors through the packaged CLI and the
skill package check proves operation-ID generation and recovery. Passing an internal
store test alone is not acceptance evidence.
