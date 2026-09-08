# W6 Attempt-Session Read Verification

Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
Authority: the frozen [design and amendment review](REVIEW.md).
Source base: `95163f586f2d9ea9dcae7f356d94e10469ec84e9`, with uncommitted W6 changes.

## Current disposition

The W6-03 failure-continuation counterexample is closed by focused Windows and
native-Linux checks and independent implementation review. Affected regression,
Windows corpus and both actual-package checks pass. Four fresh ordinary
[control/candidate packets](MEASUREMENTS.md) pass local targets, with small
cold-median reductions and mixed changes elsewhere. No material causal speedup,
hosted-budget PASS, push, CI rerun or Phase 1 exit is claimed.

Production session validation owns one generation-bound backend and one guarded
lease-row read, with zero application writes. Its retained read result always
reaches original-held validation. A rejected open/finishing proof poisons both
the acquired guard and that session; failure persistence and publication retries
check the session before acquiring another guard. The flag is private, sticky,
in-memory authority, not a persisted field or backend cache. Ordinary read/lease
errors whose finishing proof succeeds retain existing failure handling.

## Focused evidence

| Check | Windows | Native Linux |
| --- | --- | --- |
| Five store-owner tests (`--lib attempt_session`) | PASS | PASS |
| Three public substitution tests, each at prepare/finalize/release | All nine actual substitutions rejected; foreign bytes/identity preserved | All nine actual substitutions rejected; foreign bytes/identity preserved |
| Public forced lease-read error without substitution | One Failed attempt; subsequent audit completes | One Failed attempt; subsequent audit completes |
| Complete paused logical/physical state and restored recovery/retry | PASS | PASS |
| CLI fault-target Clippy, warnings denied | PASS | PASS |
| Publication, reversed ordering, crash recovery and retention races | 16 PASS | 16 PASS |
| Old-generation late publisher | 1 PASS | 1 PASS |
| Entire store library | 288 PASS | Not rerun in full |
| Ordinary staged platform package and both adapters | PASS | PASS |

The Windows store/xtask all-target Clippy checks pass with warnings denied.
The structural architecture check passes; dependency admission remains a
separate CI verdict. Formatting, whitespace and all 82 tracked/new live
Markdown documents pass. The publication-test feature union reports four
existing unused support-function warnings in the `publication` target on both
platforms; these successful test executions are not a warnings-denied lint
claim for that union. No assertion or feature prerequisite was bypassed.

The public target has four tests, not nine independently named tests. Its exact
barrier selector stops the normal audit's three different session reads and
asserts their actual durable phases. Original-backend observations travel over
the private barrier connection with complete framing; no second guard/backend
is opened while paused. Recovery preserves every previously published run's
physical identity and bytes, removes only the selected lease lock, and permits
only the phase-owned attempt/latest transitions. The complete expected logical
observation and stable public retry protect all other rows and revisions.

On both tested hosts substitution actually succeeded. Windows sharing/access
denial alternatives remain encoded but were not exercised; they are not counted
as observed platform-protection evidence.

The five owner tests include complete lease/lock comparison, malformed/missing
row finishing, final-turn receipt rejection, and actual failed-generation
session validation followed by repeated failure/publication requests. The latter
records zero new guarded backend constructions/reads/writes/commits, unchanged
foreign bytes/identity, and ordinary Interrupted recovery after restoration and
session drop. No empty or malformed row is adopted as valid.

All four public tests are registered in the existing state-lock row's standard,
determinism and store-crash invocations; the complete ordered mapping is locked
by the corpus unit test. The 53 corpus tests pass. Adding four invocations
changes exact standard shard loads from `[51,51,50,50]` to `[52,52,51,51]` and
determinism loads to `[64,35,35,35,35,35,35,34]`; no scheduler, row, feature or
assertion is removed. The affected Windows standard and determinism rows
execute and pass; determinism retains 27 semantic captures. The affected
Windows store-crash row also executes and passes. Linux direct public checks do not substitute for
a Linux corpus-lane execution.

## Independent implementation review

Reviewer `attempt_read_design_review` returned scoped PASS against the frozen
amendment, independently inspecting production entry-point fences, original-held
finishing, the observation transport, phase/recovery oracles and exact mapping.
The reviewed public test SHA-256 is
`709b3a67d14b11fcb43c189579299e96b48f1838422e871a7798c7ea46f81a30`.
The reviewer inspected both platforms' final public/owner logs and the corpus
log, but performed no edits, builds or tests and made no performance conclusion.

## External advisory and raw artifacts

The main Rust transaction is pre-write
`2026-09-08T01-56-46-749Z-f3d2de` and matching post-write
`2026-09-08T02-25-11-049Z-5494a4`. All 14 planned files were observed, with no
unexpected new files or reported parse errors. These are lifecycle/file-delta
observations; the lab did not refresh a full base audit, and no whole-repository
Rust quality or absence claim follows from them.

The raw packet is `D:\lumin-w6-attempt-session-continuation-20260908`, including
both final public transcripts, both owner transcripts, CLI Clippy and the corpus
tests. Initial corpus count failure and the test-only Clippy panic rejection are
retained separately. The lint-only follow-up uses pre-write
`2026-09-08T02-27-41-237Z-2e1019`, matching post-write
`2026-09-08T02-29-54-335Z-f78717`, and artifacts at
`D:\lumin-w6-attempt-session-lint-20260908`. It replaces two test panic sentinels
with explicit preflight-call observations and preserves the rejected result and
zero-backend assertions. Product code and the reviewed public fixture are unchanged.

The [original red transcript](IMPLEMENTATION-BLOCKER.md#observed-public-evidence)
remains retained and is not overwritten or reclassified. The completed local
[measurements](MEASUREMENTS.md) follow the frozen W6 design. Full CI acceptance
and the remaining REVIEW-005 decisions keep P1-60/P1-70 open.

The authorized publication preflight reverified all 14 Rust-source hashes and
both frozen design hashes, then passed the five Windows attempt-session owner
tests, all 53 corpus unit tests, and all four public attempt-session tests under
the locked `lifecycle-test-fault,publication-test-crash` feature union. The latter
emits the same four unused publication-support helper warnings noted above;
it is not a warnings-denied Clippy result. Formatting, all 83 live Markdown
documents, and diff whitespace also pass. No Rust source changed after the
matching advisory and scoped Clippy/package checks recorded above.
