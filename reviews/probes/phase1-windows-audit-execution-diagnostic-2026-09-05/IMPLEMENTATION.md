# W2 Implementation and Execution Evidence

Scope: the exact approved [W2 design](DESIGN.md) and [review record](REVIEW.md).
Implementation is under verification; there is no new performance-budget verdict.
P1-60/P1-70, permanent runtime metrics, allocator review, and the WSL `/mnt`
decision remain open.

## Isolated invocation

Use a clean, exact checkout and the pinned Rust 1.96.0/Python 3.13.14 tools.
Build the ordinary release package and the `audit-execution-test-profile`
release executable into distinct target directories, with the same
`LUMIN_BUILD_REVISION`. Stage only the ordinary executable. The diagnostic
feature refuses crash/fault/perturbation combinations.

The runner requires:

- `LUMIN_PACKAGE_ROOT`: the ordinary staged distribution;
- `LUMIN_AUDIT_DIAGNOSTIC_BINARY`: the separate, unshippable executable;
- `LUMIN_AUDIT_DIAGNOSTIC_BUILD_RECORD`: a retained JSON build record;
- `LUMIN_BENCHMARK_CAPTURE_ROOT`: a new archive outside checkout, package,
  and disposable scratch;
- `LUMIN_BENCHMARK_REPORT`: a new external report path; and
- the existing `PINNED_PYTHON` and optional external benchmark scratch path.

The build record binds `sourceRevision`, `lockfileSha256`, `target`, `toolchain`,
`controlCommand`, `diagnosticCommand`, `controlFeatures: []`,
`diagnosticFeatures: ["audit-execution-test-profile"]`, `controlSha256`, and
`diagnosticSha256`. The CI step records its actual build commands and hashes;
the runner validates the clean checkout, pinned target/toolchain, lockfile,
features, payload hashes, and each binary's capabilities build scope. Both
payloads are hashed again after use, including an invalid packet.

```text
lumin-xtask benchmark foundation --diagnose-cold-audit
```

The explicit public-child test partition consumes
`LUMIN_AUDIT_CONTROL_BINARY` and `LUMIN_AUDIT_DIAGNOSTIC_BINARY`:

```text
cargo test -p lumin-cli --test audit_diagnostic --features audit-execution-profile-probe --locked
```

That probe feature selects only the test target. It does not enable diagnostic
instrumentation in the test driver or combine the instrumented executable with
the CLI's development-only gate fault dependency. Missing executable paths fail
the partition rather than skipping it.

## Evidence retention and remaining execution

The initial archive inventory is explicitly incomplete. Process/query bytes
are captured before decoding, and the final manifest records completed,
invalid, and not-run cells plus capture hashes. The normal benchmark may use
its own `LUMIN_BENCHMARK_CAPTURE_ROOT`; its numeric failure is never converted
to a diagnostic success. Windows CI preserves that failure, runs the isolated
diagnostic after successful package prerequisites, and uploads both archives
even on failure.

Local structural/unit verification on 2026-09-06: 178 xtask tests; 102
feature-off model/protocol/engine library tests; 8 feature-on recorder and audit
freshness tests; 12 CI-policy tests; and 2 process-observer tests passed.
Architecture checks, scoped xtask/CLI Clippy (including the external probe
target), formatting, and whitespace checks passed. The external Rust pre/post
advisories retained their invocation-specific file inventories: the main change
declared all 8 new Rust files. The separate test-partition advisory also observed
`audit_diagnostic.rs` as new; that file was already authorized by the main
advisory, not unplanned product scope. Rust's TS-type-escape and scan-parity
lanes were explicitly not applicable; no base-audit quality claim follows.

Actual optimized public-child, package, and Windows packet execution remain
pending at this source checkpoint. A local default-eight-worker
packet cannot identify the four-worker CI bottleneck. No performance change
is selected by this diagnostic implementation.
