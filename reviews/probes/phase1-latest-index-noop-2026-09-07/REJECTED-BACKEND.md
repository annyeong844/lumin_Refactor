# W5 Rejected-Backend Teardown Extension

This candidate extends only the failure-teardown clause of [DESIGN.md](DESIGN.md)
(`bbaec3408fe9c178b585f37cb2fc442bf714c64851857463c176431407b78e55`).
ARCH-002 Section 2.0 remains the owner. No weakened assertion, public contract,
schema, durability, numeric budget or backend-lifetime change is proposed.
Implementation awaits author and independent review of these exact bytes.

## Grounded failure

The first exact public Windows store-substitution test reached the intended
barrier, installed a byte-identical different physical store, and received the
specific original-store identity error. Its unchanged foreign-bytes assertion
then failed. The no-op abort correctly rejected the original binding, but
`with_lock_core` still considered the guard admitted and ran final
`require_idle`/`validate_complete`. That reopened the substitute using writable
redb, whose normal open/drop can change private recovery/allocator metadata.
The four index owner tests and eight publication fault tests passed; neither
those results nor the earlier read-only code review overrules this failed proof.

## Private terminal rejection

Add a private sticky backend-rejection flag to the existing namespace guard.
It is per acquisition, never persisted and never cleared. Preserve the guard's
existing concurrency traits with a standard-library atomic flag; retain no new
file/backend handle and introduce no cache or alternate storage backend.

- The owned no-op abort marks the guard rejected before propagating any failure
  of its original-object/namespace/receipt checks or the explicit abort. The
  transaction still unwinds against its original owned backend; it never moves
  or disposes a substitute. No diagnostic-string matching is used.
- A rejected guard cannot open another `StoreDatabase`, start a read/write
  transaction through an already-held `StoreDatabase`, or commit a wrapped
  transaction. These guards run before those backend actions.
- Final lock teardown checks rejection before `require_idle` or any backend
  reopening. For this rejected state only, it repeats the existing no-follow
  namespace/marker/lock/managed-parent proof, unlocks, and returns failure. The
  original operation error remains authoritative unless a final binding or
  unlock error takes precedence under the existing combination rules.
- If an internal caller catches that original error and returns `Ok`, teardown
  must still return an integrity failure. Restoring a path cannot revive this
  guard; a fresh acquisition must repeat ordinary complete admission.
- All unpoisoned paths, including other operation errors, retain their existing
  full final validation. Do not use a blanket `result.is_err()` bypass. Normal
  success, actual mutation commits, migration, lock ordering, and handle lifetime
  remain unchanged. No extra hot-path backend open or metadata write is added.

Scope: the existing `namespace.rs` guard lifecycle, `namespace/database.rs`
wrapped access/abort, and the W5 focused owner/public tests. The original W5
equality algorithm and exact barrier remain unchanged.

## Required proof

Keep the existing full foreign bytes/identity and authentic complete logical
snapshot assertions. The public process must fail after the actual store swap
without reopening or modifying the substitute, and recover only under a fresh
guard after restoration. Linux requires an actual swap; any Windows OS-prevented
rename branch is prevention evidence, not post-abort rejection evidence.

Add a supporting owner test that marks a live guard rejected, proves new backend
open/read/write and preexisting transaction commit all fail, deliberately returns
`Ok` from the guarded closure, and proves the outer result still fails with no
logical mutation. A fresh guard must remain usable. This test exercises only
the private token disposition; the public substitution fixture proves the real
abort failure sets it. Retain pre-commit failure, changed-pointer recovery,
concurrency, namespace, migration and retry tests.

Open a new matching external Rust pre/post pair before editing the expanded
source scope. Repeat failed focused proof before broader checks or performance
measurement. Earlier code-review PASS is withdrawn pending this extension and
implementation review. P1-60/P1-70 remain open.
