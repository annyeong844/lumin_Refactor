# REVIEW-003: Dependency-Edge Identity Amendment

Document role: focused Architecture v1 amendment and freeze gate

Status: **REOPENED** by independent GitHub review `4897134273` of
`9ec5952395ce92fd7445b76019fd280737c3b882`. Its four actionable threads reject Cargo
target/toolchain argument overrides, an undefined cross-platform archive-path identity,
and unauthenticated registry resolution facts. The repository owner's approval of
`9ec5952` does not carry to this amended candidate. Implementation and merge remain
blocked until new owner and independent verdicts bind the next exact
architecture-content commit.

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
- replace an extracted registry package in place with ordinary directories and files at
  its expected cache path, preserving every metadata source and path identity while
  changing the bytes compiled into the checker;
- point `RUSTC`, `RUSTC_WRAPPER`, or their Cargo configuration environment aliases at
  an executable that mutates the cache during metadata's compiler-information probe.
- pass `--target-dir` to escape the job-private target directory or `cargo +toolchain`
  to select a compiler other than the pinned repository toolchain;
- use archive names that collide only under a host filesystem's case or Unicode rules,
  so two platforms derive different source maps from the same authenticated archive;
- alter crates.io index dependency or feature facts while preserving locked package
  identities, checksums, and authentic archive bytes.

These changes alter platforms, the default graph, crate-visible API names, dependency
code, build cost, or the transitive surface. They are concrete fail-open policy bypasses
and therefore reopen only this part of ARCH-000.

## Amended Contract

The CI dependency boundary has a pre-Cargo source-provenance stage and a post-metadata
location stage. The pre-Cargo stage is the checked-in
`tools/xtask/bootstrap/source_provenance.py` guard. It requires Python 3.11 or newer,
uses only the standard library (including `tomllib`), and imports no repository module.
Registry byte verification is a separate, directly executed
`tools/xtask/bootstrap/registry_snapshot.py` program rather than an import; the guard
also delegates canonical metadata comparison through stdin to the separately executed
`tools/xtask/bootstrap/metadata_snapshot.py`. The guard verifies both helpers' exact
digests before launching them with the same `python -I -S` interpreter; neither helper
imports the guard or another repository module. These are three cohesive bootstrap trust
artifacts owned beside xtask, not product/runtime analysis paths. A Cargo-built checker
cannot replace this stage because replacement code could alter the checker before
admission.

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
- `RUSTC`, `RUSTC_WRAPPER`, `RUSTC_WORKSPACE_WRAPPER`, `RUSTDOC`, `RUSTFLAGS`,
  `RUSTDOCFLAGS`, `RUSTC_BOOTSTRAP`, `RUSTUP_TOOLCHAIN`,
  `CARGO_ENCODED_RUSTFLAGS`, every matching `CARGO_UNSTABLE_*`,
  `CARGO_BUILD_RUSTC*`/`CARGO_BUILD_RUSTDOC*`/`CARGO_BUILD_RUSTFLAGS`,
  `CARGO_BUILD_TARGET`, `CARGO_BUILD_TARGET_DIR`, and target rustflags, linker, or runner
  configuration environment variable; the pinned workflow and `rust-toolchain.toml` own
  compiler selection, flags, target, and output location;
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
- Cargo `--target-dir VALUE` or `--target-dir=VALUE` before the argument vector's first
  literal `--`; arguments after that delimiter belong to the selected Cargo subcommand
  and cannot reconfigure Cargo's artifact directory;
- Cargo `--manifest-path`, `--lockfile-path`, `--target`, `--artifact-dir`, `-C`,
  `--directory`, or `-Z` in split or equals/attached form before the first literal `--`;
  the guard runs from the exact physical repository root and admits only its pinned
  manifest, lockfile, stable feature surface, and native supported-host resolution lane;
- a rustup `+toolchain` selector immediately after the exact `cargo` executable; the
  guard accepts no alternate executable or command-line toolchain identity;
- a missing or non-exact root `[workspace].resolver = "3"` declaration.

