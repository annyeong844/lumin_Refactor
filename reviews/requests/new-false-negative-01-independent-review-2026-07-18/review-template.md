# NEW-FALSE-NEGATIVE-01 Independent Verdict

## Identity

- Exact candidate:
- Parent/tree/subject:
- 16-file manifest SHA-256:
- Reviewer independence:

## Decision

```text
B-07 exact binding:                         PASS / REOPEN
NEW-FALSE-NEGATIVE-01:                      PASS-CLOSED / REOPEN
Canonical/default-query finding visibility: PASS / REOPEN
Filter/count/cursor contract:               PASS / REOPEN
Disposition/delta/gate ownership:           PASS / REOPEN
Unmuted performance-sample contract:        PASS / REOPEN
H/R and prior-gate regression:              NONE / list
New finding:                                NONE / list
Exact candidate disposition:                PASS / REOPEN
Overall Phase 0 freeze:                     BLOCKED
Phase 1 implementation:                     BLOCKED
```

## Adversarial cases

| Case | Result | Exact evidence |
| --- | --- | --- |
| Generated zero-fan-in row appears in default query | | |
| Vendored zero-fan-in row appears in default query | | |
| `ReviewOnly` remains in canonical count and finding ID set | | |
| Projection/SARIF/skill cannot silently omit `ReviewOnly` | | |
| Omitted filter normalizes to `{}` and `scopeTotal == total` | | |
| Explicit filter is echoed and cursor-bound | | |
| Disposition change is not finding removal | | |
| Muted/reduced benchmark output is rejected | | |

## Findings

List concrete counterexamples with exact `path:line` evidence. Do not promote the
included verifier or author prose to independent PASS evidence.

## Remaining gate

On closure PASS, numeric target approval remains the sole Phase 0 gate. Product
packages, skills, public behavior, native path round trips, and achieved budgets remain
Phase 1 acceptance.
