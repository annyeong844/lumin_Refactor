# Phase 0 Clean Pinned-Upstream Provenance Probe

Status: **NEW-PROVENANCE-01 closure candidate; independent adversarial review pending**

Source manifest SHA-256:
`14185a4c6c74cac84283b89ce2002f4da8c4afb44f50e5f21e2f236aa299d7f3`

Native clean evidence manifest SHA-256:
`439eff660625b3792c9c6438be6d063a94dce07f6a40802b2368a962e0509b68`

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
frozen Git objects -> machine-artifact projection -> exact HTTPS request descriptors
-> status/URL/encoding/length/retained-byte binding -> exact SHA-256/SHA-512
-> safe npm tar inspection -> exact derived bytes -> 122-entry extraction
-> host/GITHUB_SHA/result runner binding -> successful workflow head binding
-> retained evidence -> detached offline re-verification
```

Pre-existing evidence, redirects, non-identity encoding, wrong retained paths, unsafe or
duplicate tar members, package identity disagreement, tag disagreement, byte
substitution, forged host/runner identity, and stale output are hard stops.

## Current Result

The replacement GitHub `ubuntu-24.04` run reproduced all seven upstream byte identities,
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
| Built-in negative controls | **9/9 PASS** |
| Resealed-evidence adversarial scenarios | **9/9 PASS** |
| Evidence members | **18/18 PASS** |

The resealed attacks changed source bytes, `typescript.js`, the oracle, negative-control
status, packet inventory, HTTPS status/final URL/content encoding, host identity, and
result runner identity, then regenerated the evidence manifest. They were rejected by
the semantic byte, transport, and clean-runner checks rather than by an unchanged outer
seal.

Clean runner binding:

- workflow run: `29642350675`;
- job: `88074824267`;
- exact runner commit: `7e6ebd097cd69318669494fbd95acecbf627b5b4`;
- artifact ID: `8428995583`;
- artifact ZIP SHA-256:
  `4688a9a192349efe7114fc823474732797ee6ee1f3cf49301056a101dc6857c9`;
- capture Node: `v22.23.1`; detached replay Node: `v25.7.0`.

The direct artifact download matched GitHub's declared digest. The extracted evidence
then passed the checked-in offline verifier unchanged. These author checks do not close
the gate by themselves; an independent reviewer must bind the exact candidate and
report PASS or an explicitly accepted risk.

See [source/PROBE-CONTRACT.md](./source/PROBE-CONTRACT.md) for the full oracle.

## Reproduction

Clean capture is restricted to the checked-in GitHub runner and refuses a local or dirty
environment. From an exact complete-history checkout of this closure candidate, replay
the retained evidence and workflow binding with:

```bash
python3 reviews/probes/phase0-pinned-upstream-provenance-2026-07-18/source/verify_provenance.py \
  verify \
  --repository-root . \
  --evidence reviews/probes/phase0-pinned-upstream-provenance-2026-07-18/evidence/native-linux-clean
```

The checked-in runner workflow performs the same commands on a fresh GitHub
`ubuntu-24.04` runner with complete Git history and uploads only the raw evidence.
