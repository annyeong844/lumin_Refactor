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
| Prior externally reverified candidate | `f2d1fa663a0b1ba3e3fec84821dc0861c96cce54`; supplied bytes were checked but exact Git binding remained pending |
| Latest externally reverified candidate | `eb815bbe5447214e980aae98dfee50fef09eae7c`; 11 supplied candidate files matched the packet, while exact Git binding and both Korean-source bytes remained pending |
| Current resolution candidate | `1382ec71c42ba584a348b109d74ee9598dacfb74`; independent verification still required |
| Verifier | external independent reviewer, report supplied by the user; not the architecture-authoring Codex session |
| Earlier externally reverified input manifest/report | `2ff71a19ebd5fd2939f1aa6da77a2d3276c320791a19a1364670ab78d9c2210e` / `67a001b2cbfd6af36d3e60712d8dfa2bab6dfcc4a7756114f87b9d34d5530611` |
| Prior externally reverified input manifest/report | `9d2366afa0fa360397fbf4ae7c0ad4205d34739f20f8f7acff70207b2152b6fd` / `07df439ded1101b3ddea1328880579be918db1cfbe8d7861d28c0ca1d18ad20a` |
| Latest externally reverified input manifest/report | `e710a147b73dd5ed8f720e7a0ee681af829f98b4bc2e7b17a2ece4ad979acf2c` / `6c8613d5a908d9c97ca25fb71841078e6abb00a6fd4fb942ef33187355d6dbd1` |
| Verification result | `eb815bb` remained blocked: G-02/G-03/G-06 passed, G-04 passed with an API follow-up, G-01/G-07 and retention integrity reopened, H-01 through H-07 were identified, and B-07 remained pending |

The first report verified the exact first amended revision. Later reports verified supplied byte manifests but could not prove loose uploads byte-identical to the declared Git commits or inspect both canonical Korean files. This record does not call the current resolution independently verified until a separate reviewer checks the exact current candidate checkout.

## Freeze Blocker Resolution Ledger

