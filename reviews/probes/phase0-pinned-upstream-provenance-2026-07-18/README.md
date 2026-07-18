# Phase 0 Clean Pinned-Upstream Provenance Probe

Status: **clean-runner evidence complete; independent adversarial review pending**

Source manifest SHA-256:
`0f39d6782d79e980e128a7a70ed316ac0cae314d9e2812bec8e6825422406b92`

Native clean evidence manifest SHA-256:
`2228b0ac40afe62d4d72c12919e7dae1a8d1c8a6921f507c6ea6790e56dfc28f`

This standalone verifier reproduces the seven upstream byte identities frozen by
`inventory-config-semantics.v1.json` and `resolver-config-semantics.v1.json`. It also
recomputes the TypeScript 122-entry compiler-option name/shape digest from the exact
verified `typescript.js` bytes.

## Claim Boundary

This packet may establish only that a clean probe environment independently retrieved
and verified:

- the exact `typescript@6.0.0-beta` npm tarball and its SHA-512 integrity;
- the exact `package/lib/typescript.js` member and embedded npm package identity;
- the pinned TypeScript module-resolver and config-parser sources;
- the pinned Node package document and ESM resolver source;
- the pinned pnpm workspace document;
- the Node `v24.14.1` tag-to-commit relation; and
- the frozen 122-entry TypeScript compiler-option name/shape digest.

The probe does not implement Lumin, expose a product command or DTO, analyze a
repository, package a skill, approve a product binary, or select a numeric Phase 1
budget. Phase 1 architecture checks must later enforce the same identities.

## Authorization Model

`source/oracle.json` is a projection, not a second semantic owner. Before any network
retrieval, the verifier reads the two machine artifacts from frozen architecture commit
`9a0dbe5c89463892c001e864c4f18eeab9e0eaed`, reconstructs its 16-file manifest, and
requires every projected pin to match those exact artifact bytes.

Capture then uses one fresh evidence directory and performs this chain:

```text
frozen Git objects -> machine-artifact projection -> HTTPS response bytes
-> exact SHA-256/SHA-512 -> safe npm tar inspection -> exact derived bytes
-> 122-entry extraction -> retained evidence -> offline re-verification
```

Pre-existing evidence, redirects, unsafe or duplicate tar members, package identity
disagreement, tag disagreement, byte substitution, and stale output are hard stops.

## Current Result

The fresh GitHub `ubuntu-24.04` run reproduced all seven upstream byte identities,
the npm SHA-512 integrity, package `gitHead`, Node annotated-tag target, and the exact
122-entry compiler-option digest. Its 18-entry evidence manifest passed capture-time
verification and a detached offline replay on a different Node version.

| Check | Result |
| --- | --- |
| Frozen 16-file architecture manifest | **PASS** |
| Resolver/inventory machine owners | **2/2 PASS** |
| Upstream byte identities | **7/7 PASS** |
| TypeScript npm SHA-512 and package identity | **PASS** |
| Node `v24.14.1` annotated tag target | **PASS** |
| Compiler-option extraction | **122 entries PASS** |
| Built-in negative controls | **6/6 PASS** |
| Resealed-evidence adversarial scenarios | **6/6 PASS** |
| Evidence members | **18/18 PASS** |

The resealed attacks changed source bytes, `typescript.js`, the oracle, negative-control
status, and packet inventory, then regenerated the evidence manifest. They were rejected
by the frozen byte/oracle/derived-evidence checks rather than by an unchanged outer seal.

Clean runner binding:

- workflow run: `29638671368`;
- job: `88065321460`;
- exact runner commit: `25bf5c5dd11da351c68c90da54e40b44e62120ce`;
- artifact ID: `8427910952`;
- artifact ZIP SHA-256:
  `d5f25626b8c37808da2115483c41bc3facb14338a21cc68e310da332dde9009d`;
- capture Node: `v22.23.1`; detached replay Node: `v25.7.0`.

The direct artifact download matched GitHub's declared digest. The extracted evidence
then passed the checked-in offline verifier unchanged. These author checks do not close
the gate by themselves; an independent reviewer must bind the exact candidate and
report PASS or an explicitly accepted risk.

See [source/PROBE-CONTRACT.md](./source/PROBE-CONTRACT.md) for the full oracle.

## Reproduction

From an exact complete-history checkout:

```bash
python3 reviews/probes/phase0-pinned-upstream-provenance-2026-07-18/source/verify_provenance.py \
  capture \
  --repository-root . \
  --evidence reviews/probes/phase0-pinned-upstream-provenance-2026-07-18/evidence/native-linux-clean

python3 reviews/probes/phase0-pinned-upstream-provenance-2026-07-18/source/verify_provenance.py \
  verify \
  --repository-root . \
  --evidence reviews/probes/phase0-pinned-upstream-provenance-2026-07-18/evidence/native-linux-clean
```

The checked-in runner workflow performs the same commands on a fresh GitHub
`ubuntu-24.04` runner with complete Git history and uploads only the raw evidence.
