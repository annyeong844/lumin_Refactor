# REVIEW-002: Architecture v1 Independent Verification

Document role: current Architecture v1 freeze verification owner

Status: seven document findings addressed; independent re-verification pending

Freeze decision: blocked

## Revision and Independence Record

| Field | Value |
| --- | --- |
| Original reviewed revision | `637218f89d9f963590af9788e6d90259aa145e4b` |
| First amended revision | `75ca5eda7457341d7e51d9cf2f63fb7d40198bd6` |
| Independent verification revision | `75ca5eda7457341d7e51d9cf2f63fb7d40198bd6` |
| Resolution candidate revision | `c0e49fbd15c82ce11d6efd6257ea41059fea6256`; independent verification still required |
| Uploaded verification-set manifest SHA-256 | `af9f166b28f753c347d52ccd06b0c522054a2d02fe6fb359249978c83fe298a3` |
| Independent report SHA-256 | `8580e750819b6d0552c1515069d281d0d613587eecf0a3eae1ca154fc4553f26` |
| Verifier | external independent reviewer, report supplied by the user; not the architecture-authoring Codex session |
| Verification result | blocked: F-03/F-07/F-11 reopened and B-04 through B-07 newly discovered |

The report verified the exact first amended revision above. This record does not call the current resolution independently verified until a separate reviewer checks the exact resolution candidate commit.

## Freeze Blocker Resolution Ledger

| Finding | Decision | Canonical resolution |
| --- | --- | --- |
| B-01 / F-03 identity meaning | Accept | ARCH-000/001 split software-only `AnalysisContractId` from repository `AnalysisInputId`; gates store both. |
| B-02 / F-11 query pinning | Accept | ARCH-002 requires run-scoped queries, gate revisions, scope-bound cursors, and explicit nested continuation flags. |
| B-03 / F-07 classification | Accept | SLICE-001 fixes scan precedence, source-role evidence, module-format selection, and public-lane union policy. |
| B-04 gate aggregation | Accept | ARCH-002 defines owner-issued `GateEffect`, fixed precedence, and the complete lifecycle transition table. |
| B-05 close-time drift | Accept | ARCH-002 binds close analysis and authorization to one revalidated `GateObservationId`. |
| B-06 crash publication | Accept | ARCH-002 defines the durable multi-step publication/recovery protocol and crash-point probes. |
| B-07 review identity | Accept | REVIEW-001/002 and WORKBOARD identify exact revisions, independence, results, and one current freeze owner. |

## High-Priority Follow-Up Ledger

| Finding | Resolution |
| --- | --- |
| Product AC crosswalk | SLICE-001 maps all 15 product ACs to slice proofs or completion gates. |
| Rename detection | ARCH-002 recognizes only unique persistent-file-identity rename; ambiguity is remove plus add. |
| Mixed filesystem case behavior | ARCH-002 records comparison behavior per existing parent/physical identity instead of assuming one root policy. |
| WSL `/mnt` benchmark | SLICE-001 makes it mandatory report-only diagnostic, excluded from blocking AC 16 budgets. |
| Probe reproducibility | ARCH-002/SLICE-001 retain exact source, fixture, toolchain, commands, invariants, and raw results under `reviews/probes`. |
| Retention command | ARCH-000 adds `lumin runs`; ARCH-002 owns paged immutable prune plans and plan-ID confirmation for run/gate retention. |
| SFC dialect extensibility | ARCH-000/001 and SLICE-001 define one dialect-aware SFC pipeline: Vue is the first production dialect, recognized unsupported dialects stay visible, and framework policy cannot leak into engine, resolver, or graph owners. |

## Remaining Freeze Gates

1. Independently re-verify the exact resolution candidate revision against B-01 through B-07 and the follow-up ledger.
2. Run and record the store correctness/measurement comparison, including every publication crash point.
3. Run and record OXC memory/stack feasibility.
4. Run and record Windows/Linux packaging feasibility.
5. Approve numeric Phase 1 budgets from named probe evidence.
6. Obtain the separate independent design review required by repository policy.

Every gate above must be `passed` or an explicitly reviewed accepted risk with rationale, owner, scope, and expiry. There are currently no accepted risks. Architecture v1 remains draft and Phase 1 implementation remains blocked.