| Finding | Decision | Canonical resolution in the current candidate |
| --- | --- | --- |
| B-01 / F-03 identity meaning | External PASS | ARCH-000/001 keep software-only `AnalysisContractId` separate from repository `AnalysisInputId`; gates store both. |
| B-02 / F-11 query pinning | External PASS | ARCH-002 requires run-scoped queries, immutable gate revisions, scope-bound cursors, and explicit nested continuation flags. |
| B-03 / F-07 original classification scope | External PASS | SLICE-001 fixes scan precedence, source-role evidence, module-format selection, and public-lane union policy. |
| C-01 / D-01 active profile and override | External PASS | Invocation override applies to every physical and embedded-script importer; Vue template binding is not a second resolver lane. Post-write preserves the caller override while validated self-written config may recompute effective importer profiles. |
| C-02 gate-effect authority and root escape | External REOPEN under H-03 -> accept | Model-owned `GateSignal` crosses the existing Cargo DAG; `lumin-evidence::gate_policy` alone maps effects. The engine registry has explicit compiled-profile availability authority without substitute analysis; caller path escape remains malformed with no record, while later containment uses named signals. |
| C-03 exact observation | External REOPEN under H-01/H-02 -> accept | ARCH-001/002 split path demand from exact consultation, reserve before capture/consumption, replay full owner outcomes, and persist typed sealed or unsealed observation bindings without partial IDs. |
| C-04 shared-worktree attribution | External PASS with H-01/H-05 dependencies | Immutable transition reconciliation remains state-based; demand closure supplies the sealed read set and active-gate references retain every terminal capsule required by a later close. |
| C-05 crash outcome conflict | External PASS | The publication crash table gives every point one result; a run renamed before terminal success remains an interrupted, unadoptable orphan. |
| C-06 commit/transport recovery | External PASS | The operation contract covers pre/post, abandon, pin/unpin, prune-plan creation, and confirmation, with one general `lumin operation show` recovery query. |
| C-07 retention/latest integrity | External REOPEN under H-05/H-06 -> accept | Latest closures remain ineligible; active-gate transition references protect reconciliation proof, and generation-fenced lifecycle-store migration preserves the complete logical catalog across replacement. |
| B-07 exact review identity | Pending proof | This ledger names the exact current candidate, but only a new independent checkout verification can mark it passed. |
| E-01 semantic-read closure | External REOPEN under H-01/H-02 -> accept | Owners emit unconsumed path demands first; the engine reserves/captures before cold continuation or prerequisite-keyed cache replay, and only complete owner outcomes may seal exact consulted inputs. |
| E-02 command-wide mutation idempotency | External PASS | Every gate/retention lifecycle mutation requires `OperationId`; read-only plan/show/page commands do not. |
| E-03 retention deletion crash state | External REOPEN under H-05/H-06 -> accept | Canonical deletion states remain queryable, terminal transition capsules stay protected for active gates, and migration has transaction-scoped handles, generation fencing, and a crash table. |
| E-04 executable retention proof | External PASS | SLICE-001 includes public run/gate retention commands and requires corpus fault injection through child-process public DTOs rather than private store writes. |
| E-05 Vue/profile precedence | External PASS | Embedded Vue scripts use ordinary importer profile precedence; template binding consumes resolved bindings and `<script src>` remains an exact SFC source reference. |
| E-06 explicit entry contract | External PASS | Repeated `--entry` replaces config entries; normalization, containment, exclusions, empty/private behavior, `AnalysisInputId`, and gate read participation are normative. |
| E-07 exhaustive limitation scope | External REOPEN under H-04 -> accept | SLICE-001 separates static scope/absence meaning from a total owner delta relation over every semantic payload dimension before signal/effect mapping. |
| G-01 warm-cache semantic-read bypass | External REOPEN under H-01/H-02 -> accept | Cache replays prerequisite-keyed demand steps and a complete finished owner outcome; it cannot consume an unreserved input, omit capability state/diagnostics, or replay request-specific gate effects. |
| G-02 nonauthorizing observation shape | External PASS | `ObservationBinding` is sealed baseline/close identity or typed unsealed attempted-domain data. Active gates require sealed openings; unsealed rejected/close revisions omit authoritative input IDs and complete fingerprints. |
| G-03 retention-plan identity and cursor | External PASS | Model-owned, store-allocated `RetentionPlanId` and immutable content identity scope plan pages independently of unrelated catalog mutation; confirmation still revalidates current state. |
| G-04 collection ordering | External PASS with H-07 API follow-up -> accept | Every collection has one versioned total key, and current-binary/run capabilities now expose the same explicit cursor continuation surface as their ordering contract. |
| G-05 public pruning/pruned truth | External local PASS; retention integrity REOPEN under H-05/H-06 -> accept | Typed public tombstones remain intact while active transition references and lifecycle generations protect the durable proof around them. |
| G-06 scan invocation surface | External PASS | Audit/pre-write expose include/exclude/role flags; post-write reuses the caller override tier, rejects replacement, and containment classes have one result. |
| G-07 static reason versus lifecycle delta | External REOPEN under H-04 -> accept | Static limitation rows own scope, absence effect, and relevance only. Post-write owners apply a total introduced/unchanged/regressed/improved/mixed/resolved/unavailable relation before signals. |
| H-01 semantic-input discovery before reservation | Accept finding | `OwnerStep::NeedsInputs` carries only unconsumed exact-path/unbounded demands. The engine normalizes, conflict-checks, and reserves before inventory capture; cold owners resume from owned continuations. |
| H-02 complete cold/warm owner outcome | Accept finding | `CachedOwnerStep` is keyed to exact supplied prerequisites and task/profile parameters; finished replay includes outcome state, diagnostics, payload, limitations, gate-neutral signals, and consulted inputs. Gate projection is owner-pure and uncached. |
| H-03 unavailable-capability signal authority | Accept finding | ARCH-000 names the engine capability registry as availability fact/signal owner with `engine -> model/evidence`; it cannot own fallback analysis. |
| H-04 total lifecycle delta relation | Accept finding | `DeltaKey` plus closed dimension changes classify additions/removals, affected domains, confidence, grounding, evidence, and owner payload as introduced, unchanged, regressed, improved, mixed/incomparable, resolved, or baseline unavailable. |
| H-05 active-gate transition retention | Accept finding | Every terminal close atomically creates references from earlier active gates to its sole reconciliation capsule; referenced terminal closure is prune-ineligible until close/abandon releases the reference. |
| H-06 lifecycle-store generation fencing | Accept finding | Stable `lifecycle.lock`, transaction-scoped backend handles, durable migration intent, generation revalidation, and the migration crash table prevent old-generation late commits. |
| H-07 capabilities continuation surface | Accept finding | ARCH-002 and SLICE-001 expose `lumin capabilities [--run <run-id>] [--cursor <cursor>]` with binary/run scope corpus traversal. |

