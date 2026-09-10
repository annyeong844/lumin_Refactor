# W5 Design Review

Reviewed [DESIGN.md](DESIGN.md) SHA-256
`bbaec3408fe9c178b585f37cb2fc442bf714c64851857463c176431407b78e55`
against product HEAD `d63281bb613a907219f3e78348f29f7c777f102a`.

## Author review

Scoped PASS before Rust implementation. The change removes only an unchanged
derived-index commit. It retains exclusive ownership, transaction-scoped backend
lifetime, document authority, receipt verification and immediate durability for
actual mutations. ARCH-002 requires no public contract change for this branch.

## Independent adversarial review

`latest_index_noop_review` independently inspected the design, source, pinned
redb 4.1.0 implementation, owner contracts and public recovery hook. It returned
scoped PASS for the exact hash above after two grounded corrections:

- The private wrapper must own the consuming abort; `Deref` cannot transfer its
  backend transaction. Propagate abort errors.
- Namespace validation alone does not authenticate the original lifecycle.store
  handle. Retain original identity/generation checks across abort, plus committed
  receipt verification afterward; final admission reopening is insufficient.

The reviewer confirmed provisional empty-table rollback and the unchanged real
two-key commit path. The commit-failure test is explicitly a fault before the
real commit, not a rollback claim for post-commit validation errors. The exact
substitution regression must use an otherwise admissible replacement or establish
that original-object validation fired; malformed-file rejection alone is not proof.

This is a read-only design review, not implementation/test execution evidence,
user approval of an unseen hash, a measured speedup, or Phase 1 exit authority.
Implementation verification and new measurements remain required.

## Exact failure-teardown extension

The first implementation review found no static blocker, but its PASS was
withdrawn when the public Windows substitution proof failed its unchanged
foreign-bytes assertion. The [rejected-backend extension](REJECTED-BACKEND.md)
retains this result and the precise writable-reopen path; the assertion is not
relaxed.

Author and `latest_index_noop_review` independent design reviews pass extension
SHA-256 `7baed116f3b63b4da18278f25e021f7fe7f1a922033329bc84b955665a5b6233`.
It is a private terminal guard disposition enforcing the existing fail-closed
contract, not a general error bypass or backend cache. The exact original
design remains immutable; this extension supersedes only its erroneous
failure-teardown assumption.

The independent source review of that extension found no grounded blocker:
rejection is sticky for the acquired guard, blocks wrapped backend access before
final reopening, preserves the original error, and prevents a caught error from
becoming success. Unrejected paths retain full validation. The reviewer did not
execute tests. The author subsequently reran the exact Windows public proof:
1 passed with its foreign-byte preservation predicate retained. All 5 latest-index
owner tests also passed, including rollback and fresh-guard admission after a
rejected backend. These results close the withdrawn scoped implementation review;
broader affected checks, package probes, and performance measurements remain
separate requirements, not conclusions of this source review.
