# SLICE-001: Native JS/TS/SFC Foundation, Vue Evidence, and Write Gate

Document role: first implementation specification

Status: draft, blocked by Architecture v1 review

Revision: 2026-07-16

Parents: PRODUCT-000, ARCH-000, ARCH-001, ARCH-002

## 0. One-Line Definition

The first slice ships a production-grade native path from Codex/Claude invocation through parallel JS/TS analysis and the dialect-extensible SFC pipeline, with Vue as the first production dialect, export-level dead evidence, bounded queries, and a durable pre/post transaction on Windows and Linux prebuilt binaries.

## 1. Why This Slice Is First

This slice crosses every permanent system boundary and directly attacks the legacy product's highest-impact failures:

- Node producer orchestration and repeated parsing;
- SFC imports classified as non-source assets and aborting the graph;
- public re-export protection applied to an entire file;
- reachable files excluded from export-level dead analysis;
- `import.meta.glob` and dynamic-use precision drift;
- artifact warehouses and duplicated counts;
- JSON write-gate intent transport;
- runtime Cargo compilation and WSL platform confusion.

Completing this slice proves the architecture. It is not permission to bypass final boundaries temporarily.

## 2. Implementation Scope

SLICE-001 creates only crates that contain real slice behavior:

- `lumin-model`;
- `lumin-evidence`;
- `lumin-inventory`;
- `lumin-js`;
- `lumin-sfc` with the permanent dialect-extensible SFC boundary and complete Vue ownership for the declared corpus;
- `lumin-resolve`;
- `lumin-graph`;
- `lumin-dead`;
- `lumin-store`;
- `lumin-engine`;
- `lumin-protocol`;
- `lumin-cli`.

The development-only `lumin-xtask` crate contains architecture, corpus, determinism, and package verification commands. It is not a product capability or runtime dependency.

The Rust, clone, structure, and discipline analysis crates are not created in this slice. Shape and type-escape intent lanes therefore remain unavailable; requesting either returns visible unavailable/incomplete evidence rather than a temporary implementation in `lumin-js` or `lumin-engine`.

## 3. Supported Source Contract

### 3.1 Inventory and Scan Policy

| Input class | Normative first-slice behavior |
| --- | --- |
| Source extensions | Include `.js`, `.jsx`, `.mjs`, `.cjs`, `.ts`, `.tsx`, `.mts`, `.cts`, `.d.ts`, `.d.mts`, `.d.cts`, `.vue`, `.svelte`, and `.astro` under the canonical root. Vue is analyzable; Svelte and Astro are admitted SFC sources with explicit unavailable capability in this slice. |
| Ignore policy | Apply the precedence below. Always exclude `.git`, `.lumin`, and dependency-owned `node_modules`; do not prune an authored directory merely because its basename is `target`, `build`, or `coverage`. |
| Generated/vendor | Apply versioned role rules below. In-scope uses may contribute liveness, but generated or vendored definitions are not default dead-removal candidates. |
| Tests | Apply versioned test-role rules below. Full audit counts their fan-in separately; production liveness does not treat test-only consumers as production consumers. |
| Declarations | Parse declaration files for type-space facts only. A declaration cannot satisfy a runtime value edge or become a value dead-removal candidate. |
| Symlink/junction | Do not recursively traverse directory links by default. An explicitly included root-contained target is deduplicated by physical file identity; an outside-root target is rejected and reported. |
| Semantic inputs | Snapshot applicable ignore files, package manifests, lockfiles, tsconfig files, workspace metadata, and explicit entry configuration even when they are not source files. |

`lumin-inventory` owns `ScanPolicy`, its semantic version, and every `SourceClassification`. The first slice accepts one optional strict root `lumin.json` plus typed invocation overrides; it does not consult machine-global Git excludes or infer hidden configuration. `lumin.json`, applicable `.gitignore` files, and invocation policy all participate in `AnalysisInputId` and the gate semantic-read set.

The root configuration shape is closed in this slice:

```json
{
  "schemaVersion": "lumin-config.v1",
  "scan": {
    "include": ["src/**"],
    "exclude": ["src/legacy/**"],
    "roles": [{"pattern": "test/**", "role": "test"}]
  }
}
```

Unknown fields, unknown roles, conflicting role declarations, or a second config file are configuration failures. Patterns are canonical-root-relative, slash-normalized Git-wildmatch patterns. Repeated CLI `--include <pattern>`, `--exclude <pattern>`, and `--role-at <pattern> <role>` values form the invocation tier; they do not mutate `lumin.json`.

Scan admission uses this order:

1. Reject root escapes and hard exclusions: `.git`, `.lumin`, and dependency-owned `node_modules`.
2. Apply invocation excludes, then root `lumin.json` excludes. Exclusion wins over every inclusion.
3. If invocation includes exist, use them as the inclusion domain; otherwise use configured includes; otherwise use the canonical root.
4. An explicit inclusion may reinclude a repository-ignored path that is not excluded above. All other paths apply root-to-leaf `.gitignore` files with Git ordering and negation semantics; the last matching rule wins.
5. Persist every admitted, excluded, ignored, out-of-domain, or unobservable result with `scan-policy.v1`, the matching rule, configuration source, and precedence tier.

