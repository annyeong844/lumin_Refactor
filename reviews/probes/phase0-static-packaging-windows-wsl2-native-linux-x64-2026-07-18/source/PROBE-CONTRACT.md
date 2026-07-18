# Frozen Static-Packaging Probe Contract

Architecture candidate: `9a0dbe5c89463892c001e864c4f18eeab9e0eaed`

Architecture manifest SHA-256:
`e2ca379a8a659f2febbc4e277c89db67bb02035a6b10467cf78a5663f21cd99a`

## Positive Oracle

Every scope must use the exact checked-in source manifest and lockfile under Rust
`1.96.0`. The release artifact must:

1. be x86-64 PE32+ for `x86_64-pc-windows-msvc`, or x86-64 ELF for Linux;
2. parse the constant TypeScript fixture with exact OXC `0.126.0` without errors;
3. execute a two-thread local Rayon pool and return the expected sum `4950`;
4. create, commit, reopen, and read one value through exact redb `4.1.0` in a temporary
   database, then remove the database;
5. report the compiled OS, architecture, and target environment;
6. report the exact source-manifest SHA-256 and frozen architecture commit/manifest
   compiled into the executable;
7. be hashed, copied into a fresh seal-owned execution directory, rehashed, directly
   inspected as PE/ELF, executed from that exact copy, and rehashed after execution by
   one sealing boundary;
8. have exactly one Cargo `links` declaration,
   `rayon-core@1.13.0:rayon-core`; its pinned build script states that it links no
   native library and uses the key only as a one-version uniqueness sentinel;
9. for GNU, contain a program interpreter and dynamic `NEEDED` entry; for musl,
   contain neither.

Windows, WSL2 GNU/musl, and native Linux GNU/musl must all emit the same schema and
dependency-smoke values. Host identity, filesystem, exact toolchain identity, Cargo
metadata/tree, raw build output, exact-artifact run output, direct binary inspection,
pre/post-execution SHA-256/size, and source manifest identity are retained. Pre-existing
run, inspection, execution, summary, or manifest files are stale input and make sealing
fail before artifact authorization.

## Hard Stops

The probe fails rather than degrading when:

- the source manifest or lockfile differs;
- the host/scope or filesystem class is wrong;
- a target or exact toolchain is unavailable;
- Cargo resolves a different direct dependency version;
- Cargo reports any `links` declaration other than the exact Rayon Core uniqueness
  sentinel;
- build, startup, OXC, Rayon, or redb smoke fails;
- a musl artifact has an interpreter or dynamic dependency;
- a GNU artifact lacks its expected interpreter/dynamic dependency;
- the exact execution copy differs before or after invocation;
- run output omits or disagrees with the source or architecture identity;
- retained run/inspection/execution evidence differs from a fresh invocation of the
  detached artifact supplied to verification;
- an unrelated native executable, a source-identity-tampered executable, stale generated
  output, or a dynamic GNU binary labeled as musl is accepted;
- required raw evidence is absent or not listed by `SHA256SUMS`.

## Non-Claims

This harness is not a Lumin implementation or package. It must not:

- use the executable name `lumin`;
- expose or emulate any public Lumin API, command, DTO, gate, query, or process flow;
- read or analyze a repository;
- implement path/root codecs or platform round trips;
- contain Codex/Claude skill logic;
- claim runtime-without-Cargo product acceptance;
- approve product time, memory, stack, jobs, or binary-size budgets.

Those are Phase 1 product acceptance surfaces. This probe only tests static packaging
and dependency viability before implementation begins.
