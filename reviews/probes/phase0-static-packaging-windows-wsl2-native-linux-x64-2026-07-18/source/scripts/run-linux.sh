#!/usr/bin/env bash
set -euo pipefail

if [[ -d "$HOME/.cargo/bin" ]]; then
  export PATH="$HOME/.cargo/bin:$PATH"
fi

usage() {
  printf 'usage: %s --scope wsl2-ext4|native-linux-ext4 --evidence PATH\n' "$0" >&2
  exit 2
}

scope=
evidence=
while (($#)); do
  case "$1" in
    --scope)
      (($# >= 2)) || usage
      scope=$2
      shift 2
      ;;
    --evidence)
      (($# >= 2)) || usage
      evidence=$2
      shift 2
      ;;
    *) usage ;;
  esac
done
[[ "$scope" == "wsl2-ext4" || "$scope" == "native-linux-ext4" ]] || usage
[[ -n "$evidence" ]] || usage

script_root=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
source_root=$(cd "$script_root/.." && pwd)
evidence=$(python3 -c 'import os,sys; print(os.path.abspath(sys.argv[1]))' "$evidence")
packager="$script_root/package_evidence.py"

python3 "$packager" verify-source --source "$source_root"
if [[ -e "$evidence" ]]; then
  printf 'refusing existing evidence directory: %s\n' "$evidence" >&2
  exit 1
fi

uname_text=$(uname -a)
filesystem_type=$(findmnt -T "$source_root" -n -o FSTYPE)
filesystem_detail=$(findmnt -T "$source_root" -n -o TARGET,FSTYPE,SOURCE)
[[ "$filesystem_type" == "ext4" ]] || {
  printf 'ext4 source worktree required; observed: %s\n' "$filesystem_detail" >&2
  exit 1
}
if [[ "${uname_text,,}" == *microsoft* || "${uname_text,,}" == *wsl* ]]; then
  host_kind=wsl2
else
  host_kind=native-linux
fi
if [[ "$scope" == "wsl2-ext4" && "$host_kind" != "wsl2" ]]; then
  printf 'WSL2 runner required; observed: %s\n' "$uname_text" >&2
  exit 1
fi
if [[ "$scope" == "native-linux-ext4" && "$host_kind" != "native-linux" ]]; then
  printf 'native non-WSL Linux runner required; observed: %s\n' "$uname_text" >&2
  exit 1
fi
for command in cargo rustc file ldd readelf findmnt python3; do
  command -v "$command" >/dev/null || {
    printf 'required command missing: %s\n' "$command" >&2
    exit 1
  }
done

rustc_version=$(rustc --version)
rustc_verbose=$(rustc --version --verbose)
cargo_version=$(cargo --version)
[[ "$rustc_version" == "rustc 1.96.0 "* && "$cargo_version" == "cargo 1.96.0 "* ]] || {
  printf 'exact Rust 1.96.0 required; observed %s / %s\n' "$rustc_version" "$cargo_version" >&2
  exit 1
}

mkdir -p "$evidence"
quality="$source_root/target/probe-runner-logs/$scope"
mkdir -p "$quality"

export LUMIN_PROBE_SCOPE="$scope"
export LUMIN_PROBE_HOST_KIND="$host_kind"
export LUMIN_PROBE_SOURCE_ROOT="$source_root"
export LUMIN_PROBE_FILESYSTEM_DETAIL="$filesystem_detail"
export LUMIN_PROBE_RUSTC_VERSION="$rustc_version"
export LUMIN_PROBE_RUSTC_VERBOSE="$rustc_verbose"
export LUMIN_PROBE_CARGO_VERSION="$cargo_version"
export LUMIN_PROBE_UNAME="$uname_text"
python3 - <<'PY' >"$evidence/host.json"
import json
import os

print(json.dumps({
    "arch": "x86_64",
    "cargoVersion": os.environ["LUMIN_PROBE_CARGO_VERSION"],
    "ciRepository": os.environ.get("GITHUB_REPOSITORY"),
    "ciRunAttempt": os.environ.get("GITHUB_RUN_ATTEMPT"),
    "ciRunId": os.environ.get("GITHUB_RUN_ID"),
    "ciSha": os.environ.get("GITHUB_SHA"),
    "filesystemDetail": os.environ["LUMIN_PROBE_FILESYSTEM_DETAIL"],
    "filesystemType": "ext4",
    "hostKind": os.environ["LUMIN_PROBE_HOST_KIND"],
    "os": "linux",
    "rustcVerbose": os.environ["LUMIN_PROBE_RUSTC_VERBOSE"],
    "rustcVersion": os.environ["LUMIN_PROBE_RUSTC_VERSION"],
    "schema": "lumin-phase0-static-packaging-host-v1",
    "scope": os.environ["LUMIN_PROBE_SCOPE"],
    "sourcePath": os.environ["LUMIN_PROBE_SOURCE_ROOT"],
    "uname": os.environ["LUMIN_PROBE_UNAME"],
}, indent=2, sort_keys=True))
PY

cd "$source_root"
cargo fmt --all -- --check >"$quality/fmt.stdout.log" 2>"$quality/fmt.stderr.log"
cargo test --locked --target x86_64-unknown-linux-gnu \
  >"$quality/test-gnu.stdout.log" 2>"$quality/test-gnu.stderr.log"
cargo clippy --all-targets --locked --target x86_64-unknown-linux-gnu -- -D warnings \
  >"$quality/clippy-gnu.stdout.log" 2>"$quality/clippy-gnu.stderr.log"
cargo clippy --all-targets --locked --target x86_64-unknown-linux-musl -- -D warnings \
  >"$quality/clippy-musl.stdout.log" 2>"$quality/clippy-musl.stderr.log"

cargo metadata --locked --format-version 1 \
  >"$evidence/cargo-metadata.json" 2>"$evidence/cargo-metadata.stderr.log"

artifacts=()
for mode in gnu musl; do
  target="x86_64-unknown-linux-$mode"
  label="linux-$mode"
  cargo tree --locked --target "$target" \
    >"$evidence/cargo-tree-$label.txt" 2>"$evidence/cargo-tree-$label.stderr.log"
  cargo build --release --locked --target "$target" \
    >"$evidence/build-$label.stdout.log" 2>"$evidence/build-$label.stderr.log"
  binary="$source_root/target/$target/release/lumin-phase0-static-packaging-probe"
  [[ -f "$binary" ]] || {
    printf 'release artifact missing: %s\n' "$binary" >&2
    exit 1
  }
  "$binary" >"$evidence/run-$label.json" 2>"$evidence/run-$label.stderr.log"
  {
    file "$binary"
    ldd "$binary" || true
    readelf -lW "$binary"
    readelf -dW "$binary"
  } >"$evidence/linkage-$label.txt" 2>&1
  artifacts+=(--artifact "$label=$binary")
done

python3 "$packager" seal \
  --scope "$scope" \
  --source "$source_root" \
  --evidence "$evidence" \
  "${artifacts[@]}"
python3 "$packager" verify --source "$source_root" --evidence "$evidence"
printf 'PASS: %s\n' "$evidence"
