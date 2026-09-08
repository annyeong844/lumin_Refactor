# W6 Windows Four-Logical-CPU Comparison

Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
Scope: the separately declared comparison allowed by the frozen
[W6 design](DESIGN.md), following the retained ordinary
[eight-worker packets](MEASUREMENTS.md).

Status: **comparison rejected: the control did not obtain the declared worker
topology**. Resource admission succeeded, but the report observed 12 available
processors and eight default workers despite the launcher's four-CPU affinity.
The candidate was not run. All control evidence is retained; its numeric PASS
does not constitute four-worker evidence or Phase 1 acceptance.

## Question and fixed inputs

Compare the unchanged W5 control with the already-built W6 candidate while
exposing four logical processors to both launch trees. Do not rebuild either
package or substitute a feature-enabled diagnostic binary. Use these exact
retained Windows packages and require their executable hashes before each run:

| Packet | Package root | Executable SHA-256 |
| --- | --- | --- |
| control | `D:\lumin-w5-windows-20260907\candidate-package` | `c2225570626fc311de7ba71998d56aef6ee05f342613e663d47d2210e6a7e953` |
| candidate | `D:\lumin-w6-windows-20260908\candidate-package` | `ab08c698ba0c77a1f77e03b6d1f6b4cf187a25e76a1e15221cd60e6b58f34b40` |

Both hashes were rechecked during preparation. The source base remains
`95163f586f2d9ea9dcae7f356d94e10469ec84e9` plus the unchanged, uncommitted W6
implementation. Package/build, fixture, complete semantic truth, observer, and
14-file Rust-source identities remain those in MEASUREMENTS.md. The existing
Windows `lumin-xtask.exe` hash is
`4e1bcfb8809511ab681c4950c46a4400e6e3e2bad9d70e16f711451618a8b04a`.
The source-provenance bootstrap and locked Cargo command remain the invocation
route; any required rebuild must finish outside measurement and be recorded.

## Declared topology and launch order

Use Windows process affinity `0x000F` (logical processors 0 through 3) on the
dedicated launcher **before** starting the bootstrap, Cargo, or xtask. Restore
that launcher's previous mask in `finally`; do not alter any existing user,
system, or WSL process. Microsoft documents
[child-process affinity inheritance](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-setprocessaffinitymask).
The unchanged process observer creates its product child without changing
affinity, and its private job continues to require zero limit flags.

A lightweight launch preflight succeeded: the pinned Python child reported
process mask `15`, system mask `4095`, four allowed logical processors, and 12
host logical processors. This checks inherited OS eligibility, not an observed
engine pool size. Do not label four logical processors as four physical cores.

Run exactly two fresh ordinary packets serially, **control then candidate**.
Use all seven modes, the existing three measured repetitions, all 13
conditioning/setup cells, the complete authored 256-finding truth, and the
ordinary Windows process-lifetime observer. Keep default CLI invocations
unchanged; do not replace default mode with an explicit `--jobs 4`. The only
explicit worker argument remains the owner's `--jobs 1` reference mode.

The runner must report `observedAvailableParallelism: 4` and `defaultJobs: 4`.
Any disagreement invalidates the topology comparison, not a reason to relabel
or silently retry it. Those policy-derived fields still do not close the
permanent pool-produced runtime-observation gap in REVIEW-005.

## Resource admission and retained outputs

Before each packet, require at least 4 GiB of free host RAM and no concurrent
build, test, or benchmark. Check that the observed Windows servicing activity
has settled. This is a local launch-admission guard, not a changed product RSS
budget. Do not terminate user/system processes, flush machine-global caches,
shut down WSL, raise priority, or change global CPU settings to meet it.

Retain results under the separate, previously unused root
`D:\lumin-w6-four-worker-windows-20260908`, with `control-report.json`,
`candidate-report.json`, and corresponding `*-captures` directories. Never
overwrite the ordinary eight-worker packets or reuse an existing capture root.
Capture the exact command, exit status, source/package hashes, launcher mask,
and pre/post memory observations outside the timed interval. Preserve an
incomplete packet, resource interference, semantic failure, or numeric miss as
observed; do not rerun until green or discard samples after seeing their time.

For each packet, set `PINNED_PYTHON` to
`D:/lumin-phase1-python-store-copy-20260905/python.exe`, `PINNED_CARGO` to the
external Rust 1.96.0 Windows Cargo executable, `CARGO_BUILD_JOBS=1`, and
`CARGO_TARGET_DIR=D:/lumin-phase1-validation-target-20260905`. Set
`LUMIN_PACKAGE_ROOT`, `LUMIN_BENCHMARK_REPORT`, and
`LUMIN_BENCHMARK_CAPTURE_ROOT` to that packet's exact paths above. From the
dedicated performance worktree, inside the already-constrained launcher, run:

```powershell
& $env:PINNED_PYTHON -I -S tools/xtask/bootstrap/source_provenance.py -- cargo run -p lumin-xtask --locked -- benchmark foundation
```

Require all 34 completed cells, exact archive inventory/lengths/SHA-256 values,
all ordinary lifetime receipts, the complete stable finding mapping, exact
report/raw process agreement, and independently recomputed medians, RSS and
ratio. Compare every mode and retain regressions as well as improvements.
Numeric targets, cold-cache definition and the existing observer remain
unchanged. No new Rust edit or external Rust advisory is needed for this
measurement-only action.

## Observed control and rejected comparison

