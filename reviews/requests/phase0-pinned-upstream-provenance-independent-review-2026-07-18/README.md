# Phase 0 Pinned-Upstream Provenance Independent Review Request

Status: **ready for an external independent verdict; author-side output is not PASS evidence**

## Exact Subject

- Repository: `annyeong844/lumin_Refactor`
- Exact evidence candidate: `e6147b1b2dfea45d223c87f3ba7ffec543e9f82d`
- Candidate parent: `e2a451e31ab648e17d8ae478b2578c1ccdef0fee`
- Candidate tree: `448005cbb75869a680849332a1d6efe23cf088ba`
- Commit message: `Bind clean upstream provenance evidence`
- Frozen architecture candidate: `9a0dbe5c89463892c001e864c4f18eeab9e0eaed`
- Gate base: `d65f8a49250bf94a6a05903ee4d8d2a07e64f197`
- Gate under review: **clean pinned-upstream provenance reproduction**

The exact candidate adds only the standalone provenance packet. The temporary GitHub
workflow is absent from the candidate tree; its exact runner commit remains in candidate
ancestry and the checked-in workflow copy is byte-identical to the runner workflow blob.

## Exact Bindings

| Item | Exact identity |
| --- | --- |
| 16-file candidate manifest | `ca46f77997c696f8eeefc2feabdb9c1031a6e58e36fcb6f2a7ed4ad1bca84fcd` |
| Provenance packet | 34 entries; `77f9790453b7ebad9ba4ba5856f8d6de40bf971f43ec40c899d19fa272762482` |
| Verifier source | 4 entries; `0f39d6782d79e980e128a7a70ed316ac0cae314d9e2812bec8e6825422406b92` |
| Clean evidence | 18 entries; `2228b0ac40afe62d4d72c12919e7dae1a8d1c8a6921f507c6ea6790e56dfc28f` |
| Frozen architecture | 16 entries; `e2ca379a8a659f2febbc4e277c89db67bb02035a6b10467cf78a5663f21cd99a` |
| Clean runner commit | `25bf5c5dd11da351c68c90da54e40b44e62120ce` |
| GitHub workflow run/job | `29638671368` / `88065321460` |
| GitHub artifact | ID `8427910952`; ZIP SHA-256 `d5f25626b8c37808da2115483c41bc3facb14338a21cc68e310da332dde9009d` |

## Provenance Oracle

The machine artifacts, not request prose or `source/oracle.json`, own the pins. The
verifier reconstructs the frozen architecture manifest, reads those exact Git blobs,
and requires the oracle to be an exact projection before retrieval.

Seven retained upstream byte identities are under review:

1. `typescript@6.0.0-beta` npm tarball;
2. exact tar member `package/lib/typescript.js`;
3. pinned TypeScript module resolver source;
4. pinned TypeScript config parser source;
5. pinned Node package document;
6. pinned Node ESM resolver source; and
7. pinned pnpm workspace document.

The packet also binds npm SHA-512 integrity, npm `name/version/repository/gitHead`, the
Node `v24.14.1` annotated tag target, and the 122-entry compiler-option name/shape
digest extracted from exact `typescript.js`.

## Independence Boundary

The request, probe verifier, packet verifier, summaries, workflow records, and author
adversarial checks were produced by the authoring session. Their PASS statuses are
consistency aids only. Do not use them as independent PASS evidence. Address immutable
Git objects and detached bytes directly, write an independent checker, and reproduce
the attacks before deciding the gate.

## Required Adversarial Review

1. Verify the detached transport ZIP, complete-history bundle, exact candidate object,
   parent, tree, subject, and candidate-to-base path scope.
2. Reconstruct the 16-file candidate manifest from exact candidate Git blobs.
3. Recompute the 34-entry packet, 4-entry source, and 18-entry evidence manifests with
   ordinal UTF-8 paths, strict JSON, and exact inventory checks.
4. Independently fetch all immutable URLs and reproduce all seven SHA-256 values plus
   the npm SHA-512 integrity.
5. Inspect the npm tar without unsafe extraction. Reject duplicate, traversal,
   non-regular, missing, or substituted members; independently bind package identity
   and `gitHead` to the TypeScript source commit.
6. Resolve Node tag `v24.14.1` independently and prove it targets exact commit
   `d89bb1b...`.
7. Independently derive the 122 non-command-only TypeScript option name/shape rows and
   digest `f2fb5da0...` from exact verified `typescript.js`.
8. Change source bytes, `typescript.js`, oracle bytes, negative-control status, and
   evidence inventory, regenerate `SHA256SUMS`, and confirm each attack is rejected by
   an inner authority rather than only by the old manifest.
9. Query workflow run `29638671368` and artifact `8427910952`, download the artifact
   independently, verify ZIP SHA-256 `d5f25626...`, and compare every extracted byte to
   the exact candidate evidence.
10. Confirm the candidate remains a standalone Phase 0 verifier with no product API,
    DTO, package, skill, gate/query/process behavior, or implementation scaffold, and
    report any H/R/product regression or new finding.

Use [review-template.md](./review-template.md). A PASS closes only clean pinned-upstream
provenance. Numeric Phase 1 target approval remains the sole Phase 0 gate; Phase 1
implementation stays blocked until Phase 0 freezes.
