# Phase 0 Backend-Selection Independent Review Request

Status: **ready for an external independent verdict; author-side results are not PASS evidence**

## Exact Subject

- Repository: `annyeong844/lumin_Refactor`
- Candidate: `579c2358f5e2245a977abdedcee7e06ba3f4e46e`
- Commit message: `Preserve raw store build logs in evidence packets`
- 16-file candidate manifest SHA-256:
  `b43b8b0ea9c3c0c8938363091aaf4de0e7a4a3b3babb225582a85b050a104375`
- Superseded candidate: `58b10608eb2bb740e411281dbcc313d5ff23707c`

The superseded candidate must not receive review credit. Its WSL2 and native-Linux
manifests named 20 raw build logs that were absent from the exact Git tree. The current
candidate preserves those bytes and narrows the ignore exception to probe evidence.

## Independence Boundary

The candidate was authored in the same session that produced this request and verifier.
Therefore:

- do not cite `author-preflight.json`, REVIEW-002 Author-Side Preflight, or this README
  as PASS evidence;
- inspect the verifier before running it;
- open the exact candidate through an independently obtained Git object database;
- derive every `PASS`, `REOPEN`, or `NEW` result from candidate bytes, raw probe
  evidence, and owner contracts;
- keep package, public-behavior, upstream-provenance, and numeric-budget gates pending.

## Reproduction

From a fresh clone containing the request packet and candidate object:

```text
python reviews/requests/phase0-backend-selection-independent-review-2026-07-18/verify_candidate.py --repo .
```

The script reads candidate files with `git cat-file`; it does not trust the current
working tree for candidate or evidence bytes. Expected author-side result:

```text
status: PASS
candidate files: 16
evidence packet entries: 168
WSL2/native raw build logs: 20
```

The checked-in [author-preflight.json](./author-preflight.json) records that author-side
run for transport debugging only. It is explicitly nonauthorizing.

Reviewers should independently reproduce the 16-file manifest as ordinal UTF-8 path
order with `sha256`, two spaces, repository-relative path, and LF. They should also
recompute each committed packet manifest rather than treating the script's output as an
oracle.

## Amendment Review

The architecture baseline `65e60216891bb3d826a4778f84cb8aaa377abe92` already had
an accepted exact-tree document/design review. This amendment adds measured evidence
and makes one architecture choice:

```text
production persistence: exact redb 4.1.0 only
comparison-only backend: exact rusqlite 0.39.0 with bundled SQLite
runtime selector/fallback/second production backend: forbidden
```

The external report must verify:

1. B-07 exact Git binding for all 16 candidate files, including both Korean canonical
   sources and all three machine artifacts.
2. All five packet manifests and all 168 referenced Git blobs, including the previously
   omitted 20 WSL2/native build logs.
3. The standalone harness identity and correctness totals: 640 admission rounds, 20
   forced deaths, 470 backend/fault cases, and 190 namespace cases with zero candidate
   failures.
4. The measured comparison from raw summaries: redb wins all five durable-admission
   p50 and binary-size comparisons; SQLite wins four query comparisons, all RSS
   comparisons, and store size.
5. The decision rationale does not hide contrary evidence and matches the product's
   durable write-gate and native-distribution priorities.
6. Exact redb ownership remains private to `lumin-store`; architecture-check and the
   Slice prohibit another backend, feature, selector, or fallback.
7. Backend selection does not weaken H-06 generation fencing, H-08 publication
   serialization, H-10 state/lock/managed-parent identity, or any prior H/R/predecessor
   PASS.
8. OXC evidence remains feasibility-only: no product stack/default jobs or numeric
   budget is silently approved.

Use [review-template.md](./review-template.md) for the verdict. The request identities
and evidence list are machine-readable in [evidence-packets.json](./evidence-packets.json).

## Required Final State

An acceptable report names the exact candidate and manifest, states reviewer
independence, records accepted risks, and publishes a detached SHA-256. Passing this
amendment closes only the backend-selection exact-binding/review gate. Overall Phase 0
remains blocked by the explicitly pending execution and budget gates.