Source roles are independent recorded facts, not one lossy enum. Invocation role overrides take precedence over `lumin.json`, which takes precedence over these compiled `source-classification.v1` defaults:

- `TestLike`: a path segment exactly equal to `test`, `tests`, `__tests__`, or `__mocks__`, or a source basename ending in `.test` or `.spec` before its source extension;
- `Generated`: an exact leading-comment `@generated` marker within the first 2 KiB, or an explicit generated role; generic directory names such as `build`, `dist`, `out`, or `target` do not imply this role;
- `Vendored`: explicit role only; no authored path is muted merely because it resembles a vendor directory;
- `Declaration`: `.d.ts`, `.d.mts`, or `.d.cts`.

The typed role vocabulary is exact: `test` adds `TestLike`; `production` clears it; `generated` adds `Generated`; `vendor` adds `Vendored`; and `authored` clears `Generated` and `Vendored`. Contradictory declarations for the same axis at one precedence tier are malformed configuration. Each classification stores role, rule version, reason, and configuration source. The scan profile and every exclusion are persisted. An omitted or unobservable path is a scope limitation, not evidence that the path contains no consumers.

Package/config ownership is also deterministic. A source belongs to the nearest ancestor `package.json` inside the canonical root. Supported workspace declarations are `package.json#workspaces` (array or `packages` object member) and `pnpm-workspace.yaml#packages`; at the same directory, pnpm workspace patterns are authoritative when present, otherwise package-manifest patterns apply. The workspace owner is the nearest ancestor declaration whose root-contained patterns include that package. A dependency lockfile is the nearest ancestor `package-lock.json`, `npm-shrinkwrap.json`, `pnpm-lock.yaml`, `yarn.lock`, `bun.lock`, or `bun.lockb` between the package and workspace root. If multiple supported lockfile kinds coexist at that nearest directory, ownership is incomplete rather than selected by preference. No lockfile means no lockfile write is inferred. Every consulted manifest, workspace declaration, and lockfile identity is a semantic input.

The compiled scan/classification and ownership rule versions participate in `AnalysisContractId`; selected patterns, overrides, classifications, reasons, and configuration identities participate in `AnalysisInputId`.

### 3.2 JavaScript and TypeScript

The slice must preserve evidence for:

- ESM named, default, namespace, side-effect, and type-only imports;
- direct exports, alias exports, default exports, and re-exports;
- namespace member access and broad namespace escape;
- literal dynamic imports, member-precise dynamic imports, and nonliteral opacity;
- `import.meta.glob` relative patterns with explicit unsupported evidence for unsupported patterns;
- CommonJS `require`, exact exports, namespace use, and computed-property broad evidence;
- `.js`, `.jsx`, `.mjs`, `.cjs`, `.ts`, `.tsx`, `.mts`, `.cts`, and declaration inputs under the declared scan policy;
- extension and compiled-output fallback order proven by corpus tests;
- parse failures as scoped incomplete evidence.

Unsupported syntax is recorded by its owning file or capability and cannot become an empty successful file. Its downstream absence impact follows the explicit limitation scopes in Section 5.2.

### 3.3 SFC Foundation and First Vue Dialect

This slice opens the permanent SFC pipeline rather than a Vue-only branch. Common project-owned facts carry explicit dialect identity, per-dialect capability status, and shared embedded-source, component/resource, opacity, and finalization outcomes. `lumin-sfc` owns dialect dispatch and policy; the engine, resolver, and graph consume only the common project-owned boundary. Vue completeness cannot collapse Svelte or Astro unavailable states into aggregate SFC completeness. This is a closed internal extension seam, not a public plugin API or one trait/crate per dialect.

Vue is the first production-supported dialect. For Vue files, `lumin-sfc` owns:

- SFC block decomposition;
- inline and `src` script units;
- script language selection;
- component import and template-use facts needed by graph evidence;
- style and other non-source resource references as non-source assets;
- comments and inactive template regions required by the declared corpus;
- generated and unresolved SFC references as typed evidence.

After JS extraction, the ARCH-001 `finalize-sfc-facts` stage returns model-owned script facts to `lumin-sfc` for Vue-specific template/import binding. Inline scripts retain parent span mappings. `<script src>` references an existing inventory `SourceId`; it does not create a copied source or a second parse. A conflicting external `lang`/extension mode is unsupported evidence in this slice.

An import such as `import App from "./App.vue"` resolves to the Vue source module when present. A missing `.vue` target becomes unresolved evidence. Neither case is routed through an exception labeled `non-source-asset-specifier`.

Svelte, Astro, and other SFC dialects remain explicitly unavailable in this slice. Recognized dialects enter through the same inventory and SFC stages, produce dialect-scoped unavailable evidence, and cannot be presented as analyzed by the generic graph. Supporting a new dialect adds behavior and corpus truth inside `lumin-sfc`; it does not add an engine branch, another scheduler, or a fallback analyzer.

## 4. Resolution Contract

Resolution is performed against the immutable source inventory and semantic configuration snapshot. The first slice models the declared subset below rather than claiming complete TypeScript resolver parity. Resolution first derives host runtime candidates for the selected mode, then applies TypeScript source substitution to each candidate before advancing to the next host candidate.

`lumin-resolve` selects a typed profile for every importer with this precedence:

