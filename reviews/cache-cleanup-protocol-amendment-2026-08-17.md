# REVIEW-004: Cache Cleanup Protocol Amendment

Document role: focused Architecture v1 amendment and independent-review record

Status: review candidate; implementation is blocked

Date: 2026-08-18

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
replacement decision below removes online physical deletion instead of pretending that
Linux pathname revalidation can bind an unlink to an opened object.

## Second Review Result

Independent review bound exact candidate
`b9c26e6c3f81dec267b4db0056a74f025bc53ae6` and returned `REOPEN`. It found that an
empty-cache retry could report clean without admitting a malformed pre-existing
quarantine, process-death recovery had no public child-process acceptance fixture, first
creation of `trash/cache-evictions` was not explicitly made durable in the held trash
parent before payload moves, and stdout stream failure left stderr behavior
platform-dependent. The replacement decision below makes those four states explicit and
keeps implementation blocked.

## Third Review Result

Independent review bound exact candidate
`9be3fe613bac12162eabd32c6a6ca59b648be7a0` and returned `REOPEN`. It found that
a process death after a racing substitute was moved but before post-move comparison
discarded the only expected manifest, allowing retry to admit the substitute as ordinary
prior quarantine. It also found that ARCH-000's canonical command set did not authorize
`lumin cache clean`, while its ownership rule forbids lower-level documents from adding
commands. The replacement decision below puts a recomputable pre-move manifest digest in
each quarantine child name and adds the command through the blueprint owner.

## Decision

The owner amendments define one narrow public command:

```text
lumin cache clean [--format json]
```

It accepts at most one split-form `--format json`. Its exact successful JSON is
`{"schemaVersion":"lumin.cache-cleanup.v1","status":"clean"}`. Success exit `0`
requires an anchor-only active cache parent plus the final complete namespace proof.
Malformed input exits `2`; integrity, persistence, and delivery failures exit `1`. A
failure found before transport leaves stdout empty. A stream failure may transfer a
nonauthoritative prefix but cannot publish a complete success object: `BrokenPipe` exits
`1` with empty stderr, while any other stdout write/flush failure exits `1` and emits the
stable `lumin: cannot write stdout\n` diagnostic when stderr remains writable. The operation
changes no canonical run, gate, retention lifecycle, or operation record, so it
intentionally has no operation ID. The generic adapter and AC rules now name only gate
and retention lifecycle mutations. Repeating the idempotent command is the recovery path
when output delivery fails after active-cache eviction.

ARCH-000 authorizes the command in the canonical product surface; ARCH-002 owns its
cache-state transition, integrity, crash-retry, and delivery semantics. Cleanup first
admits the complete pre-existing quarantine even when the active cache is empty, then
validates the complete deterministic active-cache tree and atomically moves each
top-level object without replacement from the held cache parent to a disjoint
`trash/cache-evictions/<invocation-id>.<ordinal>.<manifest-sha256>` entry. The manifest
digest is computed before the move over the canonical top-level physical/tree manifest
and excludes both source and destination names so it can be recomputed after a restart.
The literal quarantine directory is reserved from retention-plan allocation and opened
without following; every existing
child name and tree must pass the full grammar, manifest-digest, kind, link, and mount
admission before mutation or success. If the directory is absent and active payloads
exist, it is created without replacement and the held trash parent is durably flushed
before any payload move.
The directory is then held and revalidated relative to the same-volume trash parent for
the invocation; it is not a fifth canonical managed parent. Cleanup reopens the moved
winner and compares its top-level identity and complete read-only descendant manifest
with the initial observation. A mismatching top-level or nested substitute remains intact
in quarantine and cleanup fails; prior moves remain quarantined and later entries remain active.
Randomness is collision routing, not authority.

That atomic detach is the command's only payload mutation. `lumin cache clean` never
unlinks, removes, or recursively deletes a cache-eviction entry, so a later destination
swap cannot be turned into deletion of the substitute. Physical reclamation is a separate
future protocol and may run only with an identity-bound final-disposition primitive or an
enforceable isolation boundary. On a platform with neither, quarantine remains. Two
deterministic barriers own the top-level and nested-child validation-to-move races and
assert remaining order plus the final durable failed snapshot.

