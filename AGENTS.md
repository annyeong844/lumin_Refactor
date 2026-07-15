# Lumin v2 Repository Rules

Lumin v2 is a native Rust repository-analysis engine and durable AI write gate. This English file is the canonical agent contract; [AGENTS.ko.md](AGENTS.ko.md) is its Korean translation.

Start at [WORKBOARD.md](WORKBOARD.md) and read only the routed owner documents. If a contract or owner is unclear, stop before editing and state the uncertainty. Exceptions require an owner-contract amendment and its required review; never invent a fallback.

Tags: `[Auto]` is enforced by repository checks or CI, `[Review]` requires grounded AI judgment, and `[Both]` requires both.

## Terms

- **Owner:** the sole authority for a durable fact or policy.
- **Vertical slice:** one observable behavior through every required boundary and acceptance check, without placeholders.
- **Projection:** a noncanonical view derived from stored evidence.
- **Gate:** one durable pre-write/post-write transaction identified by a gate ID.

## Ten Rules

1. **Follow the source-of-truth order.** `[Review]`
   - Product contract -> system blueprint -> focused architecture owner -> active slice -> this file.
   - On conflict, amend the higher owner before code; lower layers cannot silently redefine the product.

2. **Freeze the destination before implementation.** `[Both]`
   - Architecture changes require one design review and one independent adversarial review.
   - Build only reviewed production vertical slices. MVP architecture, horizontal scaffolding, empty future crates, placeholder owners, and fallback engines are forbidden because temporary boundaries become contracts.

3. **Give every fact one physical owner.** `[Both]`
   - Enforce ownership with Cargo edges, visibility, project-owned types, and architecture checks.
   - Keep third-party types inside owner crates and pass domain facts through typed in-process models, never stage-to-stage JSON.

4. **Use one deterministic execution model.** `[Auto]`
   - Follow [ARCH-001](architecture/001-execution-and-ownership.md); workers consume immutable inputs, return owned outputs, and merge through one owning deterministic step.
   - Do not add a second scheduler or pool. `jobs=1` and `jobs=N` must produce identical semantic evidence.

5. **Keep evidence canonical and queryable.** `[Both]`
   - The store owns canonical evidence; summaries, SARIF, and compatibility files are projections.
   - Missing, stale, unsupported, opaque, failed, and truncated states stay explicit. Bounded queries report scope, total, returned count, truncation, and continuation.

6. **Treat pre/post as one durable transaction.** `[Auto]`
   - Pre-write returns a gate ID and post-write requires that exact ID; agents create no intent JSON or transport files.
   - Write/write and write/semantic-read conflicts fail closed, and locks end before result transport.

7. **Isolate costly dependencies.** `[Both]`
   - Keep each costly dependency inside the owner crate named by [ARCH-000](architecture/000-system-blueprint.md).
   - Add one only with measured product value and reviewed build, size, unsafe, and transitive costs. Runtime Cargo, Node analysis dependencies, and source fallbacks remain forbidden by [PRODUCT-000](specs/000-product-contract.md) and ARCH-000.

8. **Prove behavior from authored truth.** `[Both]`
   - Expected results come from specs and authored corpus truth, never current implementation or legacy output.
   - Cover a core path, realistic edges, and required hard stops. Never weaken assertions, add arbitrary caps or timeouts, swallow failures, or skip checks to pass CI.

9. **Use the validation matrix.** `[Both]`
   - Run focused local checks before push; use the full local matrix only for shared-core changes or CI diagnosis.
   - Public CI is merge authority for clean locked builds, full corpus and determinism, packages, and dependency policy. Reproduce only a failing CI neighborhood locally.

10. **Close the owning change only.** `[Review]`
    - Recheck acceptance, update the owner spec and Workboard when their facts changed, remove generated output, and report exact checks and limitations.
    - Do not mix unrelated cleanup, copy legacy modules wholesale, or alter user work outside the active slice.

## Validation Matrix

| Change | Required locally before push | Public CI |
| --- | --- | --- |
| Documentation only | Links, formatting/whitespace, architecture consistency | Clean-checkout document checks |
| One crate or corpus case | `cargo fmt`, scoped Clippy/tests, affected corpus and edge checks | Full workspace, corpus, determinism, dependency policy |
| Shared core or packaging | Full affected workspace and package smoke; full local matrix only for diagnosis | Locked Windows/Linux builds and behavioral package probes |
