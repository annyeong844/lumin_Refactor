# Lumin Phase 0 OXC Memory and Worker-Stack Probe

This directory is a standalone feasibility harness. It is not a Phase 1 product
scaffold and exposes no Lumin product API.

The harness parses one exact Git corpus with OXC `0.126.0`. Every Rayon worker owns
its source bytes, OXC allocator, AST, and traversal state. It returns only project-owned
scalar summaries after dropping the parser result and allocator.

Prepare the named corpus from the exact legacy Git object database:

```powershell
python scripts\prepare-corpus.py `
  --repo C:\path\to\lumin-repo-lens-lab `
  --output work\corpus `
  --manifest work\corpus-manifest.json
```

Build and run the Windows matrix:

```powershell
cargo fmt --all -- --check
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo build --release --locked
python scripts\run-matrix.py `
  --binary target\release\lumin-phase0-oxc-probe.exe `
  --corpus-root work\corpus `
  --manifest work\corpus-manifest.json `
  --output evidence\windows `
  --platform windows-x64 `
  --filesystem-class ntfs
```

Run the same source and corpus bytes from an ext4-hosted WSL2 worktree for the WSL2
matrix. The packager verifies both matrices and writes the cross-platform summary and
evidence manifest. No matrix command has an elapsed-time cutoff.

See [PROBE-CONTRACT.md](./PROBE-CONTRACT.md) for exact invariants and limitations.

