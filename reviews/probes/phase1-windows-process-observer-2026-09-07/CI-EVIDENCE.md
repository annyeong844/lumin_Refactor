# W4 Hosted Evidence

Run [34092445560, attempt 1](https://github.com/annyeong844/lumin_Refactor/actions/runs/34092445560)
tests head `d63281bb613a907219f3e78348f29f7c777f102a` at merge checkout
`c38c5e07bc17a974678e6a130eee3aff26be2ea8`. All 57 jobs finished: 55 pass;
Windows package and the aggregate Required job fail. The Windows package's
only failed step is the ordinary benchmark. Both release builds, actual staged
binary/adapter probes, all other test lanes, and the isolated Windows diagnostic
pass. No rerun or numeric-target change was used.

## Blocking measurements

| Observation | Windows NTFS | Native Linux |
| --- | ---: | ---: |
| Complete capture cells | 34 / 34 | 34 / 34 |
| Cold audit default median, ns | 2,116,195,200 | 324,978,176 |
| Cold audit jobs=1 median, ns | 2,512,723,500 | 470,129,678 |
| Default / jobs=1, maximum 0.75 | **0.842191828905966, FAIL** | **0.691252203822793, PASS** |
| Other time, RSS, executable-size budgets | PASS | PASS |

The full authored semantic oracle passes in both matrices. These are distinct
hosted observations, not proof that product performance improved: W4 changes
only the Windows measurement helper. The earlier invalid Windows observation
and earlier Linux scaling miss remain retained under their original identities.

Windows executes all 11 observer tests without skips. Each of the 34 ordinary
and 14 separate diagnostic cells retains a complete private-job companion:
before total 1, after total 2, both limit flags zero, bound helper/product
memberships, and ordered process lifetimes. The changed observer source SHA-256
is `111976ad514eb539b78b8e4745ea824e99384ddd534cebb696a01bb1b709005a`
in both reports. This run has no invalid observation cell; its Windows budget
failure is a measured scaling miss, not the W3 packet's PID-ancestry rejection.

## Retained packet identities

| Artifact member | SHA-256 |
| --- | --- |
| Windows ordinary report | `7be23c04b6ea9c405019365c177fd2bfecfc079b36072f543f4483b77dc44cb9` |
| Windows ordinary manifest | `693b0b7cc452c1d606a6464a1577e65a7e7d228e9f408fdf18f96a0801b33c2b` |
| Linux ordinary report | `6113da1627d3026dcd4cbb4701717d19ea3f542ef2395df010b3a9fe26f31b71` |
| Linux ordinary manifest | `f85aab00dd31507a2872bcb61ff45e5b6525c3e1a23f8ec120615a1a1936678b` |
| Windows store-diagnostic report | `e224416abbc4cd0dd26549179fcd4c9f898c4d202ef98149c61a48cbdba66fd2` |
| Windows store-diagnostic manifest | `1c2ad14768d94e5eeb97233795376ad87ae861c46e3e447296c6f1cad1e8cdf5` |

Downloaded ordinary Windows/Linux and diagnostic inventories match all 745,
711, and 379 recorded file sizes and SHA-256 values, respectively, with no
unrecorded files except each enclosing manifest. Raw retrieval hints are
`D:\lumin-w4-ci-34092445560\windows`, `\linux`, and `\diagnostic` beneath
that same root. GitHub artifact names are `lumin-foundation-benchmark-windows-x64`,
`lumin-foundation-benchmark-linux-x64`, and
`lumin-audit-store-diagnostic-windows-x64`.

## Diagnostic boundary and next decision

All 14 W3 diagnostic cells complete with the original 23/52 phase inventories.
Its status remains `DIAGNOSTIC_ONLY`; it cannot substitute for either blocking
matrix. Default-worker diagnostic process times are 3,373.0453, 3,520.9225,
and 3,021.7507 ms. Large self-time intervals vary among bootstrap, lock-wrapper
validation, pointer publication, and lease release; they are not a stable
single-syscall attribution. Mixed-sign control/diagnostic round differences
remain visible. Neither the backend flush cost nor a product speedup is proven.

The narrow next candidate is [unchanged latest-index synchronization](../phase1-latest-index-noop-2026-09-07/DESIGN.md).
It does not remove validation or broaden backend-handle lifetime. Windows
scaling, permanent run/gate metrics, allocator approval, and the `/mnt` diagnostic
disposition remain open; P1-60/P1-70 do not close from this packet.