1. An explicit invocation-wide `--resolution-profile <bundler|node|node16|nodenext>` override.
2. The explicit `compilerOptions.moduleResolution` in the importer's nearest controlling `tsconfig`: `bundler` -> `bundler`, `node`/`node10` -> `node`, `node16` -> `node16`, and `nodenext` -> `nodenext`; unsupported values follow the incomplete rule below.
3. The named first-slice product default, `bundler`, when no explicit supported value exists.

Without an invocation override, an explicit unsupported value such as `classic` or an unknown value makes resolution incomplete for affected importers; it never falls through to the product default. The invocation override supersedes only `moduleResolution` profile selection. An unreadable controlling config remains incomplete even under an override because aliases, package ownership, or other semantic inputs may be unknown. Without an override, mixed workspaces retain importer-local profiles from their nearest configs. The override deliberately applies to every importer in the invocation. Vue script edges remain a dialect-owned `bundler` lane and record that reason separately.

Audit and pre-write accept the typed override; post-write reuses the profile facts stored in its baseline and cannot replace them. Every selected profile records mode, source (`invocation`, config path, or `product-default`), and reason. Those values and consulted configs participate in `AnalysisInputId`; `resolution-profile-selection.v1`, its mappings, and the default participate in `AnalysisContractId`.

| Specifier or host candidate | Ordered first-slice probes |
| --- | --- |
| Explicit TypeScript or SFC source path | Exact path only for `.ts`, `.tsx`, `.mts`, `.cts`, `.vue`, `.svelte`, or `.astro`. Vue targets are analyzable; Svelte and Astro targets resolve as SFC sources with unavailable analysis evidence. Explicit declaration paths are exact and type-space only. JavaScript runtime extensions use the substitution rows below even when written explicitly. |
| Runtime `.js` candidate | Value space: `.ts`, `.tsx`, `.js`, `.jsx`. Type space inserts `.d.ts` after `.tsx`. |
| Runtime `.jsx` candidate | Value space: `.tsx`, `.jsx`. Type space inserts `.d.ts` after `.tsx`. |
| Runtime `.mjs` candidate | Value space: `.mts`, `.mjs`. Type space inserts `.d.mts` after `.mts`. |
| Runtime `.cjs` candidate | Value space: `.cts`, `.cjs`. Type space inserts `.d.cts` after `.cts`. |
| Extensionless path in a permitting mode | Derive the host `.js` candidate and apply its substitution row. Do not invent extensionless `.mts`, `.cts`, `.mjs`, or `.cjs` candidates. |
| Directory in a permitting mode | Resolve its supported `package.json` entry under the package-field rules, then derive an `index.js` host candidate and apply the `.js` substitution row. |
| Unsupported explicit extension | Return `NonSourceAsset` or typed `Unsupported` evidence. Do not substitute a declaration sidecar. |

A declaration may satisfy type-space resolution but never proves that a runtime value target exists. When a value import also has declaration evidence, the resolver records that type companion separately from the value target.

Specifier and configuration policy is:

| Class | Contract |
| --- | --- |
| Resolution mode | Support `bundler`, legacy `node`, `node16`, and `nodenext`. Bundler/legacy-node and CJS lanes permit extensionless and directory fallback; Node16/NodeNext ESM lanes require an explicit relative extension and skip the extensionless and directory rows. Unsupported modes make resolution incomplete rather than selecting a fallback mode. |
| Importer format | In Node16/NodeNext, `.mts`/`.mjs` are ESM and `.cts`/`.cjs` are CJS. `.ts`/`.tsx`/`.js`/`.jsx` and matching declarations use the nearest package `"type": "module"`, otherwise CJS. Static import/export selects `import` for ESM and `require` for CJS; `require()` always selects `require`, and dynamic `import()` selects `import`. Vue script edges use the bundler lane. |
| Relative | Resolve inside the canonical root with the probe order above. Route-group characters such as `(doc)` are ordinary path bytes. |
| Tsconfig | Use the importer's nearest config, root-contained relative/workspace-package `extends`, child override semantics, and the `baseUrl` of the config that declares each mapping. Cycles are incomplete configuration evidence. External-package extends and project-reference redirection are unsupported in this slice. |
| `paths` | Exact key before wildcard; wildcard keys permit one `*` and use longest literal prefix then declaration order. Probe mapped targets before `baseUrl` and package resolution. |
| Workspace package | Resolve `exports` exact key before one-star patterns. Within a condition object, the first supported matching branch in declaration order wins; the active set includes `types` for type space, `import` or `require` for the edge mode, `node`, and `default`. Edge resolution selects one lane; external public-surface protection follows the supported-lane union in Section 5.1. Unsupported condition shapes remain visible. |
| Package fields without `exports` | Type space probes `types`, then `typings`, then a declaration companion for the selected value target. Value space uses `module` then `main` in bundler mode and `main` in Node modes, followed by permitted directory fallback. A type field never proves runtime value liveness. |
| Bare external | Classify as `External` after workspace ownership lookup; never probe a similarly named relative file. |
| Absolute, URL, package `imports`, or unsupported alias | Return typed `Unsupported` or `Unresolved` evidence with the limitation scope below; never skip the record. |
| Generated virtual | Resolve only through an observed generated mapping; otherwise retain a typed virtual limitation. |

