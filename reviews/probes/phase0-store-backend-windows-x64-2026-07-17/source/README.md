# Lumin Phase 0 Store Backend Probe

This is a standalone evidence harness, not a Phase 1 product scaffold. It tests the
backend-neutral lifecycle-store requirements frozen at architecture commit
`65e60216891bb3d826a4778f84cb8aaa377abe92`.

Run the Windows x64 evidence commands from this directory:

```powershell
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --release --all-features
.\target\release\lumin-phase0-store-probe.exe run --backend all --rounds 32 --output evidence\admission-windows-x64.json
.\target\release\lumin-phase0-store-probe.exe fault-matrix --backend all --watchdog-ms 30000 --output evidence\fault-matrix-windows-x64.json
powershell -ExecutionPolicy Bypass -File scripts\collect-build-metrics.ps1 -OutputPath evidence\build-surface-windows-x64.json
.\target-measure-redb\release\lumin-phase0-store-probe.exe benchmark --backend redb --records 10000 --record-bytes 256 --durable-transactions 200 --output evidence\benchmark-redb-windows-x64.json
.\target-measure-sqlite\release\lumin-phase0-store-probe.exe benchmark --backend sqlite --records 10000 --record-bytes 256 --durable-transactions 200 --output evidence\benchmark-sqlite-windows-x64.json
```

The parent-process watchdog detects a wedged backend or harness. It is test
infrastructure only and does not define a Lumin product timeout, retry cap, or degraded
analysis policy.

Windows results alone cannot select the Architecture v1 backend. Linux/musl,
filesystem, corruption, and full platform fault evidence remain blocking inputs.

See [PROBE-CONTRACT.md](./PROBE-CONTRACT.md) for the oracle, measurement method, and
rejection rules.
