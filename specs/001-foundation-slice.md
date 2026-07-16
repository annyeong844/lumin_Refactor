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
| Ignore policy | Apply the precedence below. Always exclude `.git`, the reserved `.lumin` state namespace by lexical and physical identity, and dependency-owned `node_modules`; do not prune an authored directory merely because its basename is `target`, `build`, or `coverage`. |
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
  "entries": ["src/main.ts"],
  "scan": {
    "include": ["src/**"],
    "exclude": ["src/legacy/**"],
    "roles": [{"pattern": "test/**", "role": "test"}]
  }
}
```

Unknown fields, unknown roles, malformed patterns, conflicting role declarations, or a second root config are request/configuration hard-stops: no completed run or authorizing gate decision is published. They are not converted into scoped limitations. Patterns are canonical-root-relative, slash-normalized Git-wildmatch patterns. Repeated CLI `--include <pattern>`, `--exclude <pattern>`, and `--role-at <pattern> <role>` values form the invocation tier; they do not mutate `lumin.json`.

Every native path is lowered before classification into ARCH-002 `repo-path.v1`: exact UTF-8 components are preserved without Unicode normalization, Unix non-UTF-8 components retain raw bytes, and Windows non-scalar components retain WTF-16 code units. Stable IDs, sort keys, cache keys, cursor anchors, scan matching, and gate sets use the canonical binary form. Machine DTOs always include its base64 form plus nonauthoritative display text; `--path` uses native arguments and `--paths0-from` uses Unix raw bytes or Windows WTF-8 NUL records. The first slice must not omit, lossy-convert, or merge an admitted path merely because it is not printable Unicode.

Git-wildmatch operates over the ARCH-002 slash-separated match bytes without Unicode normalization. `.gitignore` retains its pattern bytes; invocation and JSON patterns are UTF-8. A wildcard may match a native-only component, but display escaping never participates in matching.

Repeated `--entry <repo-path>` values on audit or pre-write form the invocation entry tier. When at least one is supplied they replace, rather than append to, `lumin.json.entries`; otherwise configured entries apply. Post-write reuses the baseline tier. Entry paths are canonical-root-relative source paths, are normalized and deduplicated by `SourceId`, and do not override hard exclusions or scan exclusion. A caller entry that escapes the root is malformed; a configured, missing, ignored, excluded, or out-of-domain entry is typed incomplete configuration evidence. Effective entries and their configuration source participate in `AnalysisInputId` and gate semantic reads.

Path containment outcomes are distinct:

| Input condition | First-slice result |
| --- | --- |
| caller-supplied lexical or resolved physical root escape | malformed request, exit `2`, no operation/gate/lease |
| caller-supplied `.lumin` path, descendant, or physical alias of reserved state | malformed request, exit `2`, no operation/gate/lease |
| any repository-owned config field declared as a repository path (entry/scan pattern target, root-contained tsconfig `extends`/`baseUrl`/`paths` target, or workspace source glob) with lexical or resolved physical root escape | malformed configuration hard-stop; no completed run or authorizing gate |
| `.lumin` or any managed parent is a symlink/junction/reparse/mount crossing, has foreign contents/schema, or carries a different repository/root identity | `ForeignStateNamespace` integrity hard-stop; no scan result or gate revision |
| canonical `.lumin` state changes outside validated Lumin operations | store-integrity hard-stop, never an unplanned source edit or clean absence |
| root-contained missing, ignored, excluded, or out-of-domain entry | typed `ExplicitEntryUnavailable` incomplete evidence in the derived package/workspace scope |
| admitted alias/symlink identity later escapes | existing ARCH-002 `Stale` baseline or final containment `Block` contract |
| external or unsupported config semantics whose target cannot be bounded | typed scoped incomplete evidence; never a hidden outside-root read |

Supported tsconfig/workspace metadata may name external packages as package semantics, but no repository source/config read crosses the canonical root without an explicit supported external-capability contract.

Scan admission uses this order:

1. Reject root escapes and hard exclusions: `.git`, the no-follow admitted `.lumin` namespace and all aliases/descendants, and dependency-owned `node_modules`.
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

The compiled scan/classification, `repo-path.v1`, Git-wildmatch, and ownership rule versions participate in `AnalysisContractId`; selected path bytes, patterns, overrides, classifications, reasons, and configuration identities participate in `AnalysisInputId`.

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
2. The effective `compilerOptions.moduleResolution`, after applying the supported root-contained `extends` chain, in the importer's nearest controlling `tsconfig`: `bundler` -> `bundler`, `node`/`node10` -> `node`, `node16` -> `node16`, and `nodenext` -> `nodenext`; unsupported values follow the incomplete rule below.
3. The named first-slice product default, `bundler`, when no explicit supported value exists.

Without an invocation override, an explicit unsupported value such as `classic` or an unknown value makes resolution incomplete for affected importers; it never falls through to the product default. The invocation override supersedes only `moduleResolution` profile selection. An unreadable controlling config remains incomplete even under an override because aliases, package ownership, or other semantic inputs may be unknown. Without an override, mixed workspaces retain importer-local profiles from their nearest configs. The override applies to every importer, including inline and external Vue script source uses. Vue template-to-component binding consumes already resolved script bindings and is not a resolver profile lane; `<script src>` is an exact SFC source reference rather than a JavaScript package-resolution fallback.

Audit and pre-write accept the typed override; post-write reuses that caller-supplied override and cannot replace it. When no override fixes the mode, post-write recomputes importer profile facts from a validated self-writable config change; a profile config changed outside this gate remains stale. Every selected current profile records mode, source (`invocation`, config path, or `product-default`), and reason. Those values and consulted configs participate in the sealed revision's `AnalysisInputId`; `resolution-profile-selection.v1`, its mappings, and the default participate in `AnalysisContractId`.

The first slice freezes `resolver-config-semantics.v1` against the `typescript-config-6.0` and declared Node/package compatibility baseline. The registry key is configuration container, field path, and value shape; every observed semantic field is classified exactly once before any probe:

| Class | First-slice contract |
| --- | --- |
| `SupportedAndModeled` | `tsconfig` `extends`; `compilerOptions.module` only through the compatibility matrix below, plus `moduleResolution`, `baseUrl`, and `paths` under the exact rules below; package `name`, `private`, `type`, `exports` supported shapes, `types`, `typings`, `module`, `main`, and supported workspace declarations. Their effective values participate in probing and input identity. |
| `KnownResolutionNeutral` | The closed complement inside the exact pinned TypeScript 6.0 key manifest after removing the supported and unsupported sets, including `target`, strictness flags, `rootDir`, `outDir`, declaration/emit flags, and checking-only options. Top-level `files`/`include`/`exclude` and `allowJs`/`checkJs` are resolver-neutral because Section 3.1 scan policy is the sole source-set owner. Unknown top-level package metadata is neutral under the frozen Node/package baseline only because this product resolver never consults it; it is still retained as observed metadata. No reviewer/implementation may classify a field neutral ad hoc. |
| `UnsupportedResolutionAffecting` | `references`/`typeAcquisition`; `moduleSuffixes`, `customConditions`, `rootDirs`, `preserveSymlinks`, `resolvePackageJsonExports`, `resolvePackageJsonImports`, `allowImportingTsExtensions`, `allowArbitraryExtensions`, `resolveJsonModule`, `noResolve`, `rewriteRelativeImportExtensions`, `moduleDetection`, `types`, `typeRoots`, `plugins`, or JSX runtime/factory/import-source fields; package `imports`, `typesVersions`, `browser`, or `react-native`; and any unsupported/malformed nested shape beneath a registered resolver/public-surface field. No simplified probe may run for the affected domain. |

An unknown `compilerOptions` or top-level `tsconfig` key absent from the pinned compatibility key set is `UnsupportedResolutionAffecting` unless this registry explicitly names it neutral; future TypeScript options therefore fail closed until a contract-version amendment. The checked-in registry manifest records the exact TypeScript key-set digest, each field/shape class, and package-field baseline; that content identity participates in `AnalysisContractId` and architecture-check rejects a build whose compiled match table differs. An invocation resolution-profile override changes only `moduleResolution` selection and cannot suppress another unsupported field. `TsconfigSemanticsUnsupported` scopes affected importers to one package when provable, otherwise the workspace. Package `imports` uses `PackageImportsUnsupported`; `typesVersions` or unsupported public/export/package shapes use `PublicSurfaceUnsupported` for the owning package. Every limitation is emitted before target selection and blocks absence in that domain. Observed field paths/shapes/values participate in exact cache keys, `AnalysisInputId`, and semantic-read closure.

The supported `module` compatibility matrix is closed: legacy `node` accepts `commonjs` and selects the `require` lane; `bundler` accepts `preserve`, `es2015`, `es2020`, `es2022`, or `esnext` and selects the `import` lane; `node16` accepts only `node16`, and `nodenext` accepts only `nodenext`, with both using per-file Node format below. With no explicit `module`, those same profile defaults apply. Any other value or profile/module pair is `TsconfigSemanticsUnsupported`; the resolver never guesses an emit condition lane.

| Specifier or host candidate | Ordered first-slice probes |
| --- | --- |
| Explicit TypeScript or SFC source path | Exact path only for `.ts`, `.tsx`, `.mts`, `.cts`, `.vue`, `.svelte`, or `.astro`. Vue targets are analyzable; Svelte and Astro targets resolve as SFC sources with unavailable analysis evidence. Explicit declaration paths are exact and type-space only. JavaScript runtime extensions use the substitution rows below even when written explicitly. |
| Runtime `.js` candidate | Value space: `.ts`, `.tsx`, `.js`, `.jsx`. Type space inserts `.d.ts` after `.tsx`. |
| Runtime `.jsx` candidate | Value space: `.tsx`, `.jsx`. Type space inserts `.d.ts` after `.tsx`. |
| Runtime `.mjs` candidate | Value space: `.mts`, `.mjs`. Type space inserts `.d.mts` after `.mts`. |
| Runtime `.cjs` candidate | Value space: `.cts`, `.cjs`. Type space inserts `.d.cts` after `.cts`. |
| Extensionless path in a permitting mode | Derive the host `.js` candidate and apply its substitution row. Do not invent extensionless `.mts`, `.cts`, `.mjs`, or `.cjs` candidates. |
| Directory in a permitting mode | Resolve its supported `package.json` entry under the package-field rules, then derive an `index.js` host candidate and apply the `.js` substitution row. |
| Unsupported explicit extension | Return grounded `NonSourceAsset`. Do not substitute a declaration sidecar or create an internal-consumer limitation. |

A declaration may satisfy type-space resolution but never proves that a runtime value target exists. When a value import also has declaration evidence, the resolver records that type companion separately from the value target.

Specifier and configuration policy is:

| Class | Contract |
| --- | --- |
| Resolution mode | Support `bundler`, legacy `node`, `node16`, and `nodenext`. Bundler/legacy-node and CJS lanes permit extensionless and directory fallback; Node16/NodeNext ESM lanes require an explicit relative extension and skip the extensionless and directory rows. Unsupported modes make resolution incomplete rather than selecting a fallback mode. |
| Importer format | First apply the supported profile/`module` matrix above. Legacy `node` static imports use `require`; bundler static imports use `import`. In Node16/NodeNext, `.mts`/`.mjs` are ESM and `.cts`/`.cjs` are CJS; `.ts`/`.tsx`/`.js`/`.jsx` and matching declarations use the nearest package `"type": "module"`, otherwise CJS. Static import/export then selects `import` for ESM and `require` for CJS; `require()` always selects `require`, and dynamic `import()` selects `import`. Embedded Vue script uses the same selected profile and importer-format rules as a physical source with that script mode. |
| Relative | Resolve inside the canonical root with the probe order above. Route-group characters such as `(doc)` are ordinary path bytes. |
| Tsconfig | Use the importer's nearest config, root-contained relative/workspace-package `extends`, child override semantics, and the `baseUrl` of the config that declares each mapping. Cycles are incomplete configuration evidence. External-package extends, project-reference redirection, and every `resolver-config-semantics.v1` unsupported field/shape are incomplete before probing. |
| `paths` | Exact key before wildcard; wildcard keys permit one `*` and use longest literal prefix then declaration order. Probe mapped targets before `baseUrl` and package resolution. |
| Workspace package | Resolve `exports` exact key before one-star patterns. Within a condition object, the first supported matching branch in declaration order wins; the active set includes `types` for type space, `import` or `require` for the edge mode, `node`, and `default`. Edge resolution selects one lane; external public-surface protection follows the supported-lane union in Section 5.1. Unsupported condition shapes remain visible. |
| Package fields without `exports` | Type space probes `types`, then `typings`, then a declaration companion for the selected value target. Value space uses `module` then `main` in bundler mode and `main` in Node modes, followed by permitted directory fallback. A type field never proves runtime value liveness. |
| Bare external | Classify as `External` after workspace ownership lookup; never probe a similarly named relative file. |
| Root-absolute internal-looking specifier, package `imports`, or unsupported alias | Return typed `Unsupported` or `Unresolved` evidence with the limitation scope below; never skip the record. URL imports are complete external/non-source outcomes and do not create an internal-consumer limitation. |
| Generated virtual | Resolve only through an observed generated mapping; otherwise retain a typed virtual limitation. |

Every source use receives one `ResolutionOutcome`. A skipped record without a typed reason is a contract failure. The resolver policy and closed configuration-registry versions participate in `AnalysisContractId`; consulted configuration field/value identities participate in exact cache keys and `AnalysisInputId`.

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
| Entry root | Invocation entries replace configured entries; otherwise `lumin.json.entries` apply. Package public targets selected across the supported profile lanes are additional roots for non-private packages. No heuristic `src/index` entry is invented. A private package with neither an effective explicit entry nor a real workspace consumer has incomplete entry coverage, so package-wide unreachable absence claims are disabled rather than treating every module as dead. |
| Public value surface | When `exports` exists, evaluate it independently for every supported external value lane (`node-import`, `node-require`, and `bundler-import`) and union the selected targets. Without `exports`, union `module` for bundler consumers and `main` for Node plus bundler fallback. A target protects only identities it actually exposes; a barrel never protects unexported siblings. |
| Public type surface | Evaluate type-enabled import and require lanes in declaration order, then `types`/`typings` when `exports` is absent, and union type identities only. A value target or type branch cannot protect the other namespace. |
| Public-surface opacity | An unsupported condition shape or unresolved selected public branch makes the affected package surface incomplete; it cannot silently protect the whole file or permit a dead-identity absence claim. |
| Private package | `private: true` disables external-public protection from package fields; explicit entries still affect reachability and real workspace consumers still contribute fan-in. |
| Test consumer | Contributes test fan-in and protects `dead-in-test`, but leaves a production-zero identity eligible for `dead-in-production` review. |
| Side-effect/broad consumer | Preserves module liveness or marks target identities broad/unknown without incrementing exact identity fan-in. |
| Generated/vendor definition | May receive and contribute edges but is muted from default removal candidates with its classification reason. |

### 5.2 Uncertainty Propagation

An exact absence candidate is emitted only when no potential-consumer limitation intersects that identity. An intersecting limitation produces queryable incomplete liveness evidence, not a deletion candidate. The following registry is exhaustive for first-slice typed incomplete, unsupported, and opaque outcomes:

| Reason variant | Fact owner | Scope and target derivation | Downstream absence effect | Gate relevance before lifecycle delta |
| --- | --- | --- | --- | --- |
| `JsRecoverableParseLocal` | `lumin-js` | `File`; module-use extraction is proven complete. | Keep extracted uses; disable unsupported local-definition absence only. | Required evidence gap; emit `RequiredEvidenceIncomplete` only when the gate needs the missing local fact. |
| `JsModuleUseUnknown` | `lumin-js` | `Workspace`; the file may hide a consumer anywhere in scan scope. | Disable intersecting workspace absence claims. | Required evidence gap; emit `RequiredEvidenceIncomplete` when intersecting required gate evidence. |
| `SourcePayloadUnavailable` | `lumin-inventory` | `Workspace` for an admitted unreadable source. | Disable workspace absence because its imports cannot be bounded. | Required evidence gap; emit `RequiredEvidenceIncomplete` when intersecting required gate evidence. |
| `DynamicImportNonLiteral` | `lumin-js` | `ExplicitTargets` from a static inventory prefix, otherwise `Workspace`. | Treat bounded targets as broad consumers; otherwise disable workspace absence. | Normalized opacity fact enters lifecycle delta classification; unbounded target derivation required by the gate remains `RequiredEvidenceIncomplete`. |
| `ImportMetaGlobUnsupported` | `lumin-js` | `ExplicitTargets` from a literal static base, otherwise the importer's `Package`. | Disable absence only in the derived target/package domain. | Normalized opacity fact enters lifecycle delta classification; an unbounded required target remains `RequiredEvidenceIncomplete`. |
| `CommonJsComputedMember` | `lumin-js` | `Module` for the resolved target. | Mark all value exports on that module broadly consumed, without exact fan-in. | Normalized opacity fact enters lifecycle delta classification. |
| `VueTemplateOpaque` | `lumin-sfc` | Imported component candidates and observed global registrations as `ExplicitTargets`; otherwise the parent `Package`. | Disable component-identity absence in that domain. | Normalized opacity fact enters lifecycle delta classification; unbounded required binding remains `RequiredEvidenceIncomplete`. |
| `SfcDecompositionUnknown` | `lumin-sfc` | `Workspace`; script/resource boundaries or module-use completeness are unknown. | Disable workspace consumer absence. | Required evidence gap; emit `RequiredEvidenceIncomplete` when intersecting required gate evidence. |
| `VueExternalScriptModeConflict` | `lumin-sfc` | Parent and external source owner `Package`; `Workspace` when their owners differ or cannot be proven. | Disable script-consumer and template-binding absence in that domain. | Required evidence gap; emit `RequiredEvidenceIncomplete` when intersecting required gate evidence. |
| `SfcDialectUnavailable` | `lumin-sfc` | `Workspace`. | No dead-consumer absence may rely on the unparsed dialect. | Required owner gap; emit `RequiredOwnerUnavailable` when the gate requires that dialect. |
| `InternalSpecifierUnresolved` | `lumin-resolve` | Ordered probe candidates as `ExplicitTargets`; `Workspace` when target configuration is opaque. | Disable absence for candidates; opaque configuration disables workspace absence. | Complete normalized unresolved fact enters lifecycle delta classification; opaque target derivation remains `RequiredEvidenceIncomplete`. |
| `PackageImportsUnsupported` | `lumin-resolve` | The importer's `Package`. | Disable package-local absence because `#` mappings may hide internal consumers. | Required evidence gap; emit `RequiredEvidenceIncomplete` when intersecting required gate evidence. |
| `AliasShapeUnsupported` | `lumin-resolve` | `Package` when every affected importer has one owner, otherwise `Workspace`. | Disable absence in the affected configuration domain. | Required evidence gap; emit `RequiredEvidenceIncomplete` when intersecting required gate evidence. |
| `AbsoluteInternalSpecifierUnsupported` | `lumin-resolve` | `Workspace`. | Disable workspace absence; the target may be any root-contained source. | Required evidence gap; emit `RequiredEvidenceIncomplete` when intersecting required gate evidence. |
| `TsconfigPayloadUnavailable` | `lumin-inventory` | `Package` when all importers controlled by the unreadable input share one owner, otherwise `Workspace`. | Disable absence in that configuration domain. | Required evidence gap; emit `RequiredEvidenceIncomplete` when intersecting required gate evidence. |
| `TsconfigSemanticsUnsupported` | `lumin-resolve` | `Package` when all importers affected by a cycle, external-package extends, project-reference redirect, unsupported mode, unknown compatibility key, or `resolver-config-semantics.v1` unsupported field/shape share one owner; otherwise `Workspace`. | Disable absence in that configuration domain before any simplified probe. | Required evidence gap; emit `RequiredEvidenceIncomplete` when intersecting required gate evidence. |
| `PublicSurfaceUnsupported` | `lumin-resolve` | The owning `Package`, including `typesVersions`, unsupported `exports`, or another registered package public-surface shape. | Do not protect every sibling and do not emit package-surface absence. | Required evidence gap; emit `RequiredEvidenceIncomplete` when intersecting required gate evidence. |
| `GeneratedVirtualUnknown` | `lumin-resolve` | Observed generated-map targets as `ExplicitTargets`; otherwise the importer's `Package`. | Disable absence in the derived domain. | Normalized opacity fact enters lifecycle delta classification; unbounded required targets remain `RequiredEvidenceIncomplete`. |
| `ScanOrIgnoreInputUnobservable` | `lumin-inventory` | `Workspace`. | Disable workspace absence because scan membership is unknown. | Required evidence gap; emit `RequiredEvidenceIncomplete` when intersecting required gate evidence. |
| `PackageMetadataUnobservable` | `lumin-inventory` | The known owner `Package`, otherwise `Workspace`. | Disable owner/public/dependency absence in that domain. | Required evidence gap; emit `RequiredEvidenceIncomplete` when intersecting required gate evidence. |
| `DependencyOwnerAmbiguous` | `lumin-inventory` | The source owner's `Package`; `Workspace` when package ownership is also ambiguous. | Disable dependency-owner absence and inferred lockfile writes in that domain. | Required evidence gap; emit `RequiredEvidenceIncomplete` when intersecting required gate evidence. |
| `WorkspaceOwnershipUnsupported` | `lumin-inventory` | `Workspace`. | Disable workspace/package ownership absence. | Required evidence gap; emit `RequiredEvidenceIncomplete` when intersecting required gate evidence. |
| `ExplicitEntryUnavailable` | `lumin-inventory` | The derivable owner `Package`, otherwise `Workspace`. | Disable unreachable-module absence for that domain. | Required evidence gap; emit `RequiredEvidenceIncomplete` when intersecting required gate evidence. |
| `CapabilityUnavailable` | `lumin-engine` capability registry | Declared paths/analysis area as `ExplicitTargets`; SFC dialects use the stricter row above. | Disable unavailable language, shape, clone, or discipline claims without rerouting ownership. | Required owner gap; emit `RequiredOwnerUnavailable` when the gate requires that capability. |

