# Windows gate-barrier failure diagnosis

Scope: the two `write_gate` failures in [run 34129034468](https://github.com/annyeong844/lumin_Refactor/actions/runs/34129034468),
head `8b1c79372a08680069b9c13ceca11e8072380777`. This is a test-harness
diagnostic correction, not a product, concurrency-policy, or performance change.

## Retained hosted verdict

Run 34129034468 completed with 55 successful jobs and three failures: Windows
package scaling, Windows integration 1/6, and the dependent Required check. All
six Windows store-library partitions passed, including the five publication
owner tests. Both platform and staged-adapter behavioral package probes passed.
Windows scaling was `0.8827104102678495` (1,800,521,800 ns default versus
2,039,765,000 ns jobs=1); Linux was `0.7025425044896499` (339,093,554 ns versus
482,666,247 ns). The unchanged limit is `0.75`.

All 745 Windows and 711 Linux captures, their 34-cell inventories, medians,
process observations and semantic projections were verified. All 379 captures
in the separate 14-cell diagnostic were also verified; it is not budget authority.
The complete packets remain at `D:\lumin-w5-ci-34129034468`, with terminal report
SHA-256 values `44b58d506dc01f96526deadeb95b7145f6993ec250c11aab631127ed0d56aa18`
(Windows), `090edb4d67b2ec77ca8892c8e6f88fb7c998e951ea8b99eff7f3ff3230962fbd`
(Linux), and `b90fe516d755d1143631610b2fb6619d25ad460991470e48fcc6f7b084a791d5`
(diagnostic). The earlier misses remain retained; a later Linux pass does not
prove a routing-only change improved performance.

## Observed problem and acceptance

The semantic-input pre-write and stale-protection post-write helpers exhausted
their existing 30-second arrival watchdogs while their children remained alive.
The errors omitted the exact command, child ID, and child output. Returning from
either helper also dropped a live `Child` without terminating and reaping it.
The hosted log does not establish whether progress was slow or blocked.

The affected helpers must retain their existing real TCP barriers, 30-second
watchdogs, final-state assertions, and exact same-operation retries. On failure
they must report the command, child ID, elapsed wait, observed exit state, and
collected stdout/stderr, and terminate/reap their own still-running child. An
unexpected successful exit before the barrier is still a test failure. Error or
assertion unwinding must not leave an owned child running. Diagnostic timestamps
and process IDs are not semantic evidence or timing assertions.

Acceptance uses real child processes for early error exit, early successful exit,
watchdog failure forced only after an explicit admission-barrier handshake, and
early scope exit. The latter requires an actual collected wait result with empty
output, so the child's own later protocol-timeout exit cannot satisfy cleanup.
The two original public gate scenarios must still pass without relaxed assertions.

## Reproduction limits

Before edits, both failed tests passed together locally in 9.89 seconds. The exact
Windows integration shard 1/6 target list also passed with `--jobs 4`, four libtest
threads, and a four-CPU process affinity inherited by its children. Cargo builds
used one job to respect available local memory. All nine targets remained selected.
The local machine is not the hosted Windows Server 2022 image and has different
storage and CPU performance; these passes do not invalidate the hosted failures.
The raw local command/result and process samples are retained outside the checkout
at `D:\lumin-gate-barrier-diagnostics-20260907\shard-reproduction.json`.

The hosted failure's root cause remains unproven. No timeout increase, test
serialization, assertion removal, numeric-budget waiver, or retry-to-green claim
is part of this correction. Hosted verification must remain authoritative.

## Correction verification

- Locked Windows `write_gate`: all 39 tests pass with four libtest threads and
  four-CPU affinity (50.73 seconds). The complete nine-target integration shard
  1/6 also passes after the correction with its unchanged `--jobs 4` selection.
- Locked Linux `write_gate`: the four child-lifetime regressions and the two
  original gate scenarios pass (six tests, 5.60 seconds). The initial launch
  refused before tests because native `rustc` was absent from `PATH`; the actual
  test invocation used the exact native 1.96.0 toolchain directory explicitly.
- Windows `gate-final-observation` and `gate-immutable-opening-delta` both pass
  standard and paired determinism runs. Their determinism variants retain 11 and
  seven semantic captures respectively; diagnostic logs do not enter the oracle.
- Scoped locked `write_gate` Clippy passes with warnings denied on Windows and
  Linux. Workspace fmt, diff whitespace, and all 76 live Markdown documents pass.
- External Rust advisory `2026-09-07T14-27-04-904Z-5a1edf` is paired with post-write
  `2026-09-07T14-40-37-471Z-5cf73e`: all three planned files observed, one planned
  new file, no unexpected/missing file or parse error. TS checks are inapplicable
  and the unrefreshed base audit is not absence evidence.
- The final fixture-only clarification explicitly sets the accepted test socket
  to blocking mode before reading its frame, matching the existing gate tests.
  Advisory `2026-09-07T14-51-22-457Z-486204` and post-write
  `2026-09-07T14-54-08-000Z-672445` cover that one-line change with no missing,
  unexpected, or malformed Rust file. All six focused tests and scoped Clippy
  pass again on both platforms; the earlier full-target/corpus evidence is
  retained rather than relabeled as a new run.

No product Rust source, dependency, public DTO, CI partition, watchdog duration,
or numeric limit changed. Fresh hosted execution is still pending.
