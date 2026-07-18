# NEW-EVIDENCE-01 Independent Closure Review Request

Status: **ready for an external independent verdict; author-side output is not PASS evidence**

## Exact Subject

- Repository: `annyeong844/lumin_Refactor`
- Previously reviewed candidate: `579c2358f5e2245a977abdedcee7e06ba3f4e46e`
- Evidence-closure candidate: `b8ff840b5a400d2404d693b290c0fb8d18e59062`
- Commit message: `Remove stale Windows evidence packet seal`
- 16-file candidate manifest SHA-256:
  `b43b8b0ea9c3c0c8938363091aaf4de0e7a4a3b3babb225582a85b050a104375`
- Previous independent report SHA-256:
  `a2a46171ec93927bac5fb0edb34e6547f668dcc47b56e4a0020b6b404623963a`

The previous report passed B-07, the backend amendment, all five canonical evidence
manifests and their 168 entries, store correctness/comparison, the bounded OXC evidence
claim, and H/R regression review. It reopened the exact candidate only because the
unmarked file below claimed superseded packet hashes alongside the canonical Windows
manifest:

```text
reviews/probes/phase0-store-backend-windows-x64-2026-07-17/PACKET-SHA256SUMS
```

The closure candidate deletes that unreferenced competing seal. It does not alter any
of the 16 candidate paths or any canonical evidence-manifest member.

## Independence Boundary

This request, its verifier, and `author-preflight.json` were produced by the authoring
session. They are consistency aids only. The external reviewer must inspect an
independently obtained Git object database, address the full immutable candidate SHA,
and derive the verdict from exact objects rather than this packet's PASS output.

## Reproduction

From a fresh clone containing both immutable candidate objects:

```text
python reviews/requests/new-evidence-01-closure-independent-review-2026-07-18/verify_closure.py --repo .
```

Expected author-side summary:

```text
status: PASS
candidate files: 16
canonical evidence entries: 168
WSL2/native raw build logs: 20
competing packet-wide seals: 0
```

## Required Review

1. Bind `b8ff840...` to its exact Git commit and commit message.
2. Confirm the stale path has blob `e6fd32ebd14bfc4406349145de1fb6497d093d17`
   in `579c235...` and is absent in `b8ff840...`.
3. Reproduce the unchanged 16-file manifest and verify all 16 blob identities are
   unchanged from the previously reviewed candidate.
4. Recompute all five canonical manifests, all 168 unique referenced blobs, and the 20
   WSL2/native raw build logs from `b8ff840...`.
5. Confirm no `PACKET-SHA256SUMS` remains under `reviews/probes` and no canonical probe
   byte changed except deletion of the stale noncanonical seal.
6. Check the intervening noncandidate diff is limited to REVIEW-002 history and the
   prior independent-review handoff.
7. Preserve AR-BACKEND-01 and AR-MEASURE-01 exactly; NEW-EVIDENCE-01 is not an accepted
   risk.

Use [review-template.md](./review-template.md). A closure PASS does not pass package,
public-behavior, clean upstream-provenance, or numeric-budget gates. Overall Phase 0 and
Phase 1 therefore remain blocked.
