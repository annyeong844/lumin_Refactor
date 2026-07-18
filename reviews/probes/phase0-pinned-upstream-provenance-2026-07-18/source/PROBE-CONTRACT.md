# Frozen Pinned-Upstream Provenance Contract

Architecture candidate: `9a0dbe5c89463892c001e864c4f18eeab9e0eaed`

Architecture manifest SHA-256:
`e2ca379a8a659f2febbc4e277c89db67bb02035a6b10467cf78a5663f21cd99a`

## Positive Oracle

The verifier must first reconstruct the frozen 16-file architecture manifest from Git
objects and load the exact resolver and inventory machine artifacts. The checked-in
oracle must be an exact projection of those artifact fields; prose or the oracle alone
cannot authorize a pin.

A successful clean capture must retain and independently verify these seven byte
identities:

1. `typescript@6.0.0-beta` npm tarball;
2. `package/lib/typescript.js` extracted from that exact tarball;
3. TypeScript `src/compiler/moduleNameResolver.ts` at the pinned commit;
4. TypeScript `src/compiler/commandLineParser.ts` at the pinned commit;
5. Node `doc/api/packages.md` at the pinned commit;
6. Node `lib/internal/modules/esm/resolve.js` at the pinned commit; and
7. pnpm.io `docs/pnpm-workspace_yaml.md` at the pinned commit.

The npm tarball must also match the pinned SHA-512 integrity. Its package metadata must
name exact `typescript`, version `6.0.0-beta`, repository
`https://github.com/microsoft/TypeScript.git`, and `gitHead` equal to the pinned
TypeScript source commit. The Node tag API response must resolve exact `v24.14.1` to the
pinned Node commit, following one annotated-tag object when required.

After byte verification, the exact `typescript.js` must be loaded by the checked-in
extractor. Unique non-command-line-only option declarations are normalized to
`name<TAB>shape<LF>`, sorted by UTF-8 byte order, and must reproduce exact count `122`
and SHA-256 `f2fb5da0cf33ea694a8bf4ccae909a1526e7978693c15ac1a6b10b3cdfbc9d9a`.

All eight HTTPS retrieval records have machine-derived IDs, URLs, and retained paths.
Each must occur exactly once, return status 200 without a redirect or non-identity
content encoding, and bind its content-length header, actual byte length, and SHA-256 to
the retained bytes. The annotated-tag object URL is derived from the exact retained tag
ref rather than trusted as free-form metadata.

The clean-runner host record, `GITHUB_SHA`, result `runnerCommit`, and retained workflow
`headSha` must name one exact source-bearing runner commit. The workflow run ID,
repository, job, ref, OS, and architecture are bound through the host and result records.
Capture-time verification may establish only the host/result half of this relation; the
detached `verify` command requires the retained successful workflow record before it can
authorize the packet.

Raw bytes, verified response metadata, derived compiler-option rows, host/tool versions,
negative controls, the exact oracle, the frozen architecture manifest, and the source
manifest identity are retained under one `SHA256SUMS`.

## Hard Stops

The probe fails rather than degrading when:

- the frozen Git commit, 16-file manifest, or owner artifact bytes disagree;
- the oracle differs from the machine-artifact projection;
- the current owner artifacts differ from the frozen owner bytes;
- the evidence directory already exists;
- HTTPS status, URL, redirect, content encoding, length, or byte hash disagrees;
- a fetch ID is missing, duplicated, reordered, or bound to a different retained path;
- host, GitHub environment, result, or workflow runner identities disagree;
- the npm SHA-512 integrity or package identity disagrees;
- a tar member is duplicated, unsafe, non-regular, or a required member is absent;
- extracted `typescript.js` differs from the retained derived object;
- the Node tag does not resolve to the pinned commit;
- Node extraction is unavailable or the option count/name/shape digest differs;
- any manifest member is missing, extra, unsafe, duplicated, or hash-mismatched; or
- a one-byte mutation, same-size substitution, malformed tar, oracle mutation, stale
  evidence input, redirected fetch record, forged host, or substituted result runner is
  accepted by the negative-control suite.

## Non-Claims

This probe is not a Lumin implementation and must not expose or emulate product APIs,
DTOs, path codecs, skills, gates, queries, process recovery, package behavior, or
performance budgets. It proves only clean reproducibility of frozen upstream identity.