`Accept finding` records the architecture-authoring decision. It is not an independent `PASS`.

## Author-Side Preflight

The architecture-authoring session re-read the current candidate before packaging it for external review. This is not independent verification. For `1382ec7`, author-side adversarial review checked the externally reported H findings and two additional interactions: stale recursive cache demands and repeated cold parsing across demand closure.

| Check | Current candidate resolution |
| --- | --- |
| Demand-before-consumption | Owners may inspect only supplied snapshots. A demanded path is normalized, conflict-checked, and reserved before inventory captures bytes; unbounded demand yields typed unsealed incompleteness. |
| Cold/cache step equivalence | Cache replays one prerequisite-keyed owner step at a time. A changed intermediate config cannot over-reserve a stale nested demand; finished replay carries the full owner outcome. |
| Single-consumption cold path | A fully owned, parser-free capability continuation resumes after capture; a cached-demand miss starts one cold owner with all supplied snapshots and does not trigger a second parse. |
| Gate-context projection | Repository-input cache stores only gate-neutral signals. Capability owners recompute request-specific projection from model-owned context without I/O or late demands. |
| Physical type ownership | Owner diagnostics, confidence/grounding ranks, demand/outcome values, signals, and delta dimensions live in `lumin-model`; evidence owns canonical records/effects without forcing forbidden capability edges. |
| Total lifecycle delta | Every semantic field is key, closed comparison dimension, or non-semantic metadata; mixed target sets, narrowed scopes, ranks, evidence, and owner-payload changes have one classification/signal path. |
| Retention reference closure | Terminal transition capsules are the sole reconciliation payload and remain prune-ineligible while any earlier active gate references them. |
| Migration generation | All backend handles are transaction-scoped under stable `lifecycle.lock`; every replacement crash point and old-generation late mutation has one recovery rule. |
| Capability continuation query | Binary- and run-scoped capability pages both expose `--cursor` and use `capabilities.v1`. |
| Failed-close baseline | Delta comparison remains tied to the immutable opening semantic baseline; unsealed or sealed-stale attempts cannot replace current read protection. |

This author-side preflight found no remaining known H-01 through H-07 contradiction after those repairs. That is not a freeze approval: the exact candidate still requires independent checkout review and all measured gates below. Earlier D-02, D-03, and D-04 remain externally passed; D-01 remains covered by the externally passed profile contract.

## High-Priority Follow-Up Ledger