Every source use receives one `ResolutionOutcome`. A skipped record without a typed reason is a contract failure. The resolver policy version participates in `AnalysisContractId`; consulted configuration identities participate in exact cache keys and `AnalysisInputId`.

## 5. Graph and Dead-Export Contract

The graph indexes every successfully lowered source file, including files reachable from entries and tests.

Dead classification is export-identity based:

- exact import fan-in is tracked per exported identity;
- type-space and value-space fan-in remain distinct;
- broad use is represented separately and cannot inflate exact scalar fan-in;
- module reachability does not suppress export-level analysis inside a reachable module;
- a package public surface protects only identities actually exported through that surface;
- public protection of one identity cannot protect unrelated siblings in the same file;
- side-effect imports preserve module reachability without marking every export exactly consumed;
- opaque dynamic or computed use limits absence claims with visible evidence;
- production and test consumers remain distinguishable.

The default query reports candidates, confidence, protection reasons, and limitations. It does not label every zero-fan-in symbol safe to delete.

### 5.1 Entry, Public Surface, and Consumer Policy

| Fact | Contract |
| --- | --- |
| Entry root | Explicit CLI entries and the targets selected for the active resolution profile establish module reachability. No heuristic `src/index` entry is invented. |
| Public value surface | When `exports` exists, evaluate it independently for every supported external value lane (`node-import`, `node-require`, and `bundler-import`) and union the selected targets. Without `exports`, union `module` for bundler consumers and `main` for Node plus bundler fallback. A target protects only identities it actually exposes; a barrel never protects unexported siblings. |
| Public type surface | Evaluate type-enabled import and require lanes in declaration order, then `types`/`typings` when `exports` is absent, and union type identities only. A value target or type branch cannot protect the other namespace. |
| Public-surface opacity | An unsupported condition shape or unresolved selected public branch makes the affected package surface incomplete; it cannot silently protect the whole file or permit a dead-identity absence claim. |
| Private package | `private: true` disables external-public protection from package fields; explicit entries still affect reachability and real workspace consumers still contribute fan-in. |
| Test consumer | Contributes test fan-in and protects `dead-in-test`, but leaves a production-zero identity eligible for `dead-in-production` review. |
| Side-effect/broad consumer | Preserves module liveness or marks target identities broad/unknown without incrementing exact identity fan-in. |
| Generated/vendor definition | May receive and contribute edges but is muted from default removal candidates with its classification reason. |

### 5.2 Uncertainty Propagation

An exact absence candidate is emitted only when no potential-consumer limitation intersects that identity. An intersecting limitation produces queryable incomplete liveness evidence, not a deletion candidate.

| Condition | Limitation scope |
| --- | --- |
| Recoverable parse with complete module-use extraction | `File`; extracted target facts remain usable, while unsupported local definitions stay limited to the file. |
| Unrecoverable parse or unknown module-use completeness | `Workspace`; the file could hide a consumer anywhere in the supported scan scope. |
| Nonliteral dynamic import | `ExplicitTargets` when a static path prefix bounds inventory matches; otherwise `Workspace`. |
| Unsupported `import.meta.glob` | `ExplicitTargets` for a literal static base; otherwise the importer's `Package`. |
| Computed CommonJS property on a resolved module | `Module` for that target and broad use across its value exports. |
| Opaque Vue template | Imported component candidates and observed global registrations as `ExplicitTargets`; `Package` when that set cannot be bounded. |
| Unsupported SFC dialect | `Workspace`; its unparsed script or template could hide consumers anywhere in the supported scan scope. |
| Unresolved internal relative/configured alias | Resolver probe candidates as `ExplicitTargets`; `Workspace` when configuration opacity prevents a bounded domain. |
| Unknown generated virtual module | Observed generated-map targets as `ExplicitTargets`; otherwise the importer's `Package`. |

The limitation and its scope are canonical evidence. Reducers may narrow a scope only with additional grounded targets and may never silently drop it.

## 6. Canonical Evidence and Query Contract

A successful run publishes:

```text
.lumin/latest.json
.lumin/lifecycle.store
.lumin/attempts/<attempt-id>/attempt.json
.lumin/runs/<run-id>/run.json
.lumin/runs/<run-id>/evidence.store
```

No legacy analysis JSON is emitted by default.

The slice implements:

```text
lumin audit [--resolution-profile <bundler|node|node16|nodenext>]
lumin overview
lumin findings --run <run-id> --area dead-code [--cursor <cursor>]
lumin explain --run <run-id> <finding-id> [--evidence-cursor <cursor>] [--relations-cursor <cursor>]
lumin related --run <run-id> <finding-id> [--cursor <cursor>]
lumin files --run <run-id> <path> [--cursor <cursor>]
lumin capabilities
lumin export sarif --run <run-id>
```

All collection queries are bounded, deterministic, and cursor-resumable. Required capability failure appears in `overview` before ordinary findings.

## 7. Write-Gate Contract

The slice implements:

