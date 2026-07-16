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
| Earlier externally reverified candidate | `aef40633a24d377b2dc04db4d709113f7c4dd018`; supplied bytes were checked but exact Git binding remained pending |
| Latest externally reverified candidate | `f2d1fa663a0b1ba3e3fec84821dc0861c96cce54`; 11 supplied candidate files matched the packet, while exact Git binding and both Korean-source bytes remained pending |
| Current resolution candidate | `eb815bbe5447214e980aae98dfee50fef09eae7c`; independent verification still required |
| Verifier | external independent reviewer, report supplied by the user; not the architecture-authoring Codex session |
| Earlier externally reverified input manifest/report | `2ff71a19ebd5fd2939f1aa6da77a2d3276c320791a19a1364670ab78d9c2210e` / `67a001b2cbfd6af36d3e60712d8dfa2bab6dfcc4a7756114f87b9d34d5530611` |
| Latest externally reverified input manifest/report | `9d2366afa0fa360397fbf4ae7c0ad4205d34739f20f8f7acff70207b2152b6fd` / `07df439ded1101b3ddea1328880579be918db1cfbe8d7861d28c0ca1d18ad20a` |
| Verification result | `f2d1fa6` remained blocked: E-02/E-04/E-05/E-06 passed, E-01/E-03/E-07 and C-03/C-07 reopened, G-01 through G-07 were identified, and B-07 remained pending |

The first report verified the exact first amended revision. Later reports verified supplied byte manifests but could not prove loose uploads byte-identical to the declared Git commits or inspect both canonical Korean files. This record does not call the current resolution independently verified until a separate reviewer checks the exact current candidate checkout.

## Freeze Blocker Resolution Ledger

| Finding | Decision | Canonical resolution in the current candidate |
| --- | --- | --- |
| B-01 / F-03 identity meaning | External PASS | ARCH-000/001 keep software-only `AnalysisContractId` separate from repository `AnalysisInputId`; gates store both. |
| B-02 / F-11 query pinning | External PASS | ARCH-002 requires run-scoped queries, immutable gate revisions, scope-bound cursors, and explicit nested continuation flags. |
| B-03 / F-07 original classification scope | External PASS | SLICE-001 fixes scan precedence, source-role evidence, module-format selection, and public-lane union policy. |
| C-01 / D-01 active profile and override | External PASS | Invocation override applies to every physical and embedded-script importer; Vue template binding is not a second resolver lane. Post-write preserves the caller override while validated self-written config may recompute effective importer profiles. |
| C-02 gate-effect authority and root escape | External PASS | Model-owned `GateSignal` crosses the existing Cargo DAG; `lumin-evidence::gate_policy` alone maps effects. Caller path escape is malformed with no record; later containment failures use named stale/block signals. |
| C-03 exact observation | External REOPEN under G-01/G-02 -> accept | ARCH-001/002 now replay complete owner cache envelopes, close semantic reads by fixed point, and persist typed sealed or unsealed observation bindings without partial IDs. |
| C-04 shared-worktree attribution | External PASS with E-01 dependency | Immutable transition reconciliation remains state-based; fixed-point semantic-read closure now supplies the complete read set required by that model. |
| C-05 crash outcome conflict | External PASS | The publication crash table gives every point one result; a run renamed before terminal success remains an interrupted, unadoptable orphan. |
| C-06 commit/transport recovery | External PASS | The operation contract covers pre/post, abandon, pin/unpin, prune-plan creation, and confirmation, with one general `lumin operation show` recovery query. |
| C-07 retention/latest integrity | External REOPEN under G-03/G-05 -> accept | Latest closures remain ineligible; retention now has a model-owned plan ID/scope, public tombstone lookup, explicit `Prepared -> Pruning -> Pruned` ownership, and crash recovery. |
| B-07 exact review identity | Pending proof | This ledger names the exact current candidate, but only a new independent checkout verification can mark it passed. |
| E-01 semantic-read closure | External REOPEN under G-01 -> accept | Cold and validated-cache owners return the complete envelope; every read-set expansion rechecks reservations/conflicts and reruns affected owners until a finite fixed point before observation sealing. |
| E-02 command-wide mutation idempotency | External PASS | Every gate/retention lifecycle mutation requires `OperationId`; read-only plan/show/page commands do not. |
| E-03 retention deletion crash state | External REOPEN under G-03/G-04/G-05 -> accept | Canonical plans, versioned total ordering, public tombstones, same-filesystem trash ownership, logical `Pruned` commit, and idempotent reclamation now form one queryable crash contract. |
| E-04 executable retention proof | External PASS | SLICE-001 includes public run/gate retention commands and requires corpus fault injection through child-process public DTOs rather than private store writes. |
| E-05 Vue/profile precedence | External PASS | Embedded Vue scripts use ordinary importer profile precedence; template binding consumes resolved bindings and `<script src>` remains an exact SFC source reference. |
| E-06 explicit entry contract | External PASS | Repeated `--entry` replaces config entries; normalization, containment, exclusions, empty/private behavior, `AnalysisInputId`, and gate read participation are normative. |
| E-07 exhaustive limitation scope | External REOPEN under G-07 -> accept | SLICE-001 separates the closed static reason/scope/absence/relevance registry from owner-produced lifecycle delta signals and effect mapping. |
| G-01 warm-cache semantic-read bypass | Accept finding | `CachedCapabilityOutput` contains owner version, exact key, facts, signals, limitations, and consulted inputs; a hit validates and replays the whole envelope or becomes a full miss/incomplete result. |
| G-02 nonauthorizing observation shape | Accept finding | `ObservationBinding` is sealed baseline/close identity or typed unsealed attempted-domain data. Active gates require sealed openings; unsealed rejected/close revisions omit authoritative input IDs and complete fingerprints. |
| G-03 retention-plan identity and cursor | Accept finding | Model-owned, store-allocated `RetentionPlanId` and immutable content identity scope plan pages independently of unrelated catalog mutation; confirmation still revalidates current state. |
| G-04 collection ordering | Accept finding | Every public collection has one versioned total key. Retention record-kind ranks cover attempts, runs, gates/revisions, findings, evidence, operations, transitions, references, orphans, and tombstones. |
| G-05 public pruning/pruned truth | Accept finding | Direct record, plan, and operation lookup project typed `Live`, `Pruning`, `Pruned`, `NeverExisted`, or `Corrupt` state; deletion never appears as an empty finding set. |
| G-06 scan invocation surface | Accept finding | Audit/pre-write expose include/exclude/role flags; post-write reuses the caller override tier, rejects replacement, and containment classes have one result. |
| G-07 static reason versus lifecycle delta | Accept finding | Static limitation rows own scope, absence effect, and relevance only. Post-write owner classifications alone emit introduced/expanded/unchanged/resolved/baseline-unavailable adverse signals. |

