#!/usr/bin/env bash
set -euo pipefail

packet_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
repo_root=$(cd "$packet_root/../../.." && pwd)
source_root="$repo_root/reviews/probes/phase0-store-backend-windows-x64-2026-07-17/source"
evidence="$source_root/evidence-native-linux"

if [[ -e "$evidence" ]]; then
  printf 'refusing existing evidence directory: %s\n' "$evidence" >&2
  exit 1
fi
mkdir -p "$evidence"

uname_text=$(uname -a)
filesystem=$(findmnt -T "$source_root" -n -o TARGET,FSTYPE,SOURCE)
if [[ "${uname_text,,}" == *microsoft* || "${uname_text,,}" == *wsl* ]]; then
  printf 'native runner required, observed: %s\n' "$uname_text" >&2
  exit 1
fi
if [[ " $filesystem " != *" ext4 "* ]]; then
  printf 'ext4 source worktree required, observed: %s\n' "$filesystem" >&2
  exit 1
fi

python3 - <<'PY' >"$packet_root/runner-provenance.json"
import json
import os
import platform

print(json.dumps({
    "schema": "lumin-phase0-native-runner-provenance-v1",
    "github_repository": os.environ.get("GITHUB_REPOSITORY"),
    "github_sha": os.environ.get("GITHUB_SHA"),
    "github_run_id": os.environ.get("GITHUB_RUN_ID"),
    "github_run_attempt": os.environ.get("GITHUB_RUN_ATTEMPT"),
    "runner_name": os.environ.get("RUNNER_NAME"),
    "runner_os": os.environ.get("RUNNER_OS"),
    "runner_arch": os.environ.get("RUNNER_ARCH"),
    "platform": platform.platform(),
}, indent=2, sort_keys=True))
PY

cd "$source_root"
cargo fmt --all -- --check
cargo test --all-features --locked
cargo clippy --all-targets --all-features --locked -- -D warnings

/usr/bin/time -f '%e' -o "$evidence/build-gnu-all-seconds.txt" \
  cargo build --release --all-features --locked \
  >"$evidence/build-gnu-all.log" 2>&1
gnu_harness="$source_root/target/release/lumin-phase0-store-probe"
"$gnu_harness" identity >"$evidence/identity-gnu.json"
"$gnu_harness" run --backend all --rounds 32 \
  --output "$evidence/admission-linux-gnu-ext4-x64.json" >/dev/null
"$gnu_harness" fault-matrix --backend all --watchdog-ms 30000 \
  --output "$evidence/fault-matrix-linux-gnu-ext4-x64.json" >/dev/null
{
  file "$gnu_harness"
  ldd "$gnu_harness"
} >"$evidence/linkage-gnu.txt" 2>&1

python3 "$packet_root/scripts/collect-build-metrics.py" \
  --source "$source_root" \
  --evidence "$evidence" \
  --mode gnu \
  --target x86_64-unknown-linux-gnu \
  --harness "$gnu_harness" \
  --output "$evidence/build-surface-linux-gnu-x64.json"

for backend in redb sqlite; do
  binary="$source_root/target-measure-gnu-$backend/release/lumin-phase0-store-probe"
  "$binary" identity >"$evidence/identity-gnu-$backend.json"
  "$binary" benchmark --backend "$backend" --records 10000 --record-bytes 256 \
    --durable-transactions 200 \
    --output "$evidence/benchmark-$backend-linux-gnu-x64.json" >/dev/null
done

/usr/bin/time -f '%e' -o "$evidence/build-musl-all-seconds.txt" \
  cargo zigbuild --release --all-features --locked \
  --target x86_64-unknown-linux-musl --target-dir "$source_root/target-musl-all" \
  >"$evidence/build-musl-all.log" 2>&1
musl_harness="$source_root/target-musl-all/x86_64-unknown-linux-musl/release/lumin-phase0-store-probe"
"$musl_harness" identity >"$evidence/identity-musl.json"
"$musl_harness" run --backend all --rounds 32 \
  --output "$evidence/admission-linux-musl-ext4-x64.json" >/dev/null
"$musl_harness" fault-matrix --backend all --watchdog-ms 30000 \
  --output "$evidence/fault-matrix-linux-musl-ext4-x64.json" >/dev/null
{
  file "$musl_harness"
  ldd "$musl_harness" || true
  readelf -l "$musl_harness"
  readelf -d "$musl_harness"
} >"$evidence/linkage-musl.txt" 2>&1

python3 "$packet_root/scripts/collect-build-metrics.py" \
  --source "$source_root" \
  --evidence "$evidence" \
  --mode musl \
  --target x86_64-unknown-linux-musl \
  --harness "$musl_harness" \
  --output "$evidence/build-surface-linux-musl-x64.json"

for backend in redb sqlite; do
  binary="$source_root/target-measure-musl-$backend/x86_64-unknown-linux-musl/release/lumin-phase0-store-probe"
  "$binary" identity >"$evidence/identity-musl-$backend.json"
  "$binary" benchmark --backend "$backend" --records 10000 --record-bytes 256 \
    --durable-transactions 200 \
    --output "$evidence/benchmark-$backend-linux-musl-x64.json" >/dev/null
  {
    file "$binary"
    ldd "$binary" || true
    readelf -l "$binary"
    readelf -d "$binary"
  } >"$evidence/linkage-musl-$backend.txt" 2>&1
done

python3 "$packet_root/scripts/package-evidence.py" \
  --source "$source_root" --evidence "$evidence"
(cd "$evidence" && sha256sum --check SHA256SUMS)

cp -a "$evidence" "$packet_root/evidence"