```text
lumin pre-write --operation-id <operation-id> [typed intent flags]
lumin post-write <gate-id> --operation-id <operation-id>
lumin gate show <gate-id> [--revision <revision>]
lumin gate findings <gate-id> --revision <revision> [--cursor <cursor>]
lumin gate explain <gate-id> --revision <revision> <finding-id> [--evidence-cursor <cursor>] [--relations-cursor <cursor>]
lumin gate list --active [--cursor <cursor>]
lumin gate operation show <operation-id>
lumin gate abandon <gate-id> --reason <text>
```

Required behavior:

- no request JSON file;
- caller-retained operation IDs make pre-write and post-write retries idempotent;
- one durable gate ID returned by pre-write;
- baseline built from exact worktree bytes and returned as `GateBaselineObservationId`;
- language and nearest dependency owner inferred from planned paths;
- mixed JS/TS/SFC paths handled inside one gate, with unsupported dialects remaining explicit;
- write/write and write/semantic-read conflicts rejected;
- nonconflicting gates may analyze concurrently and reconcile close in immutable transition order;
- post-write detects unplanned changed, new, removed, and renamed paths;
- post-write checks dead-code, resolution, dependency-owner, and opacity deltas owned by this slice;
- shape and type-escape lanes remain visibly unavailable;
- post-write requires the explicit gate ID and checks actual writes against other active gates;
- post-write does not launch a full audit unless explicitly requested;
- storage/scan/operation-liveness locks released before result transport while an active gate's durable path lease remains;
- completed gate remains queryable.

First-slice owners emit typed signals; `lumin-evidence::gate_policy` assigns these effects:

| Signal | Signal/fact owner | Effect |
| --- | --- | --- |
| final planned-path containment violation, lease conflict, unexplained transition, unplanned write, or terminal cross-gate conflict | engine gate service from typed inventory/store outcomes | `Block` |
| newly unresolved internal edge | `lumin-resolve` | `Block` |
| missing declared dependency ownership | `lumin-inventory` | `Block` |
| new dead export with zero exact/broad fan-in, complete potential-consumer coverage, no public protection, and no generated/vendor mute | `lumin-dead` | `Block` |
| required owner unavailable/failed, newly opaque evidence intersecting the planned affected set, unobservable required delta input, or changed path awaiting another active gate's terminal transition | owning capability or engine gate service | `Incomplete` |
| baseline/config/source drift, changed admitted alias escaping root, or intervening transition touching semantic reads | owning observation fact plus engine gate service | `Stale` |
| unchanged pre-existing finding or grounded low-confidence/advisory candidate | owning capability | `Warn` |

Caller-declared root escape is not a signal: it is malformed input, exits `2`, and creates no operation, gate ID, record, or lease. Signal types stay dependency-light in `lumin-model`; the closed mapping and `gate-policy.v1` stay in `lumin-evidence`; the engine only invokes the mapping and ARCH-002 reducer. Pre-write rejection creates no durable gate lease; failed post-write remains active and records the attempted revision.

A Rust path in this slice produces an explicit unsupported-language gate finding and cannot be silently routed to the JS owner.

## 8. Execution Contract

The slice uses the final ARCH-001 runtime:

- one local Rayon pool;
- Kahn scheduling over the actual task DAG;
- a profile-fixed stage set with empty batches for absent languages;
- file-level parallel extraction;
- `lumin-sfc` finalization after inline and external JS facts are available;
- deterministic reducers;
- independent graph-dependent analysis tasks where applicable;
- one store writer;
- no global pool, nested pool, or shared mutable graph;
- no JSON between stages;
- exact-byte cache identity;
- `jobs=1` as the reference execution of the same engine.

There is no sequential compatibility engine.

## 9. Truth Corpus

The implementation creates repository fixtures with hand-authored expected truth, not expectations copied from the legacy output.