URL imports and grounded non-source assets are complete external/non-source outcomes, not limitations. Caller root escape and reserved-state input are malformed, while foreign/redirection/state mutation is an integrity hard-stop; none is forced into this table. Private owner enums convert to these model reasons through exhaustive matches, and `lumin-xtask architecture-check` fails if a reason lacks a scope/absence/relevance mapping. Reducers may narrow a scope only with additional grounded targets and may never silently drop it.

This registry owns static fact meaning, limitation scope, and absence impact; it never assigns `GateEffect` directly. A missing fact required to decide the operation may emit `RequiredEvidenceIncomplete` because no sound comparison exists. A complete adverse or opacity fact first enters the total lifecycle delta policy in Section 7, which alone compares it with the immutable opening baseline and classifies every payload relation before any adverse effect is chosen.

## 6. Canonical Evidence and Query Contract

A successful run publishes:

```text
.lumin/repository.json
.lumin/latest.json
.lumin/lifecycle.lock
.lumin/lifecycle.store
.lumin/attempts/<attempt-id>/attempt.json
.lumin/runs/<run-id>/run.json
.lumin/runs/<run-id>/evidence.store
```

No legacy analysis JSON is emitted by default.

The slice implements:

```text
lumin audit [--include <pattern> ...] [--exclude <pattern> ...] [--role-at <pattern> <role> ...] [--entry <repo-path> ...] [--resolution-profile <bundler|node|node16|nodenext>]
lumin overview
lumin findings --run <run-id> --area dead-code [--cursor <cursor>]
lumin explain --run <run-id> <finding-id> [--evidence-cursor <cursor>] [--relations-cursor <cursor>]
lumin related --run <run-id> <finding-id> [--cursor <cursor>]
lumin files --run <run-id> <path> [--cursor <cursor>]
lumin capabilities [--run <run-id>] [--cursor <cursor>]
lumin export sarif --run <run-id>
```

