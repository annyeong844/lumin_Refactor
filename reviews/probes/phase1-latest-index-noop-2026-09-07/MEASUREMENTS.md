# W5 Local Control/Candidate Comparison

Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
Scope: the exact [W5 design](DESIGN.md), [teardown extension](REJECTED-BACKEND.md),
and [implementation checks](IMPLEMENTATION.md).

## Disposition

All four fresh ordinary matrices completed with valid process/semantic evidence
and passed their local numeric targets. This does not establish a meaningful
cold-audit speedup or resolve the hosted Windows miss. Windows cold-default
median changed from 2,905.1733 ms to 2,896.8209 ms (0.29% lower); Linux changed
from 812.302988 ms to 811.806675 ms (0.06% lower). Other modes moved in both
directions, including a 5.44% higher Linux 32-file post-write median.

These are one three-repetition packet per binary/platform, not confidence
intervals or an isolated measurement of commit/flush cost. Both unchanged
controls already pass locally. The local default-worker policy is 8; the
[failed hosted Windows packet](../phase1-windows-process-observer-2026-09-07/CI-EVIDENCE.md)
uses 4 and retains ratio 0.842191828905966. Do not extrapolate the local results
into hosted acceptance, reinterpret that failure, or change the 0.75 target.

P1-60/P1-70 remain open. No new commit, push, hosted result, allocator approval,
permanent execution-observation approval, or Phase 1 exit is claimed.

## Measurement protocol and identity

The unchanged source-provenance guard and `benchmark foundation` runner used
Rust 1.96.0, Python 3.13.14, seven modes with three measured repetitions each,
and 13 conditioning/setup cells: 34 retained cells per packet. The packets ran
serially in this order: Linux control, Linux candidate, Windows control,
Windows candidate. No build or test ran concurrently with measurement. Memory
headroom was rechecked before measurement; WSL remained running.

The ordinary observer SHA-256 remains
`111976ad514eb539b78b8e4745ea824e99384ddd534cebb696a01bb1b709005a`.
No diagnostic-feature binary or diagnostic-time subtraction is used. Windows
repositories/packages are on NTFS; Linux repositories/packages are on native
WSL2 ext4, not `/mnt`. The host is an i7-9750H with 12 reported logical
processors. The configured/default worker policy is 8 and the source stack
constant is 4 MiB; neither is substituted for the permanent pool-produced
execution observations still required by REVIEW-005. Cold means a fresh
repository/state/process after the frozen conditioning run, not flushed global
OS caches. No global cache flush, numeric waiver, sample exclusion, or
repeat-until-green run was performed.

All packets use the same 780-file, 7,461,511-byte fixture and 256 authored
finding mappings:

- Content manifest SHA-256:
  `9e51b070a934c027e6d2d9a4610fac764592ecb8f05bf41a2ab6f5eb46158d3e`.
- Fixture manifest SHA-256:
  `635240fea1f05e571f47e306b76f9cc307ff46089c2380521e0f349612467cd3`.
- Authored truth SHA-256:
  `1230a9c577fefd0b8df4844c832f26b9e2a33945acd27d867d03132bc36512f0`.
- Full semantic finding-ID mapping SHA-256:
  `93b1b99df26f8b7a5872425cc84f0f2dcb36e5af65f53eb37756e32a8532c523`.

## Executables and source snapshot

Controls are the unchanged, previously retained ordinary packages at
`D:\lumin-w3-control-package-20260906` and
`/home/endof/.cache/lumin-w3-control-package-20260906`; they are not newly built
HEAD executables. The product-source diff from
`a0f5032dfa19dcfc3546f17ff1a4b13e6b7b42d6` to
`d63281bb613a907219f3e78348f29f7c777f102a` is empty for `crates`,
`Cargo.toml`, and `Cargo.lock`. Their executable hashes were rechecked.

Candidates use the explicit revision label
`d63281bb613a907219f3e78348f29f7c777f102a+w5-uncommitted-20260907`.
This label identifies a working-tree candidate, not a Git commit. Both actual
staged platform probes and both adapter probes pass. The Linux candidate is a
stripped x86-64 static-PIE musl executable.

| Executable | Bytes | SHA-256 |
| --- | ---: | --- |
| windows-control | 10,482,176 | `3f09b2edbea7e35c36e5fe1d5186417b2f2a5468a6e72046874ece8acee4d1de` |
| windows-candidate | 10,496,000 | `c2225570626fc311de7ba71998d56aef6ee05f342613e663d47d2210e6a7e953` |
| linux-control | 11,922,160 | `a9befa9370a09bc9150d17b7d4ff22cd8c6b70bb29baf4a5e7fc2a2d2f932e82` |
| linux-candidate | 11,938,544 | `ac08f78d68cd32ed1fa7ec78b2f665402b4487fd9f09e702c0643de751aca00b` |

Control build ID:
`build_7fa2f8378b3ee91199bb6a4f873b85b01ff4c060c566f6859641e6f9114664b2`.
Candidate build ID:
`build_76fc88b7dd33fafdae9224d9a5784b15b23cc378fd0aa1098df3666935aea3db`.
Candidate package-manifest SHA-256 values are
`2aa25fe159c96f4382e15fe9e146171b003a06a8ed9b264a50f717686c5de970`
(Windows) and
`69724403799df766c2c6641ec515897d262e4a50e6c15d330afdff0bf3aa8c0f`
(Linux). Both adapters retain
`a21d74d5d54f34dbaa909c9e7c82f3158b9a31c82493e93cc2ff55cf8bb0e482`.

These are the seven changed Rust files relative to the product base. Other
product source, dependencies, schemas, budgets, and observer bytes are unchanged.

