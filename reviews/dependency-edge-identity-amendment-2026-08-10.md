# REVIEW-003: Dependency-Edge Identity Amendment

Document role: focused Architecture v1 amendment and freeze gate

Status: candidate; implementation is blocked until the exact architecture-content
commit passes the design review and independent adversarial review below and is merged
to `main`. That merge freezes this amendment without authorizing any wider dependency
policy change.

## Definition

The canonical direct-dependency policy identifies an edge by owner, declared name,
resolved package, dependency kind, and target predicate so a narrow approval cannot be
reused by a materially different edge.

## Concrete Counterexample

The frozen architecture required a canonical dependency-edge policy, but named only a
workspace edge at the enforcement sentence. The current architecture check compares
only owner package, resolved package name, and dependency kind. Cargo metadata also
reports the declared dependency name and each dependency kind's target predicate.

Consequently, either of these changes can reuse an existing approval:

- move a Windows-only `windows-sys` or `winapi-util` dependency to an unconditional
  dependency, or place it under a different `cfg(...)` predicate;
- rename an approved dependency while resolving it to the same package.

The first change expands platforms, build cost, and the transitive surface. The second
changes the crate-visible API name and can conceal manifest drift. Both are concrete
fail-open policy bypasses and therefore reopen only this part of ARCH-000.

## Amended Contract

For each direct edge from a production workspace member, the architecture check uses
this exact identity from `cargo metadata --all-features --locked`:

```text
(
  owner workspace package,
  resolve.nodes[].deps[].name,
  resolved package name,
  dep_kinds[].kind,
  dep_kinds[].target
)
```

The declared name is Cargo metadata's dependency name after rename handling. The target
is the optional metadata predicate string; absence is distinct from every predicate,
and the checker does not invent predicate equivalence. Each `dep_kinds[]` entry is a
separate edge identity.

The canonical workspace and third-party allowlists must match all five dimensions.
Unknown dependency kinds, duplicate allowlist identities, new resolved identities, and
stale allowlist identities fail the architecture check. Package-family owner isolation
remains an additional boundary and cannot substitute for the exact edge allowlist.

## Non-Goals

- This amendment does not approve a new crate, version, feature, target, or dependency
  direction.
- It does not make transitive edges part of the direct-edge allowlist.
- It does not replace exact workspace dependency versions, `Cargo.lock`, cargo-deny
  version/source policy, or the existing dependency-cost review.
- It does not change the development-only `lumin-xtask` DAG exception.

## Acceptance Criteria

1. The current locked workspace graph matches the exact five-dimensional policy.
2. Changing a Windows-only approved edge to no target or another predicate fails.
3. Renaming an approved workspace or third-party dependency fails.
4. A new direct edge fails even when its resolved package already has an approved owner.
5. A stale or duplicate policy identity fails.
6. Unknown dependency kinds fail rather than falling back to a normal edge.
7. The check remains independent of dependency traversal and metadata insertion order.

## Required Reviews

### Design review

The repository owner must verify that the five dimensions are the intended approval
boundary, that the non-goals do not weaken Rule 7, and that the acceptance criteria
cover both workspace and third-party edges.

### Independent adversarial review

An independent reviewer must bind its verdict to the exact architecture-content commit
and attempt at least these bypasses:

- remove or change the Windows target predicate;
- rename a dependency while preserving its resolved package;
- reuse a package approval under another dependency kind;
- add a duplicate or stale policy identity;
- reorder metadata nodes and dependency-kind entries.

The verdict is `PASS`, `REOPEN`, or a new concrete finding. Author checks, prior code in
a closed implementation PR, and implementation tests are not independent PASS evidence.

## Verification Commands After Freeze

```text
cargo test -p lumin-xtask --locked metadata::tests
cargo run -p lumin-xtask --locked -- architecture-check
```

Implementation may begin only after both required reviews pass and this amendment is
merged. The implementation PR must then prove every acceptance criterion without
weakening the frozen contract.
