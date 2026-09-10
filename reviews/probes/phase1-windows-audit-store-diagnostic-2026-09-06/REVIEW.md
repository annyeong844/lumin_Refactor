# W3 Store Diagnostic Review Record

Date: 2026-09-06. Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).

## Candidate and authority

| Item | Disposition |
| --- | --- |
| Candidate | [DESIGN.md](DESIGN.md), W3, UTF-8/LF |
| Candidate SHA-256 | `6d916f24a3b78e99de8b04e5164e6c0199ea30cae401dea5e1a5e93485025fa8` |
| Source inspected | `062964192a1d3f16f5b7739f0d98eb85ac5bef4d` |
| Author design review | PASS for this diagnostic-only candidate; source trace and checks below |
| Independent adversarial review | Scoped PASS from read-only reviewer `store_diagnostic_design_review` for the revised candidate hash above |
| Owner freeze / implementation approval | **Granted on 2026-09-06 for the exact candidate hash above** |
| Product / numeric outcome | Unchanged; actual Windows ratio `0.9834988947164788` misses `0.75` |

This record does not reuse W2's independent PASS or approval for a broader
candidate. The owner approved freezing W3 and implementing its isolated store
diagnostic after both reviews passed. This record freezes the exact DESIGN.md
bytes above without rewriting the reviewed candidate's status text. It grants
only the instrumentation, feature/build isolation, and verification specified
there, not optimization, permanent metrics, or numeric changes. P1-60/P1-70
remain open.

## Author source review

1. `crates/application/store/src/lib.rs`, `RepositoryStore::open` (382) and
   `from_namespace` (397): namespace open and publication recovery are separate
   existing calls. No timing belongs on persistent/shared store state.
2. `crates/application/store/src/namespace/bootstrap.rs`, `bootstrap_namespace` (28), `finish_bootstrap`
   (116), and `publish_repository_marker` (179): fresh setup, bound parent
   creation, marker publication, store creation, and final validation have
   distinct existing boundaries. Their handles, flush order, and fallback
   publication protocol must remain unchanged.
3. `crates/application/store/src/namespace.rs`, `with_lock` (400): initial identity validation/lock/admission
   precede the operation; final validation/unlock follow it. The proposed
   entry/tail spans are deliberately not labeled lock-wait time. Natural guard
   destruction remains in the enclosing residual instead of being relocated.
4. `crates/application/store/src/publication/liveness.rs`, `begin` (22) and `recover` (101), and
   `crates/application/store/src/publication/liveness/records.rs`, `reserve` (52), `activate` (106): each
   named attempt transition is already a synchronous owner call. Its complete
   elapsed duration includes nested validation, database access, transaction
   work, and ordinary resource release, not only a commit.
5. `crates/application/store/src/publication/run.rs`, `prepare_publication` (37), `finalize_publication`
   (69), `finalize_under_guard` (93), `publish_directory` (201), and
   `write_staging` (288): shared prepare and exclusive finalization remain
   separate. Final-input preflight is nested in preparation. Candidate
   revalidation, catalog publication, latest publication, and lease release
   are measured without changing their ordering or guard scope.
6. `crates/application/store/src/lib.rs`, `write_evidence_store` (728): create, begin-write,
   chunk insertion, commit, and an already explicit database drop provide
   concrete immutable-evidence backend boundaries. They do not measure all
   lifecycle-store commits or prove device flush latency.
7. `crates/application/store/src/namespace/database.rs`, `commit_with_test_barrier` (94): repeated bound
   entry/current database/receipt checks surround the backend commit. W3
   leaves this protocol untouched and does not relabel a containing owner
   interval as the one backend API call.
8. `crates/application/engine/src/audit_profile.rs` and the adjacent `audit_publication.rs`: existing W2
   measurements and store preflight share one engine-owned recorder. Returning
   a separate store-owned observation avoids lending it into the parallel
   callback or changing W2's 23-phase parent/residual definitions.
9. `crates/foundation/model/src/audit_diagnostic.rs`, `crates/application/protocol/src/audit_diagnostic.rs`,
   `tools/xtask/src/benchmark/diagnostic.rs`, and
   `tools/xtask/bootstrap/source_provenance.py`: W2 is closed v1, the schedule
   and recorder are bounded, and hosted target admission uses exact command
   vectors. W3 therefore needs a distinct frame/report, explicit feature
   closure, and an exact new command/target map, not silent v1 extension or a
   feature-substring exception. The existing public-child probe feature
   remains separate from either measured product feature.

Paths are repository-relative. Line numbers describe the exact inspected
source, not a future implementation.

## Author consistency and adversarial cases

The phase table has 52 unique ordered rows, three independent roots, and
five optional bootstrap rows; every child names an earlier parent. Three
mutable local recorders return owned values, leaving store/guard/session
sharing unchanged and keeping the recorder out of parallel child tasks. Root-scoped
merging must reject duplicate/missing roots or foreign-subtree data.

