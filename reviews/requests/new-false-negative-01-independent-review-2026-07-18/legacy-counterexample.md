# Legacy Mute False-Negative Counterexample

This file is supporting evidence, not a contract owner and not PASS evidence.

## Immutable source

- Repository: `annyeong844/lumin-repo-lens-lab`
- Exact commit: `35290cb683a37e83bc8c915d1d0f9ca0f3f96fd0`
- Commit subject: `Satisfy strict Rust lint in audit summary tests`

| Git blob | Path | Relevant exact lines |
| --- | --- | --- |
| `3187242366bc02aae23620c535669022bffe8c4c` | `skills/lumin-repo-lens-lab/_engine/producers/classify-dead-exports.mjs` | 294-309, 597-630 |
| `12cf83fbfaf44052a841ccda3544e03f7b294858` | `skills/lumin-repo-lens-lab/_engine/producers/rank-fixes.mjs` | 119-124 |
| `efe906b420cf89d35a1c9553b6e19657630f1d70` | `skills/lumin-repo-lens-lab/references/cli-options.md` | 72-76 |
| `a3211746d44dd6b07c5edc55123b03ac1a3ff01b` | `tests/test-rank-fixes.mjs` | 96-105, 607-613 |
| `79c77e11ea81be9e44d98189732ae2a4a01b694f` | `tests/test-sarif-fix-plan.mjs` | 97-105 |

## Exact behavior to reconstruct

1. The classifier says policy filters short-circuit a symbol out of classification and
   records it in `excludedCandidates`.
2. Config, public API, script/HTML entrypoint, framework, VitePress, declaration
   sidecar, dynamic-import opacity, and test-consumer policy branches all call
   `recordExcluded(...)` and `continue` before ordinary classification finishes.
3. Rank output labels the resulting tier `MUTED (policy-excluded - not a finding)`.
4. The CLI contract states `MUTED` is not emitted to SARIF.
5. Tests require policy exclusion to win "regardless of other evidence" and require
   SARIF to contain no muted result.

That chain turns a policy classification into a user-visible absence. Retaining the row
in an audit-only side array does not cure the false negative when ordinary queries,
projections, and agents are told it is not a finding.

The independent reviewer should read these blobs from the exact legacy commit instead
of trusting this summary.
