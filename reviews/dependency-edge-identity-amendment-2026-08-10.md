# REVIEW-003: Dependency-Surface Policy Amendment

Document role: focused Architecture v1 amendment and freeze gate

Status: **CANDIDATE**. This text has no implementation authority until the repository
owner approves one exact architecture-content commit and an independent adversarial
review passes that same commit.

## Decision

Lumin freezes the workspace resolver, workspace feature maps, dependency declarations,
and their resolved direct packages. It does not build a second package manager, a
hermetic build system, or an in-repository root of trust.

The bootstrap guard owns this dependency-policy verdict before a CI Cargo command may
compile repository or dependency code. `lumin-xtask architecture-check` owns the
remaining repository architecture checks and does not spawn Cargo to authenticate the
dependencies used to build itself.

## Problem

An approval identified only by owner package, resolved package name, and dependency kind
can be reused after a material graph change. Concrete examples are:

- moving a Windows-only dependency to an unconditional edge;
- renaming a dependency while retaining the same resolved package;
- changing optionality, default-feature use, or requested features;
- changing a workspace feature activation while all features happen to be enabled;
- substituting a path, Git, or alternate-registry package for an approved crates.io or
  workspace dependency;
- changing a `lumin-xtask` dependency under a development-tool exemption; or
- changing `[workspace].resolver` and therefore Cargo feature unification.

Those are dependency-policy failures. A compromised hosted runner, malicious compiler,
host linker injection, adversarial test sandboxing, and byte-for-byte authentication of
Cargo's extracted registry cache are different problems and are not claimed here.

## Trust Boundary

Public CI trusts:

- protected review and the reviewed Git commit as the authority for changes to the
  workflow, guard, policy, manifests, and lockfile;
- fresh GitHub-hosted runner isolation and pinned setup actions;
- the exact Rust toolchain selected by `rust-toolchain.toml` and CI setup;
- Cargo's locked resolution and crates.io checksum verification; and
- the official crates.io source identity accepted by Cargo and cargo-deny.

The repository does not claim that a mutable guard can cryptographically authenticate
itself or a policy changed in the same pull request. Machine checks expose drift and
make the reviewed surface exact; protected review authorizes an intentional policy
change. A coherent compromise of the named external trust roots is outside this
in-repository architecture boundary.

The following inputs are untrusted until the guard admits them:

- workspace manifests and `Cargo.lock`;
- repository, ancestor, and active-Cargo-home Cargo configuration;
- Cargo source, path, registry, toolchain, and compiler-selection environment overrides;
- the Cargo argument vector; and
- the checked-in dependency policy.

## Contract

### 1. Authoritative CI flow

Every public-CI command that resolves, checks, builds, tests, documents, or lints Cargo
dependencies runs as:

```text
<PINNED_PYTHON> -I -S tools/xtask/bootstrap/source_provenance.py -- cargo ...
```

`PINNED_PYTHON` is the absolute interpreter path provisioned by the pinned Python setup
action. The sole exception is a non-resolving version probe. `cargo-audit` and
`cargo-deny` are installed by pinned CI actions and invoked directly rather than through
Cargo external subcommand lookup.

For each admitted Cargo invocation, the Python 3.11+ standard-library guard:

1. validates its environment, repository root, command surface, manifests, lockfile,
   and checked-in policy without importing repository modules;
2. runs exact locked, all-features Cargo metadata with the selected CI toolchain and no
   shell;
3. compares the complete workspace feature and direct-dependency surface with the
   policy;
4. verifies the resolved source and loaded location rules below; and
5. only then launches the original Cargo command without a shell.

Normal validation never rewrites the policy. A maintenance mode may print a canonical
candidate policy to stdout, but updating the checked-in artifact is an explicit reviewed
change.

### 2. Input admission

Before running metadata, public CI fails closed on:

- Python older than 3.11 or execution without both `-I` and `-S`;
- a working directory or root manifest other than the physical repository root;
- `.cargo/config.toml` or `.cargo/config` in the repository, a Cargo-searched ancestor,
  or the active Cargo home;
- `CARGO_SOURCE_*`, `CARGO_PATHS`, registry-index overrides, `CARGO`, `RUSTC`,
  `RUSTC_WRAPPER`, `RUSTC_WORKSPACE_WRAPPER`, or `RUSTUP_TOOLCHAIN` inherited by the
  guarded CI command;
- `[patch]` or `[replace]` in the root or a workspace member manifest;
- Git dependencies, alternate registries, or a path dependency that does not resolve
  directly to an exact workspace member;
