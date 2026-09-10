# W4 Local Implementation Evidence

Scope: [frozen W4](DESIGN.md), with [design reviews](REVIEW.md), based on
`a0f5032dfa19dcfc3546f17ff1a4b13e6b7b42d6`. This working-tree checkpoint
corrects the Windows observer only; it is not a new numeric benchmark packet.
No product crate, dependency, lockfile, schema, scheduler, or budget was changed.

## Implemented boundary

The disposable Python helper establishes a fresh private inherited job before
the direct product launch. It binds held-handle creation/exit times, membership,
actual limit flags, and cumulative counts, without global PID ancestry. Normal
exits between RSS polls remain counted. Observation failures propagate, including
thread failures and the mandatory post-exit RSS query. Failure cleanup requests
termination of that private job only; it does not claim completed cleanup.

The existing v1/v2 measurement envelopes remain unchanged. Windows captures add
the exact `lumin.windows-process-observation.v1` companion; xtask independently
binds the launcher PID and rejects noncanonical, opaque, partial, mistyped,
contradictory, or wrong-platform companions. Diagnostic PID binding still reaches
the unchanged W2/W3 frames. Raw companion bytes and hashes survive archive cleanup.

Observer source SHA-256:
`111976ad514eb539b78b8e4745ea824e99384ddd534cebb696a01bb1b709005a`.
W2, W3, and W4 frozen design hashes were rechecked unchanged. The next report's
existing script identity binds this observer change; no historical capture was
rewritten. Linux's periodic observer is unchanged and gains no new absence claim.

## Local verification

Rust 1.96.0; pinned Python 3.13.14. Windows is local Windows 11 x64; Linux runs
under WSL2 with native Linux scratch storage. Neither substitutes for hosted CI.

- Windows: **188 locked xtask tests** pass, including native real-child rejection,
  strict companion/launcher tests, invalid-prefix capture retention, explicit
  companion size/hash preservation, and the required Windows CI-route mutations.
- Linux: **187 locked xtask tests** pass; the Windows-only native test is compiled
  and executed on Windows, not counted as Linux evidence.
- Observer suite: **11 tests pass on Windows**, including native child/grandchild,
  already-terminated child, fast exit, nested host jobs, both envelopes, and exact
  socket-barrier cleanup of a held child while an unrelated process remains alive.
  The Linux partition passes eight applicable tests and explicitly skips the
  three native Windows cases. Windows skips none.
- Authored faults exercise create, assign, membership, times, flags, accounting,
  RSS, observer thread, receipt write, close, and failed cleanup. A deterministic
  watchdog test requires bounded communication, owned-helper kill, bounded drain,
  and re-raise; it cannot enter Popen's unbounded context-manager exit wait.
- Both platforms pass 35 source-provenance bootstrap tests and 12 CI-policy tests.
  Locked Cargo closeout uses the admitted tool wrapper. Windows and Linux
  all-target xtask Clippy with `-D warnings` and workspace formatting pass.
  The current Windows xtask binary returns structural architecture PASS; that
  does not claim execution of deferred corpus/package/benchmark matrices.
- Live/new Markdown link/UTF-8 policy and whitespace pass. Archived upstream
  document fragments remain excluded exactly as the CI owner specifies; they are
  not live repository documentation. Generated import bytecode was removed.

The existing observer command is now scheduled in the required Windows platform
lane as well as Ubuntu bootstrap tests. Policy tests reject removing or retargeting
that Windows execution. No new CI run, rerun, push, or PR transition was performed.

## Actual prebuilt public-child smoke

The unchanged W3 ordinary and diagnostic release payloads from the
[W3 local executable bindings](../phase1-windows-audit-store-diagnostic-2026-09-06/IMPLEMENTATION.md)
each run public `audit` with jobs=1/default through the new observer: four cells
on Windows, four on native WSL2 storage. Each returns exactly the authored
`main.ts / observerSmoke` finding and zero limitations; public `findings` retrieves
that row. Diagnostic cells bind PID/run/attempt and exact 23/52 phase inventories.
Both platforms preserve the ordinary/diagnostic envelopes; Windows retains total
job counts 1 before and 2 after. Executable hashes are unchanged before/after.

These are functional smoke checks, not the frozen full scaling fixture or budgets.
An initial smoke-driver query omitted required `--area dead-code` and failed; its
partial directory is retained and not counted. The corrected public query uses
a separate create-new capture directory, never overwrites the failed attempt.

| Retained smoke receipt | SHA-256 |
| --- | --- |
| External smoke driver | `f7013d89bb1da422535bdd86981ecec5e2d3f6d33c6f8bf0736461c78dbcdb7b` |
| Windows four-cell receipt | `0cc5756ef671188f41b38b5f0915a6f2dbc153ca4d476d7f5de775b2435b1b74` |
| Linux four-cell receipt | `8d449f07dbdb810ef99113e70a2980f8c6b239b72499c9c41acffa3ffc4c014e` |

Retrieval hints: `D:\lumin-w4-observer-smoke-20260907\windows-v2` and
`/home/endof/.cache/lumin-w4-observer-smoke-20260907`; each retains raw helper,
measurement, product output, and public findings captures. The driver remains
at `D:\lumin-w4-observer-smoke-20260907\smoke.py`.

## Independent code review and Rust advisory

`observer_adversarial_review` performed a read-only implementation review. Its
one P2 watchdog finding was corrected and deterministically tested; follow-up
returned scoped PASS with no remaining grounded blocker. It also rechecked the
new companion archive assertion. This was independent inspection, not an
independent claim of test execution or numeric acceptance.

Generated Rust intents were streamed through stdin; invocation-specific artifacts
remain outside the checkout. Both pairs include tests, no exclusions, no parse
errors, and no unexpected/missing planned files. Lifecycle-only advisories provide
no refreshed base-audit absence claim; TS type-escape/scan-parity lanes are explicitly
inapplicable, not treated as Rust evidence.

| Scope / external directory | Pre ID | Matching post ID |
| --- | --- | --- |
| Consumer/policy; `D:\lumin-w4-observer-gate-20260907` | `2026-09-07T05-10-12-989Z-f7dfbb` | `2026-09-07T05-30-17-291Z-860c20` |
| Capture assertion; `D:\lumin-w4-observer-archive-gate-20260907` | `2026-09-07T05-26-44-061Z-c6dad5` | `2026-09-07T05-30-21-790Z-49ffdd` |

## Remaining boundary

The [actual W4 CI packet](CI-EVIDENCE.md) now has complete valid observations on
both platforms. Native Linux passes its numeric matrix; Windows still misses
scaling. This correction neither explains the four historic W3 PIDs nor proves
a product speedup. Any product change requires new blocking artifacts.
P1-60/P1-70, permanent metrics, allocator approval, and `/mnt` disposition remain
open. The user's original worktree changes remain untouched.
