# NEW-PROVENANCE-01 Independent Closure Review Request

Status: **ready for an external independent verdict; author-side output is not PASS evidence**

## Exact Subject

- Repository: `annyeong844/lumin_Refactor`
- Previous evidence candidate: `e6147b1b2dfea45d223c87f3ba7ffec543e9f82d`
- Exact closure candidate: `658b5c7334fed8f0e42dd14b9910c9b719f3e55b`
- Candidate parent: `a1c9f2ee73da78f755fe967d174524f074799683`
- Candidate tree: `15b8777486f7dfe45a1d57650a230b287f21df7b`
- Commit message: `Regenerate exact-bound provenance evidence`
- Previous independent report SHA-256:
  `6826af8f1d578f3a423ae3725be1c3fb254a00e373ff2db4df7e2d4fc9537aaa`
- Finding under review: `NEW-PROVENANCE-01`

The previous review passed exact Git and packet binding, all seven upstream byte
identities, npm package/tar safety, Node tag identity, and the 122-entry compiler-option
derivation. It reopened the gate because the detached verifier accepted resealed
`302`/redirect/gzip fetch metadata and a forged platform/repository/GITHUB_SHA record.

## Closure Candidate

The v2 verifier now requires one transitive relation:

```text
frozen owner bytes
-> exact fetch ID/URL/retained path
-> status/final URL/encoding/content length/actual length/SHA-256
-> retained upstream bytes and Node tag-ref-derived tag-object URL
-> host repositoryHead and GITHUB_SHA
-> result runnerCommit and cleanRunner run identity
-> retained successful workflow head SHA and job
```

Capture may validate the host/result half while the workflow is still running. Detached
`verify` has no such authorization path: it requires the retained workflow record and
checks all four runner identities. The exact closure candidate contains no temporary
workflow and retains the workflow only as evidence copied from the exact runner commit.

## Exact Bindings

| Item | Exact identity |
| --- | --- |
| B-07 candidate manifest | 16 entries; `ca46f77997c696f8eeefc2feabdb9c1031a6e58e36fcb6f2a7ed4ad1bca84fcd` |
| Frozen architecture authority inside evidence | 16 entries; `e2ca379a8a659f2febbc4e277c89db67bb02035a6b10467cf78a5663f21cd99a` |
| Provenance packet | 34 entries; `cc887e448e2a560801c09cd082f55304f689148a74d6f91e837582de70df65a3` |
| Verifier source | 4 entries; `14185a4c6c74cac84283b89ce2002f4da8c4afb44f50e5f21e2f236aa299d7f3` |
| Clean evidence | 18 entries; `439eff660625b3792c9c6438be6d063a94dce07f6a40802b2368a962e0509b68` |
| Clean runner | `7e6ebd097cd69318669494fbd95acecbf627b5b4` |
| Workflow run / job | `29642350675` / `88074824267` |
| Artifact ID | `8428995583` |
| Official artifact ZIP | `4688a9a192349efe7114fc823474732797ee6ee1f3cf49301056a101dc6857c9`, 6,257,298 bytes |

## Independence Boundary

This request, `verify_candidate.py`, packet verifier, retained summaries, built-in
negative-control rows, resealed-attack rows, and `author-preflight.json` were produced
by the authoring session. Their PASS statuses are consistency aids only. Bind immutable
Git objects, inspect the verifier, derive transport/runner expectations independently,
and reproduce the attacks before deciding closure.

## Required Adversarial Review

1. Verify the complete-history bundle and bind exact candidate, parent, tree, subject,
   B-07 packet, source fix, clean runner, and probe-only closure range.
2. Recompute the 34-entry packet, 4-entry source, and 18-entry evidence manifests from
   exact candidate Git blobs with safe-path and duplicate-key rejection.
3. Derive the eight expected fetch IDs, URLs, and retained paths from the frozen oracle
   and exact Node tag ref. Check status 200, no redirect/non-identity encoding, exact
   content length, actual length, and retained-byte SHA-256.
4. Reseal evidence after changing a fetch row to status 302, an evil final URL, and gzip.
   Detached verification must fail `fetch-metadata-invalid`.
5. Reseal evidence after forging platform, repositoryHead, and GITHUB_SHA. It must fail
   `host-runner-mismatch`. Substitute result runnerCommit separately and require the same
   hard stop.
6. Query workflow run `29642350675` and artifact `8428995583` independently. Bind
   workflow head, host/result runner, exact source blobs, artifact API digest, direct ZIP
   digest, and all 18 artifact members.
7. Confirm the candidate contains no temporary workflow and remains a standalone Phase
   0 verifier with no product API, DTO, package, skill, query/gate/process behavior, or
   implementation scaffold.
8. Report any H/R/product regression or new finding. Preserve `AR-BACKEND-01` and
   `AR-MEASURE-01` unless exact counter-evidence reopens them.

Use [review-template.md](./review-template.md). A closure PASS closes only clean pinned
upstream provenance. Overall Phase 0 and Phase 1 remain blocked by numeric Phase 1 target
approval.
