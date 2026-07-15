# REVIEW-002: Architecture v1 Independent Verification

Document role: current Architecture v1 freeze verification owner

Status: second resolution candidate recorded; exact independent re-verification pending

Freeze decision: blocked

## Revision and Independence Record

| Field | Value |
| --- | --- |
| Original reviewed revision | `637218f89d9f963590af9788e6d90259aa145e4b` |
| First amended revision | `75ca5eda7457341d7e51d9cf2f63fb7d40198bd6` |
| Independent verification revision | `75ca5eda7457341d7e51d9cf2f63fb7d40198bd6` |
| First resolution candidate | `c0e49fbd15c82ce11d6efd6257ea41059fea6256` |
| First verification-set manifest/report | `af9f166b28f753c347d52ccd06b0c522054a2d02fe6fb359249978c83fe298a3` / `8580e750819b6d0552c1515069d281d0d613587eecf0a3eae1ca154fc4553f26` |
| First-candidate re-verification upload/report | `2ce8e687527d29908928c261c8b18b6a5b05632b508f97556016bfb699bb5ba0` / `841bb2a7ca77dfb219d84a78616fa05dc12da1260b141c600eb27b94f68a771d` |
| Current resolution candidate | `924abb5965ea84c7d6e6c9db81137e02fbd5cbe9`; independent verification still required |
| Verifier | external independent reviewer, report supplied by the user; not the architecture-authoring Codex session |
| Verification result | first resolution candidate blocked: B-01/B-02 passed, prior B-03 scope passed, and C-01 through C-07 plus two proof gaps were identified |

The first report verified the exact first amended revision. The second report verified the supplied byte manifest but could not prove that loose upload was byte-identical to the declared Git commit. This record does not call the current resolution independently verified until a separate reviewer checks the exact current candidate commit.

## Freeze Blocker Resolution Ledger

| Finding | Decision | Canonical resolution in the current candidate |
| --- | --- | --- |
| B-01 / F-03 identity meaning | External PASS | ARCH-000/001 keep software-only `AnalysisContractId` separate from repository `AnalysisInputId`; gates store both. |
| B-02 / F-11 query pinning | External PASS | ARCH-002 requires run-scoped queries, immutable gate revisions, scope-bound cursors, and explicit nested continuation flags. |
| B-03 / F-07 original classification scope | External PASS | SLICE-001 fixes scan precedence, source-role evidence, module-format selection, and public-lane union policy. |
| C-01 active resolution profile | Accept finding | `lumin-resolve` owns invocation > nearest supported tsconfig > recorded `bundler` default selection; values enter `AnalysisInputId`, policy version enters `AnalysisContractId`. |
| C-02 gate-effect authority and root escape | Accept finding | Model-owned `GateSignal` crosses the existing Cargo DAG; `lumin-evidence::gate_policy` alone maps effects. Caller path escape is malformed with no record; later containment failures use named stale/block signals. |
| C-03 exact pre-write observation | Accept finding | Operation-scoped provisional reservation plus controller quiescence binds authorization to `GateBaselineObservationId`; close uses a distinct `GateCloseObservationId`. |
| C-04 shared-worktree attribution | Accept finding | ARCH-002 authorizes state transitions, not process authorship, and reconciles disjoint gates through an immutable ordered `WorktreeTransition` ledger. |
| C-05 crash outcome conflict | Accept finding | The crash table gives every point one result; a run renamed before terminal success remains an interrupted, unadoptable orphan. |
| C-06 commit/transport recovery | Accept finding | Caller-retained `OperationId` makes mutations idempotent and queryable after delivery failure; runtime locks are distinct from active durable path leases. |
| C-07 retention/latest integrity | Accept finding | Both latest-pointer targets and their linked attempt/run closure are prune-ineligible, with pointer state revalidated at confirmation. |
| B-07 exact review identity | Pending proof | This ledger names the exact current candidate, but only a new independent checkout verification can mark it passed. |

`Accept finding` records the architecture-authoring decision. It is not an independent `PASS`.

## High-Priority Follow-Up Ledger

| Finding | Resolution |
| --- | --- |
| Product AC crosswalk | PRODUCT-000 now has 16 ACs. SLICE-001 maps all 16 and adds explicit skill-adapter, process-reopen, operation-retry, crash, and retention proofs. |
| Rename detection | ARCH-002 recognizes only unique persistent-file-identity rename; ambiguity is remove plus add. |
| Mixed filesystem case behavior | ARCH-002 records comparison behavior per existing parent/physical identity instead of assuming one root policy. |
| WSL `/mnt` benchmark | SLICE-001 makes it mandatory report-only diagnostic, excluded from blocking AC 16 budgets. |
| Probe reproducibility | ARCH-002/SLICE-001 retain exact source, fixture, toolchain, commands, invariants, and raw results under `reviews/probes`. |
| Retention command | ARCH-000 adds `lumin runs`; ARCH-002 owns paged immutable prune plans, plan-ID confirmation, and latest-linkage exclusions for run/gate retention. |
| SFC dialect extensibility | ARCH-000/001 and SLICE-001 define one dialect-aware SFC pipeline: Vue is the first production dialect, recognized unsupported dialects stay visible, and framework policy cannot leak into engine, resolver, or graph owners. |

## Remaining Freeze Gates

1. Independently re-verify exact commit `924abb5965ea84c7d6e6c9db81137e02fbd5cbe9` against B-01/B-02, C-01 through C-07, B-07, and the follow-up ledger.
2. Run and record the store correctness/measurement comparison, including every publication, operation-recovery, and retention crash point.
3. Run and record OXC memory/stack feasibility.
4. Run and record Windows/Linux packaging feasibility, including both packaged skill adapters.
5. Approve numeric Phase 1 budgets from named probe evidence.
6. Obtain the separate independent design review required by repository policy.

Every gate above must be `passed` or an explicitly reviewed accepted risk with rationale, owner, scope, and expiry. There are currently no accepted risks. Architecture v1 remains draft and Phase 1 implementation remains blocked.