- Cargo `--config`, `--manifest-path`, `--lockfile-path`, `-Z`, or rustup
  `+toolchain` relocation in split, equals, or attached form as applicable;
- a missing or non-exact root `[workspace].resolver = "3"` value;
- an implicit root package or explicit workspace member omitted from policy comparison;
  or
- parse failure, duplicate policy JSON keys, unknown policy fields, zero workspace
  members, or an incomplete metadata result.

Cargo option parsing ends at the first literal `--`; later tokens are test-harness or
launched-program input and cannot be reclassified as Cargo `--target` or relocation
options. The required Linux musl release target remains an admitted distribution lane;
this amendment neither schedules nor replaces its package acceptance owner.

Local runs use the same semantic checks but are diagnostic only. Public CI remains merge
authority because local Cargo homes and host state are not controlled evidence.

### 3. Canonical policy identity

`tools/xtask/dependency-surface-policy.v2.json` is a small, human-reviewable artifact.
It contains only:

- schema version and exact workspace resolver;
- the exact workspace-member set, with each member classified as production or the
  single development tool `lumin-xtask`;
- every member's exact feature-name to canonical activation-set map;
- the complete `[workspace.dependencies]` catalog, including currently unused entries;
  and
- every direct normal, build, and development dependency declaration linked to one
  resolved destination.

The authored declaration identity is:

```text
(
  owner workspace member,
  dependency kind,
  exact optional target predicate,
  exact authored alias,
  dependency package name,
  exact authored version requirement,
  optionality,
  default-feature setting,
  canonical requested-feature set,
  source kind
)
```

The guard reads the authored alias and requirement from strict-parsed TOML rather than
reconstructing them from Cargo-normalized names. Absent values remain distinct from
present empty values. Feature activation and request lists are sets and are sorted and
deduplicated; strings whose Cargo meaning is not set-like remain exact.

The linked resolution identity is:

```text
workspace dependency: (exact destination workspace member)
third-party dependency: (exact crates.io package name, version, source)
```

Every manifest declaration and every resolved direct binding has exactly one policy
counterpart. Missing, additional, duplicate, stale, ambiguous, or disagreeing identities
fail. Unknown dependency kinds fail. `lumin-xtask` has a separate class but receives the
same exact checks; development-only is not an exemption.

`Cargo.lock` is the canonical transitive package/version/source/checksum pin. The policy
does not duplicate every transitive package definition, target, feature, or resolution
lane. A lockfile change is an explicit reviewed dependency change and remains subject to
`--locked`, cargo-deny, and the existing dependency-cost review.

### 4. Resolved source and loaded location

The authoritative dependency-policy job uses a fresh job-private Cargo home and does not
restore a dependency cache. Cargo downloads locked registry packages and performs its
normal checksum verification.

For every non-workspace package in metadata, the guard requires:

- the official crates.io registry source;
- an absolute manifest path under the active Cargo home's `registry/src`; and
- lexical and physical registry/package paths that agree without symlink or junction
  redirection, with the package physically outside the repository.

A missing source, directory source, Git source, alternate registry, non-workspace path
package, repository-contained manifest, or path outside the active registry source
fails. Workspace path dependencies must resolve to the exact admitted member.

The guard does not parse `.crate` archives, compare extracted trees byte for byte,
authenticate registry index transport, or attest host filesystem internals. Fresh hosted
jobs, Cargo checksum verification, the lockfile, and the named external trust roots own
that boundary.

### 5. Ownership and CI routing

The Python bootstrap owns dependency admission. The Rust architecture checker must not
spawn Cargo or call the bootstrap again to recreate a stronger verdict after it has
started. Its source-policy check verifies that the reviewed workflow routes every
dependency-resolving Cargo command through the guard and that audit/deny use their pinned
direct tools.

That workflow check prevents accidental routing drift; it is not a cryptographic defense
against a pull request that changes the workflow and checker together. Protected review
owns that authorization.

Test suites that can contaminate module-load configuration, process-global state, DOM
mode, or sockets remain separate processes under Rule 15. This amendment does not turn
ordinary test batches into security sandboxes or require one executable per CI job.

### 6. Failure semantics

Missing, malformed, stale, unsupported, ambiguous, or incomplete policy evidence yields
no dependency verdict and no guarded Cargo launch. The guard prints the first owned
reason and exits nonzero. It never converts unavailable metadata or an unknown manifest
shape into an empty dependency set.

## Non-Goals

- Hermetic, reproducible, or hostile-runner builds.
- Compiler, linker, SDK, temporary-directory, or every Cargo environment-channel
  attestation.
