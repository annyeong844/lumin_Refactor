#!/usr/bin/env bash
set -euo pipefail

[[ "${GITHUB_ACTIONS:-}" == "true" ]] || {
  printf 'this runner is restricted to the recorded GitHub Actions environment\n' >&2
  exit 1
}

repository_root=$(git rev-parse --show-toplevel)
packet_root="$repository_root/reviews/probes/phase0-pinned-upstream-provenance-2026-07-18"
evidence="$packet_root/evidence/native-linux-clean"

[[ "$(git rev-parse HEAD)" == "${GITHUB_SHA:?}" ]] || {
  printf 'GITHUB_SHA does not match the checked-out commit\n' >&2
  exit 1
}
[[ ! -e "$evidence" ]] || {
  printf 'clean evidence path already exists\n' >&2
  exit 1
}

python3 "$packet_root/source/verify_provenance.py" \
  capture \
  --repository-root "$repository_root" \
  --evidence "$evidence"

python3 "$packet_root/source/verify_provenance.py" \
  verify \
  --repository-root "$repository_root" \
  --evidence "$evidence"
