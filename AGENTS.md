# Lumin v2 Repository Rules

Lumin v2 is a native Rust repository-analysis engine and durable AI write gate. Every coding or review agent reads this file before changing the repository, then starts at [WORKBOARD.md](WORKBOARD.md) and opens only the routed owner documents.

If a contract or owner is unclear, stop before editing, route through the Workboard, and state the uncertainty. Do not invent a fallback. An exception requires an owner-contract amendment and its required review before implementation.

## Reading Key

- `[Auto]`: repository checks or CI must prove it.
- `[Review]`: the acting AI must inspect context and record a grounded judgment.
- `[Both]`: automated evidence plus AI judgment are required.

## Terms

- **Owner document:** the only document allowed to define a durable fact or policy.
- **Vertical slice:** one externally observable behavior implemented through every required boundary and acceptance check, with no placeholder path.
- **Canonical evidence:** the stored source of truth from which every output is derived.
- **Projection:** a noncanonical view such as a summary, SARIF, or compatibility file.
- **Gate:** one durable pre-write/post-write transaction identified by a gate ID.

Current parser, scheduler, and store choices belong to their routed architecture owners. This file states the invariants agents must preserve.

## Ten Rules

1. **Respect the source-of-truth order.** `[Review]`
   - Use product contract, system blueprint, focused architecture owner, active slice, then this file.
   - If they conflict, stop and amend the higher owner before code.
   - **Why:** lower-level behavior must not silently redefine the product.

2. **Freeze the destination before implementation.** `[Both]`
   - Architecture changes require one design review and one independent adversarial review.
   - Implement only production-grade vertical slices inside the reviewed destination boundaries.
   - Do not create MVP architecture, horizontal scaffolding, empty future crates, placeholder owners, or fallback engines.
   - **Why:** temporary boundaries quickly become permanent contracts.

3. **Give every fact one physical owner.** `[Both]`
   - Enforce ownership with Cargo edges, visibility, project-owned types, and architecture checks.
   - Keep parser, persistence, protocol, and framework-library types inside their owner crates.
   - Pass domain facts through typed in-process models, never stage-to-stage JSON.
   - **Why:** duplicate authority drifts even when both copies initially agree.

4. **Use one deterministic execution model.** `[Auto]`
   - Use the execution model frozen in [ARCH-001](architecture/001-execution-and-ownership.md); do not add a second scheduler or pool.
   - Workers consume immutable inputs and return owned outputs; only the owning deterministic merge step commits shared results.
   - `jobs=1` and `jobs=N` must produce identical semantic evidence.
   - **Why:** concurrency may change elapsed time, never product meaning.

5. **Keep evidence canonical and queryable.** `[Both]`
   - Canonical findings live in the store; summaries, SARIF, and compatibility files are projections.
   - Missing, stale, unsupported, opaque, failed, and truncated evidence remain explicit states.
   - Bounded queries report scope, total, returned count, truncation, and continuation.
   - **Why:** absent or partial evidence must never look clean.

6. **Treat pre/post as one durable transaction.** `[Auto]`
   - Pre-write returns a gate ID; post-write requires that exact ID.
   - Agents do not create intent JSON or clean transport files.
   - Write/write and write/semantic-read conflicts fail closed, and locks end before result transport.
   - **Why:** authorization is meaningful only against one inspectable baseline and final observation.

7. **Isolate costly dependencies.** `[Both]`
   - Keep each parser, persistence engine, and other costly dependency inside the owner crate named by [ARCH-000](architecture/000-system-blueprint.md).
   - A new dependency needs measured product value plus reviewed build, binary-size, unsafe, and transitive costs.
   - Runtime Cargo, Node analysis dependencies, and source fallbacks remain forbidden by [PRODUCT-000](specs/000-product-contract.md) and ARCH-000.
   - **Why:** dependency leakage creates hidden runtimes, duplicated owners, and fragile packaging.

8. **Prove behavior from authored truth.** `[Both]`
   - Derive expected results from specs and minimized real corpus cases, never current implementation or legacy output.
   - Cover one core path, realistic edges, and the hard-stop paths that must refuse authorization.
   - Never weaken assertions, add arbitrary caps or timeouts, swallow failures, or skip checks to make a change pass.
   - **Why:** a green test that no longer proves the contract is a product failure.

9. **Use the validation ladder.** `[Both]`
   - Before push, run formatting and the focused lint, tests, architecture rules, and corpus cases affected by the change.
   - Run the full local matrix only for shared-core changes or CI diagnosis.
   - Public CI is merge authority for clean locked builds, full corpus/determinism, packages, and dependency policy.
   - **Why:** local checks optimize feedback; public CI proves the clean release environment.

10. **Close the owning change, not the whole repository.** `[Review]`
    - Recheck acceptance criteria, update the owner spec and Workboard, remove generated output, and report exact checks and limitations.
    - Do not mix unrelated cleanup, copy legacy modules wholesale, or alter user work outside the active slice.
    - **Why:** one coherent change is reviewable; opportunistic cleanup hides causality.

## Validation Ladder

| Change | Required locally before push | Public CI |
| --- | --- | --- |
| Documentation only | Markdown links, formatting/whitespace, architecture consistency | Repeats cheap document checks from a clean checkout |
| One crate or corpus case | `cargo fmt`, scoped Clippy/tests, affected corpus and edge checks | Full workspace, full corpus, determinism, dependency policy |
| Shared model, engine, store, protocol, or packaging | Full affected workspace and package smoke; full local matrix only when needed to diagnose | Clean locked build, Windows/Linux matrix, behavioral package probes |

When public CI fails, reproduce its failing command locally, fix it, and rerun the focused neighborhood. Passing local checks is fast feedback; passing public CI is release evidence.

## Routed Owners

- Product guarantees and supported environments: [PRODUCT-000](specs/000-product-contract.md).
- Crate boundaries and dependency direction: [ARCH-000](architecture/000-system-blueprint.md).
- Scheduling, concurrency, memory, cache, or determinism: [ARCH-001](architecture/001-execution-and-ownership.md).
- Store, queries, SARIF, pre-write, post-write, or concurrent agents: [ARCH-002](architecture/002-evidence-and-write-gate.md).
- Phase 1 behavior or corpus: [SLICE-001](specs/001-foundation-slice.md).
- Working method and AI judgment: [METHOD-000](SDD.md).
