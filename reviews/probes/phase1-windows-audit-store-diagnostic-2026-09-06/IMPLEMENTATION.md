# W3 Implementation and Execution Evidence

Scope: the exact approved [W3 design](DESIGN.md), frozen through its
[review record](REVIEW.md), SHA-256
`6d916f24a3b78e99de8b04e5164e6c0199ea30cae401dea5e1a5e93485025fa8`.
This packet implements diagnostic observation, not a performance optimization.
The ordinary Windows `default / jobs=1 <= 0.75` requirement is unchanged;
P1-60/P1-70 and REVIEW-005's remaining decisions stay open.

## Implementation boundary

The store owns three local recorders, explicitly borrowed through the shared
open, attempt-begin, and publication bodies. The engine merges their owned
scalar results in root order. Recorders are not fields of store, namespace,
guard, database, or attempt-session objects and are not shared with parallel
work. Feature-off calls erase the observer arguments and timing operations.
All existing validation, transaction, recovery, publication, flushing, and
natural destruction remain in their original owner paths.

The non-default `audit-store-test-profile` feature implies W2's feature and
the reviewed model/store/protocol routing. Store and engine reject measured
fault/crash/perturbation combinations. The explicit external test-driver
feature is still `audit-execution-profile-probe`, not either measured feature.
W2's closed v1 frame/command remain separate from W3's closed v2 frame/command.
W3 retains all 23 W2 phases plus exactly 52 store observations; these overlap
and must not be added as separate work. A host parallelism observation error
is retained in the frame and rejected for comparison, not replaced with a
guessed worker count.

```text
cargo build -p lumin-cli --release --features audit-store-test-profile --locked
cargo test -p lumin-model -p lumin-engine -p lumin-store --lib --features audit-store-test-profile audit_ --locked
cargo test -p lumin-cli --test audit_store_diagnostic --features audit-execution-profile-probe --locked
lumin-xtask benchmark foundation --diagnose-cold-audit-store
```

The release child must be separate from the staged ordinary control. The
probe requires `LUMIN_AUDIT_CONTROL_BINARY` and
`LUMIN_AUDIT_DIAGNOSTIC_BINARY`; it never skips missing payloads. The packet
additionally requires the same build/archive environment as W2, with a new
W3 build record, report, and create-new capture root. The build record binds
the exact source, lockfile, payload hashes, empty control feature set, and
`diagnosticFeatures: ["audit-store-test-profile"]`. Its
`diagnosticFeatureClosure` must match the resolved, reviewed five-crate
closure, not just a command substring.

Hosted W3 compilation uses only `RUNNER_TEMP/lumin-audit-store-diagnostic-target`
and its three exact approved commands. W2's exact target map is retained;
ordinary package/probe/runner invocations use `lumin-target`. Normal budget
failure does not suppress eligible diagnostics or uploads, but remains a
failure in both the package job and `Required`. A missing/invalid diagnostic
cell cannot produce a complete summary. Signed per-round differences and
per-phase residuals remain observations, never an adjusted numeric verdict.

## Verification boundary

Local implementation verification is complete on the working tree based on
`062964192a1d3f16f5b7739f0d98eb85ac5bef4d`; no new clean-checkout CI packet
or four-worker W3 measurement is claimed here. The retained
[W2 CI packet](../phase1-windows-audit-execution-diagnostic-2026-09-05/CI-EVIDENCE.md)
remains the evidence for the outstanding Windows numeric failure.

Verified locally with Rust 1.96.0 and Python 3.13.14:

- Windows: all **184 xtask tests**, including the 10 diagnostic runner/decoder
  tests. These exercise independently authored 52-phase ordering/parents,
  root merge refusal, strict version and PID/build binding, retained malformed
  child prefixes, and signed paired comparisons.
- Windows: 12 feature-enabled model/engine/store recorder and audit freshness
  tests, including an independently driven clock and original preflight-error
  handling followed by attempt release. The separate W2 base-feature command
  also passes all eight selected model/engine tests.
- Windows: 380 feature-off library tests (27 model, 20 protocol, 55 engine,
  278 store) pass. The separately compiled publication, publication concurrency,
  publication faults, and state namespace CLI partitions pass all 23 tests;
  their fault features never enter a measured release child.
- Windows and Linux/WSL2: separately built ordinary and W3 **release** children
  each pass the three `audit_store_diagnostic` public probes, including fresh
  `jobs=1`/default truth, concrete pool/build/PID binding, existing-store absent
  bootstrap rows, diagnostic transport failure with one recoverable committed
  run, and original malformed-command failure without state initialization.
- Windows and Linux/WSL2: `package-check stage <target>`, the actual platform
  package check, and `package-check skills` pass for both newly staged ordinary
  distributions. Each uses a separately compiled current `lifecycle-test-fault`
  fixture binary; both adapters are read from the staged package, not substituted
  with the checkout sources during the behavioral probe.