All collection queries are bounded, deterministic, and cursor-resumable. Required capability failure appears in `overview` before ordinary findings.

## 7. Write-Gate Contract

The slice implements:

```text
lumin pre-write --operation-id <operation-id> [--include <pattern> ...] [--exclude <pattern> ...] [--role-at <pattern> <role> ...] [--entry <repo-path> ...] [--resolution-profile <profile>] [typed intent flags]
lumin post-write <gate-id> --operation-id <operation-id>
lumin operation show <operation-id>
lumin gate show <gate-id> [--revision <revision>]
lumin gate findings <gate-id> --revision <revision> [--cursor <cursor>]
lumin gate explain <gate-id> --revision <revision> <finding-id> [--evidence-cursor <cursor>] [--relations-cursor <cursor>]
lumin gate list --active [--cursor <cursor>]
lumin gate abandon <gate-id> --operation-id <operation-id> --reason <text>
lumin gate prune plan --terminal-before <timestamp> --operation-id <operation-id>
lumin gate prune plan show <plan-id> [--cursor <cursor>]
lumin gate prune confirm <plan-id> --operation-id <operation-id>
lumin runs list [--cursor <cursor>]
lumin runs pin <run-id> --operation-id <operation-id> --reason <text>
lumin runs unpin <pin-id> --operation-id <operation-id>
lumin runs prune plan --before <timestamp> --operation-id <operation-id>
lumin runs prune plan show <plan-id> [--cursor <cursor>]
lumin runs prune confirm <plan-id> --operation-id <operation-id>
```