- A custom crates.io archive extractor, registry-index mirror, or extracted-tree
  authenticator.
- A duplicate snapshot of the complete transitive graph already pinned by `Cargo.lock`.
- Freezing unrelated Cargo package fields, targets, profiles, lints, or benchmark flags
  under a dependency-edge owner.
- Preventing malicious repository runtime code from attacking another process in the
  same CI job.
- Making an in-repository digest or checker its own external trust root.
- Owning the Linux musl packaging schedule or public binary smoke tests.

These exclusions narrow the claim; they do not authorize a new dependency, source,
feature, edge, or runtime fallback.

## Acceptance Criteria

1. Public CI installs pinned Python 3.11+ and the exact Rust toolchain, uses a fresh
   dependency-policy Cargo home, and routes every dependency-resolving Cargo command
   through the isolated no-shell guard.
2. Config/source/path/toolchain overrides, manifest relocation, `[patch]`, `[replace]`,
   Git, alternate-registry, and non-member path dependencies fail before compilation.
3. The exact workspace resolver, complete member set, and every workspace feature map
   match the canonical policy.
4. Changing an edge's kind or target predicate fails.
5. Changing an authored alias or version requirement fails even if Cargo normalizes it
   to the same binding or resolves the same package.
6. Changing optionality, default-feature use, or requested features fails independent
   of feature-list order.
7. A new, removed, duplicated, stale, ambiguous, or unresolved direct edge fails for
   production members and `lumin-xtask` alike.
8. A third-party edge binds exact crates.io name, version, and source; a same-name path,
   Git, alternate-registry, or repository-contained replacement fails.
9. Every non-workspace metadata manifest is below the active Cargo registry source and
   outside the repository; unavailable or incomplete metadata cannot authorize launch.
10. `Cargo.lock`, `--locked`, cargo-deny, and reviewed lockfile diffs remain the sole
    transitive graph authority; no second transitive policy is required.
11. Runtime arguments after `--` are not misparsed as Cargo options, while actual Cargo
    relocation before `--` fails and the required musl target remains admissible.
12. The Rust architecture checker performs no nested provenance or Cargo invocation;
    guard success plus checker success form the CI architecture verdict.
13. Reordering Cargo metadata nodes, feature sets, or dependency-kind entries does not
    change the verdict.
14. A normal validation run never edits the policy, manifests, lockfile, Cargo home
    configuration, or repository source.

## Required Reviews

### Design review

The repository owner must approve the exact candidate commit and confirm that the trust
boundary, direct-edge identity, transitive-lock ownership, development-tool treatment,
failure semantics, and non-goals match the intended Rule 7 enforcement. Directional
approval or an earlier commit is not a freeze verdict.

### Independent adversarial review

An independent reviewer must bind its verdict to that same exact commit and attempt at
least:

- target-predicate removal, dependency rename, requirement change, optionality change,
  default-feature change, requested-feature change, and workspace-feature change;
- the same changes on `lumin-xtask`;
- a same-name workspace/non-workspace path substitution, Git source, alternate registry,
  Cargo config replacement, environment source override, and both `--config` forms;
- a changed resolver, missing member, implicit root package, unused workspace dependency,
  duplicate JSON key, stale policy row, duplicate edge, and ambiguous declaration join;
- a registry manifest outside the active Cargo home or inside the repository;
- metadata and feature/dependency traversal reordering;
- a runtime `--target` token after `--` and the exact supported musl Cargo target before
  `--`; and
- removal of the guard from one dependency-resolving CI command or restoration of a
  dependency cache in the authoritative policy job.

The reviewer must also verify that the document does not claim hermeticity, hostile-code
containment, archive-byte authentication, or self-authentication. The verdict is
`PASS`, `REOPEN`, or a new concrete finding. Author tests and an earlier clean review are
not independent evidence.

## Verification After Freeze

The implementation plan must preserve focused checks for the guard and policy before
running broader Cargo validation. At minimum:

```text
<PINNED_PYTHON> -I -S tools/xtask/bootstrap/test_source_provenance.py
<PINNED_PYTHON> -I -S tools/xtask/bootstrap/source_provenance.py --check-only
<PINNED_PYTHON> -I -S tools/xtask/bootstrap/source_provenance.py -- cargo test -p lumin-xtask --locked metadata::tests
<PINNED_PYTHON> -I -S tools/xtask/bootstrap/source_provenance.py -- cargo run -p lumin-xtask --locked -- architecture-check
```

Exact commands may be narrowed by the frozen implementation plan, but no test may claim
PASS for an acceptance criterion outside its observable surface.
