# REVIEW-002: Architecture v1 Independent Verification

Document role: current Architecture v1 freeze verification owner

Status: author preflight complete; exact independent re-verification pending

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
| Second resolution candidate | `924abb5965ea84c7d6e6c9db81137e02fbd5cbe9` |
| Current resolution candidate | `aef40633a24d377b2dc04db4d709113f7c4dd018`; independent verification still required |
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

## Author-Side Preflight

The architecture-authoring session re-read the exact second candidate before packaging it for external review. This is not independent verification. It found and corrected four additional ambiguities in the current candidate:

| Finding | Current candidate resolution |
| --- | --- |
| D-01 resolver override ambiguity | An invocation override supersedes only profile selection; unsupported explicit config is incomplete without an override, and unreadable controlling config remains incomplete under either path. |
| D-02 audit-start crash gap | Attempt catalog allocation, running-envelope publication, `latestAttempt` publication, and every later publication step now have distinct recovery outcomes. |
| D-03 operation revision/retry ambiguity | Same-ID retries join live work, retry only proven pre-commit interruption, or return one committed result; each later close revision requires a new operation ID. |
| D-04 lifecycle-store output mismatch | The repository transaction catalog is named `lifecycle.store` and is explicitly included in the bounded default output contract. |

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

## Exact Independent Re-verification Packet

The independent reviewer must inspect Git commit `aef40633a24d377b2dc04db4d709113f7c4dd018`, not this later ledger commit and not a loose export. The candidate-file manifest is sorted by repository-relative path using ordinal ordering; each line is `sha256`, two spaces, path, LF. Its SHA-256 is:

```text
2ff71a19ebd5fd2939f1aa6da77a2d3276c320791a19a1364670ab78d9c2210e
```

```text
d60f352d0db1404c70afb4bb8b2ca3fd1c610572aa40720e8a0b7baa7885418c  .gitattributes
b2c9f1a98c478549391043205022ae49b30338336e4f91f1cfd146ca1b46670e  .gitignore
ce17fef1d1331de060f0d2a6743086698531f55538c8306930e41758eb747572  AGENTS.md
18326ce8d50fd7154755912533d77e8ff987c8da48a104fc7244c24a13b8c139  README.md
2e540685f83e0ea730e1260de25649d379a142d685adce0eb5c8ea5ea45de36f  SDD.md
e4e84fb42557908c6e36e964d1d82cd564ec8ed6adb60d1f1f18ded27b4fb61b  WORKBOARD.md
a45ee5b881b55a504b3a773087b93b997e0b94ff95cd46b171de0f39fb53feb2  architecture/000-system-blueprint.md
131e86232beb6bf51ecf74329376388b8ba0cf9535ad030d044cecd967542ede  architecture/001-execution-and-ownership.md
96ab619e4dd4572c43343a2f45bc175586280417fe4e3a22ca8f62c29d3da949  architecture/002-evidence-and-write-gate.md
d542e203ff18f52bcc01a2561a12af54c5d82886a6a103f8a8adaf86fdefcd4c  specs/000-product-contract.md
7fd3df4f7683d0948c26047dd3e8c42235bbdd9a974e6ec3dd7ade5b75978625  specs/001-foundation-slice.md
84ede6d99086fa344e61ab6e453c77d4c20c5485784592a251d3ed7e0f805067  문서(한글)/AGENTS.ko.md
2456a9b89bb8f24a76b63d674cd62f0dcb64038ecada488a7d523af1476a1f28  문서(한글)/SDD.ko.md
```

The reviewer must report `PASS`, `REOPEN`, or a new finding for each of C-01 through C-07, D-01 through D-04, the packaged-skill and process-reopen proof repairs, and B-07 exact revision binding. It must also check that:

- one implementable Cargo edge exists for every gate signal/effect owner;
- pre-write and post-write each authorize one exact observation without claiming process authorship;
- concurrent shared-worktree close is serializable for active, terminal, stale, and unexplained intervening changes;
- every attempt/publication/retention crash point has one result and no automatic orphan success;
- operation retry cannot duplicate a gate or mutate two close revisions;
- default output, retention exclusions, skill probes, and process-reopen proof match the acceptance tables.

The resulting report must name the exact commit and manifest hash, state reviewer independence, preserve reopened/new IDs and accepted risks, and publish its own SHA-256. Document review cannot pass the measured backend, OXC, package, or performance gates below.

## Remaining Freeze Gates

1. Independently re-verify exact commit `aef40633a24d377b2dc04db4d709113f7c4dd018` using the packet above.
2. Run and record the store correctness/measurement comparison, including every publication, operation-recovery, and retention crash point.
3. Run and record OXC memory/stack feasibility.
4. Run and record Windows/Linux packaging feasibility, including both packaged skill adapters.
5. Approve numeric Phase 1 budgets from named probe evidence.
6. Obtain the separate independent design review required by repository policy.

Every gate above must be `passed` or an explicitly reviewed accepted risk with rationale, owner, scope, and expiry. There are currently no accepted risks. Architecture v1 remains draft and Phase 1 implementation remains blocked.