Required behavior:

- no request JSON file;
- caller-retained operation IDs make every gate and retention lifecycle mutation idempotent and recoverable through `lumin operation show`;
- one durable gate ID returned by pre-write;
- baseline and close observations built only after owner-reported semantic inputs reach a fixed point, with every added path demanded without reading, conflict-checked, and reserved before inventory capture or owner/cache consumption;
- language and nearest dependency owner inferred from planned paths;
- mixed JS/TS/SFC paths handled inside one gate, with unsupported dialects remaining explicit;
- write/write and write/semantic-read conflicts rejected;
- nonconflicting gates may analyze concurrently and reconcile close in immutable transition order;
- post-write detects unplanned changed, new, removed, and renamed paths;
- post-write checks dead-code, resolution, dependency-owner, and opacity deltas owned by this slice;
- shape and type-escape lanes remain visibly unavailable;
- post-write requires the explicit gate ID and checks actual writes against other active gates;
- post-write does not launch a full audit unless explicitly requested;
- post-write accepts no replacement scan/entry/profile override tier, reuses the caller-supplied opening overrides from the operation digest, and recomputes config-derived effective values only from validated self-writable inputs;
- storage-transaction locks, catalog-publication guard, and operation-liveness locks released before result transport while an active gate's durable path lease remains; Architecture v1 has no scan lock;
- completed gate remains queryable;
- public retention commands execute the ARCH-002 `Prepared -> Pruning -> Pruned` protocol and never bypass the lifecycle store through `lumin-xtask` internals;
- each run pin returns an independent `PinId`; unpin accepts that ID and cannot remove another consumer's protection;
- every terminal transition capsule referenced by an active gate remains prune-ineligible until that gate closes or is abandoned;
- lifecycle-store migration uses transaction-scoped handles and generation fencing, so an old-generation process cannot commit after replacement;
- latest-pointer publication/recovery and retention confirmation serialize through the exclusive catalog-publication guard and merge `latestAttempt` by sequence/phase plus `latestCompleted` by sequence;
- `.lumin` is admitted no-follow as a repository-bound reserved namespace and cannot enter a scan or gate write through an alias;
- every path and machine DTO preserves exact `repo-path.v1` canonical bytes; display text is never an identity;
- resolver probing begins only after `resolver-config-semantics.v1` classifies every observed semantic field/shape and emits incomplete evidence for unsupported affecting inputs.

For post-write, each fact owner compares the immutable opening semantic baseline with the current validated facts. A failed close revision never becomes the next comparison baseline. Each owner first canonicalizes duplicate rows by its model-owned `DeltaKey`:

| Fact family | First-slice `DeltaKey` |
| --- | --- |
| unresolved internal edge | importer `SourceId`, edge kind, normalized specifier |
| dependency ownership | consumer `SourceId`, dependency name; owner-manifest identity is comparison payload |
| dead export | stable symbol semantic identity and rule version |
| opacity/limitation | source semantic identity, reason variant, stable construct identity |

Targets, affected domain, confidence, grounding, and grounding-evidence identity are payload dimensions, not key fields. The owner then emits exactly one model-owned total classification:

```text
Introduced
| Unchanged
| Regressed { changes }
| Improved { changes }
| ChangedIncomparable { regressions, improvements, incomparable_changes }
| Resolved
| BaselineUnavailable
```

Absent-to-present is `Introduced`, present-to-absent is `Resolved`, and exact payload equality is `Unchanged`. Target and concrete affected-domain additions are regressions; removals are improvements. A `LimitationScope` is first expanded to the exact set of affected `SourceId`/package/target identities, so `Workspace -> Package` is an improvement only when the current package set is a strict subset of the prior domain. First-slice confidence ranks `Low < Medium < High`, and grounding ranks `Opaque < Partial < Grounded`; rank loss is regression and rank gain is improvement. A changed evidence identity or owner payload without a declared order is incomparable; every first-slice semantic field is registered as key, one ARCH-002 dimension, or non-semantic metadata. Any other changed semantic payload defaults to directionless `OwnerPayloadChanged` and therefore cannot be ignored or called unchanged. Only regressions produce `Regressed`, only improvements produce `Improved`, and any mixture or incomparable dimension produces `ChangedIncomparable`. `Resolved`/`Improved` persist evidence but emit no adverse signal. `BaselineUnavailable` cannot be guessed from current state and emits required-evidence incompleteness.

Pre-write has no fabricated post-write delta: complete existing adverse facts are advisory, while evidence required to authorize the planned operation must still be complete. The engine capability registry is the sole fact/signal owner for compiled-profile availability and may emit `RequiredOwnerUnavailable`; it never runs substitute analysis or claims an absent capability's facts.

After that classification, first-slice owners emit typed signals and `lumin-evidence::gate_policy` assigns these effects:

| Signal | Signal/fact owner | Effect |
| --- | --- | --- |
| final planned-path containment violation, lease conflict, unexplained transition, unplanned write, or terminal cross-gate conflict | engine gate service from typed inventory/store outcomes | `Block` |
| introduced unresolved/dependency/dead adverse fact, or a complete grounded target/domain addition for one of those families | owning resolver, inventory, or dead-analysis capability from `Introduced`, `Regressed`, or the regressive dimensions of `ChangedIncomparable` | `Block` |
| `RequiredOwnerUnavailable` | `lumin-engine` capability registry from the compiled profile; no fallback owner | `Incomplete` |
| introduced opacity or opacity-domain addition intersecting the planned affected set | owning capability from `Introduced`, `Regressed`, or regressive dimensions of `ChangedIncomparable` | `Incomplete` |
| confidence/grounding loss, directionless evidence or owner-payload replacement, baseline comparison unavailable, required owner failure, unobservable required delta input, semantic-input demand conflict, or changed path awaiting another active gate's terminal transition | owning capability, capability registry, or engine gate service according to the ARCH-000 authority table | `Incomplete` |
| external or unexplained drift of a protected semantic read outside this gate's leased-plus-actual write set, changed admitted alias escaping root, an intervening transition touching such a protected opening read, or a transition after close-read sealing | owning observation fact plus engine gate service | `Stale` |
| `PreExistingUnchanged` complete adverse fact or grounded advisory candidate | owning capability from `Unchanged` delta or pre-write static fact | `Warn` |

