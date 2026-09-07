# W4 Design Freeze

Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
Reviewed source: `a0f5032dfa19dcfc3546f17ff1a4b13e6b7b42d6`.
Frozen [DESIGN.md](DESIGN.md) SHA-256:
`94d12d567c21fca573eaac20c352cc3ebd8bec12d78487ce58b8733f6fda89dc`.

The user authorized design, independent review, and subsequent implementation
on 2026-09-07. This records that workflow approval, not user review of this hash.

## Author design review — PASS

The root agent reviewed the exact candidate before implementation. The private
helper job closes PID-reuse and between-polls child-lifetime gaps without adding
product behavior, dependencies, process limits, or new timing budgets. Cumulative
accounting, held process identities, explicit API errors, and the strict companion
are jointly required. Existing measurement envelopes and frozen W2/W3 frames
remain unchanged. Failure-only self/subtree termination is confined to the owned
job and is described as requested, never confirmed, cleanup.

Platform feasibility input came from `observer_platform_review`; that contributor
is not the independent adversarial reviewer. Its native nested-job/fast-exit
experiment is supporting feasibility evidence, not implementation acceptance.

## Independent adversarial review — scoped PASS

`observer_adversarial_review` independently inspected the exact hash and source
head, read-only, before implementation. It found no blocking design findings.
It checked private-job inheritance and cumulative accounting, the exited-process
reference caveat, strict companion/launcher binding, preservation of raw captures,
and failure cleanup limited to the disposable helper's job. It confirmed the
acceptance tests are obligations, not claims of completed execution.

Its integration checkpoint is binding: CI currently executes the observer suite
only on Ubuntu. Add the same exact command to the Windows platform lane and lock
that route with policy/mutation tests. Native Windows tests cannot be satisfied
by compilation or silently skipped on Windows.

This freeze authorizes W4 implementation only. Run `34033040113` remains invalid
on Windows and independently fails Linux scaling. No old archive, numeric verdict,
P1-60 status, or merge authority changes.