`Accept finding` records the architecture-authoring decision. It is not an independent `PASS`.

## Author-Side Preflight

The architecture-authoring session re-read each candidate before packaging it for external review. This is not independent verification. For `eb815bb`, a separate adversarial sub-review first found and then confirmed closure of these author-side contradictions:

| Check | Current candidate resolution |
| --- | --- |
| Fixed-point control flow | Open/close stop only on conflict, unbounded, or unobservable input; admissible growth extends reservation, captures identities, and reruns owners; no-growth seals. |
| Sealed/unsealed record shape | Conditional input IDs, read sets, and fingerprints exist only for sealed observations; unsealed records retain typed attempted-domain data without placeholders. |
| Self-writable semantic inputs | A config path in both this gate's leased and actual write sets is recaptured and reanalyzed; external or unexplained protected-read drift remains stale. |
| Caller override versus effective config | Post-write preserves caller flags while a validated self-written config may recompute effective profile/entry/scan facts. |
| Limitation versus lifecycle delta | Static limitation registry owns scope/absence/relevance; post-write adverse effects require owner delta classification. |
| Total collection ordering | Relation rows have stable semantic IDs, and retention plan rank covers every record kind in run/gate closure. |
| Binding constructor consistency | Open and close construct the declared `Sealed(Baseline(...))` and `Sealed(Close(...))` variants exactly. |

The same re-review returned no remaining P1/P2 finding after these repairs. Earlier D-02, D-03, and D-04 remain externally passed; D-01 is covered by the externally passed profile finding and the self-writable-config clarification above.

## High-Priority Follow-Up Ledger

| Finding | Resolution |
| --- | --- |
| Product AC crosswalk | PRODUCT-000 has 18 ACs. SLICE-001 maps all 18 through 28 slice AC rows, including warm/cold closure, honest observations, public retention, and lifecycle delta proofs. |
| Rename detection | ARCH-002 recognizes only unique persistent-file-identity rename; ambiguity is remove plus add. |
| Mixed filesystem case behavior | ARCH-002 records comparison behavior per existing parent/physical identity instead of assuming one root policy. |
| WSL `/mnt` benchmark | SLICE-001 makes it mandatory report-only diagnostic, excluded from blocking AC 16 budgets. |
| Probe reproducibility | ARCH-002/SLICE-001 retain exact source, fixture, toolchain, commands, invariants, and raw results under `reviews/probes`. |
| Retention command | ARCH-000 exposes `lumin runs` and general operation recovery; ARCH-002 owns public plan/show/confirm commands, operation IDs, tombstone/trash recovery, and latest-linkage exclusions. |
| SFC dialect extensibility | ARCH-000/001 and SLICE-001 define one dialect-aware SFC pipeline: Vue is the first production dialect, recognized unsupported dialects stay visible, and framework policy cannot leak into engine, resolver, or graph owners. |
| Explicit entry/profile selection | SLICE-001 closes config/CLI precedence, private-package empty coverage, effective tsconfig inheritance, and embedded Vue importer behavior. |
| Limitation exhaustiveness | Every first-slice typed incomplete/unsupported/opaque reason has one compile-time checked owner/scope/absence/relevance mapping; lifecycle effects are delta-owned. |
| Opening/close input relationship | Caller override values remain fixed; self-writable config facts are recomputed into the close input identity; protected external reads require exact freshness. |
| Independent run references | Every pin allocates `PinId`; one consumer cannot remove another consumer's protection. |
| Lifecycle-store migration | Copy-on-write migration preserves attempts, operations, transitions, plans/tombstones, pins, gates, revisions, and catalog sequences. |
| Tombstone lifetime | Architecture v1 retains minimal deletion tombstones for repository lifetime and measures their count/bytes; no hidden second-order pruning exists. |

