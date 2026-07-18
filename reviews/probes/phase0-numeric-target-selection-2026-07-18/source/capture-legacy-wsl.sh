#!/usr/bin/env bash
set -euo pipefail

expected_commit='35290cb683a37e83bc8c915d1d0f9ca0f3f96fd0'
repo_root="${1:?usage: capture-legacy-wsl.sh REPO_ROOT OUTPUT_ROOT cold|warm}"
output_root="${2:?usage: capture-legacy-wsl.sh REPO_ROOT OUTPUT_ROOT cold|warm}"
mode="${3:?usage: capture-legacy-wsl.sh REPO_ROOT OUTPUT_ROOT cold|warm}"

if [[ "$mode" != cold && "$mode" != warm ]]; then
  printf 'mode must be cold or warm\n' >&2
  exit 2
fi
repo_root="$(realpath "$repo_root")"
if [[ -e "$output_root" ]]; then
  printf 'refusing to overwrite output: %s\n' "$output_root" >&2
  exit 2
fi
if [[ "$(git -C "$repo_root" rev-parse HEAD)" != "$expected_commit" ]]; then
  printf 'legacy repository is not at the exact baseline commit\n' >&2
  exit 2
fi
if [[ -n "$(git -C "$repo_root" status --porcelain)" ]]; then
  printf 'legacy baseline worktree must be clean\n' >&2
  exit 2
fi

mkdir -p "$output_root"
run_audit() {
  local name="$1"
  shift
  /usr/bin/time -v -o "$output_root/$name.time.txt" \
    node "$repo_root/audit-repo.mjs" \
      --root "$repo_root" \
      --output "$output_root/$name-artifacts" \
      --profile full \
      --no-self-audit-excludes \
      "$@" \
      >"$output_root/$name.stdout.txt" \
      2>"$output_root/$name.stderr.txt"
}

if [[ "$mode" == cold ]]; then
  run_audit measured --no-incremental
else
  cache_root="$output_root/cache"
  run_audit seed --cache-root "$cache_root"
  run_audit measured --cache-root "$cache_root"
fi

python3 - "$output_root" "$mode" "$expected_commit" <<'PY'
import json
import os
import platform
import re
import subprocess
import sys
from pathlib import Path

root = Path(sys.argv[1])
mode = sys.argv[2]
commit = sys.argv[3]
time_text = (root / "measured.time.txt").read_text()

def field(label: str) -> str:
    match = re.search(rf"^\s*{re.escape(label)}:\s*(.+)$", time_text, re.MULTILINE)
    if not match:
        raise SystemExit(f"missing GNU time field: {label}")
    return match.group(1).strip()

elapsed_text = field("Elapsed (wall clock) time (h:mm:ss or m:ss)")
parts = [float(part) for part in elapsed_text.split(":")]
elapsed_seconds = parts[-1]
if len(parts) == 2:
    elapsed_seconds += parts[0] * 60
elif len(parts) == 3:
    elapsed_seconds += parts[0] * 3600 + parts[1] * 60

measurement = {
    "baselineCommit": commit,
    "elapsedMs": round(elapsed_seconds * 1000),
    "maxResidentSetBytes": int(field("Maximum resident set size (kbytes)")) * 1024,
    "mode": mode,
    "schemaVersion": "legacy-full-baseline.v1",
    "timeOwner": "GNU time -v",
}
(root / "measurement.json").write_text(
    json.dumps(measurement, indent=2, sort_keys=True) + "\n", encoding="utf-8"
)

findmnt = subprocess.check_output(
    ["findmnt", "-no", "SOURCE,FSTYPE,TARGET", "--target", str(root)], text=True
).strip()
cpu_model = next(
    line.split(":", 1)[1].strip()
    for line in Path("/proc/cpuinfo").read_text().splitlines()
    if line.startswith("model name")
)
host = {
    "architecture": platform.machine(),
    "cpu": cpu_model,
    "filesystem": findmnt,
    "kernel": platform.release(),
    "logicalProcessors": os.cpu_count(),
    "memoryBytes": int(next(line.split()[1] for line in Path("/proc/meminfo").read_text().splitlines() if line.startswith("MemTotal:"))) * 1024,
    "node": subprocess.check_output(["node", "--version"], text=True).strip(),
    "npm": subprocess.check_output(["npm", "--version"], text=True).strip(),
    "platform": "wsl2-ext4",
    "schemaVersion": "numeric-target-host.v1",
}
(root / "host.json").write_text(
    json.dumps(host, indent=2, sort_keys=True) + "\n", encoding="utf-8"
)
PY
