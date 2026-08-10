# REVIEW-003: Dependency-Edge Identity Amendment

Document role: focused Architecture v1 amendment and freeze gate

Status: **FROZEN** at exact architecture-content commit
`a524095e5e9d1321f24ac0b1b27c26fe40cbbb24`. The repository owner returned
`PASS` for that exact candidate on 2026-08-10. Independent GitHub review
`4896677400` then inspected the same commit and returned no actionable thread, the
review system's clean verdict. This status-only record does not amend the reviewed
contract. The earlier freeze at `ba4b1816ae263d07b74f91a54dfa1494a8446060`
covers only its then-reviewed content; implementation is authorized solely by the
`a524095` freeze.

## Definition

The canonical dependency-surface policy freezes every workspace feature definition and
binds each manifest declaration to its resolved package so neither production nor its
checker can reuse a narrow approval for a materially different graph.

## Concrete Counterexample

The frozen architecture required a canonical dependency-edge policy, but named only a
workspace edge at the enforcement sentence. The current architecture check compares
only owner package, resolved package name, and dependency kind. Cargo metadata carries
additional declaration and resolution facts that materially change an approval.

Consequently, any of these changes can reuse an existing approval:

- move a Windows-only `windows-sys` or `winapi-util` dependency to an unconditional
  dependency, or place it under a different `cfg(...)` predicate;
- rename an approved dependency while resolving it to the same package;
- change an approved edge between optional and unconditional, toggle default features,
  or request a different feature set;
- change a production workspace feature definition to activate or forward another
  dependency feature while all-features resolution remains unchanged;
- add or alter a `lumin-xtask` dependency under the development-tool exemption;
- replace an approved registry package with an unreviewed path package that declares
  the same name and version;
- configure Cargo `replace-with` to load altered checked-in directory-source bytes while
  metadata continues to report the original registry source and package ID;
- pass the same source replacement through Cargo's global `--config` option while the
  checked-in files and environment remain clean;
- change the root `[workspace].resolver` while preserving every package feature map and
  dependency declaration.
- redirect an already-cached registry package directory or manifest while leaving the
  Cargo-home registry root itself unredirected, so a checker build consumes substituted
  bytes before the Rust location verdict exists;
- use Cargo's compatible `default_features = false` spelling in an older-edition member
  so a pre-Cargo parser that reads only `default-features` admits a changed feature graph.

These changes alter platforms, the default graph, crate-visible API names, dependency
code, build cost, or the transitive surface. They are concrete fail-open policy bypasses
and therefore reopen only this part of ARCH-000.

## Amended Contract

The CI dependency boundary has a pre-Cargo source-provenance stage and a post-metadata
location stage. The pre-Cargo stage is the checked-in
`tools/xtask/bootstrap/source_provenance.py` guard. It requires Python 3.11 or newer,
uses only the standard library (including `tomllib`), and imports no repository module.
It is a bootstrap trust guard owned beside xtask, not a product/runtime analysis path.
A Cargo-built checker cannot replace this stage because replacement code could alter
the checker before admission.

Every CI command that resolves, checks, or builds Cargo dependencies is invoked exactly
as `python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo ...`. The guard
asserts Python isolated and no-site flags before reading the repository; this excludes
current/script paths, `PYTHON*` overrides, user site, and `sitecustomize` from its import
boundary. After validation it launches the exact supplied Cargo argument vector without
a shell and returns its exit status. The architecture source policy pins the guard and
rejects unwrapped dependency-resolving CI Cargo commands; `cargo --version`, which does
not read the graph or build code, is the sole exception. The guard strict-parses every
workspace manifest and rejects:

- `.cargo/config.toml` or `.cargo/config` in the repository or any Cargo-searched
  ancestor, and `config.toml` or `config` in the active Cargo home;
- `CARGO_SOURCE_*`, `CARGO_PATHS`, and `CARGO_REGISTRIES_*_INDEX` environment
  overrides (registry authentication variables do not change source identity);
- `[patch]` or `[replace]` tables in workspace manifests;
- redirected workspace directories or manifests, and workspace build scripts that could
  mutate source configuration after admission;
- dependency `git` or alternate-registry selectors, any non-member `path` dependency,
  and any redirected path to a workspace member; a workspace path is admitted only
  when it resolves directly to the exact explicit member and declared package;
- the legacy underscore `default_features` dependency key; the canonical checked
  spelling is exactly `default-features`, so two source spellings cannot share one
  approval;
- Cargo global configuration arguments in either `--config VALUE` or
  `--config=VALUE` form anywhere in the supplied Cargo argument vector;
- a missing or non-exact root `[workspace].resolver = "3"` declaration.