| Source path | SHA-256 |
| --- | --- |
| `crates/application/store/src/publication/latest.rs` | `671f043cbc04501525c0fbcba97c61a6575bea7a9f09448abdbdea534d17f6d6` |
| `crates/application/store/src/namespace/database.rs` | `6b18ed430899ded3cecfffd6d5c8907ff125110714f05b4cf4bea789bd5b3d1d` |
| `crates/application/store/src/namespace.rs` | `acf67065c8e915dcef34bae0b442dc34a9543aa9491283341f37417af9a1267b` |
| `crates/application/store/src/namespace/barrier.rs` | `242e8a8aeb0a31da96b35216c14bec64b34c7a876db943949a3dc2b9ae7feba4` |
| `crates/application/store/src/publication/latest/tests.rs` | `396f0cc3ef04fe1e1ab1d2342c7074bca2435adc1c228ee04c354106a0558d9a` |
| `crates/application/cli/tests/state_namespace_replacement.rs` | `1fc0ee3bb894b93185940d8ed3119e9d5035327ac27ad295ac688a5c76b1dd1f` |
| `crates/application/cli/tests/publication_faults.rs` | `aa2d00196717cfc75226ad3f1739430714db70a029e9e98c4bd7469973a6cb79` |

## Complete numerical comparison

Medians are milliseconds, rounded here to three decimals. Raw reports retain
integer nanoseconds. No slower mode is omitted.

| Mode | Windows control | Windows W5 | Linux control | Linux W5 |
| --- | ---: | ---: | ---: | ---: |
| `cold-audit-default` | 2905.173 | 2896.821 | 812.303 | 811.807 |
| `cold-audit-jobs-1` | 6268.608 | 6321.594 | 1174.395 | 1157.516 |
| `warm-audit-default` | 2339.213 | 2351.978 | 798.786 | 728.315 |
| `cold-pre-write-default` | 3767.730 | 3800.556 | 917.142 | 914.329 |
| `warm-pre-write-default` | 3289.822 | 3205.360 | 853.707 | 827.911 |
| `post-write-one-file-default` | 3816.117 | 3669.470 | 1545.411 | 1510.166 |
| `post-write-32-files-default` | 4114.207 | 4080.263 | 1645.602 | 1735.140 |

| Platform | Control default/jobs1 ratio | W5 default/jobs1 ratio | Maximum |
| --- | ---: | ---: | ---: |
| windows | 0.463447935975 | 0.458242154459 | 0.75 |
| linux | 0.691678015584 | 0.701335232384 | 0.75 |

| Packet | Peak RSS bytes |
| --- | ---: |
| windows-control | 69,464,064 |
| windows-candidate | 69,447,680 |
| linux-control | 143,953,920 |
| linux-candidate | 143,618,048 |

All four reports have an empty `targetMisses` array. Executable sizes remain
below 12,582,912 bytes and peak RSS below 536,870,912 bytes. A passing median or
ratio in this local packet does not prove that every sample or other host meets
the corresponding target.

## Retained raw evidence

Packet roots:

- Windows: `D:\lumin-w5-windows-20260907`.
- Linux: `/home/endof/.cache/lumin-w5-linux-20260907`.

Each root retains `control-report.json`, `candidate-report.json`,
`control-captures/`, `candidate-captures/`, and `candidate-package/`.
Each capture root's terminal `manifest.json` is authoritative for completion;
`inventory.json` intentionally remains the immutable initial planned inventory,
not the final execution status.

| Packet | Report SHA-256 | Terminal capture-manifest SHA-256 | Hashed capture files |
| --- | --- | --- | ---: |
| windows-control | `547c1d37fcff3c26e45556f87fab8a563386d1385384bcb30b18588a944a57bf` | `15d2cbfd035808f7e6ceb5693585a617dd97b04dcf334298961cd3012a6f31b7` | 745 |
| windows-candidate | `d11af5bfeb85de4add6b0ea00869fc135b957f643f05341553b4ab498031e498` | `6210a0a3760acfe2bced23892241a2429d87e26f82737d99b82923981f5a04c2` | 745 |
| linux-control | `2abc92268c431ddf8748aa1c5074f40c3d8a8dac37323f7d922fe671e5bd0ace` | `f1da864c66754467714f5e7468b4ceb02f1ff520a4df6f9ca9e463b5e1a4f2fa` | 711 |
| linux-candidate | `36bb31afdd15054d169046562740b404f80b96bf7d9f0aea38ebd753640d7c3d` | `7eddc5f7a6251325d695c1999a654038e5dbac54f32cb5781e2ae77b2944d96f` | 711 |

Read-only verification compared each actual file inventory with its capture map,
recomputed every listed byte count/SHA-256 (2,912 files total), required all 34
exact planned cells to be completed per packet, and recomputed the seven medians
from each three-sample group. All 84 measured process projections also match their
raw captures; the exact seven-source snapshot and all four executable hashes were
rechecked after measurement. All checks passed. All 136 raw measured/conditioning/
setup process records report success and no analysis children.

The 68 Windows private-job lifetime receipts were also checked for exact method/
schema, helper/product job membership, total process count 1 before and 2 after
launch, active count 1 before and 1..2 after, zero limit flags and terminated
processes, distinct positive process IDs, and ordered creation/exit identities.
The reports' full semantic mapping and observer identities agree across all
four packets. Raw captures are retained even though Cargo/tool-console previews
of the large reports were truncated; previews are not the evidence source.

## Remaining authority

Local correctness and these local packages/measurements do not replace fresh
blocking CI evidence for the candidate. A separately authorized commit/push and
complete hosted matrix would be required to test the Windows four-worker gap.
A larger optimization, permanent metrics, allocator approval, and the `/mnt`
disposition still follow their existing review owners; none is approved here.
