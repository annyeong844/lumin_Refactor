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
| Externally reverified candidate | `aef40633a24d377b2dc04db4d709113f7c4dd018`; supplied bytes were checked but exact Git binding remained pending |
| Current resolution candidate | `f2d1fa663a0b1ba3e3fec84821dc0861c96cce54`; independent verification still required |
| Verifier | external independent reviewer, report supplied by the user; not the architecture-authoring Codex session |
| Externally reverified input manifest/report | `2ff71a19ebd5fd2939f1aa6da77a2d3276c320791a19a1364670ab78d9c2210e` / `67a001b2cbfd6af36d3e60712d8dfa2bab6dfcc4a7756114f87b9d34d5530611` |
| Verification result | `aef4063` remained blocked: C-02/C-05 and several proof repairs passed, while C-01/C-03/C-06/C-07/D-01 reopened and E-01 through E-07 were identified |

The first report verified the exact first amended revision. Later reports verified supplied byte manifests but could not prove loose uploads byte-identical to the declared Git commits or inspect both canonical Korean files. This record does not call the current resolution independently verified until a separate reviewer checks the exact current candidate checkout.

## Freeze Blocker Resolution Ledger

| Finding | Decision | Canonical resolution in the current candidate |
| --- | --- | --- |
| B-01 / F-03 identity meaning | External PASS | ARCH-000/001 keep software-only `AnalysisContractId` separate from repository `AnalysisInputId`; gates store both. |
| B-02 / F-11 query pinning | External PASS | ARCH-002 requires run-scoped queries, immutable gate revisions, scope-bound cursors, and explicit nested continuation flags. |
| B-03 / F-07 original classification scope | External PASS | SLICE-001 fixes scan precedence, source-role evidence, module-format selection, and public-lane union policy. |
| C-01 / D-01 active profile and override | External REOPEN -> accept | Invocation override now applies to every physical and embedded-script importer; Vue template binding is not a second resolver lane. Effective inherited tsconfig values and the `bundler` default remain explicit. |
| C-02 gate-effect authority and root escape | External PASS | Model-owned `GateSignal` crosses the existing Cargo DAG; `lumin-evidence::gate_policy` alone maps effects. Caller path escape is malformed with no record; later containment failures use named stale/block signals. |
| C-03 exact observation | External REOPEN -> accept | ARCH-001/002 require owner-reported `ConsultedSemanticInputs`, monotonic reservation/capture reruns, and fixed-point sealing before baseline or close observation IDs. |
| C-04 shared-worktree attribution | External PASS with E-01 dependency | Immutable transition reconciliation remains state-based; fixed-point semantic-read closure now supplies the complete read set required by that model. |
| C-05 crash outcome conflict | External PASS | The publication crash table gives every point one result; a run renamed before terminal success remains an interrupted, unadoptable orphan. |
| C-06 commit/transport recovery | External REOPEN -> accept | The operation contract now covers pre/post, abandon, pin/unpin, prune-plan creation, and confirmation, with one general `lumin operation show` recovery query. |
| C-07 retention/latest integrity | External REOPEN -> accept | Latest closures remain ineligible, and retention now has explicit `Prepared -> Pruning -> Pruned` tombstone/trash ownership and crash recovery. |
| B-07 exact review identity | Pending proof | This ledger names the exact current candidate, but only a new independent checkout verification can mark it passed. |
| E-01 semantic-read closure | Accept finding | Owners return exact consulted inputs; every read-set expansion rechecks reservations/conflicts and reruns affected owners until a finite fixed point before observation sealing. |
| E-02 command-wide mutation idempotency | Accept finding | Every gate/retention lifecycle mutation requires `OperationId`; read-only plan/show/page commands do not. |
| E-03 retention deletion crash state | Accept finding | Canonical tombstones, same-filesystem trash ownership, logical `Pruned` commit, and idempotent reclamation have a single crash table. |
| E-04 executable retention proof | Accept finding | SLICE-001 includes public run/gate retention commands and requires corpus fault injection through child-process public DTOs rather than private store writes. |
| E-05 Vue/profile precedence | Accept finding | Embedded Vue scripts use ordinary importer profile precedence; template binding consumes resolved bindings and `<script src>` remains an exact SFC source reference. |
| E-06 explicit entry contract | Accept finding | Repeated `--entry` replaces config entries; normalization, containment, exclusions, empty/private behavior, `AnalysisInputId`, and gate read participation are normative. |
| E-07 exhaustive limitation scope | Accept finding | SLICE-001 has a closed reason registry with one fact owner, scope derivation, absence effect, and gate mapping; exhaustive owner matches are architecture-checked. |

`Accept finding` records the architecture-authoring decision. It is not an independent `PASS`.

## Author-Side Preflight

The architecture-authoring session re-read each candidate before packaging it for external review. This is not independent verification. The latest external report returned the following status for the earlier D findings:

| Finding | Current candidate resolution |
| --- | --- |
| D-01 resolver override ambiguity | External REOPEN under E-05; the current candidate removes the Vue second-lane contradiction. |
| D-02 audit-start crash gap | External PASS; attempt allocation, running-envelope publication, `latestAttempt`, and every later publication step have distinct recovery outcomes. |
| D-03 operation revision/retry ambiguity | External PASS for pre/post; E-02 extends the same contract to every remaining lifecycle mutation. |
| D-04 lifecycle-store output mismatch | External PASS; `lifecycle.store` is included in the bounded default output contract. |

## High-Priority Follow-Up Ledger