| Corpus case | Required truth |
| --- | --- |
| `plain-esm` | Exact named/default/type-only fan-in and side-effect reachability remain distinct. |
| `ignore-precedence` | Hard excludes, explicit include/exclude, nested `.gitignore`, and unobserved machine-global rules follow Section 3.1 exactly. |
| `source-role-classification` | Test, production override, generated marker, authored override, vendor, and declaration roles persist version/reason/source without generic-directory muting. |
| `extension-probe-precedence` | Explicit TypeScript/Vue paths are exact; JavaScript runtime-output substitution precedes the runtime file; extensionless, declaration, and directory behavior follows Section 4. |
| `declaration-type-space` | Declaration facts satisfy type space only and cannot make a value export live. |
| `tsconfig-aliases` | Exact, wildcard, `baseUrl`, and supported `extends` precedence matches Section 4; unsupported config remains visible. |
| `workspace-package-exports` | Exact/pattern exports and edge-specific conditions resolve deterministically and define identity-scoped public surfaces. |
| `module-format-conditions` | Node16/NodeNext importer format selects import/require conditions from extension, nearest package type, and edge syntax. |
| `public-condition-union` | Import, require, bundler, and type public lanes protect only the identities selected for each supported lane. |
| `package-fields-no-exports` | Bundler `module`, Node/bundler `main`, and type fields follow their declared resolution and public-protection roles. |
| `resolution-profile-selection` | Invocation override wins; otherwise mixed importers use nearest supported tsconfig profiles; no-config importers use the recorded `bundler` product default; explicit unsupported values remain incomplete. |
| `reachable-dead-sibling` | A live file can still contain a zero-fan-in dead export candidate. |
| `public-reexport-sibling` | One public re-export is protected; three unexported dead siblings remain candidates. |
| `vue-entry` | `main.js -> App.vue` resolves and the graph completes. |
| `vue-inline-script-setup` | Inline script facts bind template components through `finalize-sfc-facts` with parent spans. |
| `vue-external-script` | External script bytes are parsed once and attached without copied facts; conflicting mode is unsupported. |
| `vue-missing-target` | Missing `.vue` import becomes unresolved evidence without aborting other files. |
| `vue-non-source-asset` | Style/resource references do not resolve to declaration sidecars or source edges. |
| `sfc-dialect-boundary` | `.vue`, `.svelte`, and `.astro` enter one SFC stage contract; Vue completes, while Svelte/Astro return explicit dialect-scoped unavailable evidence and workspace liveness limitation without graph abortion or framework policy outside `lumin-sfc`. |
| `next-route-group` | Paths such as `(doc)/layout.tsx` are accepted and resolved normally. |
| `dynamic-literal-member` | Literal dynamic member use preserves member precision. |
| `dynamic-nonliteral` | Nonliteral dynamic import creates opacity, not empty evidence. |
| `import-meta-glob` | Supported relative patterns expand deterministically; unsupported aliases remain visible. |
| `cjs-computed` | Computed destructuring or export access degrades to broad evidence. |
| `parse-failure-propagation` | Recoverable and unrecoverable parse limitations constrain only the scopes defined in Section 5.2. |
| `nearest-manifest` | Dependency checks use the owner manifest nearest each planned path. |
| `parallel-gates` | Read/read overlap coexists; write/write and write/read conflict atomically. |
| `intervening-gate-transitions` | Disjoint A/B gates may analyze together; A reconciles B only after B publishes an exact terminal identity chain, stays incomplete while B's changed path is active, becomes stale when B touches A's semantic reads, and denies unexplained third-party changes. |
| `gate-path-identity` | New paths, aliases, directory descendants, symlinks/junctions, case policy, and rename endpoints follow ARCH-002. |
| `gate-config-drift` | A changed semantic input makes the gate stale; an actual cross-gate write is denied. |
| `gate-prewrite-observation` | Provisional admission, editor quiescence, exact baseline capture, and final store promotion bind `Allow` to one returned `GateBaselineObservationId`; interrupted admission leaves no active gate lease. |
| `gate-final-observation` | Source/config drift during post-write cannot produce `Allow` or release the active lease. |
| `gate-lifecycle-effects` | Every pre/post decision follows the fixed effect precedence and lifecycle transition table. |
| `gate-operation-idempotency` | Same operation ID/request joins live work, retries an interrupted pre-commit execution, or returns one committed gate/close revision; conflicting reuse is malformed; a new close after a failed revision needs a new operation ID; injected post-commit delivery failure is recovered through `gate operation show`. |
| `gate-reopen-after-process-exit` | Open and close a gate, terminate the process, then use a new process to show the exact gate revision and page its findings/evidence. |
| `unplanned-edit` | Unplanned changed, new, removed, and renamed paths cannot receive an allow decision. |
| `mixed-vue-gate` | JS and Vue changes share one user gate and keep owner-specific facts. |
| `required-capability-failure` | Overview warns that dead analysis is unavailable and never renders zero. |
| `snapshot-and-latest` | Mid-scan drift blocks completion; failed or interrupted attempts remain visible beside the last completed run. |
| `bounded-nested-query` | Run and gate-revision pages expose immutable scope, totals, truncation, and stable top-level and nested continuation. |
| `request-path-escape` | Caller-declared root escape exits `2` without operation record, gate ID, or lease; later admitted alias drift and final containment violation follow their distinct stale/block contracts. |
| `corrupt-store` | Corrupt canonical storage hard-stops without fallback or empty evidence. |
| `crash-publication` | Attempt allocation, running-envelope, latest-pointer, run-rename, terminal-attempt, and pointer-replacement crash points each have the single ARCH-002 outcome; a renamed orphan without terminal success remains interrupted and is never adopted as success. |
| `retention-latest-protection` | Prune plans exclude both latest-pointer targets and their linked attempt/run closure, and stale confirmation cannot create a dangling pointer. |

The corpus must include repositories synthesized from or minimized around real failure shapes, including Vue core-style package layouts and a Next.js route-group layout. A copied fixture records origin, license, source revision, and modifications in a local `PROVENANCE.md`; synthetic structure is preferred when copied code is unnecessary. Store-state fixtures are generated in a test temp root and do not require committing ignored `.lumin` output.

## 10. Differential Use of Legacy Tools

Legacy Lumin and Fallow may be run against corpus repositories to discover disagreements. They are not the expected-value owner.

Every disagreement is classified as:

- intentional parity;
- intentional Lumin v2 correction;
- unsupported and visible;
- unresolved specification question.

Code is harvested from the legacy product only when a focused behavior test proves the required contract and the code fits the new owner boundary. Whole modules and bridge layers are not copied.

## 11. Skills and Distribution

The slice ships:

- Windows x64 prebuilt `lumin`;
- Linux x64 musl prebuilt `lumin`;
- integrity metadata tied to build identity;
- one Codex skill;
- one Claude Code skill;
- behavioral package probes for both binaries and both skill adapters.

