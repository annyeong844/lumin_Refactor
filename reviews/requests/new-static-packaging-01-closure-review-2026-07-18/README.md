# NEW-STATIC-PACKAGING-01 Independent Closure Review Request

Status: **ready for an external independent verdict; author-side output is not PASS evidence**

## Exact Subject

- Repository: `annyeong844/lumin_Refactor`
- Previous evidence candidate: `e0a2810b46f6091895b5e9f7dd4454e8854fee0e`
- Exact closure candidate: `4315eb7dee35fff3de40fb04e1dd3c4a3fc990e3`
- Candidate parent: `bb79655cb94c59822a928ede1b861980e6358b6a`
- Candidate tree: `2cb4e5e055e8e9e82e26351af5ad3ddc2ca40a11`
- Commit message: `Regenerate exact-bound static packaging evidence`
- Previous independent report SHA-256:
  `b26cee869f883ec6ba1b776a46f53d276ce3c39cb1333865a050c31490c27ac0`
- Finding under review: `NEW-STATIC-PACKAGING-01`

The previous review passed B-07, exact packet/source byte binding, the standalone
non-product boundary, the actual formats and smoke behavior of all five binaries, and
native Linux workflow provenance. It reopened Windows and WSL because the author seal
could combine stale run/linkage evidence with unrelated artifact bytes. In particular,
it accepted dynamic `/bin/true` as both GNU and supposedly static musl evidence.

## Closure Candidate

The closure range replaces that independent-file composition with one exact-artifact
boundary:

1. hash the supplied artifact;
2. copy those bytes into a fresh seal-owned execution directory;
3. rehash and directly parse PE/ELF machine and linkage from that copy;
4. execute that exact copy and retain its run-v2 stdout;
5. rehash the execution copy and supplied artifact after invocation;
6. require run-v2 to report the exact source manifest and frozen architecture
   identities;
7. repeat inspection and execution when verifying detached evidence.

Pre-existing run, inspection, execution, summary, or manifest output is a hard stop.
Static musl is derived from absence of `PT_INTERP` and `DT_NEEDED`, never from a label.

## Exact Bindings

| Item | Exact identity |
| --- | --- |
| Packet manifest | 73 entries; `ad2e746441ee778ecf8e8f51a12a331d3c6b3c78a1c995fb661970ab925b6764` |
| Source manifest | 9 entries; `38c1a75d06edb12bb2798d93bc1ce788325ca33c6bc12dabd4ef10df943b677c` |
| Windows evidence | 13 entries; `bbbe8ac057f70f0993073d237a9d17715a92810f7635d58d47bca381a64e1b01` |
| WSL2 evidence | 21 entries; `fefc42bc6b06fce1f5dd7804559348b068e6f4fda52006326b65abbbc19ed1bc` |
| Native Linux evidence | 21 entries; `6c8c8f22b9aaa0e8104417a542de0bd2e02386173d3b54d8d2cccdd97e947339` |
| Windows PE | `dd7ba4cda6e5654f864c79c18e0aa0a9a96001b0591490e3038d416a11762d7a`, 1,412,608 bytes |
| WSL2 GNU ELF | `6892e467d61fc2ffcc3c0fec73323a8d8c2d789e709cfd801d0ff94ffc50caf4`, 1,794,736 bytes |
| WSL2 musl ELF | `ad9b7d8789111ede6c065805185d21cc07a87400555eedecad859952fb258a32`, 1,897,184 bytes |
| Native workflow ZIP | `2f238899ccccbb43a1c345eab3746f68da56a86208ef0d46fa11e36853cbb971`, 1,659,125 bytes |

Native workflow provenance:

- run ID `29634512936`;
- exact runner commit `b7560b443d973540020bd2de984a99b69c35d14e`;
- artifact ID `8426637860`.

## Independence Boundary

This request, `verify_candidate.py`, the probe seal, retained summaries, negative-control
records, and `author-preflight*.json` were produced by the authoring session. They are
consistency aids only. Do not use their PASS status as independent evidence. Address the
full immutable Git SHA, hash detached bytes directly, inspect PE/ELF independently, run
the exact artifacts, and reproduce adversarial substitutions before deciding closure.

## Required Adversarial Review

1. Verify the complete-history bundle and bind the exact candidate, parent, tree,
   subject, B-07 architecture packet, and closure range.
2. Recompute the 73-entry packet, 9-entry source, and 13/21/21 evidence manifests from
   exact candidate Git blobs with duplicate-key-rejecting JSON parsing.
3. Inspect `package_evidence.py` and prove that the exact artifact bytes, fresh execution
   copy, run-v2 output, inspection, source identity, and architecture identity form one
   transitive relation.
4. Rehash and directly inspect all five binaries. Execute the Windows and WSL detached
   artifacts on their native hosts and compare fresh stdout to the retained run-v2
   records.
5. Substitute `/bin/true` for WSL GNU and musl. GNU verification must fail because the
   run contract is wrong; musl inspection/verification must fail because it is dynamic.
6. Attempt to reuse pre-existing generated run evidence during sealing. It must fail
   before artifact authorization.
7. Re-download or otherwise independently bind workflow run `29634512936`, exact runner
   commit `b7560b4...`, artifact ID `8426637860`, and archive digest `2f238899...971`.
8. Confirm the candidate remains a standalone Phase 0 feasibility probe and introduces
   no product API, DTO, skill, package, query/gate/process behavior, or scaffold.
9. Report any H/R/product regression or new finding. Preserve `AR-BACKEND-01` and
   `AR-MEASURE-01` unless exact counter-evidence reopens them.

Use [review-template.md](./review-template.md). A closure PASS closes only static
packaging. Overall Phase 0 and Phase 1 remain blocked by clean pinned-upstream
provenance reproduction and numeric Phase 1 target approval.
