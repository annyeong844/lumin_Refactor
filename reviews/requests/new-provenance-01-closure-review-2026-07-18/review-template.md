# NEW-PROVENANCE-01 Independent Closure Verdict

- Exact candidate:
- Candidate tree:
- Packet manifest SHA-256:
- Source manifest SHA-256:
- Evidence manifest SHA-256:
- Independent report SHA-256:

## Decision

```text
B-07 exact architecture binding:
Exact provenance candidate binding:
34-entry packet and 18-entry evidence binding:
Seven pinned upstream byte identities:
TypeScript package/source transitive identity:
Node tag/commit identity:
122-entry compiler-option derivation:
Clean workflow/artifact binding:
NEW-PROVENANCE-01:
Standalone non-product boundary:
Existing H/R regression:
New finding:
Exact-candidate disposition:
Overall Phase 0 freeze:
Phase 1 implementation:
```

## Required Evidence

| Check | Result | Independent evidence |
| --- | --- | --- |
| Complete-history bundle and `git fsck --full --strict` |  |  |
| Exact candidate/parent/tree/subject |  |  |
| 16-entry B-07 architecture manifest |  |  |
| 34-entry packet exact Git-object inventory |  |  |
| 4-entry verifier-source manifest |  |  |
| 18-entry clean-evidence manifest |  |  |
| Eight unique machine-derived fetch bindings |  |  |
| Status/final URL/encoding/length/hash semantics |  |  |
| Node tag-object URL derived from exact tag ref |  |  |
| Host/GITHUB_SHA/result/workflow runner identity |  |  |
| Official run/job/artifact/API digest |  |  |
| Direct artifact ZIP and 18 member bytes |  |  |
| 122-entry compiler-option independent derivation |  |  |
| Nine built-in negative controls |  |  |
| Nine resealed adversarial scenarios |  |  |
| Temporary workflow absent from candidate |  |  |
| Standalone Phase 0 claim boundary |  |  |
| H/R/product regression review |  |  |

## Mandatory Resealed Attacks

Record the mutation, regenerated manifest digest, verifier exit status, and exact reason
code for each:

1. `status=302`, evil `finalUrl`, `contentEncoding=gzip`;
2. forged `platform`, `repositoryHead`, and `GITHUB_SHA`;
3. substituted `result.runnerCommit`;
4. changed retained bytes with stale fetch length/hash metadata;
5. changed workflow `headSha` against unchanged host/result runner.

## Findings

- Existing finding reopened:
- NEW finding:
- Accepted risk changed:

## Remaining Boundary

A PASS closes only NEW-PROVENANCE-01 and clean pinned-upstream provenance. Numeric Phase
1 targets remain a Phase 0 gate. Product packages, skills, native path/root product DTO
round trips, public process behavior, and achieved budgets remain Phase 1 acceptance.
