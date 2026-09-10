# W9 Guard and Release Design Review

Design date: 2026-09-09. Independent review date: 2026-09-10.
Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
Candidate: [DESIGN.md](DESIGN.md).
Status: **frozen; structural-policy correction locally verified; next hosted verdict pending; performance acceptance open**.
Examined source: `12004f39127208740518d64b13c11e43b8652a34`.
Candidate SHA-256:
`e0abf1d62c5fe5349bf621acac07d8728381d6c3c8dc236cbdd930ff2da035ca`.

## Author review

The user approved preparing the next measurement design after read-only W8
diagnosis, not implementing instrumentation, starting a benchmark/CI, changing
budgets or closing applications. This packet changes documentation only.

| Examined source boundary | Design consequence |
| --- | --- |
| `namespace.rs:461`, `954` | Four profiled guard lifetimes have eight distinct entry/exit spans, each with an explicit backend drop in `validate_complete`. W9 does not move the guard's later natural destruction inside an exit span. |
| `namespace.rs:75`, `migration.rs:316`, `store_header.rs:63`, `migration/snapshot.rs:178` | Native admission's two verification calls use detached in-memory reconstruction. Keep their opaque whole-call timing separate from writable file-backend construction/release counts. |
| `namespace/database.rs:49`, `170`, `309` | Reuse the existing database/held-read body with explicit observer lending; preserve transaction, receipt, generation and rejection boundaries. |
| `publication/liveness.rs:327`, `366` | Successful release validates its session, commits Releasing, then completes physical/row cleanup. Three observed writable backends cannot be collapsed into one transaction. |
| `publication/liveness/records.rs:153`, `241`, `559`; `recovery.rs:233` | Session read explicitly drops its backend; marking/removing rows return naturally. Keep those distinct release observations and all error-path destruction orders. |
| `publication/run.rs:186` | Only the final successful `release_session` receives W9. Earlier validations, conflict/failure cleanup and catalog insertion remain outside the nine contexts. |
| W7 model/protocol/runner and W8 frozen counts | A v4 extension is required; v3's eight contexts/13 costs and current W8 oracle are not silently expanded or reinterpreted. |

The proposed fresh count sum is `11 / 1 / 2 / 2 / 0 / 9 / 2` for
open/read/write/commit/abort/explicit-drop/return-tail. This counts selected
writable lifetimes, not admission's detached backends or all audit opens.
W3 guard time includes admission and validation as well as acquisition; neither
that interval nor the new acquisition leaf is pure kernel contention time.
The author verdict is scoped design PASS for these boundaries, not execution
or performance acceptance.

## Independent adversarial review

On 2026-09-10 the user explicitly approved assigning a separate review agent.
The read-only `lifecycle_boundary_design_review` reviewer returned **scoped
DESIGN PASS**, with no material W9 blocker. It verified the source HEAD and
candidate SHA-256 above unchanged before and after review, and found no
working-tree changes to the reviewed source, manifests, bootstrap or CI files.
Earlier W7/W8 approvals were constraints, not substitutes for this review.

The independent reviewer confirmed:

- Nine ordered contexts and 19 unique costs agree with the actual call graph.
  Eight guard intervals each contribute one writable open/explicit drop;
  final release contributes `3 / 1 / 2 / 2 / 0 / 1 / 2`, giving the authored
  total `11 / 1 / 2 / 2 / 0 / 9 / 2`. Native healthy and exact pending-recovery
  fixtures retain these new vectors because recovery is outside the selected
  regions (`namespace.rs:954`, `publication/liveness.rs:366`).
- Session validation, Releasing commit, physical lock cleanup and lease-removal
  commit remain distinct. The return-tail markers can be implemented without
  moving resource scopes (`publication/liveness/recovery.rs:233`).
- The two opaque native-admission calls in the initialized authored fixtures
  reconstruct detached memory backends, not writable lifecycle-file backends
  (`migration.rs:316`, `migration/snapshot.rs:178`).
- Entry/exit lending can remain mutually exclusive with W7. Natural guard
  destruction remains after the exit span, and validation-first errors,
  explicit unlock and sticky rejection preserve their continuations
  (`namespace.rs:546`).
- The v4-only extension, feature-off separation, ordinary external-child
  driver, separate fault fixture and exact hosted/inverse admission rules
  provide a source-backed implementation route without execution authority.

The reviewer performed no edits, builds, tests, benchmarks, network actions or
further delegation. At design-review closeout, runtime source comparison,
deterministic tests, actual Windows/Linux children and hosted acceptance were
still unperformed; the current implementation disposition is below.

## Documentation verification

