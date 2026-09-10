# W2 Implementation and Execution Evidence

Scope: the exact approved [W2 design](DESIGN.md) and [review record](REVIEW.md).
Implementation and local verification are complete for source checkpoint
`50f629e83403cf1387badfe6a2e1e4691e81972e`. The normal Windows matrix passes
locally; the diagnostic has no numeric-budget verdict and does not establish
the four-worker Windows CI outcome. P1-60/P1-70, permanent runtime metrics,
allocator review, and the WSL `/mnt` decision remain open.

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

## Verification and evidence retention

The initial archive inventory is explicitly incomplete. Process/query bytes
are captured before decoding, and the final manifest records completed,
invalid, and not-run cells plus capture hashes. The normal benchmark may use
its own `LUMIN_BENCHMARK_CAPTURE_ROOT`; its numeric failure is never converted
to a diagnostic success. Windows CI preserves that failure, runs the isolated
diagnostic after successful package prerequisites, and uploads both archives
even on failure.

Local verification on 2026-09-06 passed:

- Windows: 178 xtask tests; 102 feature-off model/protocol/engine library
  tests; 8 feature-on recorder and audit freshness tests; 3 actual optimized
  public-child tests; 12 CI-policy tests; and 2 process-observer tests.
- Linux under WSL2: 178 xtask tests and 104 feature-off
  model/protocol/engine library tests. The extra two engine tests are
  platform-specific, not skipped Windows evidence.
- Architecture checks; scoped xtask/CLI Clippy on both platforms, including
  the Windows diagnostic and external-probe configurations; formatting;
  and whitespace checks.
- Actual staged Windows and Linux-musl binaries, plus both staged skill
  adapters on each platform, passed the behavioral package probes. Linux
  used native ext4 repository scratch, not the unsupported `/mnt` namespace
  path. This is local WSL2 package evidence, not a native Linux CI verdict.
- The incompatible diagnostic/fault feature combination failed with its
  exact owner diagnostic. An accurately manifested diagnostic package was
  rejected by the ordinary Windows package probe because successful audit
  wrote the diagnostic frame to stderr.

The focused commands below use the pinned source-provenance bootstrap for
resolving Cargo operations; non-resolving `fmt` uses pinned Cargo directly.
The feature-on and external-public-child commands run on Windows. The
feature-off library/xtask tests and default Clippy commands run on both hosts.
Executable environment bindings are those described above.

```text
cargo test -p lumin-model -p lumin-protocol -p lumin-engine --lib --locked -j1
cargo test -p lumin-model -p lumin-engine --lib --features audit-execution-test-profile audit_ --locked -j1
cargo test -p lumin-xtask --locked -j1
cargo test -p lumin-cli --test audit_diagnostic --features audit-execution-profile-probe --locked -j1
cargo clippy -p lumin-xtask --all-targets --locked -j1 -- -D warnings
cargo clippy -p lumin-cli --lib --bin lumin --locked -j1 -- -D warnings
cargo clippy -p lumin-cli --bin lumin --features audit-execution-test-profile --locked -j1 -- -D warnings
cargo clippy -p lumin-cli --test audit_diagnostic --features audit-execution-profile-probe --locked -j1 -- -D warnings
cargo fmt --all --check
lumin-xtask architecture-check
lumin-xtask package-check windows-x64
lumin-xtask package-check linux-x64
lumin-xtask package-check skills
```

The external Rust pre/post
advisories retained their invocation-specific file inventories: the main change
declared all 8 new Rust files. The separate test-partition advisory also observed
`audit_diagnostic.rs` as new; that file was already authorized by the main
advisory, not unplanned product scope. Rust's TS-type-escape and scan-parity
lanes were explicitly not applicable; no base-audit quality claim follows.

## Executable and packet identity

All measured binaries were built from the clean source checkpoint above with
Rust 1.96.0 and the locked dependency graph. The lockfile SHA-256 is
`c82640e677c8c602c90f9ee8577a4286d52e65e621636067bf59f8b0257185fb`.
Their observed capabilities build scope is
`build_b44f6f5f4d4a594bfd9cfd4f9a236c6e0ad0e4dcf1f459a972040abb3c108e9a`.

