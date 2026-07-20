# Lumin v2 Workboard

Status: Phase 1 foundation implementation active

Revision: 2026-07-19

## One-Line Purpose

Lumin is a native repository-audit engine and Codex/Claude Code skill that gives AI agents grounded, queryable evidence before and after they change code.

## Spec Registry

| ID | Status | Document | One-line role |
| --- | --- | --- | --- |
| METHOD-000 | active | `문서(한글)/SDD.ko.md` (canonical; `SDD.md` English translation) | Defines the permanent Spec-Driven Development workflow for every change. |
| PRODUCT-000 | frozen | `specs/000-product-contract.md` | Defines what Lumin v2 must guarantee and what it is not. |
| ARCH-000 | frozen | `architecture/000-system-blueprint.md` | Owns the final system shape, crate boundaries, and dependency direction. |
| ARCH-001 | frozen | `architecture/001-execution-and-ownership.md` | Owns the Kahn/Rayon execution model, determinism, and memory ownership. |
| ARCH-002 | frozen | `architecture/002-evidence-and-write-gate.md` | Owns the evidence store, bounded query protocol, and pre/post transaction. |
| SLICE-001 | active | `specs/001-foundation-slice.md` | Defines the JS/TS/SFC foundation, with Vue as the first production dialect. |
| REVIEW-001 | superseded | `reviews/architecture-v1-adversarial-2026-07-15.md` | Records the first adversarial findings and first amendment decisions. |
| REVIEW-002 | complete, monitoring | `reviews/architecture-v1-independent-verification-2026-07-15.md` | Preserves the exact independent verification and may reopen only on a concrete regression or counterexample. |

## Active Work

Implement the first production Rust vertical path from native repository admission through JS/TS inventory, OXC lowering, deterministic graph construction, canonical zero-fan-in findings, and machine output. The numeric targets under `reviews/probes/phase0-numeric-target-selection-2026-07-18/` are approved Phase 1 budgets, not achieved-product claims. Their external audit is non-blocking and may reopen the contract only with a concrete counterexample. Product packages, packaged skill adapters, public behavior, native path/root product round trips, and achieved-budget proofs remain Phase 1 acceptance.

Current implementation checkpoint:

- A real twelve-crate Rust path now owns repository inventory, OXC fact lowering, Vue SFC decomposition/finalization, package.json and restricted pnpm workspace facts, JSONC config admission, config-owned resolution profiles, relative plus `baseUrl`/`paths` resolution, profile-aware workspace package surfaces, deterministic graph reduction, canonical dead-export evidence, JSON protocol output, and persisted audit/overview/findings commands.
- `jobs=1` and multi-worker analysis produce identical semantic evidence in the checked behavior tests.
- Generated and vendored findings remain canonical `ReviewOnly` rows. Parse, duplicate package identity, unsupported config, and unresolved-input uncertainty remains visible as typed incomplete evidence rather than a muted clean result. Package-local public-surface uncertainty does not erase unrelated `ReviewOnly` findings.
- Resolver semantic inputs follow demand -> inventory capture -> resume. Relative `extends` uses exact then one `.json` candidate, invocation profiles replace only config profile selection, and selected profiles are persisted in run overview evidence.
- Restricted pnpm membership replaces same-directory `package.json#workspaces`, applies positive patterns before exclusions, and hard-stops malformed or unsupported YAML without silently falling back. Package resolution owns active-profile `exports`, public entry fallback, type/value lanes, and public-surface declarations; unsupported `imports`, targets, package fields, and importer formats remain typed and package-local.
- `lumin-sfc` now owns the dialect-neutral SFC entry point and the first Vue path. Inline scripts retain `EmbeddedSourceUnitId` and parent spans through engine-routed OXC extraction; exact external scripts attach existing logical facts; template component imports and relative style resources finalize under the parent Vue module. Malformed Vue, opaque template bindings, missing or mode-conflicting external scripts, and unavailable Svelte/Astro dialects remain typed evidence.
- The durable write-gate vertical now admits existing files, bounded directories, and new source paths from their nearest existing parent. Logical leases retain canonical paths plus observed real-directory prefix identities; directory scopes conflict with descendants; physical aliases expand into caller-visible logical leases and are reanalyzed separately; late unleased alias topology is denied.
- Successful closes append an immutable monotonic worktree transition and references from every earlier active gate in the same store transaction. A disjoint gate stays `Incomplete` while another writer is active, then reconciles that writer's exact terminal before/after chain on retry. Broken chains deny, protected-read transitions are stale, and only authorizing closes release leases and transition references.
- Existing grounded dead-export findings and bounded unresolved-edge facts now lower into model-owned `DeltaKey` facts and one deterministic total lifecycle relation over identity, target/domain sets, confidence, grounding, evidence identity, and owner payload. Introduced adverse facts deny, unchanged adverse facts warn, improvements/resolutions authorize without an adverse signal, and every classification is persisted on the gate revision and public operation/gate response. Unsupported or unbounded owner semantics remain typed required-evidence gaps; they are not fabricated into comparable facts until their exact target and affected domain can be represented.
- Pre-write and close-time resolver analysis now retain one owned extraction session across `NeedsInputs` steps. Each new config path and its observed physical identity are transactionally recorded on the pending operation before inventory captures it; active/provisional writers, including physical aliases, make the attempt typed `Incomplete`, and a later writer cannot cross a live reservation. Pre-write promotion rejects a baseline that omits or physically disagrees with any reserved demand. Successful closure persists the exact finished protected read set on the revision and gate. Until an exact actual-write set is implemented, a first-seen close-time input outside the lease remains a fail-closed `UnplannedWrite`; a failed attempt cannot launder it into an authorizing retry.
- The store still does not claim the complete ARCH-002 managed-state parent binding, generation fencing, process-death reservation recovery, abandon, retention, or migration contract. New sources and directory-created descendants are admitted to the write domain and may authorize when their complete current evidence has no adverse lifecycle delta.

Next implementation order: recover interrupted pending gate operations and their provisional write/semantic-read reservations after process death without lease expiry, silent deletion, or duplicate authorization. Preserve the fixed-point, alias-topology, transition-chain, and total-delta checks; keep every unsupported branch typed and scope-limited.

## Routing Rules

- Starting any implementation, refactor, review, or specification change: read METHOD-000.
- Product identity, user-visible guarantees, supported environments: read PRODUCT-000.
- Adding, removing, or changing a crate dependency: read ARCH-000.
- Rayon, task ordering, cancellation, locks, memory, or determinism: read ARCH-001.
- Store, queries, SARIF, pre-write, post-write, or parallel agents: read ARCH-002.
- Any Phase 1 implementation or test: read SLICE-001.
- Architecture freeze or review resolution: read REVIEW-002, then follow any explicit predecessor link it cites.
- Repository working rules and close-out discipline: read `문서(한글)/AGENTS.ko.md` (canonical) or `AGENTS.md` (English translation).

## Phase Ledger

| Phase | State | Exit condition |
| --- | --- | --- |
| Phase 0: architecture | frozen | Independent document/design review, standalone feasibility, clean provenance, the unmuted finding amendment, and owner-approved numeric targets are complete. External target audit is advisory unless it supplies a concrete counterexample. |
| Phase 1: foundation slice | active | SLICE-001 passes its complete acceptance corpus on Windows and Linux prebuilt binaries. |
| Phase 2: capability growth | not started | New capabilities enter through the frozen DAG without creating a second engine. |
| Phase 3: legacy retirement | not started | Required compatibility exports and corpus parity are complete; Node analysis paths are removed. |