## Exact Independent Re-verification Packet

The independent reviewer must inspect Git commit `eb815bbe5447214e980aae98dfee50fef09eae7c`, not this later ledger commit and not a loose export. The candidate-file manifest is sorted by repository-relative path using ordinal ordering; each line is `sha256`, two spaces, path, LF. Its SHA-256 is:

```text
e710a147b73dd5ed8f720e7a0ee681af829f98b4bc2e7b17a2ece4ad979acf2c
```

```text
d60f352d0db1404c70afb4bb8b2ca3fd1c610572aa40720e8a0b7baa7885418c  .gitattributes
b2c9f1a98c478549391043205022ae49b30338336e4f91f1cfd146ca1b46670e  .gitignore
ce17fef1d1331de060f0d2a6743086698531f55538c8306930e41758eb747572  AGENTS.md
18326ce8d50fd7154755912533d77e8ff987c8da48a104fc7244c24a13b8c139  README.md
2e540685f83e0ea730e1260de25649d379a142d685adce0eb5c8ea5ea45de36f  SDD.md
e4e84fb42557908c6e36e964d1d82cd564ec8ed6adb60d1f1f18ded27b4fb61b  WORKBOARD.md
79b4059fb7332ac6523486e1c312c9727f9c42e7c910b03cdadb2d6fb93febd4  architecture/000-system-blueprint.md
7f309857cd4a429da4f2ba1fd260d7e6af2d1ac0f4503d774cd0759348528290  architecture/001-execution-and-ownership.md
4aae9cadac0ce76152e1316c23d65a5e41263e3b9f42fb115f90541241684440  architecture/002-evidence-and-write-gate.md
ab24bf28eeb43e960d1d24d32a12b701562e14ff2ba98690644300cf53c51c59  specs/000-product-contract.md
b73b0fdb08df039f434ce40cd49d17a990d9e5d67b7ea5229c96a764014d832f  specs/001-foundation-slice.md
84ede6d99086fa344e61ab6e453c77d4c20c5485784592a251d3ed7e0f805067  문서(한글)/AGENTS.ko.md
2456a9b89bb8f24a76b63d674cd62f0dcb64038ecada488a7d523af1476a1f28  문서(한글)/SDD.ko.md
```

The reviewer must report `PASS`, `REOPEN`, or a new finding for G-01 through G-07, every externally reopened E/C item, the packaged-skill and process-reopen proofs, and B-07 exact revision binding. It must also check that:

- one implementable Cargo edge exists for every gate signal/effect owner;
- cold and warm paths replay the same complete owner output and seal capability-reported semantic reads by fixed point before authorizing one exact observation;
- authorizing observations are sealed, while rejected/failed closure may use typed unsealed data without fabricated input or observation IDs;
- concurrent shared-worktree close is serializable for active, terminal, stale, and unexplained intervening changes;
- planned self-writable config is recaptured into current effective values and input identity, while external protected-read drift remains stale;
- every attempt/publication/retention crash point has one result, no automatic orphan success, and no missing-payload success;
- operation retry cannot duplicate pre/post, abandon, pin/unpin, prune-plan, or confirmation mutations;
- retention plans own immutable identities/scopes, use a complete total item ordering, and expose pruning/pruned truth through public commands rather than private store mutation;
- scan flags, explicit entries, effective resolver profiles, self-written config, and embedded Vue scripts have one precedence;
- every first-slice limitation reason has one exhaustive owner/scope/absence/relevance mapping, and post-write effects consume typed owner deltas;
- independent pins, full lifecycle-store migration, and repository-lifetime minimal tombstones preserve referential truth;
- default output, retention exclusions, skill probes, and process-reopen proof match the acceptance tables;
- both canonical Korean files and all other manifest entries are read from the exact Git tree rather than omitted from a loose upload.

The resulting report must name the exact commit and manifest hash, state reviewer independence, preserve reopened/new IDs and accepted risks, and publish its own SHA-256. Document review cannot pass the measured backend, OXC, package, or performance gates below.

## Remaining Freeze Gates

1. Independently re-verify exact commit `eb815bbe5447214e980aae98dfee50fef09eae7c` using the packet above.
2. Run and record the store correctness/measurement comparison, including every publication, lifecycle-operation recovery, and tombstone/trash retention crash point.
3. Run and record OXC memory/stack feasibility.
4. Run and record Windows/Linux packaging feasibility, including both packaged skill adapters.
5. Approve numeric Phase 1 budgets from named probe evidence.
6. Obtain the separate independent design review required by repository policy.

Every gate above must be `passed` or an explicitly reviewed accepted risk with rationale, owner, scope, and expiry. There are currently no accepted risks. Architecture v1 remains draft and Phase 1 implementation remains blocked.
