#!/usr/bin/env bash
set -euo pipefail

[[ "${GITHUB_ACTIONS:-}" == "true" ]] || {
  printf 'this runner is restricted to the recorded GitHub Actions environment\n' >&2
  exit 1
}

packet_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source_root="$packet_root/source"
evidence="$packet_root/evidence/native-linux-ext4"

bash "$source_root/scripts/run-linux.sh" \
  --scope native-linux-ext4 \
  --evidence "$evidence"