| Executable | Bytes | SHA-256 |
| --- | ---: | --- |
| Windows ordinary release | 10,482,176 | `6abf23d42fa09d25cf9259db8e6f5b345b5994b0083976963b97487b3c684654` |
| Windows diagnostic-only release | 10,508,288 | `76148e99544bbb81a4d753978057e5f9a64484c032f10c6f81901b7e94c42946` |
| Linux-musl ordinary release | 11,922,160 | `c96d2f9eb52e848d8cc3e56902dc24ea8d655f194c444213e3272613465705f9` |

The retained Windows build record is
`D:\lumin-w2-build-record-50f629e-20260906.json`, SHA-256
`e5240fc744f0ced785ff7cb4f452e2df2bc7ad09f6e1a102c3d71fefee29808b`.
It records the exact ordinary and feature-enabled release commands, each with
`--locked -j1`, separate target directories, and the same source revision.
The diagnostic runner rehashed both payloads after the packet; neither changed.
The ordinary staged packages remain separate from the diagnostic executable
and the intentionally unshippable package used only for rejection probes.

The Linux package remains at
`/home/endof/.cache/lumin-w2-linux-package-50f629e-20260906` in WSL2;
its manifest SHA-256 is
`4e611edaa4227da3f49e40e30832f1977a826ec09c840300a25d12bf10307374`.
Its release build uses `--target x86_64-unknown-linux-musl --locked -j1`
and the musl C compiler; the fixture binary is a separately rebuilt GNU
debug executable with `lifecycle-test-fault`.

The following local artifacts are retained outside the checkout. A capture
manifest is not a substitute for its raw files: every listed file's size and
SHA-256 was independently rechecked after the disposable scratch was removed.

| Artifact | Retained path | SHA-256 |
| --- | --- | --- |
| Normal report | `D:\lumin-w2-normal-report-50f629e-20260906.json` | `6b012f2fe55073c8266278c706644e14914d60a28eed724163d94eaa8960d117` |
| Normal archive manifest | `D:\lumin-w2-normal-captures-50f629e-20260906\manifest.json` | `310ac9fa153c142af6df1f02fda57e21c1513636bf4ecc1cabb79f865d2adeeb` |
| Diagnostic report | `D:\lumin-w2-diag-report-50f629e-20260906.json` | `fbf8aca973e44f6020db51dc6ac060f7f8ba3f2e535f3250175d4121f1ddbcd0` |
| Diagnostic archive manifest | `D:\lumin-w2-diag-captures-50f629e-20260906\manifest.json` | `88df2b327b27355ec9121302206f726bb44df32f3571e09923eeee9d19a8ea68` |
| Rejected normal invocation archive | `D:\lumin-w2-negative-normal-captures-50f629e-20260906\manifest.json` | `6d2feebf0d0992aa0bd471c60138585e11bb50c39136c9820ecb31267f1db21d` |

## Observed results and limits

The Windows NTFS host reported 12 available logical processors and used eight
default workers. No other task build or benchmark was run concurrently with
either timing matrix. Cold still means a fresh process/repository/state, not
flushed machine-global caches.

The ordinary matrix returned `PASS`: 21 measured samples, all 34 conditioning,
seed/setup, and measured cells complete, and 711 retained hashed files. Cold
default median was `3,095,238,700 ns` versus `7,687,853,200 ns` for `jobs=1`,
ratio `0.40261417842890135` against the unchanged `0.75` limit. Every other
time, RSS, size, and semantic target passed. These numbers are evidence for
this local source/host, not a replacement for the outstanding CI measurement.

The W2 packet returned `DIAGNOSTIC_ONLY` with `numericBudgetVerdict: null`:
all 14 cells, including 12 measured cells, completed with 365 retained hashed
files. Its seven diagnostic frames each contain the exact 23-phase inventory;
their process IDs match the independent OS observer, and their worker counts
match the actual one/eight-worker executions. All 14 cells preserve the full
256-finding truth, role/disposition metadata, empty limitation set, and the
same authored semantic ID-map SHA-256:
`93b1b99df26f8b7a5872425cc84f0f2dcb36e5af65f53eb37756e32a8532c523`.

