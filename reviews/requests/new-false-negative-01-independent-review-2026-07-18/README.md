# NEW-FALSE-NEGATIVE-01 Independent Review Request

Status: **ready for an external exact-tree verdict; the included verifier is not PASS authority**

## Exact subject

- Repository: `annyeong844/lumin_Refactor`
- Candidate: `a1e07ed7b9e05181cd58bfba5f3846c1baab8a93`
- Parent: `f2c1c2b7c5c509e2c82a85daa385a52a16476e10`
- Tree: `06d3116c3c9ab398f1f4fa0b5df38a7fe302cb5e`
- Subject: `Preserve grounded findings from mute policy`
- 16-file manifest SHA-256: `8b0d2ceddb930533e6967c48e06954f09f53abf8f7a688f4dfb0baeb050a6339`
- Later REVIEW-002 ledger commit: `6daf8229b60e50d9adc78c11018df7682784ed7b`
- Finding: `NEW-FALSE-NEGATIVE-01`

The candidate changes exactly five owner/control files:

```text
WORKBOARD.md
architecture/000-system-blueprint.md
architecture/002-evidence-and-write-gate.md
specs/000-product-contract.md
specs/001-foundation-slice.md
```

## Counterexample

The previous Slice allowed a grounded generated or vendored zero-fan-in identity to be
"muted from default removal candidates." An implementation could therefore persist a
row in a hidden policy partition while omitting the real finding from the default query,
SARIF/review projection, and ordinary agent workflow.

This is not hypothetical legacy vocabulary. Exact legacy commit
`35290cb683a37e83bc8c915d1d0f9ca0f3f96fd0` short-circuits policy-matched symbols out of
classification, calls the resulting tier `MUTED (policy-excluded - not a finding)`, and
omits `MUTED` from SARIF. See [legacy-counterexample.md](./legacy-counterexample.md).
The legacy repository is supporting evidence only, never the new contract owner.

## Candidate rule

- Every grounded finding remains canonical and appears in an unfiltered default query.
- `FindingDisposition` is only `ReviewCandidate` or visible `ReviewOnly { reason }`.
- There is no canonical `Muted` or `Suppressed` finding state.
- Every collection echoes normalized filters and reports unfiltered `scopeTotal` plus
  matched `total`; omission is allowed only by an explicit caller filter.
- Disposition changes remain in the semantic dump and use directionless
  `OwnerPayloadChanged`; they are never finding removal.
- Generated/vendored zero-fan-in identities remain findings but are `ReviewOnly`, not
  automatic removal recommendations.
- A performance sample is invalid if muting, filtering, scope reduction, sampling,
  caps, timeout, early termination, or capability omission changes authored truth.

## Required independent review

1. Bind candidate, parent, tree, subject, five-file diff, and all 16 manifest blobs.
2. Reconstruct the prior false-negative path and decide whether the candidate closes it.
3. Try to make `ReviewOnly` behave as renamed mute through default query, count,
   ordering, projection, skill, gate, cache, or performance paths.
4. Check that explicit filtering remains visible and cursor-bound and cannot become a
   skill-owned hidden default.
5. Check that `FindingDisposition` ownership and delta behavior are implementable without
   a second policy owner or crate cycle.
6. Check the authored `source-role-findings-remain-visible` corpus and AC 4 trace.
7. Confirm Slice AC/trace remain 38/38 and Product AC/coverage remain 22/22.
8. Re-run H-01 through H-12, R3/R4, NEW-H10-01, NEW-H11-01, phase-boundary,
   backend/static-packaging/provenance regression scope, and accepted risks.
9. Report `PASS`, `REOPEN`, or a new finding. Do not use this README or verifier output
   as evidence that the design is correct.

Even on PASS, numeric-target approval remains pending. Phase 0 and Phase 1 remain
blocked until that final target gate passes.

## Consistency check

From a clone containing the candidate and request commit:

```text
python reviews/requests/new-false-negative-01-independent-review-2026-07-18/verify_candidate.py --repo .
```

The script checks immutable Git bytes and contract anchors only. It does not judge the
architecture.
