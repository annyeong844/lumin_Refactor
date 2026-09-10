# W2 Diagnostic Design Review Record

Date: 2026-09-05. Scope owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).

## Exact candidate and disposition

| Item | Binding / result |
| --- | --- |
| Candidate | [DESIGN.md](DESIGN.md), W2, exact UTF-8/LF bytes |
| Candidate SHA-256 | `9bb93825979d326b946e59ef34873b0e767d79f8d6f78a093c836e046ee288de` |
| Product source HEAD used for both reviews | `a83ee602dfaf1d62c16f8f9f68f6111c81c203fb` |
| Author design review | PASS for the diagnostic scope and source trace below |
| Independent adversarial review | Scoped PASS from separately delegated, read-only agent `audit_profile_design_review` |
| Owner approval / freeze | User approved W2 and instrumentation implementation on 2026-09-05; exact DESIGN.md hash above is frozen for this diagnostic scope only |
| Performance / Phase 1 verdict | Unchanged; Windows scaling misses `0.75`, P1-60 and P1-70 remain open |

The independent reviewer inspected the exact candidate and existing source,
made no edits, and returned findings independently of the author's checks.
The user then approved proceeding from that reviewed design to implementation.
Neither review nor approval is an external GitHub-bot review or product-execution
result. The SHA binds DESIGN.md alone;
review annotations and routing-document updates are outside the candidate.

## Findings and closure

The first candidate W1 had SHA-256
`f7fc313a8406643487c5712d0dea3fade371ce83f065f7184028ba3d9a9e03b6`.
Its disposition was NEEDS REVISION. W2 resolves the following findings and
integration requirements without changing its diagnostic-only authority.

| Review item | Grounded counterexample | Exact W2 decision |
| --- | --- | --- |
| P2: wrong public build-ID authority | Audit/overview DTOs have no build ID; requiring one rejects valid samples or invites a skipped check. | Bind frame build ID to the same hashed executable's public `capabilities --format json` `/scope/buildId`; bind attempt/run separately through audit stdout and `overview --run`. |
| Observer PID integration | Existing process-measurement v1 has no child PID even though its `Popen` object knows it. | Diagnostic-only helper v2 exports actual `Popen.pid`; ordinary helper v1 stays unchanged. Wrong/missing PID rejects the sample. |
| Raw-capture retention integration | Normal benchmark scratch cleanup removes process captures even on a numeric miss; upload globs alone cannot retain them. | Separate create-new archives receive raw process and truth-query bytes before decoding/error returns, survive cleanup, and remain uploadable as explicitly incomplete prefixes on failure. |

The independent reviewer reread complete W2, retraced these source boundaries,
verified the candidate hash twice, and reported all W1 items closed with no
new concrete blocker. Its conclusion is limited to this design, not future
implementation or runtime behavior.

## Author source trace

- `crates/application/engine/src/lib.rs`: one existing local pool; audit
  admission, store open, attempt begin, capture, owner calls, finish, and
  publication boundaries match the proposed 23-phase hierarchy.
- `crates/application/engine/src/audit_publication.rs` and
  `crates/application/store/src/publication/run.rs`: final-input validation is
  nested inside store publication; resource release precedes CLI transport.
  Residual publication time is deliberately not called measured flush time.
- `crates/application/cli/src/lib.rs`, `src/main.rs`, and
  `crates/application/protocol/src/lib.rs`: request/default-job observation,
  audit DTO identity, response serialization, and post-engine output ownership
  have concrete existing call sites. Production DTOs remain unchanged.
- Pinned `rayon-core 1.13.0` source `thread_pool/mod.rs`: the concrete pool
  exposes `current_num_threads`; the observation is pool size, not busy-worker
  count. The applied stack policy is not claimed as observed OS stack usage.
- `tools/xtask/src/benchmark.rs`, `benchmark/measurement.rs`,
  `benchmark/truth.rs`, and `tools/xtask/benchmark/measure-process.py`: strict
  ordinary success stderr, raw capture loss at cleanup, observer PID source,
  fixture truth and cross-run tuple-to-ID comparison were inspected directly.
- `tools/xtask/src/package_check/artifact.rs` and `.github/workflows/ci.yml`:
  the existing capabilities build-ID query is reusable; diagnostic artifact
  isolation and after-failure execution/uploads require explicit new wiring.

The author also checked the closed phase inventory: 23 unique ordered names,
one root, and every child naming an earlier parent. This is consistency-only
evidence; deterministic clock tests and public-child cases in the design are
future implementation acceptance, not tests claimed as already executed.

## Checks and limits

Author checks: `git diff --check`; repository `ci_policy.py` documentation
validation; its `document_errors` validator explicitly applied to the new
untracked packet; and `test_ci_policy.py` (11 tests passed). The latest retained
Windows/Linux benchmark report hashes and medians were independently reread
before updating REVIEW-005's current evidence.

Independent checks: complete candidate/routing reads; owner and concrete
source inspection with `rg` and numbered direct reads; `git rev-parse HEAD`,
`git status --short`, `git diff --check`; whitespace/local-link checks; and
SHA-256 verification. No code-map, file edits, builds, product executions,
or fresh CI measurements were performed by the reviewer.

The reviewed candidate was documentation-only. At that review boundary, no Rust source, Cargo dependency,
workflow, frozen owner contract, numeric threshold, or Phase 1 checkbox was
changed. Permanent metrics, allocator cost approval, WSL `/mnt` disposition,
actual Windows diagnosis, and all required numeric evidence remain open.

Implementation commands and execution evidence are tracked separately in
[IMPLEMENTATION.md](IMPLEMENTATION.md); they do not change the reviewed design hash.