| W2 median | `jobs=1` (ns) | Default (ns) | Default/one |
| --- | ---: | ---: | ---: |
| Ordinary control | 7,749,792,400 | 3,201,305,600 | 0.4130827556103309 |
| Diagnostic | 6,859,965,900 | 2,890,351,700 | 0.42133616145234776 |

The observed diagnostic-minus-control differences are negative
(`-889,826,500 ns` for one worker; `-310,953,900 ns` for default). They remain
signed observations, not proof that instrumentation speeds up audit or an
isolated estimate of timer cost. Machine cache state, code layout, and
between-executable effects are not controlled by these three repetitions.

Diagnostic phase medians locate local inventory at approximately
`4,588 ms / 1,296 ms` and final-input validation at `971 ms / 333 ms`
(one/default). The store-publication self residual is approximately
`579 ms / 586 ms`; it remains opaque and is not backend flush or lock-wait
time. Individual phase medians are not an additive timeline. No performance
change is selected from these local observations.

The negative normal-benchmark probe used the accurately staged diagnostic
binary. It rejected the successful audit's nonempty stderr and retained one
invalid cell, 33 not-run cells, and seven hashed raw files. Scratch was removed;
no successful report was emitted. This proves diagnostic output cannot become
ordinary numeric evidence and preserves the actual failure prefix.

The optimized public-child delivery-failure case closes the stderr pipe before
launch. Audit still commits exactly one queryable run, stdout contains its
complete ordinary result, diagnostic delivery fails with exit `1`, and repeated
run lookup does not create another run or attempt. Invalid audit input remains
an ordinary failure without a completion frame or initialized state.

Remaining execution authority: run this source through the public Windows
four-worker CI diagnostic and the required clean-checkout matrix. This local
record does not close P1-60/P1-70, approve permanent runtime metrics or the
allocator, or change the `/mnt` decision. The frozen W2 design is unchanged.

## Hosted diagnostic admission

[CI run 34009039891](https://github.com/annyeong844/lumin_Refactor/actions/runs/34009039891)
tested PR head `5914f8f192cfc95aa2b324ad0c8f3ecc265949bb` through merge checkout
`49be5c88a4d9358c4d84a272fa4642ff22a9957c`. Its Linux package and numeric
matrix passed (four-worker cold ratio `0.6588600850998599`). The Windows
ordinary matrix failed only the unchanged `0.75` scaling limit: default
`1,619,004,800 ns`, one worker `1,739,429,700 ns`, ratio
`0.930767595839027`. Both normal archives have 34 completed cells and 711
independently size/hash-verified captures. The Windows report SHA-256 is
`6fc27554431197cfbef8fe0cf5a6f0afbbdd7b1b9d9af086b49e7291cb7653f2`,
and its archive manifest SHA-256 is
`2226e0a5536e7497c5fd6c76ebe9bd115e5c16253d431ca0fb90358913aeed35`.

The separate diagnostic build stopped before Cargo: the hosted source guard
still required `RUNNER_TEMP/lumin-target` for every command, rejecting W2's
reviewed `lumin-audit-diagnostic-target`. No diagnostic frame was produced;
the failed upload is not a complete packet. The bootstrap now admits that
exact separate target only for the three reviewed diagnostic build, focused
test, and incompatible-feature-check commands. Ordinary control/probe/runner
commands retain the ordinary target; crossed, redirected, arbitrary, and
repository-owned targets and shared Cargo homes fail before Cargo admission.

The correction changes only the Python bootstrap, its regression tests, and
this execution record. All 27 bootstrap tests (including seven hosted-target
tests) and 12 CI-policy tests pass on Windows and Linux/WSL2; the 19 locked
xtask Cargo-bootstrap routing tests pass on Windows. The tests exercise the
guard entrypoint with temporary repositories and isolated environment, retaining
real declaration/policy checks and replacing only external tool boundaries.
The hosted failure was reproduced before the correction. Cargo's expected
incompatible-feature failure still propagates unchanged. Numeric criteria,
the reviewed workflow body, and the frozen W2 design are unchanged; the public
four-worker diagnostic still requires a new CI packet.