- Windows and Linux/WSL2: 35 source-provenance bootstrap tests and 12 CI-policy
  tests, including exact W2/W3/control target crossovers and actual workflow
  failure/retention conditions.
- Windows: the actual incompatible-feature `cargo check` exits **101** with
  the store owner's `audit-execution-test-profile cannot be combined` diagnostic.
  A bootstrap refusal or a successful check is not accepted as this evidence.
- Windows Clippy with `-D warnings`: default CLI; W3 CLI; model/engine/store
  libraries and tests with W3; the two external diagnostic test targets with
  only the probe feature; and all xtask targets. All Cargo checks are locked
  and use the admitted pinned toolchain with `-j1`.
- `architecture-check`: structural PASS, not a claim of dependency admission.
  Cargo invocations separately pass the source-provenance wrapper. Formatting,
  document links/UTF-8, whitespace, and frozen W2/W3 design hashes pass.

The combined publication/state test command emits existing feature-partitioned
shared-support warnings; the deliberately unsupported feature combination also
emits unused-observer warnings before failing. Neither is a passing Clippy
claim, and no warning suppression or assertion weakening was added.

## Local executable bindings

These are working-tree regression payloads, explicitly built with
`LUMIN_BUILD_REVISION=w3-working-tree-2026-09-06`, not clean-checkout benchmark
provenance. All four report the observed public build ID
`build_7fa2f8378b3ee91199bb6a4f873b85b01ff4c060c566f6859641e6f9114664b2`.
The lockfile remains
`c82640e677c8c602c90f9ee8577a4286d52e65e621636067bf59f8b0257185fb`.

| Payload | Bytes | SHA-256 |
| --- | ---: | --- |
| Windows ordinary | 10,482,176 | `3f09b2edbea7e35c36e5fe1d5186417b2f2a5468a6e72046874ece8acee4d1de` |
| Windows W3 | 10,535,424 | `1ac5d4368e1580ec9560aa1be018e3fb6cb613f3cfd06bafe0c6268f8b6a920d` |
| Linux-musl ordinary | 11,922,160 | `a9befa9370a09bc9150d17b7d4ff22cd8c6b70bb29baf4a5e7fc2a2d2f932e82` |
| Linux-musl W3 | 11,979,504 | `b2de088bf7ba4b7ca1ed5cfa161550b55dc4741f30f95c2075748c45cd863bb3` |

The ordinary distributions are retained outside the checkout at
`D:\lumin-w3-control-package-20260906` and
`/home/endof/.cache/lumin-w3-control-package-20260906`. Each contains its exact
package manifest, binary, and both adapters. W3 release binaries remain in
separate targets:

- `D:\lumin-w3-audit-store-diagnostic-target-20260906\release\lumin.exe`
- `/home/endof/.cache/lumin-w3-audit-store-diagnostic-target-20260906/x86_64-unknown-linux-musl/release/lumin`

Linux release builds explicitly use `--target x86_64-unknown-linux-musl`;
the Linux external probe/tool driver is a separate GNU host build. Product
fixtures and the staged Linux package use native Linux storage, not `/mnt`.
These executions are functional regression checks, not fresh timing/RSS
benchmark evidence. Previously retained W2 payloads and CI archives are unchanged.

## Remaining authority

The actual four-worker public Windows W3 packet remains mandatory before
selecting any optimization. The diagnostic runner requires a clean, exact
source/build binding; no local bypass or fabricated build record was used.
This record is the local pre-push checkpoint; it does not claim a completed
public CI execution for this implementation. Hosted results require their own
exact commit, run, and artifact bindings.
Linux/WSL2 local execution is not a native Linux CI verdict. No diagnostic result
closes the permanent observability, allocator, `/mnt`, or Phase 1 acceptance
decisions. The ordinary Windows budget and red `Required` outcome remain
unchanged until fresh public CI evidence establishes otherwise.

## External Rust advisory bindings

Generated intent was streamed through stdin and artifacts were retained
outside the repository. Both invocations scanned Rust with tests included and
no requested exclusions; the Rust producer reported available and no parse
errors. Their lifecycle-only output makes no base-audit absence claim and
does not reinterpret the explicitly inapplicable TS lanes as Rust evidence.

| Pair | External directory | Invocation-specific pre/post IDs | File result |
| --- | --- | --- | --- |
| Store observation implementation | `D:\lumin-w3-store-diagnostic-gate-20260906` | pre `2026-09-06T09-51-06-382Z-71c12c`; post `2026-09-06T10-15-37-063Z-ec70bc` | 25/25 planned files observed; six new, all planned; none removed |
| Runner/host integration | `D:\lumin-w3-store-integration-gate-20260906` | pre `2026-09-06T10-16-01-375Z-960a71`; post `2026-09-06T10-32-49-604Z-8db44a` | 27/27 planned files observed; no new or removed files |

The advisory JSON filenames contain the exact pre ID; each post-write delta
filename contains both its matching pre ID and post ID. The Python-only
workflow-name test update is verified by the CI-policy tests, not claimed as
Rust scanner coverage.