The exact pre-run declaration is retained as `declaration-before-run.md` under
the packet root, SHA-256
`bef8be6b9228b5828e38a1db432c754ab0f41a01f688c06d0ac59025ba36b0de`.
After the user closed applications, resource admission succeeded with
7,375,122,432 free bytes; after the control it was 7,264,051,200 bytes. The
servicing process was absent, and no other Cargo, rustc, lumin or xtask process
was active at launch. The existing xtask executable did not rebuild or change.

The control ran from `2026-09-08T05:23:47.0185295Z` to
`2026-09-08T05:28:18.7970009Z`. The launcher set and read back mask `15` before
starting the bootstrap and restored its original `4095` mask afterward.
Benchmark exit status was `0`, with all 34 cells complete and numeric status
`PASS`; the separate topology guard then correctly exited `1` because
`observedAvailableParallelism` was `12` and `defaultJobs` was `8`, not `4`.
No candidate packet, retry, or replacement measurement was launched.

The control's cold-default median is `2,825,044,100 ns`, jobs=1 median is
`6,296,216,000 ns`, and ratio is `0.4486891968128158`. Peak RSS is
`68,616,192 bytes`. These are retained observations of this mismatched launch,
not a new four-worker result or an unconstrained eight-worker comparison.
Do not pool them with the earlier ordinary packets.

## Confirmed mechanism

The pinned, installed Rust 1.96.0 source at
`share/doc/rust/html/src/std/sys/thread/windows.rs.html`, lines 79-88, implements
`available_parallelism` with `GetSystemInfo().dwNumberOfProcessors`. It does not
query the process affinity mask. The source HTML SHA-256 is
`cf95740d93745958dc5dc14f1dfdc7b83928c814023de5a368b1b3cc52b25a97`.
The [published Rust API limitations](https://doc.rust-lang.org/std/thread/fn.available_parallelism.html#limitations)
also warn about overcounting under Windows process-affinity or job restrictions;
the installed 1.96.0 source, not the current documentation version, identifies
the implementation used here.

A separate lightweight native API diagnostic launches no product. Under the
same mask `15`, `GetProcessAffinityMask` reports four eligible logical CPUs
while `GetSystemInfo` still reports 12, with system mask `4095`. It restores its
original mask. This matches Microsoft's definition of
[the system processor count](https://learn.microsoft.com/en-us/windows/win32/api/sysinfoapi/ns-sysinfoapi-system_info)
and explains why the affinity-only preflight was insufficient. Restricting OS
eligibility did not change this toolchain's default-worker decision.

## Retained verification and decision

`verify-retained-control.ps1` independently rechecks the exact 34-cell order,
all 745 capture files and their lengths/hashes, all 34 ordinary lifetime
receipts, complete semantic mappings, unchanged default CLI arguments,
report/raw agreement, seven medians, peak RSS and the ratio. Its result
explicitly distinguishes verified raw integrity from `declaredTopology:
INVALID`; it does not override the topology guard's failure.

All paths below are relative to the separate packet root:

| Evidence | SHA-256 |
| --- | --- |
| `control-report.json` | `b3861d2717bd47d223e9c2cb6bdc2d065961b036eb55fa1a9eef5151ef2b4545` |
| `control-captures/manifest.json` | `93210e3e30a23c3180a87561ac7b4301a297869c1eab5a853ac6677d17d67680` |
| `control-execution.json` | `cc63a3f121249b22a7ac119fb5aa0560a1150de01e3c22e3290adaf62d6e6fe6` |
| `raw-verification.json` | `1efbc3d80bca16e907d44c57305bf88791e338310269b62a834cb0a702c59d26` |
| `affinity-observation.json` | `5699ac3cf399feb33bb9e72e50caec5ff0048474072d0553baa3349d3d676e5e` |

The exact launcher is retained as `run-packet.ps1`
(`ac68eae980ee4c2f748eda3937c60d29b06c9a11fa4b1e5e8670d282d8c7b3dc`),
the native API probe as `inspect-windows-affinity.py`
(`0955520b717f8ca9bef31d4f3c41bad1f9abd8ffa34ba2b7368fa20c2a5897c9`),
and the read-only verifier as `verify-retained-control.ps1`
(`4327e227d6afdda6387b0f7105db0f5c60f4c152535567e18e1e0fb0cd0243df`).

The original user worktree, retained packages, ordinary reports and all 14
changed Rust-source hashes remain unchanged. This turn adds only measurement
records and owner-document corrections; no product fix is inferred from the
launcher limitation. Do not change worker policy, inject an explicit default
`--jobs 4`, create a VM, or alter host CPU configuration to rescue this packet.
A real four-processor Windows environment or a separately approved new
measurement protocol is needed before resuming the comparison. Actual hosted
CI remains the recommended authority. The subsequent
[publication approval](REVIEW.md#w6-publication-and-hosted-verification)
authorizes committing/pushing the verified W6 change and inspecting its new
hosted run; it does not reopen this rejected local comparison.

Closeout checks pass for all 83 live Markdown documents and diff whitespace.
All 14 changed Rust-source hashes and the two retained Windows binaries/ordinary
reports are unchanged. No product test matrix was rerun for this measurement-
and-document-only change; the earlier exact-source checks remain separately
identified in IMPLEMENTATION.md. No Cargo, rustc, lumin or xtask process remains
active after closeout.

A local affinity-limited pass or miss cannot reproduce the hosted Windows
Server machine's CPU, physical-core layout, storage, memory pressure, or OS.
Compare only these two explicitly constrained local packets. Do not pool them
with eight-worker medians, attribute all timing changes to W6, or claim the
hosted `0.8692717421027233` miss is fixed. This packet itself authorizes no
Linux rerun, CI trigger, commit, push, worker-policy change, or budget amendment;
the later publication approval above is separate.
P1-60/P1-70 remain open.