One classification may contain several dimension changes and therefore emit several typed signals; the ARCH-002 reducer fixes their precedence. For example `{a,b} -> {b,c}` is `ChangedIncomparable` and its grounded added target `c` emits the blocking target-addition signal, while the removed target is persisted as improvement. A confidence or grounding loss emits incompleteness even when no target is added. Static limitation rows never bypass delta classification to assign an effect, and every classification/dimension pair has an exhaustive owner mapping checked by architecture-check.

Caller-declared root escape is not a signal: it is malformed input, exits `2`, and creates no operation, gate ID, record, or lease. Signal and delta types stay dependency-light in `lumin-model`; owners compute deltas, the closed mapping and `gate-policy.v1` stay in `lumin-evidence`, and the engine only invokes the mapping and ARCH-002 reducer. Pre-write rejection creates no durable gate lease; failed post-write remains active and records the attempted revision.

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
- two-step owner input discovery: `NeedsInputs` is reserved before inventory capture, cold owners resume from fully owned continuations, and only `Finished` reports exact consumed identities;
- prerequisite-keyed `CachedOwnerStep` replay followed by exact-byte finished identity and complete owner outcome/capability state, diagnostics, facts or opaque/failure payload, limitations, gate-neutral signals, and consulted inputs;
- request-specific gate signals recomputed by the owning capability from the validated cold/warm outcome and current model-owned `GateProjectionContext`;
- `jobs=1` as the reference execution of the same engine.

There is no sequential compatibility engine.

## 9. Truth Corpus

The implementation creates repository fixtures with hand-authored expected truth, not expectations copied from the legacy output.

Every corpus row, including retention and migration fault injection, drives the public `lumin` command DTOs in a child process. `lumin-xtask` may prepare fixtures and select a named test-build crash point, but it cannot import private store APIs or mutate lifecycle rows to manufacture the expected state. An old-schema migration fixture is the exact hashed output of a named prior test-schema public binary and is copied into a temp repository unchanged; its producer revision, schema/generation, command, and logical dump are preserved with the fixture.