The bootstrap also requires `.github/workflows` to contain only the reviewed `ci.yml`
and verifies its exact digest before every guarded Cargo invocation. Every Cargo job
sets `CARGO_HOME` and `CARGO_TARGET_DIR` to its job-private
`${{ runner.temp }}/lumin-cargo-home` and `${{ runner.temp }}/lumin-target`; no
repository cache action restores either directory and the guard rejects another CI
location. A Cargo argument or configuration alias cannot override the admitted target
directory. Local execution may use its active Cargo home and target, but receives the
same source-byte verification and no public-CI authority.

This boundary trusts the GitHub-hosted runner kernel and filesystem, the exact pinned
checkout/setup/install actions, and the rustup-distributed Cargo and Rust binaries reached
through the unchanged workflow `PATH`. Version/host probes confirm that trusted binary
identity before metadata. It does not claim to detect a compromised runner, forged
toolchain binary, or an administrator who changes branch protection and approves a
replacement workflow; those are outside an in-repository architecture check's possible
authority. Inside that trusted host boundary, repository, cache, command, environment,
archive, and registry-resolution drift fail closed.

Before any Cargo command may compile repository or dependency code, the guard first
rejects compiler/configuration overrides, runs exact `cargo -Vv` and `rustc -Vv` probes,
and requires Cargo release `1.96.0`/commit
`30a34c6821b57de0aaec83a901aca39f88f6778c`, rustc release `1.96.0`/commit
`ac68faa20c58cbccd01ee7208bf3b6e93a7d7f96`, and one common exact host
`x86_64-pc-windows-msvc` or `x86_64-unknown-linux-gnu`. It then runs the exact
non-compiling probes `cargo metadata --all-features --locked --format-version 1` and
`cargo metadata --all-features --locked --format-version 1 --filter-platform
<exact-host>` without a shell in the controlled environment. The unfiltered probe owns
the complete locked package-definition and cache surface; the filtered probe owns the
native-host resolution lane. Failure, non-JSON output, a toolchain/host mismatch, or an
incomplete package surface in either result yields no permission to launch the requested
command. Both metadata workspace-member ID and manifest-path sets must match the complete
manifest set already parsed by the guard.
The exact root `Cargo.lock` SHA-256
`fb453115d14301253b6a1670bee8af05e2d9919b654e37748d47359b87dfd27c`
is part of the bootstrap policy. Each resolved registry package must have one exact
crates.io lock row with a SHA-256 checksum, and the metadata registry identity set must
equal the complete crates.io identity set in that pinned lockfile. This equality uses
the unfiltered result; a host-filtered result is expected to omit packages belonging only
to other target predicates and cannot own the complete-cache claim.

The digest-pinned metadata helper then compares a stdin envelope containing both
metadata results and the pinned lock identity/checksum rows against
`tools/xtask/dependency-surface-policy.v2.json` before any compilation. The guard pins
that artifact's digest, strict-parses it, rejects unknown or duplicate fields, and
canonicalizes metadata IDs and paths to stable workspace or registry identities. The
policy contains these exhaustive, deterministically sorted surfaces:

- one shared definition surface from unfiltered metadata for every workspace and
  registry package, including package identity/checksum, optional `links` and
  `rust_version`, its complete feature-definition map, and every Cargo 1.96 dependency
  field: `name`, `rename`, `req`, `source`, `registry`, stable `path` identity, `kind`,
  `target`, `optional`, `uses_default_features`, and the requested `features` set;
- exact Windows and Linux host lanes from filtered metadata, each containing every
  `resolve.nodes[]` entry with the stable package identity, exact enabled feature set,
  and every dependency binding to a stable destination identity with all `dep_kinds[]`
  kind/target pairs, plus the exact optional root identity.

Absent values remain distinct from empty values, sets are sorted only where Cargo
defines order as irrelevant, and any missing, additional, duplicate, or changed package,
feature, dependency, binding, kind, target, or resolved feature fails. This snapshot
binds the complete native-host all-features transitive interpretation used by the
admitted command; a proxy, CA override, or altered registry index can at most cause a
mismatch and cannot
authorize compilation by preserving lock identities and archive checksums alone. The
Rust architecture check independently compares the same v2 policy after it is built.

