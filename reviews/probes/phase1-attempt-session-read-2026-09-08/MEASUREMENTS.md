# W6 Local Control/Candidate Comparison

Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
Scope: the frozen [design](DESIGN.md), [amendment](FAILURE-CONTINUATION.md),
and [implementation evidence](IMPLEMENTATION.md).

## Disposition

All four fresh ordinary matrices complete with valid process/semantic evidence
and pass their local numeric targets. Cold-default medians decrease 2.83% on
Windows and 1.65% on native Linux, while other modes move in both directions.
One three-repetition packet per binary/platform is not a confidence interval or
proof that W6 caused every observed change. No material scaling improvement or
hosted-budget fix is established.

Both platforms' focused correctness and actual-package/adapter checks pass,
as do all three affected Windows corpus modes. P1-60/P1-70 and hosted Windows
scaling remain open; no commit, push or CI rerun was performed.

## Declared comparison protocol

Four fresh ordinary seven-mode packets ran serially in the declared order:
Windows control, Windows candidate, native-Linux control, native-Linux candidate.
Each kept the existing
three measured repetitions, 13 conditioning/setup cells, complete fixture truth,
ordinary process observer and unmodified default-worker policy. All samples and
misses are retained. No build, test or other comparison ran concurrently with
measurement. Memory headroom was checked before each packet. Cold is the owner's fresh-process/state
definition after conditioning, not a globally cold filesystem cache.

The source base is `95163f586f2d9ea9dcae7f356d94e10469ec84e9`. Ordinary candidate
builds use the explicit label
`95163f586f2d9ea9dcae7f356d94e10469ec84e9+w6-uncommitted-20260908`, not a claimed
Git commit. Actual release executables and both adapters were staged with the canonical
`package-check stage` command; feature-enabled test binaries are fixture helpers
only and must not become timed products.

Controls are the retained ordinary W5 candidate packages, freshly remeasured:

| Platform | Retained package | Binary SHA-256 |
| --- | --- | --- |
| Windows | `D:\lumin-w5-windows-20260907\candidate-package` | `c2225570626fc311de7ba71998d56aef6ee05f342613e663d47d2210e6a7e953` |
| Native Linux | `/home/endof/.cache/lumin-w5-linux-20260907/candidate-package` | `ac08f78d68cd32ed1fa7ec78b2f665402b4487fd9f09e702c0643de751aca00b` |

Both controls have build ID
`build_76fc88b7dd33fafdae9224d9a5784b15b23cc378fd0aa1098df3666935aea3db`.
Their hashes were rechecked before candidate builds. The committed `crates`
diff from W5 implementation `4ec0af7` to the source base contains only three CLI
test files, not runtime changes. Controls are independent staged copies, not
release-target paths that the new build can replace.

The unchanged observer SHA-256 is
`111976ad514eb539b78b8e4745ea824e99384ddd534cebb696a01bb1b709005a`.
Retained packet roots are `D:\lumin-w6-windows-20260908` and
`/home/endof/.cache/lumin-w6-linux-20260908`; Linux measured repositories use
native ext4, never the `/mnt` source checkout. No diagnostic-time subtraction,
worker-policy change, numeric waiver or repeat-until-green run is permitted.

Local default-worker comparisons do not reproduce the hosted four-worker
topology or replace new blocking CI evidence. Any separate four-worker probe
requires its own declared packet and must retain the ordinary host-policy run.

## Results

These are medians in milliseconds; changes are candidate/control minus one.
Negative changes mean lower elapsed time. All three individual samples remain
in the reports and the verification JSON; no sample is excluded.

| Mode | Windows control | Windows candidate | Change | Linux control | Linux candidate | Change |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `cold-audit-default` | 2824.827600 | 2744.778000 | -2.83% | 790.032177 | 776.959440 | -1.65% |
| `cold-audit-jobs-1` | 6309.077500 | 6367.851000 | +0.93% | 1112.261132 | 1079.670696 | -2.93% |
| `warm-audit-default` | 2193.767900 | 2156.756400 | -1.69% | 730.920043 | 688.711779 | -5.77% |
| `cold-pre-write-default` | 4191.865700 | 3901.053000 | -6.94% | 850.983393 | 851.961961 | +0.11% |
| `warm-pre-write-default` | 3474.475700 | 3323.301600 | -4.35% | 827.349210 | 764.999279 | -7.54% |
| `post-write-one-file-default` | 3748.644800 | 3867.833000 | +3.18% | 1536.749444 | 1471.182277 | -4.27% |
| `post-write-32-files-default` | 4509.278900 | 4129.997900 | -8.41% | 1639.799342 | 1671.048410 | +1.91% |

The single-file Windows post-write median increases 3.18%; the Linux 32-file
post-write median increases 1.91%. Pre/post changes are not attributed to the
attempt-session read. The source-proven reduction in backend lifetimes is not
an isolated measurement of their destruction, allocator commit or flush cost.

| Packet | Peak RSS bytes | Default/jobs=1 cold ratio | Local targets |
| --- | ---: | ---: | --- |
| windows-control | 70,041,600 | 0.44774019656597974 | PASS; no target misses |
| windows-candidate | 70,127,616 | 0.4310367814824813 | PASS; no target misses |
| linux-control | 142,557,184 | 0.7102937918718893 | PASS; no target misses |
| linux-candidate | 146,333,696 | 0.7196263109469445 | PASS; no target misses |

