# Phase-Gate Boundary Independent Review Request

Status: **ready for an external independent verdict; author-side output is not PASS evidence**

## Exact Subject

- Repository: `annyeong844/lumin_Refactor`
- Exact semantic candidate: `9a0dbe5c89463892c001e864c4f18eeab9e0eaed`
- Candidate parent: `085828ef09d5eb43621ae992001974ff637a3db2`
- Commit message: `Separate Phase 0 feasibility from Phase 1 acceptance`
- 16-file candidate manifest SHA-256:
  `e2ca379a8a659f2febbc4e277c89db67bb02035a6b10467cf78a5663f21cd99a`
- Later REVIEW-002 ledger commit: `9b64d132768ffd9521bf623f974629ea61832f54`
- Finding: `NEW-PHASE-GATE-01`; direct predecessor: REVIEW-001 `F-12`

The candidate changes exactly two semantic owner files:

```text
WORKBOARD.md
specs/001-foundation-slice.md
```

The prior ledger required actual product packages, packaged skill adapters, product
path/root DTO round trips, and public process behavior before Phase 1 could start.
SLICE-001 simultaneously prohibited Phase 0 probes from exposing product APIs or
becoming a production scaffold. The requested evidence therefore depended on the Phase
1 product whose implementation it blocked.

Candidate `9a0dbe5` changes phase ownership only. It leaves every product requirement and
all 38 Slice acceptance criteria in place. Phase 0 retains standalone feasibility,
clean provenance, numeric target approval, and independent architecture review. Actual
packages, DTO/native-path round trips, skills, public process behavior, corpus behavior,
and achieved-product budgets remain Phase 1 exit criteria.

## Independence Boundary

This request, its verifier, and `author-preflight.json` were produced by the authoring
session. They are consistency aids only. The external reviewer must inspect an
independently opened Git object database, address the full immutable candidate SHA, and
derive the verdict from exact objects and owner contracts rather than this packet's PASS
output.

## Reproduction

From a fresh clone containing the candidate, ledger, and request commits:

```text
python reviews/requests/phase-gate-boundary-independent-review-2026-07-18/verify_candidate.py --repo .
```

The author-side verifier checks exact identities, the two-file diff, the 16-file
manifest, unchanged owner blobs/artifacts, and preservation of all acceptance rows. It
does not decide whether the amendment is architecturally correct.

## Required Independent Review

1. Bind the full candidate SHA, parent, subject, tree, and 16-file B-07 manifest.
2. Confirm the semantic diff changes only WORKBOARD and SLICE-001.
3. Reconstruct the circular counterexample and decide whether NEW-PHASE-GATE-01 and
   F-12 are closed.
4. Confirm no product guarantee, acceptance criterion, traceability row, package/skill
   requirement, public behavior, path/root proof, or achieved-budget proof was removed
   or weakened.
5. Confirm Phase 0 static packaging means standalone target/toolchain/linker/artifact/
   dependency viability and cannot create or emulate a production scaffold.
6. Confirm actual packages, skills, public behavior, native path/root product round
   trips, full corpus, and achieved budgets are Phase 1 exit criteria rather than Phase
   0 prerequisites.
7. Confirm no Rust workspace, crate, scaffold, test, or production code was added.
8. Preserve `AR-BACKEND-01` and `AR-MEASURE-01`; neither passes a remaining gate.
9. Report any H/R or product-contract regression and any new finding.

Use [review-template.md](./review-template.md). Even if this amendment passes, overall
Phase 0 and Phase 1 remain blocked by static-packaging feasibility, clean provenance,
and numeric-target approval.
