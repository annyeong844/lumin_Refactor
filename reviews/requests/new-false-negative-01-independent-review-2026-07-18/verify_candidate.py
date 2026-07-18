#!/usr/bin/env python3
"""Check immutable binding and redline anchors; never treat this as design approval."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
from typing import Any


CANDIDATE = "a1e07ed7b9e05181cd58bfba5f3846c1baab8a93"
PARENT = "f2c1c2b7c5c509e2c82a85daa385a52a16476e10"
TREE = "06d3116c3c9ab398f1f4fa0b5df38a7fe302cb5e"
SUBJECT = "Preserve grounded findings from mute policy"
LEDGER = "6daf8229b60e50d9adc78c11018df7682784ed7b"
MANIFEST_SHA256 = "8b0d2ceddb930533e6967c48e06954f09f53abf8f7a688f4dfb0baeb050a6339"
CHANGED = {
    "WORKBOARD.md",
    "architecture/000-system-blueprint.md",
    "architecture/002-evidence-and-write-gate.md",
    "specs/000-product-contract.md",
    "specs/001-foundation-slice.md",
}
HERE = Path(__file__).resolve().parent


class VerifyError(RuntimeError):
    pass


def git(repo: Path, *args: str) -> bytes:
    result = subprocess.run(
        ["git", "-C", str(repo), *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode:
        raise VerifyError(result.stderr.decode("utf-8", errors="replace").strip())
    return result.stdout


def text(repo: Path, *args: str) -> str:
    return git(repo, *args).decode("utf-8").strip()


def blob(repo: Path, rev: str, path: str) -> bytes:
    return git(repo, "cat-file", "blob", f"{rev}:{path}")


def strict_json(data: bytes, label: str) -> Any:
    def reject(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        value: dict[str, Any] = {}
        for key, item in pairs:
            if key in value:
                raise VerifyError(f"duplicate key in {label}: {key}")
            value[key] = item
        return value

    return json.loads(data.decode("utf-8"), object_pairs_hook=reject)


def section(value: str, start: str, end: str) -> str:
    return value[value.index(start) : value.index(end, value.index(start) + len(start))]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", type=Path, required=True)
    args = parser.parse_args()
    repo = args.repo.resolve()
    checks: list[dict[str, str]] = []

    def check(name: str, ok: bool, evidence: str) -> None:
        if not ok:
            raise VerifyError(f"{name}: {evidence}")
        checks.append({"name": name, "status": "PASS", "evidence": evidence})

    check("candidate-object", text(repo, "cat-file", "-t", CANDIDATE) == "commit", CANDIDATE)
    meta = text(repo, "show", "-s", "--format=%P%n%T%n%s", CANDIDATE).splitlines()
    check("parent", meta[0] == PARENT, meta[0])
    check("tree", meta[1] == TREE, meta[1])
    check("subject", meta[2] == SUBJECT, meta[2])

    rows = text(repo, "diff-tree", "--no-commit-id", "--name-status", "-r", CANDIDATE).splitlines()
    changed = {row.split("\t", 1)[1] for row in rows}
    check("five-file-diff", changed == CHANGED and all(row.startswith("M\t") for row in rows), repr(rows))

    manifest_bytes = (HERE / "candidate-manifest.txt").read_bytes()
    check("manifest-sha256", hashlib.sha256(manifest_bytes).hexdigest() == MANIFEST_SHA256, hashlib.sha256(manifest_bytes).hexdigest())
    check("manifest-framing", b"\r" not in manifest_bytes and manifest_bytes.endswith(b"\n"), "LF/final-LF")
    entries: list[tuple[str, str]] = []
    for line in manifest_bytes.decode("utf-8").splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  (.+)", line)
        if not match:
            raise VerifyError(f"bad manifest line: {line!r}")
        entries.append((match.group(1), match.group(2)))
    paths = [path for _, path in entries]
    check("manifest-cardinality", len(paths) == 16 and len(set(paths)) == 16, str(len(paths)))

    unchanged = 0
    for expected, path in entries:
        data = blob(repo, CANDIDATE, path)
        check(f"blob-sha256:{path}", hashlib.sha256(data).hexdigest() == expected, expected)
        check(f"text-framing:{path}", not data.startswith(b"\xef\xbb\xbf") and b"\r" not in data and data.endswith(b"\n"), "UTF-8/LF/final-LF")
        parent_oid = text(repo, "rev-parse", f"{PARENT}:{path}")
        candidate_oid = text(repo, "rev-parse", f"{CANDIDATE}:{path}")
        if path in CHANGED:
            check(f"changed:{path}", parent_oid != candidate_oid, candidate_oid)
        else:
            check(f"unchanged:{path}", parent_oid == candidate_oid, candidate_oid)
            unchanged += 1
    check("unchanged-owner-count", unchanged == 11, f"{unchanged}/11")

    for path in (
        "specs/inventory-config-semantics.v1.json",
        "specs/repo-path-semantics.v1.json",
        "specs/resolver-config-semantics.v1.json",
    ):
        strict_json(blob(repo, CANDIDATE, path), path)
    check("strict-artifacts", True, "3/3")

    product = blob(repo, CANDIDATE, "specs/000-product-contract.md").decode("utf-8")
    arch0 = blob(repo, CANDIDATE, "architecture/000-system-blueprint.md").decode("utf-8")
    arch2 = blob(repo, CANDIDATE, "architecture/002-evidence-and-write-gate.md").decode("utf-8")
    slice_text = blob(repo, CANDIDATE, "specs/001-foundation-slice.md").decode("utf-8")
    workboard = blob(repo, CANDIDATE, "WORKBOARD.md").decode("utf-8")

    anchors = {
        "product-grounded": "Grounded findings remain canonical evidence regardless of source role",
        "product-filter": "unfiltered scope total, matched total",
        "model-disposition": "FindingDisposition = ReviewCandidate | ReviewOnly { reason }",
        "architecture-rejects-mute": "any canonical-finding `Muted`/`Suppressed` variant or implicit default finding filter",
        "query-scope-total": '"scopeTotal": 812',
        "query-no-default-filter": "with `{}` return every canonical finding, including `ReviewOnly`",
        "projection-preserves": "finding disposition may change remediation wording but cannot omit a canonical finding",
        "slice-no-mute-variant": "there is no `Muted` or `Suppressed` finding variant",
        "slice-delta": "directionless `OwnerPayloadChanged` dimension",
        "slice-corpus": "source-role-findings-remain-visible",
        "performance-integrity": "No target may be met by muting, implicit filtering, source-scope reduction",
        "workboard-finding": "Close `NEW-FALSE-NEGATIVE-01`",
    }
    joined = "\n".join((product, arch0, arch2, slice_text, workboard))
    for name, anchor in anchors.items():
        check(name, anchor in joined, anchor)
    for stale in (
        "generated or vendored definitions are not default dead-removal candidates",
        "is muted from default removal candidates",
    ):
        check(f"stale-absent:{stale[:24]}", stale not in slice_text, stale)

    slice_ac = re.findall(r"(?m)^\d+\. ", section(slice_text, "## 14.", "## 15."))
    trace = re.findall(r"(?m)^\| \d+ \|", section(slice_text, "## 15.", "## 16."))
    product_ac = re.findall(r"(?m)^\d+\. ", section(product, "## 4.", "## 5."))
    coverage = re.findall(r"(?m)^\| \d+ [^|]+ \|", section(slice_text, "## 16.", "## 17."))
    check("slice-ac-trace", len(slice_ac) == len(trace) == 38, f"{len(slice_ac)}/{len(trace)}")
    check("product-ac-coverage", len(product_ac) == len(coverage) == 22, f"{len(product_ac)}/{len(coverage)}")

    check("ledger-object", text(repo, "cat-file", "-t", LEDGER) == "commit", LEDGER)
    ledger_rows = text(repo, "diff-tree", "--no-commit-id", "--name-only", "-r", LEDGER).splitlines()
    check("ledger-isolation", ledger_rows == ["reviews/architecture-v1-independent-verification-2026-07-15.md"], repr(ledger_rows))
    request = strict_json((HERE / "review-request.json").read_bytes(), "review-request.json")
    check("request-candidate", request["candidate"] == CANDIDATE, request["candidate"])

    print(json.dumps({"status": "pass", "checkCount": len(checks), "checks": checks}, indent=2))


if __name__ == "__main__":
    main()
