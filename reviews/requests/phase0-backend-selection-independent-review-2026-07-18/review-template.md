# Lumin Phase 0 Backend-Selection Independent Review

- Review date:
- Reviewer independence statement:
- Repository:
- Exact candidate: `579c2358f5e2245a977abdedcee7e06ba3f4e46e`
- Candidate manifest SHA-256:
  `b43b8b0ea9c3c0c8938363091aaf4de0e7a4a3b3babb225582a85b050a104375`
- Accepted risks: `NONE` or explicit list with owner, scope, rationale, and expiry

## Decision

```text
B-07 exact Git binding: PASS / FAIL
Backend amendment review: PASS / REOPEN
Store correctness evidence: PASS / REOPEN / PENDING
Store measured comparison: PASS / REOPEN / PENDING
OXC evidence boundary: PASS / REOPEN
Overall Phase 0 freeze: BLOCKED
Phase 1 implementation: BLOCKED
```

Do not mark overall Phase 0 passed: package, public-behavior, clean upstream-provenance,
and numeric-budget gates are outside this review packet.

## Byte And Evidence Results

| Check | Result | Independent evidence |
| --- | --- | --- |
| Exact commit object |  |  |
| 16 Git blobs |  |  |
| Candidate manifest |  |  |
| Windows store 12 entries |  |  |
| WSL2 store 37 entries |  |  |
| Native Linux store 37 entries |  |  |
| OXC 80 entries |  |  |
| Selection 2 entries |  |  |
| WSL2/native build logs 20 |  |  |

## Amendment Findings

List findings first, ordered by severity. Every finding must include an exact
counterexample, owner document/artifact, affected predecessor, and closure condition.

If no finding exists, state that explicitly and record residual runtime risk.

## Required Regression Matrix

| Finding family | Result | Notes |
| --- | --- | --- |
| H-01 through H-05 |  |  |
| H-06 generation fencing |  |  |
| H-07 capability continuation |  |  |
| H-08 latest publication |  |  |
| H-09 path identity |  |  |
| H-10 state/lock/managed-parent identity |  |  |
| H-11 resolver configuration |  |  |
| H-12 scan-lock removal |  |  |
| R3 findings 6/6 |  |  |
| R4 findings 6/6 |  |  |
| NEW-H10-01 / NEW-H11-01 |  |  |
| Relevant B/C/E/G predecessors |  |  |
| Packaged-skill/process-reopen specification |  | execution remains pending |

## Store Evidence Assessment

Record independent totals and any disagreement with `selection.json`.

| Metric | Derived result |
| --- | ---: |
| Admission rounds |  |
| Forced admission deaths |  |
| Backend/fault cases |  |
| Namespace cases |  |
| Candidate failures |  |
| redb durable-p50 wins |  |
| redb binary-size wins |  |
| SQLite query-p50 wins |  |
| SQLite RSS wins |  |

Explain whether durable mutation and native distribution justify selecting redb despite
SQLite's query, RSS, and store-size advantages. A digest match alone is not a design
verdict.

## Remaining Gates

- Windows/Linux package, native path/root, and packaged skills
- public process-reopen, recovery, pagination, cold/warm, and jobs determinism
- clean pinned-upstream provenance reproduction
- approved Phase 1 time, RSS, worker-stack, default-jobs, and binary-size budgets

## Final Verdict

State one exact verdict and why. Attach the report's detached SHA-256 sidecar.