| Corpus case | Required truth |
| --- | --- |
| `plain-esm` | Exact named/default/type-only fan-in and side-effect reachability remain distinct. |
| `ignore-precedence` | Hard excludes, explicit include/exclude, nested `.gitignore`, and unobserved machine-global rules follow Section 3.1 exactly. |
| `scan-invocation-containment` | Audit/pre-write scan flags round-trip into the operation digest and `AnalysisInputId`; post-write rejects replacement flags; caller/config root escapes, later alias drift, and root-contained excluded entries produce their distinct Section 3.1 outcomes. |
| `source-role-classification` | Test, production override, generated marker, authored override, vendor, and declaration roles persist version/reason/source without generic-directory muting. |
| `extension-probe-precedence` | Explicit TypeScript/Vue paths are exact; JavaScript runtime-output substitution precedes the runtime file; extensionless, declaration, and directory behavior follows Section 4. |
| `declaration-type-space` | Declaration facts satisfy type space only and cannot make a value export live. |
| `tsconfig-aliases` | Exact, wildcard, `baseUrl`, and supported `extends` precedence matches Section 4; unsupported config remains visible. |
| `tsconfig-module-suffixes-unsupported` | `moduleSuffixes` prevents simplified relative probing and emits scoped `TsconfigSemanticsUnsupported` before any target/fan-in claim. |
| `tsconfig-custom-conditions-unsupported` | `customConditions` prevents package-condition selection for affected importers and cannot fall through to `node`/`default`. |
| `tsconfig-root-dirs-unsupported` | `rootDirs` prevents ordinary relative probing for affected importers and disables absence in the configuration domain. |
| `resolver-config-registry` | Supported and neutral TypeScript 6.0 fields follow their registered class; an unknown compiler option, unsupported affecting field, or malformed semantic shape emits incomplete evidence before probing, and the invocation profile override cannot hide it. |
| `workspace-package-exports` | Exact/pattern exports and edge-specific conditions resolve deterministically and define identity-scoped public surfaces. |
| `package-types-versions-unsupported` | `typesVersions` emits package-scoped `PublicSurfaceUnsupported`; type resolution cannot silently use the unspecialized `types` target. |
| `package-exports-unsupported-shapes` | Unsupported nested export/public-surface shapes remain package-scoped incomplete and never protect a whole file or select a fallback branch. |
| `module-format-conditions` | Node16/NodeNext importer format selects import/require conditions from extension, nearest package type, and edge syntax. |
| `public-condition-union` | Import, require, bundler, and type public lanes protect only the identities selected for each supported lane. |
| `package-fields-no-exports` | Bundler `module`, Node/bundler `main`, and type fields follow their declared resolution and public-protection roles. |
| `resolution-profile-selection` | Invocation override wins for physical and embedded-script importers; otherwise mixed importers use nearest effective supported tsconfig profiles; no-config importers use the recorded `bundler` product default; explicit unsupported values remain incomplete. |
| `explicit-entry-selection` | Repeated invocation entries replace configured entries, aliases deduplicate by `SourceId`, excluded/missing entries stay incomplete, public package targets remain roots, and a private package with no grounded root cannot produce package-wide unreachable absence. |
| `reachable-dead-sibling` | A live file can still contain a zero-fan-in dead export candidate. |
| `public-reexport-sibling` | One public re-export is protected; three unexported dead siblings remain candidates. |
| `vue-entry` | `main.js -> App.vue` resolves and the graph completes. |
| `vue-inline-script-setup` | Inline script facts bind template components through `finalize-sfc-facts` with parent spans. |
| `vue-external-script` | External script bytes are parsed once and attached without copied facts; conflicting mode is unsupported. |
| `vue-resolution-override` | A Vue embedded script follows invocation `node16`/`nodenext` extension rules and bundler override rules exactly; template binding consumes the resulting resolved script binding rather than selecting another lane. |
| `vue-missing-target` | Missing `.vue` import becomes unresolved evidence without aborting other files. |
| `vue-non-source-asset` | Style/resource references do not resolve to declaration sidecars or source edges. |
| `sfc-dialect-boundary` | `.vue`, `.svelte`, and `.astro` enter one SFC stage contract; Vue completes, while Svelte/Astro return explicit dialect-scoped unavailable evidence and workspace liveness limitation without graph abortion or framework policy outside `lumin-sfc`. |
| `next-route-group` | Paths such as `(doc)/layout.tsx` are accepted and resolved normally. |
| `dynamic-literal-member` | Literal dynamic member use preserves member precision. |
| `dynamic-nonliteral` | Nonliteral dynamic import creates opacity, not empty evidence. |
| `import-meta-glob` | Supported relative patterns expand deterministically; unsupported aliases remain visible. |
| `cjs-computed` | Computed destructuring or export access degrades to broad evidence. |
| `parse-failure-propagation` | Recoverable and unrecoverable parse limitations constrain only the scopes defined in Section 5.2. |
| `limitation-scope-exhaustiveness` | Every first-slice private reason converts through an exhaustive match to one Section 5.2 model reason, scope, absence effect, and gate relevance; missing mappings fail architecture-check and no static row assigns a lifecycle effect. |
| `nearest-manifest` | Dependency checks use the owner manifest nearest each planned path. |
| `parallel-gates` | Read/read overlap coexists; write/write and write/read conflict atomically. |
| `intervening-gate-transitions` | Disjoint A/B gates may analyze together; A reconciles B only after B publishes an exact terminal identity chain, stays incomplete while B's changed path is active, becomes stale when B touches A's sealed opening reads, and denies unexplained third-party changes. |
| `gate-path-identity` | New paths, aliases, directory descendants, symlinks/junctions, case policy, and rename endpoints follow ARCH-002. |
| `repo-path-lossless` | Non-UTF-8/native repository roots, Linux byte-distinct names, and Windows Unicode/non-scalar names retain distinct root/`repo-path.v1` IDs, deterministic ordering/cursors, native NUL-input round trips, and exact DTO base64 while display text remains nonauthoritative. |
| `reserved-state-namespace` | `.lumin` symlink/junction/reparse/mount parents, nested redirected state parents, foreign/preexisting contents, `RepositoryId` mismatch, caller-declared state writes, physical aliases, and external state mutation each produce the exact malformed/integrity outcome without source evidence or gate success. |
| `state-namespace-initialization` | Faults before/after `.lumin`, lock, marker temp/rename/flush, and managed-parent/store creation recover only the ARCH-002 absent, resumable matching-marker, or foreign-state outcome. |
| `gate-config-drift` | An external or unexplained change to a protected semantic input outside this gate's leased-plus-actual write set makes the gate stale; an actual cross-gate write is denied. |
| `gate-self-semantic-write` | A planned manifest, lockfile, tsconfig, or root Lumin config path present in both this gate's leased and actual write sets is recaptured and reanalyzed into the close `AnalysisInputId` and delta; config-derived effective values are recomputed under the unchanged caller override tier, while an unplanned or external config change remains stale. |
| `gate-prewrite-observation` | Provisional admission, editor quiescence, exact baseline capture, and final store promotion bind `Allow` to one returned `GateBaselineObservationId`; interrupted admission leaves no active gate lease. |
| `gate-semantic-read-closure` | A discovers an import/config demand for a path leased by active B; cold owner execution returns `NeedsInputs` without reading it, reservation conflict keeps A incomplete, and only after B is terminal may inventory capture the exact bytes and resume the owned continuation without reparsing the primary payload. |
| `gate-semantic-read-closure-warm-cache` | Each cached demand step is keyed only by already supplied exact prerequisites and cannot validate or consume its demand before reservation; when an intermediate config changes its nested demand, warm execution does not over-reserve the stale old leaf and still matches the cold outcome, reads, effects, binding, and semantic dump. |
| `cache-gate-context-projection` | The same validated cached limitation intersects one gate intent but not another; the owning capability recomputes request-specific signals and no repository-input-only cache replays the first gate's effect. |
| `capability-availability-authority` | Shape, clone, discipline, and Rust lanes have no compiled owner or fallback; the engine capability registry alone emits their availability fact/`RequiredOwnerUnavailable`, while Svelte/Astro dialect status remains owned by the existing `lumin-sfc` boundary and evidence policy alone maps effects. |
| `gate-unsealed-observation` | Pre-write and post-write closure conflicts/unbounded inputs persist queryable typed `Unsealed` results without a fabricated observation ID; conflict-free sealed `Deny` remains distinguishable. |
| `gate-analysis-input-reconciliation` | Close preserves caller-supplied opening scan/entry/profile overrides, records a current `AnalysisInputId` only for a sealed revision, recomputes config-derived effective values for self-writes, and accepts other source/config differences only through this gate's leased-plus-actual writes or exact reconciled terminal transitions. |
| `gate-final-observation` | External or unexplained source/config drift after capture or sealing cannot produce `Allow` or release the active lease; a planned self-writable input is accepted only after current recapture and owner reanalysis. |
| `gate-lifecycle-effects` | Introduced, unchanged, regressed, improved, mixed/incomparable, resolved, and baseline-unavailable unresolved/opacity facts produce exactly the dimension-owned signals and effects; cases include `{a,b}->{b,c}`, Workspace-to-Package and Package-to-Workspace scope, confidence/grounding rank changes, owner-manifest payload replacement, and changed evidence identity. |
| `gate-immutable-opening-delta` | Repeated nonauthorizing close attempts always compare with the immutable opening semantic baseline; a prior failed close cannot turn an introduced blocker into unchanged advisory evidence, and a sealed stale snapshot never replaces current protected reads. |
| `lifecycle-operation-idempotency` | Pre/post, abandon, pin/unpin, prune-plan creation, and prune confirmation use one operation contract: same ID/request joins, safely retries, or returns one committed result; conflicting reuse is malformed; injected post-commit delivery failure is recovered through `operation show`. |
| `gate-reopen-after-process-exit` | Open and close a gate, terminate the process, then use a new process to show the exact gate revision and page its findings/evidence. |
| `unplanned-edit` | Unplanned changed, new, removed, and renamed paths cannot receive an allow decision. |
| `mixed-vue-gate` | JS and Vue changes share one user gate and keep owner-specific facts. |
| `required-capability-failure` | Overview warns that dead analysis is unavailable and never renders zero. |
| `snapshot-and-latest` | Mid-scan drift blocks completion; failed or interrupted attempts remain visible beside the last completed run. |
| `bounded-nested-query` | Run and gate-revision pages expose immutable scope, totals, truncation, and stable top-level and nested continuation. |
| `collection-ordering` | Findings, evidence, relations, files, runs, active gates, and plan items traverse exactly once under their versioned ordering despite randomized insertion/backend traversal. |
| `capabilities-pagination` | Current-binary and exact-run capability collections each exceed the test page size and traverse exactly once through `--cursor` under `capabilities.v1` without crossing scope. |
| `request-path-escape` | Caller-declared root escape exits `2` without operation record, gate ID, or lease; later admitted alias drift and final containment violation follow their distinct stale/block contracts. |
| `corrupt-store` | Corrupt canonical storage hard-stops without fallback or empty evidence. |
| `crash-publication` | Attempt allocation, running-envelope, latest-pointer, run-rename, terminal-attempt, and pointer-replacement crash points each have the single ARCH-002 outcome; a renamed orphan without terminal success remains interrupted and is never adopted as success. |
| `concurrent-latest-publication` | Sequence 10/11 publishers both read the older pointer and complete in forced reverse order, while one sequence publishes `Running` then terminal; exclusive publication preserves the highest `latestAttempt` sequence/phase and independent highest `latestCompleted` without lost update or stranded `Running`. |
| `publication-retention-race` | Publication and prune confirmation race for the same target under the exclusive catalog guard: publication-first makes confirmation stale, while retention-first prevents pointer publication with a typed result and never creates a dangling target. |
| `retention-latest-protection` | Public prune-plan/show/confirm commands exclude both latest-pointer targets and linked closure, and stale confirmation cannot enter `Pruning` or create a dangling pointer. |
| `retention-plan-pagination` | A prepared plan allocates one repository-scoped ID/content identity; unrelated repository mutation does not break its cursor, while cross-plan cursor reuse is malformed. |
| `retention-public-lookup` | At every fault point, direct run/gate lookup, plan show, and operation show agree on `Live`, `Pruning`, or `Pruned`; tombstones never appear as empty findings or plain not-found. |
| `retention-independent-pins` | Two consumers receive distinct `PinId` values for one run; removing either leaves the other protection intact and prune eligibility changes only after the last reference is removed. |
| `retention-active-transition-reference` | A opens, disjoint B closes, and a prune plan for B reports A's transition reference and excludes B/capsule; A later reconciles and closes, after which a new plan may include the released closure. |
| `retention-crash-protocol` | Faults before/after tombstone, during each canonical-to-trash move, before `Pruned`, and during physical reclamation recover to the single ARCH-002 state without treating a missing payload as successful deletion. |
| `lifecycle-store-migration` | One process holds an old generation token while another migrates; transaction-scoped handles close before replacement, every migration crash point selects the ARCH-002 recovery rule, all logical records/references survive, and the old process must reopen/revalidate before its late mutation can commit. |

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
7. No analyzed source/config payload is read or parsed more than once for extraction in a cold run, including demand-closure continuation and cached-demand miss paths; the separate final hash-only freshness pass is measured and does not reparse.
8. No runtime path executes Node or Cargo.
9. Windows/Linux packages and Codex/Claude Code adapters pass one public binary behavior contract without embedded semantic fallbacks.
10. A user can perform pre-write and post-write using path-scoped typed flags, stable machine output, caller-retained operation IDs, and one explicit gate ID; the completed gate reopens in a new process.
11. Gates with nonconflicting read/write sets may analyze concurrently; close reconciles immutable terminal transitions in order, while write/write and write/read conflicts fail before edits are authorized.
12. Query output is bounded and exhaustive results are reachable through cursors.
13. The default run emits no legacy artifact warehouse.
14. Required failures, snapshot freshness, and unsupported capabilities are prominent and queryable.
15. Strict workspace formatting, lint, unit, integration, corpus, dependency-edge, and package checks pass.
16. The public binary meets the approved Phase 1 performance and memory targets on every blocking environment and reports the `/mnt/<drive>` diagnostic separately.
17. Operation-ID retry and post-commit delivery recovery never duplicate any gate or retention lifecycle mutation.
18. Publication and public retention commands preserve one crash outcome per point, one deletion truth, and intact latest/pin/transition-reference closure.
19. Pre-write and post-write reserve every owner-demanded semantic input before inventory capture or owner/cache consumption and seal only finished exact consulted inputs before deriving an authorizing observation ID.
20. Explicit entry and resolution-profile precedence, including embedded Vue scripts, is deterministic and fully represented in `AnalysisInputId`.
21. Every first-slice incomplete/unsupported/opaque reason has an exhaustive owner, limitation scope, absence effect, and gate relevance without directly assigning lifecycle effect.
22. Warm cache replay validates and returns the complete owner outcome/capability state, diagnostics, payload, limitations, gate-neutral signals, and consulted inputs, preserving cold-run request-specific effects, observation binding, and semantic dump.
23. Every authorizing result has a sealed observation ID, while a nonauthorizing closure failure returns typed unsealed evidence without a fabricated ID.
24. Every public collection uses one versioned canonical ordering and continuation surface, including current-binary/run capabilities; immutable retention-plan pages remain resumable across unrelated repository mutations.
25. Public lookup distinguishes live, pruning, pruned, never-existing, and corrupt records and agrees with plan/operation state at every retention crash point.
26. Audit and pre-write expose the complete scan invocation tier; post-write reuses the caller override tier without replacement, recomputes validated config-derived values, and gives every caller/config containment case one result.
27. Static limitation meaning and lifecycle delta policy are separate; the total introduced/unchanged/regressed/improved/mixed/resolved/unavailable relation classifies every target, domain, confidence, grounding, and evidence change before signals are mapped.
28. Independent run pins cannot remove one another's protection, and lifecycle migration/tombstone rules preserve the complete durable catalog.
29. The engine capability registry alone emits compiled-profile unavailable facts/signals and never substitutes analysis for an absent capability owner.
30. Retention cannot remove a terminal transition capsule while an active gate references it, and releasing that reference cannot alter the active gate's later reconciliation result.
31. Lifecycle-store migration uses transaction-scoped handles and generation fencing; every crash point has one recovery rule and an old-generation late writer cannot commit without reopening and revalidation.
32. Every post-write delta compares with the immutable opening semantic baseline, and a sealed stale or prior failed close cannot silently replace that baseline or current protected reads.
33. Repository-input cache entries contain only gate-neutral signals; the owning capability recomputes request-specific signals for each current `GateProjectionContext`.
34. Concurrent publishers, recovery, retention confirmation, and migration serialize latest-pointer comparison/replacement through one exclusive catalog guard; sequence/phase and completed-sequence maxima never regress, strand `Running`, or lose an update.
35. Every Unix/Windows native path round-trips one lossless `repo-path.v1` identity through IDs, ordering, cache, cursors, native NUL input, and machine DTOs without display or Unicode normalization collisions.
36. `.lumin` is a no-follow, repository-bound reserved namespace; aliases, redirected parents, foreign contents/identity, caller writes, and external mutation cannot enter source evidence or a successful gate revision.
37. `resolver-config-semantics.v1` classifies every first-slice resolver field/shape, and unsupported affecting or unknown compatibility inputs emit scoped incomplete evidence before target probing.
38. Architecture v1 contains no scan lock; architecture-check proves that scheduler coordination is not used as repository safety authority.

