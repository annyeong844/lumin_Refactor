# W6 Design Review

Reviewed [DESIGN.md](DESIGN.md) SHA-256
`853fc908deab61e351a05ba099b9b4d5c36829e0134166d4940a4f8620d8fa3a`
against product source HEAD
`95163f586f2d9ea9dcae7f356d94e10469ec84e9` on 2026-09-08.

Status: **amended design frozen; focused implementation review PASS**.
The [implementation evidence](IMPLEMENTATION.md) closes W6-03's focused public
counterexample and owns the completed local regression/package checks.
[Local measurements](MEASUREMENTS.md) retain the four ordinary comparisons;
this scoped review is not CI acceptance or proof of a material causal speedup.

## Author review

Scoped PASS for the exact candidate above. The two source-level backend opens
can serve one generation-bound lease-row read without retaining a backend
across a mutation, lock release, scan, or result transport. The candidate keeps
the complete persisted-lease comparison, external liveness-lock authentication,
original held-object proof, generation fence, and validation receipts.

The proposed savings are backend lifetimes, not application write commits or
required durability barriers of real state transitions. The pinned redb source
supports the allocator-state commit on ordinary Database destruction. No timing
for that commit alone, causal attribution of all Windows store cost, measured
speedup, or budget PASS follows from that source observation.

The all-exits policy is required: even a malformed/missing/read-error result
must reach same-held finishing. A failed generation-bound open or failed final
proof rejects further backend access through that acquired guard. Natural
backend destruction can still change its own recovery metadata. The design
does not promise byte-immutable authentic lifecycle.store or rollback of
already committed attempt/publication state.

## Independent adversarial review

Reviewer `attempt_read_design_review` independently read the exact proposal,
current store/publication owners, pinned redb source, and existing public
replacement tests. The initial candidate SHA-256
`6d6eecc230691ac5df7e359cc0cf8a8d5a98adf09596703f8d4fc38b71a33f0f`
received **REOPEN** with two grounded findings:

1. W6-01: an early row-read/decoding failure could skip finishing and sticky
   rejection, allowing enclosing teardown to reopen an otherwise valid foreign
   canonical replacement in writable mode.
2. W6-02: requiring every Windows rename attempt to succeed and the command to
   reject would misclassify legitimate open-handle protection. A denied swap
   also cannot require an otherwise successful mutating audit to leave its
   authentic state unchanged.

Both are closed **at design level** in the reviewed hash. W6-01 now has a
retained result, all-path finishing, explicit failed-open rejection and
validation-first error ordering, plus combined error/substitution tests.
W6-02 now distinguishes actual Linux substitution, successful Windows
substitution, specifically verified Windows open-handle denial, and an
independently forced read failure. Arbitrary fixture errors cannot become a
passing platform-protection branch. The single lease-row transaction is
explicitly separate from existing header/receipt read transactions.

The reviewer returned scoped PASS with no remaining grounded design blocker.
This was a read-only review; the reviewer performed no edits, builds, tests,
benchmarks, or external PR actions.

## Authority and next action

On 2026-09-08 the owner explicitly approved implementation and local
Windows/Linux control/candidate measurements of the reviewed hash with
“응응 해주세요”. This record freezes those exact DESIGN.md bytes; its original
candidate status is retained as part of the reviewed artifact. Approval is
limited to W6 and is not implementation acceptance, permission to push or rerun
CI, or approval of the remaining REVIEW-005 decisions.

During the approved implementation, reviewer `attempt_read_design_review`
independently confirmed **W6-03** and explicitly reopened the earlier scoped
PASS: the engine's failure continuation acquires a new guard and can reopen an
otherwise admissible substituted store writable after the original guard
correctly rejects it. Three exact Linux public-child barriers reproduce changed
foreign bytes. The [implementation blocker](IMPLEMENTATION-BLOCKER.md) owns the
counterexample and the additional authority needed before continuing. Preserve
the reviewed DESIGN.md bytes; do not silently broaden its original-guard-only
algorithm or its exclusion of recovery-ordering changes.

### W6-03 amendment

The owner approved the same-attempt continuation extension on 2026-09-08 with
“응응 선생님. ;ㅅ;”. The author and independent reviewer
`attempt_read_design_review` returned scoped PASS for
[FAILURE-CONTINUATION.md](FAILURE-CONTINUATION.md), SHA-256
`7c33624177baba1a860c8af838b00d7ef83ab0a144ba53d4a4899c6845581bad`.
Those exact bytes are frozen; their candidate status is retained as reviewed.

The first amendment candidate received a verification-only REOPEN: a
prepare-only fixture did not cover finalization or lease release, and a second
observer guard would deadlock under their exclusive guard. The frozen amendment
requires all three actual durable phases, length-framed observation through the
already held backend, and preservation of already Completed attempts in late
Windows denied-substitution/error cases. The reviewer found no remaining
grounded design blocker and performed no edits, builds, or tests.

Implementation may now resume only within that amendment. The original red
transcript remains evidence; its public assertions, restoration/recovery,
negative controls, and corpus registration must pass before measurements.

Subsequent Rust work needs its own pre/post advisory, focused
public and owner checks, retained control/candidate measurements, and new
blocking Windows/Linux CI evidence. P1-60/P1-70 remain open.

### W6 publication and hosted verification

On 2026-09-08 the owner approved committing and pushing the verified W6 change
to its existing branch/PR and inspecting the resulting actual Windows/Linux CI
with “응응 그렇게해주세요”. This supersedes the earlier publication restriction
only for this change. The PR remains draft; merging, changing worker policy or
budgets, broadening the optimization, and retrying a failed CI run to obtain a
pass are not authorized. The rejected local affinity comparison remains
negative setup evidence, not a four-worker result. P1-60/P1-70 stay open until
their independent remaining acceptance requirements are met.
