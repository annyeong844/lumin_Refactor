# W6-03: Failure Continuation Reopens the Rejected Store

Status: **focused counterexample closed; original negative evidence retained**.
The [implementation verification](IMPLEMENTATION.md) records the scoped fix,
Windows/Linux public PASS and independent review under the approved amendment.
The original counterexample below remains unchanged evidence. Broader acceptance
and performance results are not supplied by those focused checks.

This counterexample reopens the owner-approved [design](DESIGN.md), SHA-256
`853fc908deab61e351a05ba099b9b4d5c36829e0134166d4940a4f8620d8fa3a`.
The source base is `95163f586f2d9ea9dcae7f356d94e10469ec84e9`; the local
prototype implements the one-database lease read and all-exits original-object
proof. No push, CI rerun, performance measurement, or budget change was made.

## Counterexample

1. A public audit has durably published its second Running attempt. Its first
   session validation holds the original generation-bound database A.
2. At an exact barrier, replace the canonical name with a closed database B.
   Before this audit, B was independently admitted through public `overview`
   at that canonical name: its native-v13 namespace, generation and validation
   receipts are valid. Its different physical identity is intentional.
3. The prototype's finishing proof correctly rejects B with
   `lifecycle.store physical identity changed` and poisons the original guard.
4. `engine/src/lib.rs:405` handles publication failure by calling
   `store.fail_attempt`; `publication/liveness.rs:120` enters a new exclusive
   Ordinary guard. That guard's rejection flag starts false. For native v13
   without a journal, `migration::require_idle` rejects orphan migration names
   but does not remember A. Ordinary admission calls `validate_complete` and
   opens/drops B writable before the new session check can reject its missing
   attempt lease.
5. B retains its physical identity but its bytes change. Returning the combined
   publication/persistence error cannot undo that backend access.

The independent reviewer confirmed this exact source path and explicitly
reopened the prior scoped PASS. More checking inside the original guard cannot
fence a later, newly acquired guard.

## Observed public evidence

The targeted Linux test invocation uses the existing public CLI fault binary:

```text
cargo test -p lumin-cli --test state_namespace_replacement \
  --features lifecycle-test-fault --locked attempt_session:: \
  -- --test-threads=1 --nocapture
```

Rust 1.96.0, the pinned source-provenance wrapper, one Cargo build job, WSL2
Ubuntu with test repositories on its native Linux temporary filesystem.
All three tests execute and fail the unchanged-foreign-bytes assertion after
proving original-held identity rejection:

| Exact turn | Result |
| --- | --- |
| `after-attempt-session-open` | Foreign physical identity preserved; bytes changed. |
| `before-attempt-session-return` | Foreign physical identity preserved; bytes changed. |
| `after-attempt-session-open-read-error` | Retained read failure still reaches original-held rejection; later foreign bytes change. |

The raw transcript, including before/after SHA-256 identities, is retained at
`D:\lumin-w6-attempt-session-read-20260908\linux-public-session-substitution.txt`.
Its SHA-256 is
`813b85a3aaa16a2684228ef3dce3b226d77069bdb817514ee074da568af9ce61`.
These are negative counterexamples, not passing crash evidence. Their assertions
stop before the subsequent restoration/recovery proof; that proof and the
Windows public branches have not been accepted. The new tests are not mapped
as a completed corpus obligation. Existing mapped rows are unchanged.

The focused Windows owner invocation, `cargo test -p lumin-store --lib
attempt_session --locked -- --test-threads=1`, passes all five tests. It proves
one actual database construction and guarded lease read without application
writes, all-exits missing/malformed handling, exact persisted lease and lock
comparison, post-turn receipt checking, and rejected-open original-guard
teardown. This supporting PASS does not close the public failure continuation.

## Original required authority

The frozen amendment now supplies this authority; the following records the
constraint identified by the original counterexample, not a new work route.

The narrow follow-up to review is attempt-scoped rejected-read authority that
survives the failure continuation and refuses failure persistence **before any
new guard/backend admission**. Original-object, namespace, generation or
receipt rejection must not be swallowed; ordinary read/decode errors with a
successful original-object proof must retain their existing failure-persistence
behavior. Do not introduce repository-global poisoning or a backend cache.

Retain the authentic Running attempt after refusal. Only authentic restoration
and the existing owner-liveness recovery may publish its interrupted outcome;
there must be no invented successful run or duplicate catalog insertion.
Recheck the successful and read-error cases at both original-held turns on
Linux and the precisely distinguished Windows substitution/OS-denial branches.

This additional failure-continuation rule changes authority outside the frozen
algorithm and needs an exact design, author and independent review, and explicit
owner approval before implementation. Until then, retain the prototype and
counterexample, perform no benchmark or broad validation, and keep P1-60/P1-70
open. The original performance miss is not reclassified by this finding.
