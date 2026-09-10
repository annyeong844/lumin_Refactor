# W8 Local Control/Candidate Comparison

Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
Scope: the frozen [design](DESIGN.md) and [implementation](IMPLEMENTATION.md).

## Declared protocol

Status: the predeclared four-packet protocol is complete. All local numeric and
integrity verdicts pass; the mixed timings do not establish a causal speedup.

After correctness and staged-package verification, the declared protocol runs
four fresh ordinary seven-mode packets serially: Windows control, Windows candidate, native Linux
control, native Linux candidate. Keep the existing three measured repetitions,
13 conditioning/setup cells, complete fixture truth and process observer.
Use the unchanged default-worker policy, no diagnostic-time subtraction,
budget waiver, selective sample removal or repeat-until-green run. Preserve
every raw capture, numerical miss and setup failure. Check memory before each
packet; do not run builds, tests or another packet during measurement.

The user's browsers and other in-use applications stay open. This is a local
interactive-machine comparison, not a quiescent-host measurement. Record that
background-activity limitation with the results; do not close user applications
or selectively rerun slower samples to remove it.

Controls are the retained ordinary W7 release packages, copied/staged before
this change rather than mutable release-target paths. Their bytes and package
manifests were rechecked before candidate builds:

| Platform | Control package | Binary SHA-256 |
| --- | --- | --- |
| Windows | `D:/lumin-w7-control-package-20260908` | `df2b768f27deb288c9aaa4b93ac2c569de3a3912e85db4a44b7ebf40da603fb5` |
| Native Linux | `/home/endof/.cache/lumin-w7-control-package-20260908` | `3e2de41a9f6f11fdc9177b720442823f11e27849f31d214c46dd701c9b3faf5f` |

Both control packages report build ID
`build_c9b893c216f043635a8f0f77161e4ea7752b14c5f70f65aca48b7f2acaabb68b`.
Linux is the actual static musl release shape. Candidate ordinary and W7
diagnostic builds use separate targets and the explicit source label
`4f03cc1f8a25e83265a81c0fb1b2c604ce6ee8bf+w8-uncommitted-20260909`;
this label does not claim a new Git commit. Stage the ordinary binary and both
adapters through the canonical package command. Feature-enabled fixture and
diagnostic binaries are never the timed ordinary product.

Windows packet paths are retained under
`D:/lumin-w8-empty-latest-20260909/measurements/windows/`; Linux packets and
runtime fixtures use `/home/endof/.cache/lumin-w8-empty-latest-20260909/`,
not the `/mnt` checkout. Cold retains the owner's fresh process/repository/state
definition after conditioning, not a flushed machine-global filesystem cache.

Local worker topology is not the hosted four-worker configuration. This
comparison cannot close either hosted scaling miss or P1-60/P1-70. W7 is only
explanatory count evidence; ordinary matrices and separately authorized public
CI retain their own authority.

## Verified candidate artifacts

Before timing, actual staged platform and adapter checks pass on both platforms.
Separate W7 releases pass all three external release-child tests per platform,
including fresh/healthy/pending exact counts, durable recovery, and diagnostic
transport/failure behavior. Their driver Clippy passes with warnings denied.
This does not claim a new clean-checkout fourteen-cell W7 diagnostic packet.

Both ordinary candidate package manifests bind build ID
`build_6e792ba4ccba6538d45ef923d02a363b43429bc881d24d014262644cd8009bb8`.
The staged adapters are unchanged: each is 2,869 bytes with SHA-256
`a21d74d5d54f34dbaa909c9e7c82f3158b9a31c82493e93cc2ff55cf8bb0e482`.

| Platform / role | Bytes | SHA-256 |
| --- | ---: | --- |
| Windows ordinary candidate | 10,505,728 | `8e1f06b8f6685e1c6b309f0bfc2b8f1ba7f93d71a7ee16f684e1f65a979b74ce` |
| Windows W7 diagnostic | 10,589,696 | `154c6aafc432747072728ebe941b320d56bdadfdb334c4d8713de98a4c333e6e` |
| Native Linux ordinary candidate | 11,946,736 | `254a3a40278916e2f7917b8303ada9c45649c67a2a9239efee31c5287d2383aa` |
| Native Linux W7 diagnostic | 12,036,848 | `b29f1352fe853dc511f3e3e1a421a7625a22f7fcf0a827cf2e7e21dedfba47af` |

Both Linux release objects are stripped x86-64 static-pie ELF payloads built
for `x86_64-unknown-linux-musl`. The copied fixture binaries are supporting
tools, never timed products: Windows 34,914,304 bytes / SHA-256
`20f774010a948dd5a7688e94a033c2bb143eae26904cfaf634c5dcb7301cb03b`;
Linux 211,525,824 bytes / SHA-256
`877c92179ae93cd35793594068992c4f773ccde47d353bb84018850561097322`.

## Results and interpretation