| Finding | Resolution |
| --- | --- |
| Product AC crosswalk | PRODUCT-000 has 18 ACs. SLICE-001 maps all 18 through 33 slice AC rows, including demand reservation, full cold/warm outcomes, honest observations, capability paging, transition retention, migration fencing, and total lifecycle delta proofs. |
| Rename detection | ARCH-002 recognizes only unique persistent-file-identity rename; ambiguity is remove plus add. |
| Mixed filesystem case behavior | ARCH-002 records comparison behavior per existing parent/physical identity instead of assuming one root policy. |
| WSL `/mnt` benchmark | SLICE-001 makes it mandatory report-only diagnostic, excluded from blocking AC 16 budgets. |
| Probe reproducibility | ARCH-002/SLICE-001 retain exact source, fixture, toolchain, commands, invariants, and raw results under `reviews/probes`. |
| Retention command | ARCH-000 exposes `lumin runs` and general operation recovery; ARCH-002 owns public plan/show/confirm commands, operation IDs, tombstone/trash recovery, and latest-linkage exclusions. |
| SFC dialect extensibility | ARCH-000/001 and SLICE-001 define one dialect-aware SFC pipeline: Vue is the first production dialect, recognized unsupported dialects stay visible, and framework policy cannot leak into engine, resolver, or graph owners. |
| Explicit entry/profile selection | SLICE-001 closes config/CLI precedence, private-package empty coverage, effective tsconfig inheritance, and embedded Vue importer behavior. |
| Limitation exhaustiveness | Every first-slice typed incomplete/unsupported/opaque reason has one compile-time checked owner/scope/absence/relevance mapping; every semantic delta field is key/dimension/metadata registered and lifecycle effects are delta-owned. |
| Opening/close input relationship | Caller override values remain fixed; self-writable config facts are recomputed into the close input identity; protected external reads require exact freshness. |
| Independent run references | Every pin allocates `PinId`; one consumer cannot remove another consumer's protection. |
| Active transition references | A terminal transition/capsule remains prune-ineligible while any earlier active gate may require it; close/abandon releases the reference atomically. |
| Lifecycle-store migration | Stable-lock, transaction-scoped-handle, generation-fenced copy-on-write migration preserves attempts, operations, transitions/references, plans/tombstones, pins, gates, revisions, and catalog sequences. |
| Tombstone lifetime | Architecture v1 retains minimal deletion tombstones for repository lifetime and measures their count/bytes; no hidden second-order pruning exists. |

## Exact Independent Re-verification Packet

The independent reviewer must inspect Git commit `1382ec71c42ba584a348b109d74ee9598dacfb74`, not this later ledger commit and not a loose export. The candidate-file manifest is sorted by repository-relative path using ordinal ordering; each line is `sha256`, two spaces, path, LF. Its SHA-256 is:

```text
d68e38d76b2757bd34f74116364135c896244fa322cae0beb56c7f899ad410af
```

```text
d60f352d0db1404c70afb4bb8b2ca3fd1c610572aa40720e8a0b7baa7885418c  .gitattributes
b2c9f1a98c478549391043205022ae49b30338336e4f91f1cfd146ca1b46670e  .gitignore
ce17fef1d1331de060f0d2a6743086698531f55538c8306930e41758eb747572  AGENTS.md
18326ce8d50fd7154755912533d77e8ff987c8da48a104fc7244c24a13b8c139  README.md
2e540685f83e0ea730e1260de25649d379a142d685adce0eb5c8ea5ea45de36f  SDD.md
e4e84fb42557908c6e36e964d1d82cd564ec8ed6adb60d1f1f18ded27b4fb61b  WORKBOARD.md
12f318334535cc8b8936e14b2fb1c98e0027ef3cd08d15174886ca9ba5d63cf7  architecture/000-system-blueprint.md
502a1da51366eec68c39bf14dfa79c364a628cd4f2dba459067949dfee0a4eba  architecture/001-execution-and-ownership.md
9e84eb4311288a2d5d40f43d333a39e3affef52c8f0ee99f0bd41ea0bcc94492  architecture/002-evidence-and-write-gate.md
282d760b3be49dcad17b066da4ef971d2a40e37846bb31e2ca79ade675e4af17  specs/000-product-contract.md
b5b2f858e5a3f0bc41160c345069e52dc82ab05d5d6ae90c85cf2adc1e51c4d4  specs/001-foundation-slice.md
84ede6d99086fa344e61ab6e453c77d4c20c5485784592a251d3ed7e0f805067  문서(한글)/AGENTS.ko.md
2456a9b89bb8f24a76b63d674cd62f0dcb64038ecada488a7d523af1476a1f28  문서(한글)/SDD.ko.md
```

