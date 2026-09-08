# W7 Lifecycle Diagnostic Review

Date: 2026-09-08. Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).

Candidate: [DESIGN.md](DESIGN.md). Status: **author and independent scoped
design reviews PASS; exact candidate frozen; diagnostic implementation authorized**.

Current candidate SHA-256:
`eaf2133f6ac6aeaa9f89538cc173a31788e0211e1a4a9bf77a9fde131198ecb8`.

The owner approved preparing and reviewing this narrower diagnostic, then
explicitly accepted the proposal to freeze it and implement the diagnostic.
This grants no product optimization, push, new CI run or budget amendment.
The exact source is `964c91c6c89ec84a5b2b9bb42851caf664464dfa`; prior W2/W3/W6
approvals do not approve W7's different frame or observation paths.

## Author design review

The source and owner-contract review covers ARCH-001's execution, freshness and
observability boundaries, ARCH-002's durable namespace/publication rules,
SLICE-001 Section 12, REVIEW-003's command/feature admission, and the exact
frozen W2/W3/W4 diagnostic contracts. W5/W6's failure-continuation obligations
remain constraints, not permission to change another backend lifetime.

| Reviewed boundary at the source head above | Design conclusion |
| --- | --- |
| `namespace/database.rs:42-154`, `169-198`, `234-314` | Separate explicit open, validation, transaction and existing drop expressions. Preserve final generation validation on a failed mutation, guarded commit/abort checks and sticky backend rejection. |
| `publication/latest.rs:54-79`, `112-165`, `382-579` | The sequential document callbacks can borrow one recorder explicitly. Derived POINTERS, catalog-read and sync-index returns retain their original scopes, including the unchanged-index abort. |
| `publication/liveness/records.rs:196-202`, `liveness.rs:253-277`, `350-390` | Measure the lease read's natural return before its caller performs the lock proof; do not profile or redesign W6's session-owned read. Creation and terminal publication retain generation fences. |
| `publication/liveness.rs:152-157`, `publication/liveness/recovery.rs:185-192` | Pending latest validation precedes unobserved lease recovery, which restores a Completed frontier before the new attempt. Later measured contexts therefore use the healthy-seeded vectors. |
| `publication/run.rs:316-404`, `publication/files.rs:37-132` | Selected staging/JSON movement and sync API calls remain in place; serialization, payload checks and residual work are not mislabelled backend time. |
| `platform.rs:152-173` | Whole JSON write/flush and directory-sync API intervals are measurable. The Windows directory-sync implementation is a no-op, not an observed directory flush. |
| `tools/xtask/bootstrap/source_provenance.py:61-81`, `205-256`, `610-660` | Exact positive target routing also needs an inverse rejection for unlisted diagnostic-bearing commands; otherwise they fall through to the ordinary target. |
| `tools/xtask/Cargo.toml:15-16` and model/protocol feature maps | Preserve the ordinary runner's decoder-only feature edge without enabling CLI/engine/store instrumentation. Feature-selector rejection is not a ban on these decoder types. |

The eight fresh vectors have totals `17 / 5 / 4 / 2 / 2 / 8 / 9 / 3 / 4 / 6`
in the design's ten-cost order. Every context balances opens against explicit
drops plus natural return tails. These are authored from the latest-document
transitions, not sampled timing output. Healthy second-audit and fresh-first
pending-latest fixtures have separate exact expectations and preparation.

The proposed one-body optional recorder, thirteen disjoint leaves, checked
residual and versioned frame can distinguish the selected owner API costs
without moving a resource boundary. No complete-validation, physical flush,
raw lock-wait or internal redb destruction attribution is approved. This is a
source-backed feasibility/design PASS, not proof that an implementation already
preserves those constraints.

## Independent adversarial findings and resolution

The initial independently reviewed candidate was SHA-256
`cb21a0fb919f5f486ba587e65d07371e117056a1c7a3059b131393141f110c65`.
The read-only reviewer `lifecycle_boundary_design_review` verified that hash and
the source HEAD before and after review and returned **NEEDS REVISION**:

1. **W7-R1, destruction-proof scope:** the draft required deterministic traces
   of concrete table/transaction/held-entry/backend destruction without any
   supported observer seam. Constructed `StoreDatabase` field destruction and
   failed pre-construction open locals also have different original orders.
   The revised acceptance separates unchanged source/ownership-scope review,
   supporting canaries of the actual recorder primitive, and real helper/public
   counts and durable outcomes. Canaries are explicitly not redb traces; no
   new backend wrapper, Drop hook or altered scope is authorized.
2. **W7-R2, inverse hosted admission:** the reviewer independently reproduced
   admission of a reordered W3 build and an appended repeated diagnostic-feature
   build into ordinary `lumin-target` using the existing validators. The revised
   design rejects unlisted instrumentation selectors on every hosted target
   before metadata/Cargo, including reordered, repeated, equals/short/qualified
   forms and artifact-producing all-features commands. Its negative tests cover
   ordinary as well as diagnostic targets while preserving reviewed decoder
   edges, the probe feature and internal metadata.

Re-review of candidate SHA-256
`99e2779571d76d1723198d1ee691458faa099083c0438302dac30047e521d9de`
closed W7-R1/R2 but returned **NEEDS REVISION** for **W7-R3, pending-fixture
oracle**. Unobserved lease recovery restores the interrupted publication's
Completed frontier before attempt begin. The newly specified pending fixture
therefore cannot use fresh counts for its later `attempt-latest` and
`finalize-latest` calls. Author source review confirmed that transition; the
current candidate adds both exact `4 / 3 / 1 / 1 / 0 / 0 / 4 / 1 / 1 / 1` rows
and explicitly keeps that intervening recovery work outside W7.

The revisions also make Windows sync semantics and non-fresh fixture preparation
explicit. They add no product behavior or performance claim. Final independent
review returned **scoped PASS** for the current candidate hash above, resolving
W7-R1/R2/R3 with no remaining material design finding. The reviewer verified
both candidate hash and source HEAD before and after review and confirmed that
the final candidate differs from `99e277...` only by the six stated R3 additions.
Neither earlier NEEDS REVISION disposition is being relabelled PASS.

## Documentation closeout

- Documentation/link policy passes for all 87 tracked and new live Markdown
  files; tracked diff and new Markdown whitespace checks pass.
- Static design checks confirm eight contexts, thirteen costs, eight balanced
  fresh vectors, five explicit non-fresh vectors and the exact fresh totals
  above. This validates the authored document, not a running implementation.
- Frozen W2/W3/W4/W6 design SHA-256 values remain unchanged. The dedicated
  worktree's implementation files and the original user's dirty worktree are
  unchanged by this task.

## Authority and remaining acceptance

On 2026-09-08 the owner replied "응응 선생님" to the explicit proposal to
freeze this exact design and implement its diagnostic feature. The candidate
SHA-256 above is the frozen authority; its bytes and earlier review-stage status
line remain unchanged. This approval is specific to W7, not inherited from
W2/W3/W6. Implementation must now pass
the design's actual public-child, lifecycle/fault, strict transport, hosted
admission and package checks using a new external Rust pre/post advisory.

The completed design review changed documentation only and ran no product
command, build, benchmark, commit, push or CI job. Implementation and runtime
evidence must be recorded separately; this design PASS is not a runtime or
budget PASS.
