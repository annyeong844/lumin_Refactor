# REVIEW-003: Dependency-Edge Identity Amendment

Document role: focused Architecture v1 amendment and freeze gate

Status: candidate; implementation is blocked until the exact architecture-content
commit passes the design review and independent adversarial review below and is merged
to `main`. That merge freezes this amendment without authorizing any wider dependency
policy change.

## Definition

The canonical dependency-surface policy freezes production workspace feature
definitions and binds each manifest declaration to its resolved package so a narrow
approval cannot be reused by a materially different graph.

## Concrete Counterexample

The frozen architecture required a canonical dependency-edge policy, but named only a
workspace edge at the enforcement sentence. The current architecture check compares
only owner package, resolved package name, and dependency kind. Cargo metadata carries
additional declaration and resolution facts that materially change an approval.

Consequently, either of these changes can reuse an existing approval:

- move a Windows-only `windows-sys` or `winapi-util` dependency to an unconditional
  dependency, or place it under a different `cfg(...)` predicate;
- rename an approved dependency while resolving it to the same package;
- change an approved edge between optional and unconditional, toggle default features,
  or request a different feature set;
- change a production workspace feature definition to activate or forward another
  dependency feature while all-features resolution remains unchanged;
- replace an approved registry package with an unreviewed path package that declares
  the same name and version.

These changes alter platforms, the default graph, crate-visible API names, dependency
code, build cost, or the transitive surface. They are concrete fail-open policy bypasses
and therefore reopen only this part of ARCH-000.

## Amended Contract

The architecture check reads `cargo metadata --all-features --locked`. Before matching
edges, it compares every production workspace package's complete
`packages[].features` map against a canonical policy. Feature names are exact and each
activation list is a sorted, deduplicated set. An absent `default` feature is distinct
from an empty `default` feature. This preserves the declared feature graph even though
all-features resolution activates every available feature.

For each direct dependency from a production workspace member, the checker then links
one declaration identity to one resolution identity.

The declaration identity is:

```text
(
  owner workspace package,
  packages[].dependencies[].name,
  packages[].dependencies[].rename,
  packages[].dependencies[].req,
  packages[].dependencies[].kind,
  packages[].dependencies[].target,
  packages[].dependencies[].optional,
  packages[].dependencies[].uses_default_features,
  canonical_set(packages[].dependencies[].features)
)
```

`name` is the dependency package name and `rename` is the exact optional manifest
rename. They remain separate; `resolve.nodes[].deps[].name` is only a normalized join
fact and cannot own the declared alias. The requested feature list is sorted and
deduplicated as a set so manifest ordering is not policy. Every other string is exact;
absence is distinct from every value, and the checker does not invent target-predicate
or version-requirement equivalence.

The linked resolution identity is:

```text
workspace dependency: (exact workspace member package)
third-party dependency: (resolved package name, version, source)
```

For a workspace dependency, membership and the exact destination member distinguish it
from a same-named external package. For a third-party dependency, source is required and
must match the exact approved registry source. A resolved non-workspace package with no
source is an unapproved path dependency and fails; an allowlist entry cannot authorize
it by name or version alone.

The checker joins declarations to `resolve.nodes[].deps[]` using the exact owner,
destination package, kind, target, and Cargo-normalized binding only after retaining the
unnormalized `name`/`rename`. A missing, ambiguous, or disagreeing join fails closed.
Each declaration kind/target pair and each `dep_kinds[]` entry must have exactly one
counterpart.

The canonical feature, workspace-edge, and third-party-edge policies must match the
complete metadata surface. Unknown dependency kinds, duplicate policy identities, new
feature/declared/resolved identities, and stale policy identities fail the architecture
check. Package-family owner isolation remains an additional boundary and cannot
substitute for the exact edge allowlist.

## Non-Goals

- This amendment does not approve a new crate, version, source, workspace feature,
  dependency feature, target, optionality, rename, or dependency direction.
- It does not make transitive edges part of the direct-edge allowlist.
- It does not replace `Cargo.lock`, cargo-deny version/source policy, or the existing
  dependency-cost review; it adds a fail-closed direct-edge boundary around them.
- It does not change the development-only `lumin-xtask` DAG exception.

## Acceptance Criteria

1. The current locked workspace feature maps and graph match every canonical feature and
   linked declaration/resolution identity without an ambiguous join.
2. Adding or changing a production workspace feature or its activation set fails.
3. Changing a Windows-only approved edge to no target or another predicate fails.
4. Renaming an approved workspace or third-party dependency fails.
5. Changing optionality, default-feature use, or the requested feature set fails.
6. Changing a third-party resolved version/source or substituting a non-workspace path
   package fails even when name and version appear approved.
7. A new direct edge fails even when its resolved package already has an approved owner.
8. A stale or duplicate feature/declaration/resolution policy identity fails.
9. Unknown dependency kinds and missing, ambiguous, or disagreeing joins fail rather
   than falling back to a normal edge.
10. The check remains independent of feature activation order, dependency traversal, and
   metadata insertion order.

## Required Reviews

### Design review

The repository owner must verify that the workspace feature map plus linked
declaration/resolution identity is the intended approval boundary, that the non-goals do
not weaken Rule 7, and that the acceptance criteria cover both workspace and third-party
edges.

### Independent adversarial review

An independent reviewer must bind its verdict to the exact architecture-content commit
and attempt at least these bypasses:

- remove or change the Windows target predicate;
- rename a hyphenated dependency while preserving its normalized binding and resolved
  package;
- alter a production workspace feature activation while preserving the all-features
  resolved graph;
- change optionality, default-feature use, or requested features;
- replace a registry package with a same-name/version non-workspace path package;
- reuse a package approval under another dependency kind;
- add a duplicate or stale policy identity;
- create a missing, ambiguous, or disagreeing declaration/resolution join;
- reorder semantically set-valued feature activations, requested feature lists, metadata
  nodes, and dependency-kind entries.

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