## 15. Acceptance Traceability

| AC | Behavior test | Corpus/fixture | Command | Expected proof |
| --- | --- | --- | --- | --- |
| 1 | `foundation_corpus_contract` | all Section 9 rows | `lumin-xtask corpus foundation` | Every expected query value matches authored truth. |
| 2 | `framework_failures_are_scoped` | SFC dialect, Vue, and route-group rows | `lumin-xtask corpus foundation` | `overview` reports per-dialect Vue completion or scoped unavailable dialect evidence, never aggregate SFC completeness, process abort, or framework policy outside `lumin-sfc`. |
| 3 | `public_surface_is_identity_scoped` | 20-module re-export matrix | `lumin-xtask corpus foundation` | 60 candidates and 20 protected identities. |
| 4 | `reachable_module_keeps_dead_exports` | `reachable-dead-sibling` | `lumin-xtask corpus foundation` | The unused sibling remains a candidate. |
| 5 | `semantic_dump_is_worker_invariant` | full foundation corpus | `lumin-xtask corpus foundation --determinism` | Canonical semantic dump and finding IDs match. |
| 6 | `scheduler_completion_order_is_irrelevant` | randomized stage-result fixture | `cargo test -p lumin-engine` | Repeated randomized completion yields one semantic dump. |
| 7 | `source_payload_is_extracted_once` | read-counter, semantic-demand continuation/cache-miss, plus Vue external script | `lumin-xtask corpus foundation` | Every source/config payload is consumed once; owned continuation and cached-demand miss do not trigger a second parse, while final hash validation remains distinct. |
| 8 | `runtime_has_no_source_fallback` | package runtime probe | `lumin-xtask package-check <target>` | Execution succeeds with Node and Cargo unavailable. |
| 9 | `packages_and_skills_share_behavior_contract` | package fixture set plus packaged Codex/Claude adapters | both target package checks plus `lumin-xtask package-check skills` | Windows/Linux query values match; both adapters invoke the same public commands/DTOs with no embedded semantic table or source fallback. |
| 10 | `gate_round_trip_requires_ids_and_reopens` | `mixed-vue-gate`, `gate-reopen-after-process-exit` | `lumin-xtask corpus foundation` | Operation/gate IDs complete the round trip, then a new process queries the exact completed revision and paged evidence. |
| 11 | `gate_conflicts_and_transitions_are_serializable` | parallel/config/self-semantic-write/path identity/intervening-transition rows | `lumin-xtask corpus foundation` | Read/read admits; direct conflicts reject; this gate's leased-plus-actual config writes are recaptured; disjoint terminal chains reconcile; active or unexplained changes cannot authorize. |
| 12 | `all_pages_are_reachable` | `bounded-nested-query` | `lumin-xtask corpus foundation` | Run and gate-revision cursor traversal returns exactly `total` top-level and nested items without following a newer scope. |
| 13 | `default_publication_is_bounded` | output-layout fixture | `lumin-xtask corpus foundation` | Only the repository state marker, lifecycle lock/store, attempt/run envelopes, canonical evidence store, and latest pointer are published; the migration intent exists only during migration. |
| 14 | `failure_and_freshness_are_visible` | required-failure, parse, snapshot, request-path-escape, and corrupt-store rows | `lumin-xtask corpus foundation` | `overview` or the gate response exposes incomplete/stale/failed/malformed states and never zero. |
| 15 | `repository_policy_suite` | workspace and source/lock policy | fmt, Clippy, workspace test, architecture-check | Every required quality command exits successfully, including path-owner, resolver-registry, and no-scan-lock checks. |
| 16 | `release_performance_matrix` | named benchmark corpora | `lumin-xtask benchmark foundation` | Blocking time/memory targets are met and the `/mnt/<drive>` diagnostic is reported. |
| 17 | `lifecycle_mutations_are_idempotent` | `lifecycle-operation-idempotency` | `lumin-xtask corpus foundation` | Every mutation retry returns one committed result and `operation show` recovers injected delivery failure. |
| 18 | `publication_and_retention_have_one_crash_truth` | publication, concurrent-latest, publication-retention, and retention crash rows | `lumin-xtask corpus foundation --store-crash` | Public commands drive every fault/race point; pointer maxima, tombstone/trash recovery, and latest/pin/transition-reference closure survive. |
| 19 | `semantic_reads_are_reserved_before_consumption` | `gate-semantic-read-closure`, `gate-self-semantic-write` | `lumin-xtask corpus foundation` | A new demand is conflict-checked/reserved before capture or consumption, self-writable inputs are recaptured, and only finished exact reads can seal. |
| 20 | `entry_and_profile_selection_are_canonical` | entry/profile and Vue override rows | `lumin-xtask corpus foundation` | Effective entries and every importer profile match precedence and persisted input identity. |
| 21 | `limitation_registry_is_exhaustive` | `limitation-scope-exhaustiveness` plus failure rows | `lumin-xtask architecture-check` and corpus foundation | Every private reason maps scope/absence/relevance exactly once and cannot directly choose lifecycle effect. |
| 22 | `warm_cache_replays_owner_semantics` | `gate-semantic-read-closure-warm-cache` | `lumin-xtask corpus foundation` and determinism | Cold/warm outcome state, diagnostics, payload, limitations, reads, signals/effects, binding, and semantic dump match; an active config writer still blocks warm authorization. |
| 23 | `observation_binding_is_honest` | `gate-unsealed-observation` | `lumin-xtask corpus foundation` | Authorizing results are sealed; closure failures persist typed unsealed domains without a partial ID and retry returns the same binding. |
| 24 | `collection_orders_and_plan_scope_are_stable` | `bounded-nested-query`, `collection-ordering`, `capabilities-pagination`, `retention-plan-pagination` | `lumin-xtask corpus foundation --determinism` | Every collection, including binary/run capabilities, traverses exactly once under its ordering ID; unrelated mutations do not invalidate immutable plan pages. |
| 25 | `retention_state_is_public` | `retention-public-lookup` plus retention crash rows | `lumin-xtask corpus foundation --store-crash` | Direct target, plan, and operation queries expose the same typed live/pruning/pruned truth and never empty deletion evidence. |
| 26 | `scan_tier_and_containment_are_canonical` | `scan-invocation-containment`, `gate-self-semantic-write` | `lumin-xtask corpus foundation` | Audit/pre-write flags persist in the digest/input ID; post-write replacements fail; validated self-written config recomputes effective values; every root-containment class matches Section 3.1. |
| 27 | `gate_delta_policy_is_total` | `gate-lifecycle-effects` | `lumin-xtask corpus foundation` and architecture-check | Every key/payload relation, including mixed sets, narrowed/expanded domains, ranks, and incomparable evidence, has one exhaustive classification and signal path. |
| 28 | `references_and_lifecycle_migration_preserve_truth` | `retention-independent-pins`, `retention-active-transition-reference`, and lifecycle migration fixtures | `lumin-xtask corpus foundation --store-crash` | Independent pins/references survive one another; migration preserves attempts, operations, transitions, plans, tombstones, pins, and gates. |
| 29 | `capability_unavailability_has_one_owner` | `capability-availability-authority` | `lumin-xtask architecture-check` and corpus foundation | The engine registry alone emits availability facts/signals, evidence policy maps them, and no fallback capability runs. |
| 30 | `active_gates_protect_transition_proof` | `retention-active-transition-reference` | `lumin-xtask corpus foundation --store-crash` | B's terminal capsule remains excluded while A references it; A later reconciles exactly and reference release enables a new plan. |
| 31 | `lifecycle_migration_fences_generations` | `lifecycle-store-migration` | `lumin-xtask corpus foundation --store-crash` | Multi-process fault injection preserves one generation and rejects/reopens every old-generation late mutation. |
| 32 | `failed_close_keeps_opening_baseline` | `gate-immutable-opening-delta` | `lumin-xtask corpus foundation` | Retry still classifies against opening facts, and stale historical evidence never becomes current read protection. |
| 33 | `cache_projection_is_gate_contextual` | `cache-gate-context-projection` | `lumin-xtask corpus foundation --determinism` | One cached outcome yields owner-recomputed signals for each intent; no prior gate effect is replayed. |
| 34 | `latest_publication_is_serializable` | `concurrent-latest-publication`, `publication-retention-race` | `lumin-xtask corpus foundation --store-crash` | Forced reverse completion, same-sequence terminal promotion, and publication/retention races preserve monotonic fields, one winner, and no dangling pointer. |
| 35 | `repository_paths_are_lossless` | `repo-path-lossless` | `lumin-xtask corpus foundation --determinism` plus Windows/Linux package checks | Raw Unix and native Windows names retain distinct stable identities and round-trip canonical DTO/NUL input across every public query. |
| 36 | `state_namespace_is_reserved` | `reserved-state-namespace`, `state-namespace-initialization` | `lumin-xtask corpus foundation --store-crash` | No-follow admission and initialization recover only named states; redirected/foreign/mismatched paths hard-stop, caller aliases are malformed, and external mutation cannot authorize. |
| 37 | `resolver_configuration_fails_closed` | resolver-registry, module-suffix/custom-condition/root-dirs/typesVersions/export-shape rows | `lumin-xtask corpus foundation` and architecture-check | Every field/shape has one class; unsupported affecting inputs emit the expected limitation before probing and cannot create false edges/absence. |
| 38 | `scan_lock_is_not_a_contract` | workspace source/lock policy | `lumin-xtask architecture-check` | No product `ScanLock` or long-lived scan safety primitive exists; repository safety uses the named snapshot/reservation/store contracts. |

