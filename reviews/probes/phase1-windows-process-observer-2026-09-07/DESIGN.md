# Windows Benchmark Process Observation

Candidate W4. Owner: [REVIEW-005](../../phase1-performance-evidence-review-2026-09-05.md).
Status: design candidate; implementation requires author and independent review.

## Definition, evidence, and non-goals

Correct Windows benchmark child-process classification without changing Lumin,
its commands, public/private product schemas, analysis, durability, scheduler,
allocator, or numeric budgets. The user approved correction design, independent
review, and subsequent implementation on 2026-09-07. This is not approval to
reclassify prior failures, rerun until green, merge PR #135, or close P1-60.

Run [34033040113](https://github.com/annyeong844/lumin_Refactor/actions/runs/34033040113)
retains an invalid Windows warm-pre-write cell with four alleged child PIDs.
Its observer hash is `a4d1ef16472c30e9dec9d56c942fe9446681010743a064b3ec2ab709d00a201f`.
PID-only ancestry demonstrably adopts children of a previous process lifetime;
unchecked Toolhelp errors can return empty or partial inventories. The retained
run lacks lifetime evidence, so its particular failure remains unexplained and
invalid. The independent Linux ratio miss remains blocking.

Creation-time filtering alone also cannot discover a child that starts and
exits between periodic snapshots. Windows will instead use kernel-maintained
cumulative accounting for a fresh private job. Linux's existing observer and
its sampling limitations remain unchanged and are not newly certified by W4.
Broker/service-created processes outside normal CreateProcess inheritance are
outside this observer's authority; this is not a malicious-code sandbox or an
attribution claim for WMI/service requests. Product no-subprocess architecture
checks and the unchanged full semantic oracle remain required.

## One helper, one direct product launch

The existing pinned, isolated Python measurement helper owns an unnamed,
non-inheritable Windows Job Object. Before starting the elapsed timer or
launching the product, it assigns **itself** to that fresh job. This process
is disposable and exclusively launched for one measurement; the xtask/CI
parent is never assigned. Nested host jobs are supported when Windows accepts
the assignment. Unavailable assignment/membership/accounting is a hard error
before product launch, never a Toolhelp fallback.

Use explicit pointer-width Win32 argument and return declarations for every
API used by this path, including job creation, assignment, membership, basic
accounting/limit queries, process times, memory, termination, and handle close.
Capture GetLastError on false returns; never infer empty data from failure.
No global process enumeration or PID/PPID ancestry participates in Windows
admission. Remove that obsolete Windows owner rather than keeping two readers.

The job has no newly imposed resource, scheduling, UI, priority, memory, active
process, breakaway, silent-breakaway, or kill-on-close flags. Verify its actual
basic limit flags are zero before and after execution. Existing outer-job
limits still apply. Confirm helper membership and capture its creation time.
Before launch, cumulative TotalProcesses and ActiveProcesses must both be one,
and TotalTerminatedProcesses must be zero. That last field counts limit-caused
terminations, not all ordinary exits.

Launch the unchanged direct command with subprocess.Popen, no shell, wrapper,
suspended interval, runtime build tool, or altered product environment. Normal
CreateProcess inheritance places it in the already established private job
before user code executes. Retain its Popen process handle and job handle;
bind the product PID, creation time, and job membership to those held handles,
not the product's output or a process-name search. A fast-exiting product is
allowed if those queries still succeed; unreadable identity is a failed sample.

Keep the existing launch-through-wait monotonic elapsed interval and 1 ms
Windows RSS polling policy. The poll reads only product PeakWorkingSetSize;
it no longer repeatedly scans the global process list. Observer-thread errors
are explicitly propagated after joining, never replaced by an earlier RSS
value. The final held-handle memory query is mandatory; a failed query after
exit is still failure, not zero. The sampling interval describes RSS observation,
not a guarantee about discovery latency. Do not subtract observer overhead
from budget times or compare changed-observer medians as product speedups.

After product exit, capture its nonzero exit FILETIME and unchanged creation
time, current job flags, and cumulative accounting. A no-child observation
requires TotalProcesses exactly two: helper plus product. Normal child and
grandchild creation increments this total even when they already exited.
ActiveProcesses is retained and must be in 1..=TotalProcesses; it need not be
exactly one while references to an exited process remain. Limit-caused
termination count must be zero and cannot exceed the total. A larger total is
a genuine extra-process rejection, never an invented PID or an accepted empty
child list. Counter regression, contradictory fields, or API failure is invalid.

## Exact companion record and existing envelopes

Keep ordinary `lumin.phase1-process-measurement.v1` and diagnostic v2 unchanged.
Windows writes the create-new companion `windows-process-observation.json`
beside `measurement.json`, outside the repository under analysis. It is one
compact, sorted-key JSON object plus one newline with exactly these fields:

- `schemaVersion`: `lumin.windows-process-observation.v1`;
- `method`: `private-inherited-job.v1`;
- `helperProcessId`, `processId`: distinct nonzero u32 values;
- `helperCreationTime100ns`, `processCreationTime100ns`, `processExitTime100ns`:
  nonzero u64 FILETIME values, ordered helper creation <= product creation <= exit;
- `helperInJob`, `processInJob`: true, obtained through held handles;
- `limitFlagsBefore`, `limitFlagsAfter`: actual u32 job flags, both zero;
- `before`, `after`: objects containing exactly `totalProcesses`,
  `activeProcesses`, and `totalTerminatedProcesses`, each a u32 OS observation.

A complete companion may record a larger after total for a rejected child
case. Write and fsync that evidence and available product streams before
rejecting. Do not emit an accepted measurement for a child, inconsistent
observation, failed observer, or unavailable API. Early failure retains available
stdout/stderr and helper diagnostics without fabricating a complete companion.
Successful Windows measurement keeps `analysisChildPids: []` only after the
cumulative no-child proof. It makes no claim to enumerate historical child PIDs.

The Rust measurement owner spawns the helper with null stdin and captured
stdout/stderr, retaining its actual launcher PID before wait_with_output.
After existing helper-success and strict measurement decoding, Windows must
strictly read the companion: exact canonical bytes, no duplicate/extra/missing
fields, typed integers/booleans, method/schema, time/order/count/membership rules,
and helper PID equality with that independently launched process. Diagnostic
v2 processId must equal the companion's product PID as well as the engine frame.
Missing/bad companion is failure. On non-Windows, an unexpected companion is
failure; existing Linux v1/v2 decoding remains unchanged.

Both benchmark and W2/W3 capture manifests retain and hash the companion as part
of the cell's exact inventory. Measurement/report schema versions, frozen W2/W3
frame bytes, build-feature isolation, fixed cell order, fixture truth, and all
numeric thresholds remain unchanged. A new report's script hash distinguishes
the observer revision; no old artifact is rewritten or silently upgraded.

## Failure cleanup and scope

Success closes the owned job handle normally, without kill-on-close. The helper
is then allowed to exit normally; no accepted execution has live descendants.

On error after the helper is confirmed in its private job, flush the available
evidence and an explicit helper stderr error, then request
TerminateJobObject(private_job, 1). This intentionally terminates the dedicated
helper and its inherited product/descendant job subtree, never the outside
runner or a PID selected from the global namespace. It applies only to already
invalid measurement. No process/resource limit is imposed on a valid execution.
The retained diagnostic says cleanup requested, not cleanup complete: this call
is not graceful child flushing and self-termination prevents an after-check.
Raw child streams can be truncated. An API failure records its exact error and
exits nonzero with cleanup unknown; do not retry, kill by PID, or report success.
Before job membership is established, close only the known owned handles and
fail normally. No unobserved process may be adopted for cleanup.

## Acceptance and focused verification

1. Independently authored job/accounting tests cover the legal PID-reuse history,
   including an old child whose own child is younger than the measured root.
   Windows acceptance must not consult that misleading global inventory. A real
   child remains a rejection, not an allowlisted exception.
2. Native isolated helpers exercise direct children, grandchildren, and a child
   fully terminated before the final observation. Pipe/event barriers force the
   order; no sleep or scheduler luck proves absence. A lingering child is held
   while the outside test obtains its exact handle, then failure cleanup kills
   it; an unrelated outside process remains alive. Watchdogs only fail hangs.
3. Exercise native nested host jobs and fast-exiting products, both measure
   modes, process/creation-time binding, unchanged canonical v1/v2 fields, raw
   stdout/stderr, complete companions and capture inventories. No native job
   test assigns the long-lived test runner or agent to the helper's private job.
4. Fault-inject create, assign, flags, membership, accounting, times, RSS,
   observer-thread, evidence-write, and cleanup failures. Each rejects, retains
   available raw evidence, and creates no accepted measurement. Distinguish
   complete count observations from missing/partial failures and prove the
   outside runner/foreign process is never a termination target.
5. Rust strict-decoder tests reject missing/opaque/duplicate/noncanonical data,
   numeric overflow/zero, wrong helper/product IDs, false membership, impossible
   times/counts/flags and companion presence on the wrong platform. Real helper
   integration must reject a process-spawning fixture and preserve its raw record.
6. Run the existing Python observer command on both platforms; native Windows
   cases are required in its Windows partition, not sampled or silently skipped
   there. Pass focused xtask tests, Clippy, fmt, affected bootstrap/CI-policy checks,
   and actual ordinary/W3 public child smoke without runtime Cargo/Node. Observe
   the Rust pre/post workflow before changing its measurement consumer.
7. Public CI remains merge authority. Retain the next full blocking matrices and
   W3 packet under new archive names when separately authorized to push; do not
   rerun the failed run, relax its verdict, or treat local correction tests as
   numeric PASS. Review and owner records must name the exact frozen design hash.

## Platform references

- [Microsoft: PID reuse and parent IDs](https://devblogs.microsoft.com/oldnewthing/20150403-00/?p=44313)
- [Microsoft: Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)
- [Microsoft: Nested Jobs](https://learn.microsoft.com/en-us/windows/win32/procthread/nested-jobs)
- [Microsoft: cumulative accounting fields](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_basic_accounting_information)
- [Microsoft: AssignProcessToJobObject](https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-assignprocesstojobobject)
- [Microsoft: GetProcessTimes](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getprocesstimes)
- [Python: ctypes return types](https://docs.python.org/3.13/library/ctypes.html#return-types)