Skills contain the concise audit/query/write-gate workflow, generate and retain operation IDs before mutating commands, and recover committed delivery failures through the public operation query. Both adapters invoke the same public binary commands and DTOs. They do not package Rust source fallback, Node analysis dependencies, duplicated semantic tables, or duplicated command contracts.

Runtime execution with Cargo unavailable is part of package acceptance.

## 12. Performance Evidence

Performance approval has two non-circular phases.

**Phase 0 feasibility:** before this document becomes active, non-production harnesses measure store locking/backend behavior, OXC parser memory and stack needs, and Windows/Linux static packaging feasibility. They cannot expose product APIs or become a production scaffold. Reproducible probe source, lock/toolchain identity, fixture hashes, commands, expected invariants, and raw results remain under `reviews/probes/<probe-id>/` outside the production workspace; disposable binaries and build output are removed.

Architecture review then approves target budgets for:

- cold full slice audit;
- warm unchanged audit;
- cold pre-write;
- warm pre-write;
- post-write for one changed file;
- post-write for a representative multi-file wave;
- peak resident memory;
- `jobs=1` versus default jobs scaling.

Targets use named hardware/corpora, legacy baselines, and Phase 0 probes. They are goals rather than claims that an unimplemented product already achieved them.

**Phase 1 acceptance:** the completed public `lumin` binary is measured against every target below. A missed target is a slice failure or an explicitly reviewed contract revision; CI cannot invent or relax a number after seeing the result.

Blocking benchmark environments are:

- native Windows on NTFS;
- WSL on ext4;
- Linux CI or a declared release-compatible Linux host.

WSL against `/mnt/<drive>` is a separately labeled report-only diagnostic because host filesystem, antivirus, and mount policy are not release-controlled. It must run and report the same metrics, but it does not participate in AC 16's pass/fail budget. A regression there remains visible and may trigger a later product-contract amendment.

Every benchmark reports source file count, total bytes, cache state, worker count, filesystem class, stage timings, and peak memory.

## 13. Non-Goals

SLICE-001 does not implement:

- Rust repository analysis;
- production-complete Svelte or Astro dialect behavior;
- function, block, or shape clones;
- full topology and discipline review;
- natural-language intent parsing;
- a daemon or MCP transport;
- default legacy artifact emission;
- runtime source compilation;
- a second fallback analyzer.

These omissions must be visible through `lumin capabilities` and relevant overview limitations.

## 14. Acceptance Criteria

1. Every corpus row passes through the public `lumin` binary.
2. The SFC boundary admits Vue, Svelte, and Astro through one stage contract: Vue and Next.js regressions complete without process abort, while unsupported dialects produce explicit per-dialect unavailable limitations and never inherit Vue completeness.
3. The 20-module public-re-export corpus reports all 60 dead siblings and protects all 20 public identities.
4. A reachable file's unused export remains a candidate.
5. `jobs=1` and repeated default-job runs produce identical canonical semantic dumps and finding IDs; runtime metrics and physical store bytes are excluded.
6. Randomized worker completion tests preserve output identity.
7. No analyzed source payload is read or parsed more than once for extraction in a cold run; the separate final hash-only freshness pass is measured and does not reparse.
8. No runtime path executes Node or Cargo.
9. Windows/Linux packages and Codex/Claude Code adapters pass one public binary behavior contract without embedded semantic fallbacks.
10. A user can perform pre-write and post-write using path-scoped typed flags, stable machine output, caller-retained operation IDs, and one explicit gate ID; the completed gate reopens in a new process.
11. Gates with nonconflicting read/write sets may analyze concurrently; close reconciles immutable terminal transitions in order, while write/write and write/read conflicts fail before edits are authorized.
12. Query output is bounded and exhaustive results are reachable through cursors.
13. The default run emits no legacy artifact warehouse.
14. Required failures, snapshot freshness, and unsupported capabilities are prominent and queryable.
15. Strict workspace formatting, lint, unit, integration, corpus, dependency-edge, and package checks pass.
16. The public binary meets the approved Phase 1 performance and memory targets on every blocking environment and reports the `/mnt/<drive>` diagnostic separately.
17. Operation-ID retry and post-commit delivery recovery never duplicate a gate or close revision.
18. Publication recovery and retention preserve one crash outcome per point and cannot leave a latest pointer dangling.

## 15. Acceptance Traceability

