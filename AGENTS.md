# Lumin v2 Repository Rules

Read `WORKBOARD.md` first. Follow its routing table and load only the owner documents for the current change.

## Ten Rules

1. **Respect the source-of-truth order.** Product contract, system blueprint, focused architecture owner, active slice, then this file. Code and legacy behavior cannot silently override an accepted contract.

2. **Freeze the destination before implementation.** Architecture changes require one design review and one independent adversarial review. Implement only production-grade vertical slices; no MVP architecture, horizontal scaffold, empty future crate, placeholder owner, or fallback engine.

3. **Make ownership physical.** Every fact and policy has one owner. Cargo edges, visibility, project-owned types, and the architecture checker enforce boundaries. Parser, persistence, protocol, and framework-library types stay inside their owner crate; domain facts never cross stages as JSON.

4. **Use one deterministic execution model.** `lumin-engine` owns one local Rayon pool and the Kahn stage scheduler. Workers consume immutable inputs and return owned outputs; reducers alone commit shared state. No global or nested pool, shared mutable graph, or corpus-wide retained source bytes. `jobs=1` and `jobs=N` must produce identical semantic evidence.

5. **Keep evidence canonical and queryable.** Canonical findings live in the store; summaries, SARIF, and compatibility files are projections. Missing, stale, unsupported, opaque, failed, and truncated evidence remain explicit states. Bounded queries report total, returned, truncation, and continuation.

6. **Treat pre/post as one durable transaction.** Pre-write returns a gate ID; post-write requires that ID. Agents do not create intent JSON or clean transport files. Write/write and write/semantic-read conflicts fail closed, and no scan or store lock survives result transport.

7. **Isolate costly dependencies.** OXC belongs only to `lumin-js`; the selected persistence engine belongs only to `lumin-store`. A new dependency needs measured product value, boundary ownership, and reviewed build, binary-size, unsafe, and transitive costs. Runtime Cargo, Node analysis dependencies, and source fallbacks are forbidden.

8. **Prove behavior from authored truth.** Expected results come from specs and minimized real corpus cases, never current implementation or legacy output. Cover one core path, realistic edges, and fail-closed behavior. Never weaken assertions, add arbitrary caps or timeouts, swallow failures, or skip checks to make a change pass.

9. **Use the validation ladder.** Before push, run formatting plus the focused lint, tests, architecture rules, and corpus cases affected by the change. Run the full local suite only for shared-core changes or CI diagnosis. Public CI is the merge authority for clean-checkout locked builds, the full workspace and corpus, determinism, Windows/Linux packages, and dependency checks. Do not run the same full matrix locally and remotely by default.

10. **Close the owning change, not the whole repository.** Recheck acceptance criteria, update the owner spec and Workboard, remove generated transport/build output, and report exact checks and limitations. Do not mix unrelated cleanup, copy legacy modules wholesale, or alter user work outside the active slice.

## Validation Ladder

| Change | Required locally before push | Public CI |
| --- | --- | --- |
| Documentation only | Markdown links, formatting/whitespace, architecture consistency | Repeats cheap document checks from a clean checkout |
| One crate or corpus case | `cargo fmt`, scoped Clippy/tests, affected corpus and edge checks | Full workspace, full corpus, determinism, dependency policy |
| Shared model, engine, store, protocol, or packaging | Full affected workspace and package smoke; full local matrix only when needed to diagnose | Clean locked build, Windows/Linux matrix, behavioral package probes |

When public CI fails, reproduce its failing command locally, fix it, and rerun the focused neighborhood. Passing local checks is fast feedback; passing public CI is release evidence.

## Routed Owners

- Scheduler, Rayon, memory, cache, or determinism: `architecture/001-execution-and-ownership.md`.
- Store, queries, SARIF, pre-write, post-write, or concurrent agents: `architecture/002-evidence-and-write-gate.md`.
- Phase 1 behavior or corpus: `specs/001-foundation-slice.md`.
- Working method and AI judgment: `SDD.md`.