The digest-verified registry helper then authenticates both cached representations for
every resolved registry package. Its `registry/cache/<registry>/<name>-<version>.crate`
archive must exist as an unredirected regular file and hash to the lock checksum. The
helper reads, but never extracts, the gzip tar archive as 512-byte USTAR records. It
accepts only regular-file type `0` and headers whose six magic plus two version bytes are
exactly `ustar\0` + `00` or `ustar ` + ` \0`; extension, link, device, directory, and
unknown record types fail. Header name and prefix fields contain ASCII followed only by
NUL padding, combine with `/` as the sole separator, start with exactly
`<package-name>-<version>/`, and contain only
components matching `[A-Za-z0-9._+-]+`. Empty, `.`/`..`, trailing-dot, and ASCII
case-insensitive Windows device-name components fail. Duplicate identity uses an exact
ASCII-lowercase key; because non-ASCII is rejected, Unicode normalization and host
casefolding cannot alter the result. Numeric fields use NUL/space-padded ASCII octal,
the header checksum is recomputed with its field treated as spaces, file data is padded
to 512-byte blocks, and exactly two terminal zero blocks end the decompressed stream.
Any collision, noncanonical field, short data, extra zero block, or trailing payload
fails. The helper streams a trusted
path/content/executable-bit map from the accepted regular records. The
corresponding `registry/src/<registry>/<name>-<version>` tree must contain no filesystem
hard-link alias, be unredirected,
remain outside the repository, and match that map exactly by relative path, regular-file
bytes, and executable bit where the host filesystem represents that bit. Missing or
extra files/directories fail; the sole permitted Cargo-created extra is the unredirected
regular root `.cargo-ok` marker, whose bytes
never authorize source content. This layout contract is exact for the pinned Cargo
1.96.0 floor and fails closed if Cargo changes the representation.

Only after the same guard process receives that byte verdict may it launch the supplied
build, test, lint, audit, or deny vector with the controlled environment and without a
shell. A job-private home, no untrusted repository execution before the first verdict,
archive authentication, and exact extracted-tree comparison prevent a pre-existing
host cache or in-place replacement from supplying compiler input. `--check-only`
performs the identical metadata and byte preflight; it is not a manifest-only shortcut.

The architecture job invokes that full metadata-and-byte guard independently before
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

The architecture check then reads the same complete unfiltered and native-host-filtered
metadata surfaces. It resolves the active Cargo home and repository root physically
before trusting package locations.
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
`"3"` and compares the complete v2 package-definition and resolved-graph snapshots,
including every workspace package's `packages[].features` map. The resolver value is an
exact policy identity, not an author-environment default. Feature names are exact and
each activation list is a sorted, deduplicated set. An absent `default` feature is
distinct from an empty `default` feature. This preserves the declared feature graph,
transitive registry interpretation, and unification contract even though all-features
resolution activates every available feature.

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
- It does not make transitive edges part of the direct-edge allowlist; their separate
  exact definition/resolution snapshot and loaded-location rules prevent drift without
  granting a new direct owner.
- It does not replace `Cargo.lock`, cargo-deny version/source policy, or the existing
  dependency-cost review; it adds a fail-closed direct-edge boundary around them.
- It does not link `lumin-xtask` into the production DAG; it removes only the checker’s
  exemption from exact dependency identity review.

## Acceptance Criteria

1. Every CI command that resolves, checks, or builds Cargo dependencies runs through the
   pinned Python 3.11+ standard-library bootstrap wrapper and digest-pinned metadata and
   registry helpers with `-I -S`; unwrapped Cargo, non-isolated Python, missing/old
   Python, parse
   failure, zero manifests, shell-based command reconstruction, or workflow drift fails.
   Only `cargo --version` is exempt. Public CI uses only its exact job-private Cargo home
   and target directory, with neither restored from a cache. `--target-dir`, its Cargo
   configuration alias, rustup `+toolchain`, workspace/lockfile relocation, artifact
   relocation, unstable Cargo flags, and target selection cannot override those
   identities. Cargo and rustc release/commit identities are exact for 1.96.0 and the
   common reported host selects only its named Windows or Linux policy lane.