At design-review closeout, the read-only document checker passed all five changed/new Markdown files and
62 local link targets, with no trailing whitespace. Tracked `git diff --check`
passes; new-file whitespace is also covered by the document checker. Linked
section anchors were source-reviewed, not checked by that path-only script.
Static document arithmetic verifies nine ordered contexts, 19 costs and nine
balanced open/release vectors with the stated totals. It is not runtime proof.
The independent-review closeout repeats the document checks and validates all
15 attribution-table cells against the retained hashed report; candidate and
source identities remain unchanged.

All seven W8 report/manifest/build hashes in the new hosted evidence record
match the retained artifact files. Per-round attribution was recomputed from
the hashed report without executing the product. The previous full capture
verification is retained evidence, not claimed as a new 1,835-file rerun here.
That documentation-only phase edited no Rust, Cargo, CI or frozen predecessor
design bytes, and performed no builds, benchmarks, CI requests, app termination,
commit or push. The original user's unrelated dirty worktree remained unchanged.

## Authority

On 2026-09-10, after receiving the independent scoped PASS, the owner answered
“응 부탁드려요” to the explicit proposal to freeze this design and implement
its instrumentation. This freezes the exact candidate SHA-256 above and
authorizes scoped implementation and local verification. The candidate bytes
remain unchanged; their historical candidate labels are superseded only by this
disposition. Neither design PASS nor this approval is runtime or numeric PASS.
No resource lifetime, product semantics or budget changes are authorized.
Commit/push, benchmark publication and another hosted run were not included in
that implementation approval. P1-60/P1-70 remain open.

After scoped local verification completed on 2026-09-10, the owner answered
“응응 선생님. ;ㅅ; 계속해주세요” to the explicit proposal to commit, push and
check CI. The publication scope is this W9 packet and one PR-synchronized hosted
run under DESIGN's fifth verification requirement: replace only the W7 Windows
diagnostic build/probe/run/artifact bindings with W9 after the unchanged ordinary
benchmark, retaining every prerequisite and failure outcome. No rerun, automatic
merge, local cold benchmark, resource-lifetime change or budget change is
authorized by this step. The existing Draft PR remains Draft.

Current implementation and completed scoped local verification are recorded in
[IMPLEMENTATION.md](IMPLEMENTATION.md). Both platforms pass ordinary/W9 release,
actual v1/v2/v3/v4 release-child, staged package/adapter, scoped owner, separated
Clippy, xtask, launcher and structural checks. The record distinguishes retained
initial-source Windows feature-off/fault tests from final-r2-source Linux tests;
it does not claim those Windows tests were rerun after the lint-only correction.
Final format/document checks and source/artifact binding also pass. Hosted CI,
numerical acceptance and Phase 1 exit were not established by those local checks.

The [completed hosted run](CI-EVIDENCE.md) passes both package/ordinary benchmark
jobs and the W9 diagnostic, but overall CI fails because the separate Rust
structural policy was not synchronized with the workflow. After this omission
was reported, the owner answered “응응 선생님. 계속해주세요” to the explicit
proposal to fix it, verify locally, and push a corrective commit after the
existing CI finished, followed by one new-head CI. This is not same-head rerun,
merge, product-change or budget-change authority. The complete failed run is
preserved; the existing Draft remains Draft.

## Independent implementation source review

On 2026-09-10 the owner explicitly approved the proposed separate read-only
implementation review. The `w9_lifetime_source_review` reviewer examined the
actual diff against `12004f39127208740518d64b13c11e43b8652a34`, including all five
new Rust files and the namespace `LockProfilePhases` correction, and returned
**scoped SOURCE PASS** with no actionable P1/P2 findings. All 26 changed Rust
files matched `source-bindings-r2.json` (SHA-256
`ccf48a623c05f4559462a1dcd29a43dca79fab3bca07660fc73e7c67ca0064ef`);
the frozen design hash above also matched.

The review independently traced guard construction, callback and validation-first
error precedence, explicit unlock and natural guard destruction after W3 exit;
database/transaction fields, failed opens, held-read completion, sticky rejection
and explicit drop/error propagation; and the separate Releasing and lease-removal
commits with both immediate-caller return tails. It confirmed the unchanged
attempt-lock validation/drop/removal/parent-sync order, exclusive locally borrowed
W7/W9 observer destinations, and the source-derived new count vector
`11 / 1 / 2 / 2 / 0 / 9 / 2`. Native admission's two detached-memory verification
calls remain opaque rather than extra writable admissions.

The reviewer also inspected the shared recorder, model/protocol/CLI/xtask changes
and new tests for the closed 9/19 inventory, checked arithmetic, feature-off
separation, predecessor wire preservation and strict v4 decoding. It performed
no edits, builds, tests, benchmark, network action or further delegation. This is
independent source evidence, not binary-erasure, platform-runtime or performance
certification; executable verification is separately recorded in IMPLEMENTATION.