The proposal explicitly retains parent residuals and the separate overlapping
W2/store views, so a reader cannot add both final-input spans or sum medians
as a timeline. It names fresh/existing bootstrap distinctions and tests absent
versus zero-duration work. It preserves W2 version rejection, product-error
unwind, committed-output failure recovery, hosted target crossovers, raw
archive failures, and ordinary red CI even when diagnostics succeed.

Independent review must still challenge actual call/lifetime feasibility,
the exact row inventory on a successful cold child, resumed/existing states,
double-counting and omitted drops, feature-off erasure, private feature
closure, hosted policy admission, and the fact that future acceptance tests
have not yet executed. Any concrete finding revises the candidate and its
hash before freeze. A successful W3 design does not select an optimization.

## Independent finding and resolution

The separately delegated, read-only reviewer `store_diagnostic_design_review`
reviewed the complete initial W3 candidate at SHA-256
`d367b303ec808a7e9978db02e78038db1f8ba9a075944f474f1aee02000acb74`.
It verified that hash, W2's frozen hash, and source HEAD before and after its
review, and returned **NEEDS REVISION** with one P2 design contradiction.

The initial DESIGN.md line 63 prohibited the recorder from ever reaching a
Rayon worker. `crates/application/engine/src/lib.rs:108` executes `work`
through `pool.install`; its coordinating audit invokes store open and attempt
begin at lines 319-334 and publication through
`crates/application/engine/src/audit_publication.rs:26`. Literal compliance
would relocate calls that this same design requires to remain unchanged.
This is a design contradiction, not an observed product defect.

The author confirmed that trace and narrowed the wording: local recording
follows the existing coordinating audit call, including when it runs on a
worker inside `pool.install`; the mutable recorder cannot be passed to,
captured by, or shared with parallel extraction/preflight child tasks, and
measurement schedules no extra task. The 52 phases, transport, feature/build
isolation, durability, and budget requirements did not change. The revised
candidate hash is the one in the table above; its re-review is recorded below.

The first review found no other material counterexample in phase inventory,
bootstrap/resume distinctions, natural destruction and lock boundaries,
v1 preservation/v2 framing, feature isolation, hosted command maps,
release-child proof, capture failures, or unchanged ordinary CI authority.
The reviewer made no edits and performed no builds, product executions,
fresh measurements, or PR actions. Its conclusion applies only to the
reviewed design; implementation acceptance remains unexecuted.

On re-review, the same independent reviewer reread the complete revised DESIGN
and this record, rechecked the engine/preflight boundaries, and returned
**PASS for the exact revised W3 design**. It verified source HEAD and both
candidate/frozen-design hashes before and after, and independently confirmed
that reversing only the corrected paragraph reproduces the original W3 hash.
It found no remaining material counterexample. Product/tooling sources
remained unchanged from HEAD, and the review stayed read-only.

After this independent review, the user explicitly approved the proposed W3
freeze and diagnostic implementation on 2026-09-06. Both reviews and that
approval bind only the revised hash above. This is not a whole-amendment
REVIEW-005 approval, runtime acceptance, numeric PASS, or merge authority.

## Verification boundary

Only source/document reads and documentation/phase-table checks were in scope
for the design-review phase. No new performance run, Rust build, product execution, external
legacy pre/post transaction, commit, push, or PR status change is claimed.
The actual W2 CI bindings are retained in its
[measurement owner](../phase1-windows-audit-execution-diagnostic-2026-09-05/CI-EVIDENCE.md),
not inferred from this design. W2 DESIGN.md remains byte-for-byte frozen.

Completed author checks, using pinned Python 3.13.14 with isolated imports and
bytecode output disabled:

- `ci_policy.py check-documentation --base HEAD --head HEAD`: 60 live documents pass.
- The same `document_errors` owner explicitly checked all five changed/new
  Markdown files, including the untracked probe: links and UTF-8 pass. Separate
  final-newline and trailing-whitespace checks pass for all five.
- `test_ci_policy.py`: 12 tests pass. These test the existing policy checker,
  not the proposed W3 instrumentation or future hosted command mappings.
- A direct table check confirms 52 unique ordered phases, the exact three
  roots, five bootstrap rows, and only earlier-parent references.
- `git diff --check`: pass. W2 design SHA-256 remains
  `9bb93825979d326b946e59ef34873b0e767d79f8d6f78a093c836e046ee288de`.

No acceptance item requiring implemented W3 code or actual W3 Windows/Linux
processes is claimed complete by this design review. Implementation and runtime
verification are recorded separately in [IMPLEMENTATION.md](IMPLEMENTATION.md),
not inferred from the design approval.
