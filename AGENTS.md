# Lumin v2 Repository Rules

[`문서(한글)/AGENTS.ko.md`](문서(한글)/AGENTS.ko.md) is the canonical repository-agent contract. This file is its English translation; if they differ, follow the Korean source.

## Product Identity

Lumin v2 is a native Rust repository-analysis engine and a durable write gate that connects pre-write and post-write through one gate ID.

## Reading Order

Start at [WORKBOARD.md](WORKBOARD.md) and read only the owner documents routed for the current work.

## Rules

1. **SSOT and uncertainty:** Follow product contract -> system blueprint -> focused architecture owner -> active slice -> this file. If a contract or owner is unclear, stop before editing and find the owner through the Workboard. Use the external legacy Lumin repository and corpus only as supporting evidence, never as the contract owner.
2. **Fail closed:** Never turn missing, stale, unsupported, opaque, failed, or truncated evidence into clean or zero findings. Silent fallbacks are forbidden.
3. **Freeze before implementation:** Freeze architecture changes through one design review and one independent adversarial review. Do not implement before that point, and do not create MVP architecture, horizontal scaffolding, empty future crates, placeholder owners, or a second engine.
4. **Physical ownership:** Enforce boundaries with Cargo dependencies, visibility, project-owned types, and architecture checks. Keep third-party types inside owner crates and pass domain facts through typed in-process models, never stage-to-stage JSON.
5. **Deterministic execution:** Follow [ARCH-001](architecture/001-execution-and-ownership.md). Workers consume immutable inputs and return owned outputs; one owner-defined deterministic merge step combines results. Do not add a second scheduler or pool. `jobs=1` and `jobs=N` must produce identical semantic evidence.
6. **Durable pre/post:** Pre-write returns a gate ID and post-write requires that exact ID. Do not create intent JSON or transport files. Block write/write and write/semantic-read conflicts. Release storage-transaction, scan, and operation-liveness locks before result transport, while an `Active` gate's durable logical path lease remains until close or abandon.
7. **Dependency isolation:** Keep costly dependencies inside the owner crate named by [ARCH-000](architecture/000-system-blueprint.md). A new dependency requires measured product value and reviewed build, size, unsafe, and transitive costs. Runtime Cargo, Node analysis dependencies, and source fallbacks are forbidden by [PRODUCT-000](specs/000-product-contract.md) and ARCH-000.
8. **Behavioral verification:** Derive expected results from specs and corpus truth authored independently of the implementation, never current implementation or legacy output. Verify a core path, realistic edges, and required hard stops. Never weaken assertions, add arbitrary caps or timeouts, swallow failures, or skip checks to pass CI. Scaffolding tests that create RED by checking only file or function existence are also forbidden.
9. **Validation scope:** Run focused local checks before push; use the full local matrix only for shared-core changes or CI diagnosis. Public CI is merge authority for clean locked builds, full corpus and determinism, packages, and dependency policy. Reproduce only the failing CI neighborhood locally.
10. **Close the current change only:** Update the owner spec and Workboard only when their owned facts changed. Remove generated output and report exact checks and limitations. Do not mix unrelated cleanup, copy legacy modules wholesale, or alter user work outside the active slice.

## Validation Matrix

- **Documentation only:** Check links, formatting/whitespace, and architecture consistency locally; public CI repeats document checks from a clean checkout.
- **One crate or corpus case:** Run `cargo fmt`, scoped Clippy/tests, and affected corpus and boundary checks locally; public CI checks the full workspace, corpus, determinism, and dependency policy.
- **Shared core or packaging:** Run the full affected workspace and package smoke locally, using the full local matrix only for diagnosis; public CI runs locked Windows/Linux builds and behavioral package probes.