2. The guard rejects Cargo config files, source/path override variables, every Cargo
   global `--config` argument form, and workspace manifest patch/replace tables before
   Cargo resolves or builds repository code. Compiler, wrapper, documentation, flag,
   target-linker, target-runner, and toolchain environment overrides also fail before
   metadata can execute a substituted program.
3. Every resolved non-workspace registry manifest is physically under the active Cargo
   home registry source cache and outside the repository; directory replacement,
   symlink escape, missing paths, and lexical/physical disagreement fail.
   Before the first checker build, its exact `.crate` archive hashes to the checksum in
   the pinned lockfile and its complete extracted tree matches the authenticated archive
   path/content/executable-bit map with no unsupported or extra member.
   Archive path parsing uses the exact ASCII USTAR grammar and ASCII-lowercase collision
   identity on every host.
4. The root manifest declares the exact canonical `[workspace].resolver = "3"` before
   Cargo runs, and the architecture policy independently matches that same value.
5. The current locked unfiltered package-definition snapshot and native-host
   all-features resolution lane, including all transitive packages and `lumin-xtask`,
   match every canonical feature and linked declaration/resolution identity without an
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
16. Altering registry dependency/feature facts while preserving lock rows, checksums,
    and archive bytes fails the complete v2 definition/resolution snapshot before any
    build.

## Required Reviews

### Design review

The repository owner must verify that the isolated pre-Cargo bootstrap, controlled
compiler environment, job-private Cargo home, archive/extracted-tree byte proof, rejected
Cargo argument/configuration channels, exact portable archive grammar, exact workspace
resolver, graph-wide loaded-location proof, complete transitive definition/resolution
snapshot, production/development-tool feature maps, and linked declaration/resolution
identities form the intended approval boundary; that the non-goals do not weaken Rule 7;
and that the acceptance criteria cover product crates, `lumin-xtask`, direct third-party,
and transitive packages.
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
- modify an ordinary extracted source file in place, add a source file, alter the archive
  while preserving the tree, and alter the tree while preserving the archive; each must
  fail against the pinned lock checksum and exact archive map before compilation;
- inject `RUSTC`, both Rust compiler wrappers, their `CARGO_BUILD_*` aliases, rustflags,
  a target linker/runner, or `RUSTUP_TOOLCHAIN` and verify that metadata never invokes it;
- pass `--target-dir` in split and equals forms, set `CARGO_BUILD_TARGET_DIR`, and pass
  `cargo +stable` or a custom linked toolchain; verify that no metadata or build command
  receives the override;
- pass `--manifest-path`, `--lockfile-path`, `--target`, `--artifact-dir`, `-C`,
  `--directory`, or `-Z`, or set `CARGO_BUILD_TARGET`, `CARGO_UNSTABLE_*`, or
  `RUSTC_BOOTSTRAP`; verify repository root, lockfile, stable Cargo surface, artifact
  location, and native host lane cannot move;
- create archive members differing by ASCII case, non-ASCII/normalization, separator,
  dot component, trailing dot, device name, or unsupported USTAR record type and verify
  the same rejection on Windows and Linux;
- alter registry index dependency, dependency-kind/target, or feature facts while
  preserving the lock identity/checksum and authentic archive; verify that the v2
  package-definition or resolved-graph snapshot rejects it before compilation;
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
implementation PR, and implementation tests are not independent PASS evidence. A review
object appearing before its inline threads is not a clean verdict; freeze waits until the
exact review exposes zero inline comments and the reviewer emits its affirmative `PASS`
text or documented clean-review reaction. Absence observed before that positive signal
is pending, not clean.

## Verification Commands After Freeze

```text
python -I -S tools/xtask/bootstrap/test_registry_snapshot.py
python -I -S tools/xtask/bootstrap/test_metadata_snapshot.py
python -I -S tools/xtask/bootstrap/test_source_provenance.py
python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo test -p lumin-xtask --locked metadata::tests
python -I -S tools/xtask/bootstrap/source_provenance.py -- cargo run -p lumin-xtask --locked -- architecture-check
```

The implementation must prove every acceptance criterion without weakening the frozen
contract.
