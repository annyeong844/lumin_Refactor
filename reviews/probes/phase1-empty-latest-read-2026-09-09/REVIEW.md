# W8 Empty Latest-Index Design Review

Date: 2026-09-09. Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
Candidate: [DESIGN.md](DESIGN.md).
Status: **author and independent scoped design PASS; exact candidate frozen;
implementation and local verification authorized**.

Candidate SHA-256:
`14d0fc5df67dc2fe7a2ffdea2020b1a277b4fcd268e7ad43e74ea9954345623b`.
Examined source HEAD: `4f03cc1f8a25e83265a81c0fb1b2c604ce6ee8bf`.
The owner approved preparing and reviewing the next narrow backend-cost design,
not freezing it, implementing it, publishing changes or starting another CI run.
W5/W6/W7 approvals do not authorize this different optimization.

## Author design review

The author checked METHOD-000, ARCH-001's ownership/execution rules,
ARCH-002's namespace/publication and transaction-generation boundaries,
SLICE-001 Section 12, and W5/W6/W7's exact scoped authorities against source.

| Source boundary at the examined HEAD | Decision |
| --- | --- |
| `publication/latest.rs:56`, `646`, `765` | The absent-document reader already observes POINTERS. Only a whole-table empty witness can remove the subsequent unchanged write/abort; nonempty or canonical-document-present cases keep W5 sync. |
| `publication/liveness.rs:25`, `162` | Both eligible fresh calls precede new attempt allocation. Existing lease recovery still follows latest recovery; empty POINTERS cannot justify skipping it. |
| `namespace/database.rs:50`, `286`, `310` | Keep the original held file, generation and receipts through completion, including read errors. Share the existing held-read finishing body rather than replacing it with a new pathname open. |
| `namespace.rs:461`, `724` | Rejected-open/completion authority must reach the outer guard's existing no-backend final path. W6's new-session failure-continuation authority is not weakened or duplicated. |
| `publication/files.rs:204` | Authenticate/remove a preexisting pending document before the read, unchanged. A later arrival at canonical or pending names is a separate shortcut failure, not permission for another removal. |
| W7 model/protocol/runner and frozen fresh-count table | Removing the two index opens changes only two fresh rows and their totals. The candidate must amend the build-bound oracle explicitly, preserving old evidence and non-fresh vectors. |
| Pinned `redb-4.1.0/src/db.rs:423`, `1061`, `1214`, `1231` | Read-only pathname admission is not a held-file replacement; writable backend drop performs maintenance. Neither zero application writes nor measured return-tail duration implies no physical backend work. |

The scoped author verdict is PASS for feasibility and preserved owner
boundaries, not runtime correctness. The expected reduction is two of W7's
17 selected fresh backend opens, with no claim that the whole audit has only
17 opens or that this will resolve either hosted scaling miss. Complete logical
snapshots, foreign physical snapshots, final-turn public faults, recovery and
strict observation checks remain implementation prerequisites.

## Independent adversarial review

The read-only `latest_index_noop_review` reviewer returned **scoped DESIGN
PASS**, with no concrete blocker. It verified the source HEAD and candidate
SHA-256 above both before and after review, and independently confirmed:

- Pending-first and canonical-present paths retain their current ordering.
  Whole-table emptiness skips only the second sync lifetime, not recovery.
- Retained-result completion follows the actual W6 record/database pattern
  and preserves the outer guard's rejection-aware teardown.
- Both eligible calls precede new attempt allocation, so no newly allocated
  session can later bypass rejection through its failure continuation.
- The W7 fresh totals reconcile, and the source/build-bound oracle amendment
  does not reinterpret previous captures or change frame shape.

The independent review performed no edits, builds, tests, commits or further
delegation. This is a source-backed design PASS, not runtime evidence, owner
freeze, implementation authority or a performance claim.

## Documentation verification

The repository document/link checker passes for all 91 tracked and new live
Markdown files. Tracked diff and new-file whitespace checks pass. An independent
static calculation from the unchanged W7 authored table confirms eight balanced
W8 fresh vectors and totals `15 / 5 / 2 / 2 / 0 / 8 / 7 / 3 / 4 / 6`.
It checks the proposed document, not a running product.

W5/W6/W7 design hashes and the frozen W6 failure-continuation hash remain
unchanged. Rust, Cargo and CI files are unchanged. The original user's dirty
worktree remains untouched; all changes are in the dedicated performance
worktree. The latest hosted result is retained separately in
[W7 CI evidence](../phase1-lifecycle-boundary-diagnostic-2026-09-08/CI-EVIDENCE.md#lint-correction-hosted-verification),
not attributed to this unimplemented candidate.

## Authority and closeout

The completed design review changed documentation only. On 2026-09-09 the owner
replied "응응.. 그렇게해보아요" to the explicit proposal to freeze this reviewed
design and proceed with implementation and Windows/Linux verification. The
candidate hash above is now the exact frozen authority; its original bytes and
review-stage status line remain unchanged. A new matching external Rust
pre/post transaction is required before code changes.

This approval covers only W8 implementation, its required correctness evidence
and local control/candidate measurements. It does not authorize commit, push,
another hosted CI run, PR-ready, merge, broader optimization or amended budgets.
Implementation and runtime evidence belong in the separate
[implementation record](IMPLEMENTATION.md); its status is not implied by this
design PASS.
P1-60/P1-70 remain open.

## Publication approval

On 2026-09-09, after the completed local verification, the owner replied
"응응 선생님" to the explicit proposal to commit and push W8 to Draft PR #135
and check one resulting public CI run. This separate approval permits that
publication and verification only. Keep the PR Draft; no PR-ready, merge,
additional hosted rerun, broader optimization or budget amendment is approved.
The frozen design and prior measured artifact bindings remain unchanged.