The reviewer must report `PASS`, `REOPEN`, or a new finding for H-01 through H-07, their reopened G/E/C predecessors, the packaged-skill and process-reopen proofs, and B-07 exact revision binding. It must also check that:

- one implementable Cargo edge exists for every gate signal/effect owner, including compiled-profile unavailability, owner diagnostics, confidence/grounding ranks, and delta dimensions;
- every new semantic input is demanded without consumption, reserved before capture, and supplied through an owned continuation or prerequisite-keyed cache step;
- cold and warm paths preserve owner outcome/capability state, diagnostics, payload, limitations, gate-neutral signals, consulted reads, request-specific effects, observation binding, and semantic dump;
- recursive cache demands cannot expose a stale downstream path before the current prerequisite key is validated, and demand closure does not reread/reparse a cold payload;
- authorizing observations are sealed, while rejected/failed closure may use typed unsealed data without fabricated input or observation IDs;
- concurrent shared-worktree close is serializable for active, terminal, stale, and unexplained intervening changes, and retention cannot delete a capsule referenced by an active gate;
- planned self-writable config is recaptured into current effective values and input identity, while external protected-read drift remains stale;
- every attempt/publication/retention crash point has one result, no automatic orphan success, and no missing-payload success;
- operation retry cannot duplicate pre/post, abandon, pin/unpin, prune-plan, or confirmation mutations;
- retention plans own immutable identities/scopes, use a complete total item ordering, and expose pruning/pruned truth through public commands rather than private store mutation;
- scan flags, explicit entries, effective resolver profiles, self-written config, and embedded Vue scripts have one precedence;
- every first-slice limitation reason has one exhaustive owner/scope/absence/relevance mapping, and every target/domain/rank/evidence/owner-payload relation has one total owner delta and signal path;
- delta retry always compares with the immutable opening semantic baseline, while sealed stale history cannot replace current read protection;
- independent pins, active transition references, repository-lifetime minimal tombstones, and generation-fenced lifecycle-store migration preserve referential truth;
- transaction-scoped backend handles and every migration crash rule prevent an old-generation process from committing after replacement;
- both current-binary and exact-run capability collections are fully reachable through `--cursor` under `capabilities.v1`;
- default output, retention exclusions, skill probes, and process-reopen proof match the acceptance tables;
- both canonical Korean files and all other manifest entries are read from the exact Git tree rather than omitted from a loose upload.

The resulting report must name the exact commit and manifest hash, state reviewer independence, preserve reopened/new IDs and accepted risks, and publish its own SHA-256. Document review cannot pass the measured backend, OXC, package, or performance gates below.

## Remaining Freeze Gates

1. Independently re-verify exact commit `1382ec71c42ba584a348b109d74ee9598dacfb74` using the packet above, including both canonical Korean sources from that Git tree.
2. Run and record the store correctness/measurement comparison, including every publication, lifecycle-operation recovery, tombstone/trash retention, and generation-fenced migration crash point.
3. Run and record OXC memory/stack feasibility.
4. Run and record Windows/Linux packaging feasibility, including both packaged skill adapters.
5. Approve numeric Phase 1 budgets from named probe evidence.
6. Obtain the separate independent design review required by repository policy.

Every gate above must be `passed` or an explicitly reviewed accepted risk with rationale, owner, scope, and expiry. There are currently no accepted risks. Architecture v1 remains draft and Phase 1 implementation remains blocked.
