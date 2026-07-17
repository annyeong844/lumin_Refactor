# Lumin Phase 0 Store Backend Probe

This is a standalone evidence harness, not a Phase 1 product scaffold. It tests the
backend-neutral lifecycle-store requirements frozen at architecture commit
`65e60216891bb3d826a4778f84cb8aaa377abe92`.

Run the Windows x64 evidence commands from this directory:

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --release --all-features
.\target\release\lumin-phase0-store-probe.exe run --backend all --rounds 32 --output evidence\admission-windows-x64.json
.\target\release\lumin-phase0-store-probe.exe fault-matrix --backend all --watchdog-ms 30000 --output evidence\fault-matrix-windows-x64.json
powershell -ExecutionPolicy Bypass -File scripts\collect-build-metrics.ps1 -OutputPath evidence\build-surface-windows-x64.json
.\target-measure-redb\release\lumin-phase0-store-probe.exe benchmark --backend redb --records 10000 --record-bytes 256 --durable-transactions 200 --output evidence\benchmark-redb-windows-x64.json
.\target-measure-sqlite\release\lumin-phase0-store-probe.exe benchmark --backend sqlite --records 10000 --record-bytes 256 --durable-transactions 200 --output evidence\benchmark-sqlite-windows-x64.json
powershell -ExecutionPolicy Bypass -File scripts\package-evidence.ps1 -EvidencePath evidence
```

The parent-process watchdog detects a wedged backend or harness. It is test
infrastructure only and does not define a Lumin product timeout, retry cap, or degraded
analysis policy.

The package step rejects missing, duplicated, failed, or renamed fault cases, dishonest
namespace outcomes, admission truth-table drift, source-manifest disagreement, and any
report whose executable SHA-256/size differs from the measured release binary.

The same oracle can be exercised on an ext4-hosted Linux worktree:

```bash
cargo fmt --all -- --check
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo build --release --all-features
./target/release/lumin-phase0-store-probe run --backend all --rounds 32 \
  --output evidence/admission-linux-ext4-x64.json
./target/release/lumin-phase0-store-probe fault-matrix --backend all \
  --watchdog-ms 30000 --output evidence/fault-matrix-linux-ext4-x64.json
```

For musl packaging, use a pinned cross-linker and
`x86_64-unknown-linux-musl`; verify the resulting ELF has no interpreter or dynamic
`NEEDED` entries and execute its live `identity` command. WSL2 ext4/musl results are
valuable platform evidence but do not replace native Linux package/filesystem proof.
No single-platform result can select the Architecture v1 backend.

See [PROBE-CONTRACT.md](./PROBE-CONTRACT.md) for the oracle, measurement method, and
rejection rules.
