# Phase 0 Clean Pinned-Upstream Provenance Probe

Status: **source oracle prepared; clean-runner evidence pending**

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