The bootstrap also requires `.github/workflows` to contain only the reviewed `ci.yml`
and verifies its exact digest before every guarded Cargo invocation. Before any Cargo
command may compile repository or dependency code, the guard runs the exact
non-compiling probe `cargo metadata --all-features --locked --format-version 1` without
a shell and captures its JSON. Failure, non-JSON output, or an incomplete package surface
yields no permission to launch the requested command. The metadata workspace-member IDs
and manifest paths must match the complete manifest set that the guard already parsed.
For every non-workspace package, the guard requires the exact crates.io source, an
absolute manifest path lexically below the active Cargo home's `registry/src`, and a
component-by-component physical path with no symlink, junction, redirected package
directory, or redirected manifest. The physical package remains below the already
unredirected registry root and outside the repository. `cargo metadata` may resolve or
download locked packages, but it does not compile a crate or execute a build script;
only after this location verdict may the wrapper launch the requested build, test, lint,
audit, or deny command. `--check-only` performs this same locked metadata-location
preflight and is not a manifest-only shortcut.

The architecture job invokes that full metadata-backed guard independently before
running any repository test code, builds xtask under the same guard, revalidates
independently, and then runs the built checker directly. Bootstrap tests run only after
that provenance verdict, when their process cannot affect a later build or verdict. The
checker invokes the isolated guard once more immediately before its nested metadata read,
so build-time mutation cannot enter the metadata window. The semantic TOML pass treats a
root package as Cargo's implicit workspace member and compares every member's
manifest-authored dependency requirement, source kind, optionality, canonical
default-feature setting, and requested feature set against the checked policy; Cargo's
normalized `req` is only a graph-join fact and cannot erase distinctions in the authored
string.

The exact policy deliberately refuses all such configuration rather than trying to
prove that a particular replacement is harmless. Public CI runs with this clean source
configuration; an incompatible local environment produces no architecture verdict and
prints the forbidden source or argument. Python absence, an older interpreter, a parse
failure, zero parsed workspace manifests, resolver drift, or guard/workflow drift is a
hard failure rather than permission to run Cargo.

The architecture check then reads `cargo metadata --all-features --locked`. It resolves
the active Cargo home and repository root physically before trusting package locations.
Every resolved non-workspace registry package's canonical `manifest_path` must be below
the canonical active Cargo home `registry/src` and outside the canonical repository
root. The registry source root itself must be outside the repository. Its physical
identity is resolved even when the final `registry/src` leaf does not exist, so a
redirected existing parent cannot become trusted on first use. Missing paths,
lexical/physical disagreement, symlink escape, a directory-source manifest, and a
non-workspace package with no source all fail. This provenance rule covers the complete
resolved graph, not only direct allowlisted edges, so a replaced transitive crate cannot
enter through an approved direct dependency.

Before matching edges, the checker requires the canonical root workspace resolver
`"3"` and compares every workspace package's complete `packages[].features` map against
either the production or development-tool policy. The resolver value is an exact policy
identity, not an author-environment default. Feature names are exact and each activation
list is a sorted, deduplicated set. An absent `default` feature is distinct from an empty
`default` feature. This preserves the declared feature graph and unification contract
even though all-features resolution activates every available feature.

For each direct dependency from any workspace member, the checker then links one
declaration identity to one resolution identity. `lumin-xtask` remains outside the
production Cargo DAG, but its separate development-tool allowlist is exhaustive rather
than skipped.

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
must match the exact approved registry source after the graph-wide loaded-location
check. An allowlist entry cannot authorize a package by name, version, or reported
source alone.

The checker joins declarations to `resolve.nodes[].deps[]` using the exact owner,
destination package, kind, target, and Cargo-normalized binding only after retaining the
unnormalized `name`/`rename`. A missing, ambiguous, or disagreeing join fails closed.
Each declaration kind/target pair and each `dep_kinds[]` entry must have exactly one
counterpart.

The canonical workspace-resolver, feature, workspace-edge, and third-party-edge policies
must match the complete manifest and metadata surface. Unknown dependency kinds,
duplicate policy identities, new feature/declared/resolved identities, and stale policy
identities fail the architecture check. Package-family owner isolation remains an
additional boundary and cannot substitute for the exact edge allowlist.

## Non-Goals

- This amendment does not approve a new crate, version, source, workspace feature,
  loaded location, dependency feature, target, optionality, rename, or dependency
  direction.
- It does not make transitive edges part of the direct-edge allowlist; the separate
  loaded-location rule still validates every transitive package.
- It does not replace `Cargo.lock`, cargo-deny version/source policy, or the existing
  dependency-cost review; it adds a fail-closed direct-edge boundary around them.
- It does not link `lumin-xtask` into the production DAG; it removes only the checker’s
  exemption from exact dependency identity review.