| AC | Behavior test | Corpus/fixture | Command | Expected proof |
| --- | --- | --- | --- | --- |
| 1 | `foundation_corpus_contract` | all Section 9 rows | `lumin-xtask corpus foundation` | Every expected query value matches authored truth. |
| 2 | `framework_failures_are_scoped` | SFC dialect, Vue, and route-group rows | `lumin-xtask corpus foundation` | `overview` reports per-dialect Vue completion or scoped unavailable dialect evidence, never aggregate SFC completeness, process abort, or framework policy outside `lumin-sfc`. |
| 3 | `public_surface_is_identity_scoped` | 20-module re-export matrix | `lumin-xtask corpus foundation` | 60 candidates and 20 protected identities. |
| 4 | `reachable_module_keeps_dead_exports` | `reachable-dead-sibling` | `lumin-xtask corpus foundation` | The unused sibling remains a candidate. |
| 5 | `semantic_dump_is_worker_invariant` | full foundation corpus | `lumin-xtask corpus foundation --determinism` | Canonical semantic dump and finding IDs match. |
| 6 | `scheduler_completion_order_is_irrelevant` | randomized stage-result fixture | `cargo test -p lumin-engine` | Repeated randomized completion yields one semantic dump. |
| 7 | `source_payload_is_extracted_once` | read-counter plus Vue external script | `lumin-xtask corpus foundation` | Read/parse counters distinguish extraction from final hash validation. |
| 8 | `runtime_has_no_source_fallback` | package runtime probe | `lumin-xtask package-check <target>` | Execution succeeds with Node and Cargo unavailable. |
| 9 | `packages_and_skills_share_behavior_contract` | package fixture set plus packaged Codex/Claude adapters | both target package checks plus `lumin-xtask package-check skills` | Windows/Linux query values match; both adapters invoke the same public commands/DTOs with no embedded semantic table or source fallback. |
| 10 | `gate_round_trip_requires_ids_and_reopens` | `mixed-vue-gate`, `gate-reopen-after-process-exit` | `lumin-xtask corpus foundation` | Operation/gate IDs complete the round trip, then a new process queries the exact completed revision and paged evidence. |
| 11 | `gate_conflicts_and_transitions_are_serializable` | parallel/config/path identity/intervening-transition rows | `lumin-xtask corpus foundation` | Read/read admits; direct conflicts reject; disjoint terminal chains reconcile; active or unexplained changes cannot authorize. |
| 12 | `all_pages_are_reachable` | `bounded-nested-query` | `lumin-xtask corpus foundation` | Run and gate-revision cursor traversal returns exactly `total` top-level and nested items without following a newer scope. |
| 13 | `default_publication_is_bounded` | output-layout fixture | `lumin-xtask corpus foundation` | Only the repository lifecycle store, attempt/run envelopes, canonical evidence store, and latest pointer are published. |
| 14 | `failure_and_freshness_are_visible` | required-failure, parse, snapshot, request-path-escape, and corrupt-store rows | `lumin-xtask corpus foundation` | `overview` or the gate response exposes incomplete/stale/failed/malformed states and never zero. |
| 15 | `repository_policy_suite` | workspace and source policy | fmt, Clippy, workspace test, architecture-check | Every required quality command exits successfully. |
| 16 | `release_performance_matrix` | named benchmark corpora | `lumin-xtask benchmark foundation` | Blocking time/memory targets are met and the `/mnt/<drive>` diagnostic is reported. |
| 17 | `gate_mutations_are_idempotent` | `gate-operation-idempotency` | `lumin-xtask corpus foundation` | Retry returns one committed gate/revision and operation query recovers an injected delivery failure. |
| 18 | `publication_and_prune_preserve_pointer_truth` | crash and retention rows | `lumin-xtask corpus foundation --store-crash` | Every crash point has one outcome and prune never removes a protected latest linkage. |

## 16. Product AC Coverage

| Product AC | Slice status | Slice proof |
| --- | --- | --- |
| 1 one native process | in scope | Slice AC 8 and architecture-check runtime-launch policy. |
| 2 prebuilt Windows/Linux | in scope | Slice AC 8-9 and package probes. |
| 3 worker determinism | in scope | Slice AC 5-6. |
| 4 required failure visible | in scope | Slice AC 14 and failure corpus. |
| 5 no intent JSON workflow | in scope | Slice AC 10 and gate round trip. |
| 6 completed gate queryable | in scope | Slice AC 10 plus restart/reopen corpus. |
| 7 resumable truncation | in scope | Slice AC 12 and nested cursor corpus. |
| 8 framework miss isolation | in scope | Slice AC 2. |
| 9 identity-scoped public export | in scope | Slice AC 3-4. |
| 10 projections are noncanonical | in scope | Slice AC 13 and projection checks. |
| 11 one skill/binary contract | in scope | Slice AC 9 and explicit `package-check skills` proof. |
| 12 corpus/platform/performance evidence | completion-gated | Slice AC 1, 9, and 16; remains unclaimed until all pass. |
| 13 latest failure visible | in scope | Slice AC 14 and `snapshot-and-latest`. |
| 14 explicit post-write gate ID | in scope | Slice AC 10. |
| 15 semantic baseline conflict | in scope | Slice AC 11 and baseline/final-observation plus intervening-transition corpus. |
| 16 idempotent gate mutation | in scope | Slice AC 17 and operation-delivery recovery corpus. |

## 17. Verification Commands

The implementation must provide stable repository commands equivalent to:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p lumin-xtask -- architecture-check
cargo run -p lumin-xtask -- corpus foundation
cargo run -p lumin-xtask -- corpus foundation --determinism
cargo run -p lumin-xtask -- corpus foundation --store-crash
cargo run -p lumin-xtask -- benchmark foundation
cargo run -p lumin-xtask -- package-check windows-x64
cargo run -p lumin-xtask -- package-check linux-x64
cargo run -p lumin-xtask -- package-check skills
```

The exact command wrappers may be finalized with the workspace, but CI and local development must invoke the same underlying checks.