Each packet ran once, in the declared order. All four report `PASS` with no
numeric target misses. There are 84 measured samples and 136 total cells:
each packet has 21 measured samples plus 13 conditioning/setup cells. Windows
retains 745 hashed captures per packet and Linux 711, totaling 2,912. Every
capture's size and SHA-256, the complete cell/sample order, package bindings,
and summary medians/ratio/peak were checked. No setup failure or numeric miss
occurred in these four packets, and none was selectively repeated.

All four full 256-entry finding mappings are identical, not merely equal in
count, with semantic SHA-256
`93b1b99df26f8b7a5872425cc84f0f2dcb36e5af65f53eb37756e32a8532c523`.
The frozen fixture is 780 files / 7,461,511 bytes, with manifest SHA-256
`635240fea1f05e571f47e306b76f9cc307ff46089c2380521e0f349612467cd3`
and authored truth SHA-256
`1230a9c577fefd0b8df4844c832f26b9e2a33945acd27d867d03132bc36512f0`.
The process observer remains SHA-256
`111976ad514eb539b78b8e4745ea824e99384ddd534cebb696a01bb1b709005a`
with Python 3.13.14 on both platforms. Each reports twelve available logical
processors, the unchanged eight-worker default and 4,194,304-byte worker stack.

Medians below are milliseconds, rounded to three decimals; retained reports
contain every unrounded nanosecond sample.

| Mode | Windows control | Windows candidate | Linux control | Linux candidate |
| --- | ---: | ---: | ---: | ---: |
| Cold audit, default | 2,922.607 | 3,008.714 | 784.141 | 782.565 |
| Cold audit, jobs=1 | 5,948.881 | 5,874.262 | 1,107.397 | 1,221.369 |
| Warm audit, default | 2,423.964 | 2,354.252 | 704.686 | 718.079 |
| Cold pre-write, default | 3,752.172 | 3,749.428 | 945.909 | 912.118 |
| Warm pre-write, default | 3,219.835 | 3,378.810 | 832.124 | 854.623 |
| Post-write, one file | 3,602.256 | 3,665.823 | 1,534.929 | 1,541.910 |
| Post-write, 32 files | 4,053.183 | 3,945.015 | 1,679.696 | 1,684.523 |

| Packet | Default/jobs=1 ratio | Peak RSS (bytes) | Ordinary binary (bytes) | Verdict |
| --- | ---: | ---: | ---: | --- |
| Windows control | 0.4912868493226306 | 69,554,176 | 10,501,120 | PASS |
| Windows candidate | 0.5121858466804721 | 69,865,472 | 10,505,728 | PASS |
| Linux control | 0.7080937949105618 | 138,334,208 | 11,942,640 | PASS |
| Linux candidate | 0.6407281203597966 | 137,863,168 | 11,946,736 | PASS |

W8's selected fresh backend-open count decreases from 17 to 15, but that is
not a measured whole-audit speedup. Windows default cold audit is 2.95% slower
in this packet; native Linux is 0.20% faster. Linux's improved scaling ratio
mainly reflects its 10.29% slower jobs=1 median, not a comparable reduction in
default latency. Other modes also move in both directions. These are three
repetitions on an interactive machine with user applications retained, not
statistical or causal attribution. The actual hosted four-worker failures
remain separate evidence; neither P1-60 nor P1-70 closes here. Further
optimization or publication requires its own authority.

## Retained packet bindings

Windows reports/captures live below the Windows measurement root declared
above; Linux reports/captures live in its native `measurements/` directory.
Each uses `<variant>-report.json` and `<variant>-captures/manifest.json`.

| Packet | Report SHA-256 | Capture-manifest SHA-256 |
| --- | --- | --- |
| Windows control | `16173ddfb9a763ec4e16d4036c88f17155d3258ea98197773b297cccda745800` | `bc3605352220216896b91b92153fee73e0fc664648ca039af33d13963afd1ff9` |
| Windows candidate | `ba65a70611f15c30eda92788fcd75e1d9210cb92d28144a5650310350722e430` | `17d9de2a27f79f89a314e220df4e7ca6019c7d12004b136c73aea11a89376783` |
| Linux control | `9b6ed32d322e6174e82dc9a4a8a881f134391e13cea049df21ed51188cbf1587` | `92587acaec46379b2e54397760687834ea63e444152aab1454fe333fa4d851e8` |
| Linux candidate | `206b44d0510efe9d5c43642376eea093bf20a999ca74ff7f07409ff1f7f7577a` | `00d0a4d8347f75649ec5f15acea9b8ef82b883a20fef7b6739c71c23f5720633` |

Separate local recomputation is retained in the external evidence root's
`verify-packet.ps1`, four `*-verification.json` outputs, `compare-packets.ps1`
and `measurement-comparison.json`. These check the original benchmark reports;
their integrity PASS is distinct from the reports' numeric PASS. Terminal
tool-display truncation does not truncate these retained reports or captures.