## Acceptance Criteria

1. Every CI command that resolves, checks, or builds Cargo dependencies runs through the
   pinned Python 3.11+ standard-library bootstrap wrapper with `-I -S`; unwrapped Cargo,
   non-isolated Python, missing/old Python, parse failure, zero manifests, shell-based
   command reconstruction, or workflow drift fails. Only `cargo --version` is exempt.
2. The guard rejects Cargo config files, source/path override variables, every Cargo
   global `--config` argument form, and workspace manifest patch/replace tables before
   Cargo resolves or builds repository code.
3. Every resolved non-workspace registry manifest is physically under the active Cargo
   home registry source cache and outside the repository; directory replacement,
   symlink escape, missing paths, and lexical/physical disagreement fail.
   This complete-graph verdict runs in the standard-library guard before the first
   checker build, including against packages already present in the cache.
4. The root manifest declares the exact canonical `[workspace].resolver = "3"` before
   Cargo runs, and the architecture policy independently matches that same value.
5. The current locked workspace feature maps and graph, including `lumin-xtask`, match
   every canonical feature and linked declaration/resolution identity without an
   ambiguous join.
6. Changing or removing the workspace resolver fails before Cargo executes.
7. Adding or changing any workspace feature or its activation set fails.
8. Changing a Windows-only approved edge to no target or another predicate fails.
9. Renaming an approved workspace or third-party dependency fails.
10. Changing optionality, default-feature use, or the requested feature set fails.
    The compatible underscore `default_features` source spelling is rejected before
    Cargo; it cannot alias the canonical `default-features` policy identity.
11. Changing a third-party resolved version/source or substituting a non-workspace path
   package fails even when name and version appear approved.
12. A new direct edge fails even when its resolved package already has an approved owner.
13. A stale or duplicate resolver/feature/declaration/resolution policy identity fails.
14. Unknown dependency kinds and missing, ambiguous, or disagreeing joins fail rather
   than falling back to a normal edge.
15. The check remains independent of feature activation order, dependency traversal, and
   metadata insertion order.

## Required Reviews

### Design review

The repository owner must verify that the isolated pre-Cargo bootstrap, rejected Cargo
configuration channels, exact workspace resolver, graph-wide loaded-location proof,
production/development-tool feature maps, and linked declaration/resolution identities
form the intended approval boundary; that the non-goals do not weaken Rule 7; and that
the acceptance criteria cover product crates, `lumin-xtask`, direct third-party, and
transitive packages.
The verdict must name the exact amended-contract commit and is not inherited from the
earlier `ba4b181` freeze.

### Independent adversarial review

An independent reviewer must bind its verdict to the exact architecture-content commit
and attempt at least these bypasses:

- remove or change the Windows target predicate;
- rename a hyphenated dependency while preserving its normalized binding and resolved
  package;
- alter a workspace feature activation while preserving the all-features resolved
  graph;
- add or alter an xtask dependency while relying on its development-tool status;
- change optionality, default-feature use, or requested features;
- replace a registry package with a same-name/version non-workspace path package;
- replace crates.io with an altered checked-in directory source while preserving the
  reported registry source and package ID;
- inject that replacement through `cargo --config VALUE` or `cargo --config=VALUE`;
- bypass the wrapper for one dependency-reading Cargo command, remove `-I -S`, run it
  with old Python, reconstruct the command through a shell, or make it import
  repository/replacement-controlled code;
- place a source replacement in repository, ancestor, Cargo-home, environment, patch,
  or path-override configuration;
- use a symlinked registry source root or package manifest to make repository bytes look
  like Cargo-home registry bytes;
- redirect one already-cached registry package directory or manifest while keeping the
  registry root unredirected, and verify rejection before any checker compilation;
- declare `default_features = false` in an edition-2021 member and verify that the
  bootstrap rejects the spelling before Cargo may normalize it;
- replace a transitive package while leaving every direct edge unchanged;
- change or remove the root workspace resolver while preserving feature maps and edge
  declarations;
- reuse a package approval under another dependency kind;
- add a duplicate or stale policy identity;
- create a missing, ambiguous, or disagreeing declaration/resolution join;
- reorder semantically set-valued feature activations, requested feature lists, metadata
  nodes, and dependency-kind entries.

The verdict is `PASS`, `REOPEN`, or a new concrete finding and must bind the exact
amended-contract candidate commit. Author checks, the earlier freeze, prior code in an
implementation PR, and implementation tests are not independent PASS evidence.

## Verification Commands After Freeze

```text
python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo test -p lumin-xtask --locked metadata::tests
python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo run -p lumin-xtask --locked -- architecture-check
```

The implementation must prove every acceptance criterion without weakening the frozen
contract.