## 16. Product AC Coverage

| Product AC | Slice status | Slice proof |
| --- | --- | --- |
| 1 one native process | in scope | Slice AC 8 and architecture-check runtime-launch policy. |
| 2 prebuilt Windows/Linux | in scope | Slice AC 8-9 and package probes. |
| 3 worker determinism | in scope | Slice AC 5-6. |
| 4 required failure visible | in scope | Slice AC 14/29 and failure/availability corpus. |
| 5 no intent JSON workflow | in scope | Slice AC 10 and gate round trip. |
| 6 completed gate queryable | in scope | Slice AC 10 plus restart/reopen corpus. |
| 7 resumable truncation | in scope | Slice AC 12/24 and nested, capabilities, collection-ordering, and retention-plan cursor corpus. |
| 8 framework miss isolation | in scope | Slice AC 2. |
| 9 identity-scoped public export | in scope | Slice AC 3-4. |
| 10 projections are noncanonical | in scope | Slice AC 13 and projection checks. |
| 11 one skill/binary contract | in scope | Slice AC 9 and explicit `package-check skills` proof. |
| 12 corpus/platform/performance evidence | completion-gated | Slice AC 1, 9, and 16; remains unclaimed until all pass. |
| 13 latest failure visible | in scope | Slice AC 14 and `snapshot-and-latest`. |
| 14 explicit post-write gate ID | in scope | Slice AC 10. |
| 15 semantic baseline conflict | in scope | Slice AC 11/23/27/30/32 and baseline/final-observation, total lifecycle-delta, plus transition-reference corpus. |
| 16 idempotent lifecycle mutation | in scope | Slice AC 17/28 and operation-delivery plus independent-pin corpus. |
| 17 semantic-read fixed point | in scope | Slice AC 19/22/23/33 and cold/warm demand-reservation, gate-context, plus honest-binding corpus. |
| 18 crash-consistent retention | in scope | Slice AC 18/24/25/28/30/31 and public retention crash/query/reference/generation corpus. |
| 19 monotonic latest publication | in scope | Slice AC 18/34 and concurrent-publication/retention race corpus. |
| 20 lossless path identity | in scope | Slice AC 35 and Unix/Windows path round-trip corpus/package probes. |
| 21 reserved state namespace | in scope | Slice AC 36 and no-follow state namespace fault corpus. |
| 22 resolver configuration honesty | in scope | Slice AC 21/37 and the closed resolver-registry unsupported-field corpus. |

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
