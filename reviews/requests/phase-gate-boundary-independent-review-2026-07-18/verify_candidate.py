#!/usr/bin/env python3
"""Verify the phase-gate amendment from immutable Git objects.

This author-provided script is a consistency aid, not independent approval evidence.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
from typing import Any


CANDIDATE = "9a0dbe5c89463892c001e864c4f18eeab9e0eaed"
PARENT = "085828ef09d5eb43621ae992001974ff637a3db2"
SUBJECT = "Separate Phase 0 feasibility from Phase 1 acceptance"
LEDGER = "9b64d132768ffd9521bf623f974629ea61832f54"
MANIFEST_SHA256 = "e2ca379a8a659f2febbc4e277c89db67bb02035a6b10467cf78a5663f21cd99a"
CHANGED_PATHS = {
    ("M", "WORKBOARD.md"),
    ("M", "specs/001-foundation-slice.md"),
}
REQUEST_DIR = Path(__file__).resolve().parent
MANIFEST_PATH = REQUEST_DIR / "candidate-manifest.txt"
REQUEST_PATH = REQUEST_DIR / "phase-gate-request.json"


class VerificationError(RuntimeError):
    pass


def run_git(repo: Path, *args: str, input_bytes: bytes | None = None) -> bytes:
    result = subprocess.run(
        ["git", "-C", str(repo), *args],
        input=input_bytes,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        raise VerificationError(
            f"git {' '.join(args)} failed: "
            f"{result.stderr.decode('utf-8', errors='replace').strip()}"
        )
    return result.stdout


def git_text(repo: Path, *args: str) -> str:
    return run_git(repo, *args).decode("utf-8").strip()


def git_blob(repo: Path, revision: str, path: str) -> bytes:
    return run_git(repo, "cat-file", "blob", f"{revision}:{path}")


def git_blob_oid(data: bytes) -> str:
    header = f"blob {len(data)}\0".encode("ascii")
    return hashlib.sha1(header + data).hexdigest()


def strict_json(data: bytes, label: str) -> Any:
    def reject_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, value in pairs:
            if key in result:
                raise VerificationError(f"duplicate JSON key in {label}: {key}")
            result[key] = value
        return result

    try:
        return json.loads(data.decode("utf-8"), object_pairs_hook=reject_duplicates)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise VerificationError(f"invalid JSON in {label}: {exc}") from exc


def section(text: str, heading: str, next_heading: str) -> str:
    start = text.index(heading)
    end = text.index(next_heading, start + len(heading))
    return text[start:end]


def parse_manifest(data: bytes) -> list[tuple[str, str]]:
    if data.startswith(b"\xef\xbb\xbf") or b"\r" in data or not data.endswith(b"\n"):
        raise VerificationError("candidate manifest must be BOM-free LF text with final LF")
    entries: list[tuple[str, str]] = []
    for raw_line in data.decode("utf-8").splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  (.+)", raw_line)
        if not match:
            raise VerificationError(f"invalid candidate manifest line: {raw_line!r}")
        entries.append((match.group(1), match.group(2)))
    paths = [path for _, path in entries]
    if len(paths) != 16 or len(set(paths)) != 16:
        raise VerificationError("candidate manifest must contain 16 unique paths")
    if paths != sorted(paths, key=lambda value: value.encode("utf-8")):
        raise VerificationError("candidate manifest paths are not UTF-8 ordinal sorted")
    return entries


def verify(repo: Path) -> dict[str, Any]:
    results: list[dict[str, str]] = []

    def check(name: str, condition: bool, evidence: str) -> None:
        if not condition:
            raise VerificationError(f"{name}: {evidence}")
        results.append({"name": name, "status": "PASS", "evidence": evidence})

    check(
        "repository",
        git_text(repo, "rev-parse", "--is-inside-work-tree") == "true",
        str(repo.resolve()),
    )
    check(
        "candidate-object",
        git_text(repo, "cat-file", "-t", CANDIDATE) == "commit",
        CANDIDATE,
    )
    metadata = git_text(repo, "show", "-s", "--format=%P%n%s%n%T", CANDIDATE).splitlines()
    check("candidate-parent", metadata[0] == PARENT, metadata[0])
    check("candidate-subject", metadata[1] == SUBJECT, metadata[1])
    check("candidate-tree", bool(re.fullmatch(r"[0-9a-f]{40}", metadata[2])), metadata[2])

    diff_rows: set[tuple[str, str]] = set()
    for line in git_text(
        repo, "diff-tree", "--no-commit-id", "--name-status", "-r", CANDIDATE
    ).splitlines():
        status, path = line.split("\t", 1)
        diff_rows.add((status, path))
    check("semantic-diff", diff_rows == CHANGED_PATHS, repr(sorted(diff_rows)))

    manifest_bytes = MANIFEST_PATH.read_bytes()
    check(
        "manifest-digest",
        hashlib.sha256(manifest_bytes).hexdigest() == MANIFEST_SHA256,
        hashlib.sha256(manifest_bytes).hexdigest(),
    )
    manifest = parse_manifest(manifest_bytes)
    check("manifest-cardinality", len(manifest) == 16, str(len(manifest)))

    changed = {path for _, path in CHANGED_PATHS}
    unchanged_count = 0
    for expected_sha, path in manifest:
        candidate_bytes = git_blob(repo, CANDIDATE, path)
        actual_sha = hashlib.sha256(candidate_bytes).hexdigest()
        if actual_sha != expected_sha:
            raise VerificationError(
                f"manifest-member-{path}: expected {expected_sha}, got {actual_sha}"
            )
        tree_oid = git_text(repo, "rev-parse", f"{CANDIDATE}:{path}")
        if git_blob_oid(candidate_bytes) != tree_oid:
            raise VerificationError(f"Git blob identity mismatch: {path}")
        if candidate_bytes.startswith(b"\xef\xbb\xbf") or b"\r" in candidate_bytes:
            raise VerificationError(f"noncanonical text encoding: {path}")
        parent_oid = git_text(repo, "rev-parse", f"{PARENT}:{path}")
        if path in changed:
            if parent_oid == tree_oid:
                raise VerificationError(f"declared changed path has unchanged blob: {path}")
        else:
            if parent_oid != tree_oid:
                raise VerificationError(f"undeclared owner blob changed: {path}")
            unchanged_count += 1
    check("manifest-members", True, "16/16 SHA-256 and Git blob identities")
    check("unchanged-owner-blobs", unchanged_count == 14, f"{unchanged_count}/14")

    for artifact in (
        "specs/inventory-config-semantics.v1.json",
        "specs/repo-path-semantics.v1.json",
        "specs/resolver-config-semantics.v1.json",
    ):
        strict_json(git_blob(repo, CANDIDATE, artifact), artifact)
    check("strict-machine-artifacts", True, "3/3 duplicate-key rejecting parses")

    workboard = git_blob(repo, CANDIDATE, "WORKBOARD.md").decode("utf-8")
    slice_text = git_blob(repo, CANDIDATE, "specs/001-foundation-slice.md").decode("utf-8")
    parent_slice = git_blob(repo, PARENT, "specs/001-foundation-slice.md").decode("utf-8")
    required_workboard = (
        "No product binary or Phase 1 behavior is a Phase 0 prerequisite.",
        "Product packages, packaged skill adapters, public behavior, native path/root "
        "product round trips, and achieved-budget proofs remain Phase 1 acceptance.",
    )
    check(
        "workboard-boundary",
        all(value in workboard for value in required_workboard),
        "Phase 0 non-product prerequisite and Phase 1 ownership clauses present",
    )

    required_slice = (
        "Everything in this section is a Phase 1 product deliverable and acceptance surface.",
        "Static packaging feasibility is limited to target toolchain, linker, "
        "artifact-format, dependency, and native-distribution viability in standalone harnesses.",
        "A Phase 0 harness cannot build, expose, invoke, or emulate public `lumin` APIs",
        "Product-native path/root round trips and packaged-adapter execution are Phase 1 acceptance.",
        "Every criterion in Section 14, every traceability row in Section 15, and every "
        "implementation command in Section 17 is a Phase 1 exit condition",
        "no `lumin-xtask` implementation command is a Phase 0 freeze prerequisite.",
        "These are Phase 1 exit criteria for the completed product.",
        "The Phase 1 implementation must provide stable repository commands equivalent to:",
    )
    check(
        "slice-boundary",
        all(value in slice_text for value in required_slice),
        "all phase-owner clauses present",
    )

    candidate_ac = re.findall(
        r"(?m)^(\d+)\. .+$", section(slice_text, "## 14.", "## 15.")
    )
    parent_ac = re.findall(
        r"(?m)^(\d+)\. .+$", section(parent_slice, "## 14.", "## 15.")
    )
    check(
        "acceptance-criteria",
        candidate_ac == parent_ac == [str(index) for index in range(1, 39)],
        "38/38 numbered criteria preserved",
    )
    candidate_rows = re.findall(
        r"(?m)^\| (\d+) \|.*$", section(slice_text, "## 15.", "## 16.")
    )
    parent_rows = re.findall(
        r"(?m)^\| (\d+) \|.*$", section(parent_slice, "## 15.", "## 16.")
    )
    check(
        "acceptance-traceability",
        candidate_rows == parent_rows == [str(index) for index in range(1, 39)],
        "38/38 traceability rows preserved",
    )
    candidate_commands = re.search(
        r"## 17\..*?```text\n(.*?)```", slice_text, re.DOTALL
    )
    parent_commands = re.search(
        r"## 17\..*?```text\n(.*?)```", parent_slice, re.DOTALL
    )
    check(
        "verification-commands",
        candidate_commands is not None
        and parent_commands is not None
        and candidate_commands.group(1) == parent_commands.group(1),
        "Phase 1 command block unchanged",
    )
    candidate_distribution = [
        line
        for line in section(slice_text, "## 11.", "## 12.").splitlines()
        if line.startswith("- ")
    ]
    parent_distribution = [
        line
        for line in section(parent_slice, "## 11.", "## 12.").splitlines()
        if line.startswith("- ")
    ]
    check(
        "distribution-requirements",
        candidate_distribution == parent_distribution and len(candidate_distribution) == 6,
        "6/6 distribution bullets preserved",
    )
    review_one = git_blob(
        repo, CANDIDATE, "reviews/architecture-v1-adversarial-2026-07-15.md"
    ).decode("utf-8")
    check(
        "f12-predecessor",
        "F-12 performance ordering cycle" in review_one
        and "separates Phase 0 feasibility/targets from Phase 1 binary acceptance" in review_one,
        "REVIEW-001 F-12 non-circular resolution present",
    )

    check("ledger-object", git_text(repo, "cat-file", "-t", LEDGER) == "commit", LEDGER)
    ledger_diff = git_text(
        repo, "diff", "--name-status", CANDIDATE, LEDGER
    ).splitlines()
    check(
        "ledger-is-later",
        ledger_diff == [
            "M\treviews/architecture-v1-independent-verification-2026-07-15.md"
        ],
        repr(ledger_diff),
    )
    ledger_text = git_blob(
        repo, LEDGER, "reviews/architecture-v1-independent-verification-2026-07-15.md"
    ).decode("utf-8")
    check(
        "ledger-binding",
        all(
            value in ledger_text
            for value in (CANDIDATE, MANIFEST_SHA256, "NEW-PHASE-GATE-01", "independent review pending")
        ),
        "candidate, manifest, finding, and pending independence recorded",
    )

    request = strict_json(REQUEST_PATH.read_bytes(), str(REQUEST_PATH))
    check(
        "request-binding",
        request.get("candidate") == CANDIDATE
        and request.get("candidate_manifest_sha256") == MANIFEST_SHA256
        and request.get("ledger_commit") == LEDGER,
        "request JSON matches exact identities",
    )

    return {
        "schema": "lumin-phase-gate-author-preflight-v1",
        "status": "PASS",
        "candidate": CANDIDATE,
        "candidate_parent": PARENT,
        "candidate_manifest_sha256": MANIFEST_SHA256,
        "checks": {"pass": len(results), "fail": 0},
        "results": results,
        "independence_boundary": (
            "Author-side consistency output only; not independent review evidence."
        ),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    try:
        result = verify(args.repo.resolve())
    except (OSError, VerificationError, ValueError) as exc:
        print(f"FAIL: {exc}", file=sys.stderr)
        return 1
    rendered = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(rendered, encoding="utf-8", newline="\n")
    print(rendered, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