Both unchanged controls already pass locally. These runs use the host's
eight-worker policy, not the four-worker hosted Windows topology that still
fails at `0.8692717421027233` against `0.75`. The reported worker count and
4-MiB stack configuration do not replace the permanent pool-produced runtime
observations still open in REVIEW-005. No four-worker W6 packet was run.

## Executables and source identity

Both ordinary candidate packages have build ID
`build_6607f0c36068f0cb02af47f2551450517eaa2ad65350da0c9333c1452fa259c0`.
The Linux candidate is a stripped static-PIE x86-64 musl executable.

| Executable | Bytes | SHA-256 |
| --- | ---: | --- |
| windows-control | 10,496,000 | `c2225570626fc311de7ba71998d56aef6ee05f342613e663d47d2210e6a7e953` |
| windows-candidate | 10,497,536 | `ab08c698ba0c77a1f77e03b6d1f6b4cf187a25e76a1e15221cd60e6b58f34b40` |
| linux-control | 11,938,544 | `ac08f78d68cd32ed1fa7ec78b2f665402b4487fd9f09e702c0643de751aca00b` |
| linux-candidate | 11,938,544 | `1e0648bd7ddea8db415e85ac361ca3f1b6aba40d93a15d11abe171e120107c8a` |

Candidate package-manifest SHA-256 values are
`02405a02710a7e31c7464c847c3add4e89ec3535500a8f56ce53f6480f7df052` (Windows)
and `434b2511432a3ab626d9f6af30a6ea630880fe118721e24eee883005a691c259` (Linux).
Both adapters retain
`a21d74d5d54f34dbaa909c9e7c82f3158b9a31c82493e93cc2ff55cf8bb0e482`.
The complete 14-file changed-Rust inventory is retained as
`D:\lumin-w6-attempt-session-continuation-20260908\candidate-rust-source-sha256.txt`,
SHA-256 `01a6bd067a217abead4070787a8defb7701052354f886f9fbda6ab754743d8e0`.
The original user worktree and retained controls were not edited.

## Raw packet verification

Each packet has `control-report.json` or `candidate-report.json` and the
matching `*-captures` directory beneath its platform root. The final capture
manifest has exactly 34 completed cells. All 2,912 capture files across the four
packets were checked for the exact inventory, byte length and SHA-256; no
truncated console output is used as the complete report.

| Packet | Capture files | Report SHA-256 | Capture-manifest SHA-256 |
| --- | ---: | --- | --- |
| windows-control | 745 | `c390015815236e429f8de74ed021ca7ba0646556eb3a3450840783f0499d1405` | `b9e711a32a814af71e35845fe023b1361b81a6109fd587161fa7bcf16d3bd6dd` |
| windows-candidate | 745 | `b3849dabae1381d3848ed938df3caf7497c9e3bfa9ab1f7e21def74df30e8d49` | `df42744c77c790cf1481531f27fe2543086c8a5b2d07f32bd2523d04b5102869` |
| linux-control | 711 | `5a1f0c25d99b98e719c629a137fc567476e1dee7d143a073fdba6f647b75b8e0` | `9866aac47d2f05d4db8015a705ab6f8e5afee06dbfce909a8e028077d87ec09d` |
| linux-candidate | 711 | `5494973d810c7e2a1f182edd2b74c38715f9840f88c9bcee2845a424ce8b2032` | `c46adc3cebb2b431119414ab9620abc90e822050ee0cad31543dc12f170c051d` |

The read-only verifier and its complete summary are retained at
`D:\lumin-w6-attempt-session-continuation-20260908\verify-measurements.ps1` and
`measurement-verification.json`. It checks the fixed cell order, all 136
ordinary process records and complete 256-finding semantic mappings, exact
report/raw process agreement, recomputed medians and peak RSS, and the actual
binary hashes. Console rendering was truncated by the tool display; the named
full JSON reports and hashed captures are the authoritative local evidence.

The fixture remains 780 files / 7,461,511 bytes, with the frozen content manifest
`9e51b070a934c027e6d2d9a4610fac764592ecb8f05bf41a2ab6f5eb46158d3e`,
fixture manifest
`635240fea1f05e571f47e306b76f9cc307ff46089c2380521e0f349612467cd3`,
authored truth
`1230a9c577fefd0b8df4844c832f26b9e2a33945acd27d867d03132bc36512f0`,
and complete finding-ID map
`93b1b99df26f8b7a5872425cc84f0f2dcb36e5af65f53eb37756e32a8532c523`.

No global cache flush, user-process termination, diagnostic subtraction or
repeat-until-green run occurred. Windows free memory before the four packets
was respectively 5.07, 4.78, 4.78 and 5.02 GiB; Linux had about 7.5 GiB available
before each Linux packet. WSL was not explicitly stopped. The pinned toolchain
is Rust 1.96.0 / Python 3.13.14, on the 12-logical-processor i7-9750H host.

## Decision boundary

Stop at this bounded candidate evidence. The small cold-median reductions and
mixed controls do not establish a material causal speedup or a hosted-budget
fix. Any next optimization needs its own bounded owner decision; a separate
four-worker comparison or new blocking CI must retain these ordinary packets.
There is no authority here to enlarge W6, change budgets, commit, push or rerun
CI. P1-60/P1-70 remain open.