| Finding | Resolution |
| --- | --- |
| Product AC crosswalk | PRODUCT-000 now has 18 ACs. SLICE-001 maps all 18 through 21 slice AC rows, including semantic-read closure and public retention proofs. |
| Rename detection | ARCH-002 recognizes only unique persistent-file-identity rename; ambiguity is remove plus add. |
| Mixed filesystem case behavior | ARCH-002 records comparison behavior per existing parent/physical identity instead of assuming one root policy. |
| WSL `/mnt` benchmark | SLICE-001 makes it mandatory report-only diagnostic, excluded from blocking AC 16 budgets. |
| Probe reproducibility | ARCH-002/SLICE-001 retain exact source, fixture, toolchain, commands, invariants, and raw results under `reviews/probes`. |
| Retention command | ARCH-000 exposes `lumin runs` and general operation recovery; ARCH-002 owns public plan/show/confirm commands, operation IDs, tombstone/trash recovery, and latest-linkage exclusions. |
| SFC dialect extensibility | ARCH-000/001 and SLICE-001 define one dialect-aware SFC pipeline: Vue is the first production dialect, recognized unsupported dialects stay visible, and framework policy cannot leak into engine, resolver, or graph owners. |
| Explicit entry/profile selection | SLICE-001 closes config/CLI precedence, private-package empty coverage, effective tsconfig inheritance, and embedded Vue importer behavior. |
| Limitation exhaustiveness | Every first-slice typed incomplete/unsupported/opaque reason has one compile-time checked owner/scope/absence/effect mapping. |

## Exact Independent Re-verification Packet

The independent reviewer must inspect Git commit `f2d1fa663a0b1ba3e3fec84821dc0861c96cce54`, not this later ledger commit and not a loose export. The candidate-file manifest is sorted by repository-relative path using ordinal ordering; each line is `sha256`, two spaces, path, LF. Its SHA-256 is:

```text
9d2366afa0fa360397fbf4ae7c0ad4205d34739f20f8f7acff70207b2152b6fd
```

```text
d60f352d0db1404c70afb4bb8b2ca3fd1c610572aa40720e8a0b7baa7885418c  .gitattributes
b2c9f1a98c478549391043205022ae49b30338336e4f91f1cfd146ca1b46670e  .gitignore
ce17fef1d1331de060f0d2a6743086698531f55538c8306930e41758eb747572  AGENTS.md
18326ce8d50fd7154755912533d77e8ff987c8da48a104fc7244c24a13b8c139  README.md
2e540685f83e0ea730e1260de25649d379a142d685adce0eb5c8ea5ea45de36f  SDD.md
e4e84fb42557908c6e36e964d1d82cd564ec8ed6adb60d1f1f18ded27b4fb61b  WORKBOARD.md
dfefa949342c7f81351a14a13c91a684e820478d8103e06fd0c46d776ac69170  architecture/000-system-blueprint.md
9cc4cfc6a0ce27cf14ee988c2f50e154054b1bfc6f5fe778a3fd766ea4b46201  architecture/001-execution-and-ownership.md
6775b173a7a4892d511799ac081235e1e02c18f75399ecacd38c378b58d31188  architecture/002-evidence-and-write-gate.md
64831ac36f05fa10893e02451bfb43463830e26c2e333a50a1c522c0ee7117bb  specs/000-product-contract.md
5608a31b7fb629c18d2cf1263037147d67f1cfe048dfedb4bd2ccef1f9534fcb  specs/001-foundation-slice.md
84ede6d99086fa344e61ab6e453c77d4c20c5485784592a251d3ed7e0f805067  문서(한글)/AGENTS.ko.md
2456a9b89bb8f24a76b63d674cd62f0dcb64038ecada488a7d523af1476a1f28  문서(한글)/SDD.ko.md
```

The reviewer must report `PASS`, `REOPEN`, or a new finding for E-01 through E-07, every externally reopened C/D item, the packaged-skill and process-reopen proofs, and B-07 exact revision binding. It must also check that:

- one implementable Cargo edge exists for every gate signal/effect owner;
- pre-write and post-write seal capability-reported semantic reads by fixed point before authorizing one exact observation;
- concurrent shared-worktree close is serializable for active, terminal, stale, and unexplained intervening changes;
- every attempt/publication/retention crash point has one result, no automatic orphan success, and no missing-payload success;
- operation retry cannot duplicate pre/post, abandon, pin/unpin, prune-plan, or confirmation mutations;
- retention corpus uses public commands rather than private store mutation;
- explicit entries, effective resolver profiles, and embedded Vue scripts have one precedence;
- every first-slice limitation reason has one exhaustive owner/scope/absence/effect mapping;
- default output, retention exclusions, skill probes, and process-reopen proof match the acceptance tables;
- both canonical Korean files and all other manifest entries are read from the exact Git tree rather than omitted from a loose upload.

The resulting report must name the exact commit and manifest hash, state reviewer independence, preserve reopened/new IDs and accepted risks, and publish its own SHA-256. Document review cannot pass the measured backend, OXC, package, or performance gates below.

## Remaining Freeze Gates

1. Independently re-verify exact commit `f2d1fa663a0b1ba3e3fec84821dc0861c96cce54` using the packet above.
2. Run and record the store correctness/measurement comparison, including every publication, lifecycle-operation recovery, and tombstone/trash retention crash point.
3. Run and record OXC memory/stack feasibility.
4. Run and record Windows/Linux packaging feasibility, including both packaged skill adapters.
5. Approve numeric Phase 1 budgets from named probe evidence.
6. Obtain the separate independent design review required by repository policy.

Every gate above must be `passed` or an explicitly reviewed accepted risk with rationale, owner, scope, and expiry. There are currently no accepted risks. Architecture v1 remains draft and Phase 1 implementation remains blocked.
