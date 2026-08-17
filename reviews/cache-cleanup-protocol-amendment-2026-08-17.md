# REVIEW-004: Cache Cleanup Protocol Amendment

Document role: focused Architecture v1 amendment and independent-review record

Status: review candidate; implementation is blocked

Date: 2026-08-17

Owners: PRODUCT-000 Section 2.9, ARCH-002 Section 2.5, SLICE-001 Sections 6, 9, 14, and 15

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

These are architecture findings, not test-only omissions. They reopen only the cache
cleanup portion of the frozen state-namespace contract.

## Decision

The owner amendments define one narrow public command:

```text
lumin cache clean [--format json]
```

It accepts at most one split-form `--format json`. Its exact successful JSON is
`{"schemaVersion":"lumin.cache-cleanup.v1","status":"clean"}`. Success exit `0`
requires an anchor-only cache parent plus the final complete namespace proof. Malformed
input exits `2`; integrity, persistence, and delivery failures exit `1`. A failure found
before transport leaves stdout empty; a stream failure may transfer a nonauthoritative
prefix but cannot publish a complete success object. The operation changes no canonical
run, gate, lifecycle, or operation record, so it intentionally has no operation ID.
Repeating the idempotent command is the recovery path when output delivery fails after
disposable payload removal.

Deletion authority comes from a held parent plus an entry atomically moved without
replacement into a cleanup-owned name under held same-mount directories, then reopened
and revalidated. It never comes from a pathname that was merely valid earlier. Top-level
and recursive removal remain relative to held directory handles, do not follow redirects
or cross mounts, and stop on any kind/identity/topology substitution without deleting the
substitute. A deterministic barrier test owns the exact validation-to-claim race.

## Non-Goals

- Cache cleanup is not retention, does not prune canonical evidence, and creates no
  tombstone or durable operation result.
- The amendment does not make cache payload deletion transactional or roll back already
  removed disposable payloads after a later integrity failure.
- The amendment does not authorize deleting or rotating the cache parent or anchor.
- Random names, a second validation followed by pathname deletion, sleeps, and scheduler
  luck are not accepted as identity binding.

## Required Independent Review

The reviewer must bind one exact candidate commit and report `PASS`, `REOPEN`, or a new
finding for each item:

1. The public grammar, JSON fields, success/failure exits, stdout/stderr behavior, and
   delivery recovery are complete and mutually consistent.
2. Omitting an operation ID does not weaken the Product operation-recovery contract,
   because no canonical lifecycle mutation or result exists.
3. Success cannot publish before the anchor-only state and complete namespace proof.
4. The claim/revalidation boundary prevents the reported anchor-substitution deletion
   and applies recursively to directory entries.
5. The barrier test is deterministic and stops the exact turn rather than relying on
   timing.
6. PRODUCT-000, ARCH-002, SLICE-001 truth, acceptance, and traceability agree without
   weakening any existing reserved-state rule.
7. No implementation code or mapped-progress claim is accepted as independent truth.

The candidate remains `REOPEN` until that exact review passes. Rust implementation and
corpus completion must be based on the reviewed owner bytes, not this document's draft
status.

## Verification After Freeze

Implementation may begin only after the candidate's exact commit receives both owner
approval and independent `PASS`. The implementation must then preserve focused checks
for the store cleanup owner, CLI grammar/response/exits and delivery rerun, and the
barrier-forced substitution before broader validation. The public `reserved-state-namespace`
row remains unmapped until both standard and determinism lanes run those behaviors through
the packaged CLI on supported Windows and Linux; passing an internal store test alone is
not acceptance evidence.
