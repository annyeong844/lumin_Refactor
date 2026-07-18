#!/usr/bin/env python3
"""Verify NEW-EVIDENCE-01 closure from immutable Git objects.

This author-provided script is a consistency aid, not independent approval evidence.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import subprocess
import sys
from typing import Any


OLD = "579c2358f5e2245a977abdedcee7e06ba3f4e46e"
NEW = "b8ff840b5a400d2404d693b290c0fb8d18e59062"
NEW_PARENT = "f74ac19c247d3f37b9b543dc7a2320afed8bcf28"
NEW_SUBJECT = "Remove stale Windows evidence packet seal"
MANIFEST_SHA256 = "b43b8b0ea9c3c0c8938363091aaf4de0e7a4a3b3babb225582a85b050a104375"
STALE_PATH = (
    "reviews/probes/phase0-store-backend-windows-x64-2026-07-17/"
    "PACKET-SHA256SUMS"
)
STALE_BLOB = "e6fd32ebd14bfc4406349145de1fb6497d093d17"
OLD_WINDOWS_MANIFEST_SHA = (
    "b370b648e2d2a1d5840e9e1ed4ec7bf2646fc5421d1765924f168df6ac82d4a0"
)
PACKETS = (
    (
        "windows-store",
        "reviews/probes/phase0-store-backend-windows-x64-2026-07-17/evidence/SHA256SUMS",
        "6d404cfc4b25ed581a9f021fc6248c6e7a94c2fdc38b668872c009c0f747ef2d",
        12,
    ),
    (
        "wsl2-store",
        "reviews/probes/phase0-store-backend-wsl2-ext4-musl-x64-2026-07-17/evidence/SHA256SUMS",
        "c65a9224bfd03482ea947661c70edc7168d1662a18eb1a0447604fa58f807b3e",
        37,
    ),
    (
        "native-linux-store",
        "reviews/probes/phase0-store-backend-native-linux-ext4-musl-x64-2026-07-17/evidence/SHA256SUMS",
        "0544d252540e14b8f9392d8a83a37209748af0995d6dd2240397ec4941e75046",
        37,
    ),
    (
        "oxc-feasibility",
        "reviews/probes/phase0-oxc-memory-stack-windows-wsl2-x64-2026-07-17/evidence/SHA256SUMS",
        "bfba3524182822ebb9e7ec35c37ae08a1b03380fa0f961675499eef5031790be",
        80,
    ),
    (
        "backend-selection",
        "reviews/probes/phase0-store-backend-selection-2026-07-17/SHA256SUMS",
        "ce14aaab83942e83e6b874d972aef06d03aa03d64c68370cefebac3339287ea6",
        2,
    ),
)
EXPECTED_DIFF = {
    ("M", "reviews/architecture-v1-independent-verification-2026-07-15.md"),
    ("D", STALE_PATH),
    ("A", "reviews/requests/phase0-backend-selection-independent-review-2026-07-18/README.md"),
    ("A", "reviews/requests/phase0-backend-selection-independent-review-2026-07-18/SHA256SUMS"),
    ("A", "reviews/requests/phase0-backend-selection-independent-review-2026-07-18/author-preflight.json"),
    ("A", "reviews/requests/phase0-backend-selection-independent-review-2026-07-18/candidate-manifest.txt"),
    ("A", "reviews/requests/phase0-backend-selection-independent-review-2026-07-18/evidence-packets.json"),
    ("A", "reviews/requests/phase0-backend-selection-independent-review-2026-07-18/review-template.md"),
    ("A", "reviews/requests/phase0-backend-selection-independent-review-2026-07-18/verify_candidate.py"),
}
REQUEST_DIR = Path(__file__).resolve().parent


class VerificationError(RuntimeError):
    pass


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def git(repo: Path, *args: str, check: bool = True) -> subprocess.CompletedProcess[bytes]:
    process = subprocess.run(
        ["git", *args],
        cwd=repo,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if check and process.returncode != 0:
        error = process.stderr.decode("utf-8", errors="replace").strip()
        raise VerificationError(f"git {' '.join(args)} failed: {error}")
    return process


def git_text(repo: Path, *args: str) -> str:
    return git(repo, *args).stdout.decode("utf-8").strip()


def blob(repo: Path, revision: str, path: str) -> bytes:
    return git(repo, "cat-file", "blob", f"{revision}:{path}").stdout


def path_exists(repo: Path, revision: str, path: str) -> bool:
    return git(repo, "cat-file", "-e", f"{revision}:{path}", check=False).returncode == 0


def parse_manifest(data: bytes, label: str) -> list[tuple[str, str]]:
    if data.startswith(b"\xef\xbb\xbf") or b"\r" in data or not data.endswith(b"\n"):
        raise VerificationError(f"noncanonical framing in {label}")
    rows: list[tuple[str, str]] = []
    for line_number, line in enumerate(data.decode("utf-8").splitlines(), start=1):
        digest, separator, path = line.partition("  ")
        if separator != "  " or len(digest) != 64 or digest.lower() != digest:
            raise VerificationError(f"malformed {label}:{line_number}")
        try:
            int(digest, 16)
        except ValueError as error:
            raise VerificationError(f"non-hex digest in {label}:{line_number}") from error
        if not path or "\\" in path or "\x00" in path:
            raise VerificationError(f"unsafe path in {label}:{line_number}")
        rows.append((digest, path))
    paths = [path for _, path in rows]
    if len(paths) != len(set(paths)):
        raise VerificationError(f"duplicate path in {label}")
    return rows


def resolve_member(manifest_path: str, member: str) -> str:
    relative = PurePosixPath(member)
    if relative.is_absolute() or ".." in relative.parts:
        raise VerificationError(f"unsafe manifest member: {member}")
    return (PurePosixPath(manifest_path).parent / relative).as_posix()


def add(checks: list[dict[str, Any]], name: str, passed: bool, observed: Any) -> None:
    checks.append({"name": name, "status": "PASS" if passed else "FAIL", "observed": observed})


def verify(repo: Path) -> dict[str, Any]:
    checks: list[dict[str, Any]] = []

    new_type = git_text(repo, "cat-file", "-t", NEW)
    add(checks, "closure candidate is a commit", new_type == "commit", new_type)
    subject = git_text(repo, "show", "-s", "--format=%s", NEW)
    add(checks, "closure commit subject", subject == NEW_SUBJECT, subject)
    parent = git_text(repo, "show", "-s", "--format=%P", NEW)
    add(checks, "closure commit parent", parent == NEW_PARENT, parent)
    ancestor = git(repo, "merge-base", "--is-ancestor", OLD, NEW, check=False).returncode == 0
    add(checks, "previous candidate is ancestor", ancestor, ancestor)

    old_stale_oid = git_text(repo, "rev-parse", f"{OLD}:{STALE_PATH}")
    add(checks, "previous stale blob identity", old_stale_oid == STALE_BLOB, old_stale_oid)
    old_stale = blob(repo, OLD, STALE_PATH)
    add(
        checks,
        "previous seal carries superseded Windows manifest",
        OLD_WINDOWS_MANIFEST_SHA.encode("ascii") in old_stale,
        OLD_WINDOWS_MANIFEST_SHA in old_stale.decode("utf-8", errors="replace"),
    )
    stale_absent = not path_exists(repo, NEW, STALE_PATH)
    add(checks, "stale seal absent from closure candidate", stale_absent, stale_absent)

    probe_paths = git_text(repo, "ls-tree", "-r", "--name-only", NEW, "--", "reviews/probes").splitlines()
    competing = [path for path in probe_paths if PurePosixPath(path).name == "PACKET-SHA256SUMS"]
    add(checks, "competing packet-wide seals", not competing, competing)

    diff_rows: set[tuple[str, str]] = set()
    raw_diff = git_text(repo, "diff", "--name-status", "--no-renames", OLD, NEW)
    for line in raw_diff.splitlines():
        status, path = line.split("\t", 1)
        diff_rows.add((status, path))
    diff_order = {"M": 0, "D": 1, "A": 2}
    ordered_diff = sorted(diff_rows, key=lambda row: (diff_order.get(row[0], 9), row[1]))
    add(checks, "exact candidate diff scope", diff_rows == EXPECTED_DIFF, ordered_diff)

    candidate_manifest = (REQUEST_DIR / "candidate-manifest.txt").read_bytes()
    candidate_rows = parse_manifest(candidate_manifest, "candidate-manifest.txt")
    add(checks, "candidate manifest SHA-256", sha256(candidate_manifest) == MANIFEST_SHA256, sha256(candidate_manifest))
    candidate_paths = [path for _, path in candidate_rows]
    add(checks, "candidate manifest entry count", len(candidate_rows) == 16, len(candidate_rows))
    add(
        checks,
        "candidate manifest ordinal path order",
        candidate_paths == sorted(candidate_paths, key=lambda value: value.encode("utf-8")),
        candidate_paths == sorted(candidate_paths, key=lambda value: value.encode("utf-8")),
    )
    bad_candidate: list[str] = []
    changed_candidate: list[str] = []
    for expected_digest, path in candidate_rows:
        current = blob(repo, NEW, path)
        if sha256(current) != expected_digest:
            bad_candidate.append(path)
        if git_text(repo, "rev-parse", f"{OLD}:{path}") != git_text(repo, "rev-parse", f"{NEW}:{path}"):
            changed_candidate.append(path)
    add(checks, "16 candidate blob digests", not bad_candidate, bad_candidate)
    add(checks, "16 candidate blob identities unchanged", not changed_candidate, changed_candidate)

    all_members: list[str] = []
    bad_packets: list[str] = []
    raw_logs: list[str] = []
    packet_counts: dict[str, int] = {}
    for packet_id, manifest_path, expected_manifest_sha, expected_count in PACKETS:
        manifest = blob(repo, NEW, manifest_path)
        rows = parse_manifest(manifest, manifest_path)
        packet_counts[packet_id] = len(rows)
        packet_bad = sha256(manifest) != expected_manifest_sha or len(rows) != expected_count
        for expected_digest, member in rows:
            full_path = resolve_member(manifest_path, member)
            all_members.append(full_path)
            member_bytes = blob(repo, NEW, full_path)
            if sha256(member_bytes) != expected_digest:
                packet_bad = True
            if packet_id in {"wsl2-store", "native-linux-store"} and member.endswith(".log"):
                raw_logs.append(full_path)
                if not member_bytes:
                    packet_bad = True
        if packet_bad:
            bad_packets.append(packet_id)
    packet_observed: Any = packet_counts if not bad_packets else {"counts": packet_counts, "bad": bad_packets}
    add(checks, "five canonical packet manifests", not bad_packets, packet_observed)
    add(checks, "canonical evidence entry count", len(all_members) == 168, len(all_members))
    add(checks, "canonical evidence paths unique", len(set(all_members)) == 168, len(set(all_members)))
    add(checks, "WSL2/native raw build logs", len(raw_logs) == 20, len(raw_logs))

    failed = [row["name"] for row in checks if row["status"] == "FAIL"]
    return {
        "schema": "lumin-new-evidence-01-author-preflight-v1",
        "status": "PASS" if not failed else "FAIL",
        "candidate": NEW,
        "previous_candidate": OLD,
        "candidate_files": len(candidate_rows),
        "canonical_evidence_entries": len(all_members),
        "raw_build_logs": len(raw_logs),
        "competing_packet_wide_seals": len(competing),
        "checks": checks,
        "failed_checks": failed,
        "independence_boundary": "Author-side consistency output; not external PASS evidence.",
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    try:
        result = verify(args.repo.resolve())
    except (OSError, UnicodeError, ValueError, VerificationError) as error:
        result = {
            "schema": "lumin-new-evidence-01-author-preflight-v1",
            "status": "FAIL",
            "candidate": NEW,
            "error": str(error),
            "independence_boundary": "Author-side consistency output; not external PASS evidence.",
        }
    if args.json:
        print(json.dumps(result, indent=2, ensure_ascii=False))
    else:
        print(f"status: {result['status']}")
        if "candidate_files" in result:
            print(f"candidate files: {result['candidate_files']}")
            print(f"canonical evidence entries: {result['canonical_evidence_entries']}")
            print(f"WSL2/native raw build logs: {result['raw_build_logs']}")
            print(f"competing packet-wide seals: {result['competing_packet_wide_seals']}")
        if result.get("failed_checks"):
            print("failed checks: " + ", ".join(result["failed_checks"]))
        if result.get("error"):
            print("error: " + result["error"])
    return 0 if result["status"] == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