Process death creates no cleanup operation record. The no-replace move atomically couples
the moved winner with the expected pre-move manifest digest in its destination name, so
whole-command retry can rederive the moved tree and reject a mismatch without the first
process's memory or a completion marker. Retry first validates the complete existing
quarantine, retains it, and freshly evicts only the current active-cache set; missing or
duplicated disposable payload bytes are never promoted into canonical deletion truth,
while malformed or digest-mismatching quarantine remains an integrity hard-stop even
when the active cache is empty. One deterministic public child-process fixture terminates
after a fully validated first move and its durability flushes but before the second move,
then proves that restart preserves prior quarantine, evicts the remaining active set
under a new invocation ID, and publishes exactly one clean response. A second substitutes
the manifested source, terminates after its durable move but before comparison, and proves
that restart preserves the substitute yet rejects its name-bound digest mismatch before
any further move or success.

## Non-Goals

- Cache cleanup is not retention, does not prune canonical evidence, and creates no
  tombstone or durable operation result.
- The command evicts active cache objects; it does not promise immediate disk-space
  reclamation and performs no physical deletion from quarantine.
- The amendment does not roll back already detached disposable payloads after a later
  integrity failure.
- The amendment does not authorize deleting or rotating the cache parent or anchor.
- Random names, a second validation followed by pathname deletion, the advisory lifecycle
  lock, sleeps, and scheduler luck are not accepted as final-disposition authority.

## Required Independent Review

The reviewer must bind one exact candidate commit and report `PASS`, `REOPEN`, or a new
finding for each item:

1. The public grammar, JSON fields, success/failure exits, stable `BrokenPipe` versus
   non-pipe stdout/stderr behavior, and delivery recovery are complete and mutually
   consistent.
2. ARCH-000's canonical command set authorizes `lumin cache clean`, ARCH-002 owns its
   lower-level state/recovery semantics, and no lower owner invents a command outside the
   blueprint surface.
3. The Product, Slice adapter, and Slice AC wording consistently require operation IDs
   only for gate/retention lifecycle mutations and explicitly route cache delivery
   recovery through whole-command rerun.
4. Every invocation admits the full existing quarantine before active-cache work or
   success, including empty-cache retry, and final validation allows exactly the admitted
   initial tree plus this invocation's manifest-digest-matching additions.
5. First creation of the quarantine is durably flushed in the held trash parent before
   any payload move, and success cannot publish before the anchor-only active-cache state,
   durable cache/quarantine moves, and complete namespace proof.
6. Cache cleanup performs no final unlink/rmdir; moved objects remain quarantined, and
   physical reclamation cannot silently fall back to pathname revalidation.
7. Each destination name durably binds the canonical expected pre-move manifest before
   the atomic move; both immediate post-move comparison and restart admission preserve a
   racing substitute and fail on top-level, child, or digest disagreement.
8. Both substitution barriers stop the exact turn, assert prior/later relative order and
   the final durable snapshot, and do not rely on timing.
9. The two public process-death fixtures distinguish a fully validated move from death
   after a substitute's durable move but before comparison; restart accepts only the
   former, without a canonical operation record or interpretation of quarantine as
   lifecycle truth. Delivery failure is likewise safely rerunnable.
10. Standard, determinism, store-crash, Windows/Linux package, and skill-adapter commands
   are assigned only to rows and behavior they can actually execute.
11. PRODUCT-000, ARCH-000, ARCH-002, SLICE-001 truth, acceptance, and traceability agree
   without weakening any existing reserved-state rule.
12. No implementation code or mapped-progress claim is accepted as independent truth.

The candidate remains `REOPEN` until that exact review passes. Rust implementation and
corpus completion must be based on the reviewed owner bytes, not this document's draft
status.

## Verification After Freeze

Implementation may begin only after the candidate's exact commit receives both owner
approval and independent `PASS`. The implementation must then preserve focused checks
for the store eviction/quarantine owner, manifest-bound existing-quarantine admission,
durable first creation, CLI grammar/response/exits and exact stream-failure behavior,
delivery rerun, both public process-death restart outcomes, and both barrier-forced
substitutions before broader validation. The public
`reserved-state-namespace` row remains unmapped until standard and determinism lanes plus
Windows/Linux package checks run those behaviors through the packaged CLI and the skill
package check proves the no-operation-ID exception. Store-crash continues to prove its
three applicable state rows but cannot substitute for cleanup acceptance; passing an
internal store test alone is not acceptance evidence.
