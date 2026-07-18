#!/usr/bin/env python3
"""Verify immutable candidate bytes and anchors; never treat this as target approval."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
import tempfile
from typing import Any


CANDIDATE = "a410605ff9f5512cadd1cb105d346444044398ce"
PARENT = "5125b4b2ade1c98ff9fc667a7363a659c6799564"
TREE = "529a35b9ce677d2a241dd059f81769f8a427a182"
SUBJECT = "Select Phase 1 numeric targets"
LEDGER = "bee8a2ab685862825c9fcd0efd9ef147715b94c3"
B07_MANIFEST_SHA256 = "6b23f500e633f28611c27bedb9998fb7ff44a69af6682d58b8554e7b5b33d86c"
PACKET_MANIFEST_SHA256 = "a749a5d80600295fe765edbccf8ed23be170a9211151186923e2f3040349c2f4"
AUTHOR_CHECKS_SHA256 = "c1a316e84a34bb058b8ceeeb1000b632b7ae7bbdde08e611d24c7c41fb632358"
PACKET = "reviews/probes/phase0-numeric-target-selection-2026-07-18"
OWNER_PATHS = {
    "WORKBOARD.md",
    "architecture/001-execution-and-ownership.md",
    "specs/001-foundation-slice.md",
}
TARGETS = {
    "binarySizeBytesMax": 12582912,
    "coldFullAuditP50MsMax": 30000,
    "coldPreWriteP50MsMax": 6000,
    "defaultJobsFormula": "max(1,min(8,available_parallelism))",
    "defaultJobsVsJobs1ColdFullP50RatioMax": 0.75,
    "peakRssBytesMax": 536870912,
    "postWrite32FilesP50MsMax": 8000,
    "postWriteOneFileP50MsMax": 4000,
    "repetitionsPerEnvironment": 3,
    "warmPreWriteP50MsMax": 4000,
    "warmUnchangedAuditP50MsMax": 8000,
    "workerStackBytes": 4194304,
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


def git_text(repo: Path, *args: str) -> str:
    return git(repo, *args).decode("utf-8").strip()


def blob(repo: Path, revision: str, path: str) -> bytes:
    return git(repo, "cat-file", "blob", f"{revision}:{path}")


def strict_json(data: bytes, label: str) -> Any:
    def reject(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        value: dict[str, Any] = {}
        for key, item in pairs:
            if key in value:
                raise VerifyError(f"duplicate key in {label}: {key}")
            value[key] = item
        return value

    return json.loads(data.decode("utf-8"), object_pairs_hook=reject)


def parse_manifest(data: bytes, label: str) -> list[tuple[str, str]]:
    if data.startswith(b"\xef\xbb\xbf") or b"\r" in data or not data.endswith(b"\n"):
        raise VerifyError(f"invalid framing in {label}")
    entries: list[tuple[str, str]] = []
    for line in data.decode("utf-8").splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  (.+)", line)
        if not match:
            raise VerifyError(f"bad manifest line in {label}: {line!r}")
        path = match.group(2)
        pure = PurePosixPath(path)
        if pure.is_absolute() or ".." in pure.parts or "\\" in path:
            raise VerifyError(f"unsafe path in {label}: {path}")
        entries.append((match.group(1), path))
    paths = [path for _, path in entries]
    if paths != sorted(paths, key=lambda value: value.encode("utf-8")):
        raise VerifyError(f"non-ordinal manifest: {label}")
    if len(paths) != len(set(paths)):
        raise VerifyError(f"duplicate manifest path: {label}")
    return entries


def section(value: str, start: str, end: str) -> str:
    begin = value.index(start)
    return value[begin : value.index(end, begin + len(start))]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", type=Path, required=True)
    args = parser.parse_args()
    repo = args.repo.resolve()
    checks: list[dict[str, str]] = []

    def check(name: str, ok: bool, evidence: object) -> None:
        if not ok:
            raise VerifyError(f"{name}: {evidence}")
        checks.append({"name": name, "status": "PASS", "evidence": str(evidence)})

    request_entries = parse_manifest((HERE / "SHA256SUMS").read_bytes(), "SHA256SUMS")
    request_files = {path.name for path in HERE.iterdir() if path.is_file() and path.name != "SHA256SUMS"}
    check("request-manifest-count", len(request_entries) == 8, len(request_entries))
    check("request-exact-inventory", {path for _, path in request_entries} == request_files, len(request_files))
    for expected, path in request_entries:
        check(f"request-sha256:{path}", hashlib.sha256((HERE / path).read_bytes()).hexdigest() == expected, expected)

    check("candidate-object", git_text(repo, "cat-file", "-t", CANDIDATE) == "commit", CANDIDATE)
    meta = git_text(repo, "show", "-s", "--format=%P%n%T%n%s", CANDIDATE).splitlines()
    check("parent", meta[0] == PARENT, meta[0])
    check("tree", meta[1] == TREE, meta[1])
    check("subject", meta[2] == SUBJECT, meta[2])

    rows = git_text(repo, "diff-tree", "--no-commit-id", "--name-status", "-r", CANDIDATE).splitlines()
    parsed_rows = [row.split("\t", 1) for row in rows]
    changed = {path for _, path in parsed_rows}
    packet_tree = git_text(repo, "ls-tree", "-r", "--name-only", CANDIDATE, PACKET).splitlines()
    check("packet-tree-count", len(packet_tree) == 43, len(packet_tree))
    check("candidate-diff-count", len(changed) == 46, len(changed))
    check("candidate-diff-scope", changed == OWNER_PATHS | set(packet_tree), len(changed))
    check("owner-modifications", all(["M", path] in parsed_rows for path in OWNER_PATHS), OWNER_PATHS)
    check("packet-additions", all(["A", path] in parsed_rows for path in packet_tree), len(packet_tree))

    candidate_manifest = (HERE / "candidate-manifest.txt").read_bytes()
    check("b07-manifest-sha256", hashlib.sha256(candidate_manifest).hexdigest() == B07_MANIFEST_SHA256, B07_MANIFEST_SHA256)
    b07_entries = parse_manifest(candidate_manifest, "candidate-manifest.txt")
    check("b07-count", len(b07_entries) == 16, len(b07_entries))
    changed_b07 = 0
    for expected, path in b07_entries:
        data = blob(repo, CANDIDATE, path)
        check(f"b07-sha256:{path}", hashlib.sha256(data).hexdigest() == expected, expected)
        check(f"b07-framing:{path}", not data.startswith(b"\xef\xbb\xbf") and b"\r" not in data and data.endswith(b"\n"), path)
        old_oid = git_text(repo, "rev-parse", f"{PARENT}:{path}")
        new_oid = git_text(repo, "rev-parse", f"{CANDIDATE}:{path}")
        if path in OWNER_PATHS:
            check(f"b07-changed:{path}", old_oid != new_oid, new_oid)
            changed_b07 += 1
        else:
            check(f"b07-unchanged:{path}", old_oid == new_oid, new_oid)
    check("b07-changed-count", changed_b07 == 3, changed_b07)

    packet_manifest = (HERE / "packet-manifest.txt").read_bytes()
    exact_packet_manifest = blob(repo, CANDIDATE, f"{PACKET}/SHA256SUMS")
    check("packet-manifest-copy", packet_manifest == exact_packet_manifest, len(packet_manifest))
    check("packet-manifest-sha256", hashlib.sha256(packet_manifest).hexdigest() == PACKET_MANIFEST_SHA256, PACKET_MANIFEST_SHA256)
    packet_entries = parse_manifest(packet_manifest, "packet-manifest.txt")
    check("packet-manifest-count", len(packet_entries) == 42, len(packet_entries))
    expected_packet_tree = {f"{PACKET}/{path}" for _, path in packet_entries} | {f"{PACKET}/SHA256SUMS"}
    check("packet-exact-inventory", set(packet_tree) == expected_packet_tree, len(packet_tree))
    for expected, path in packet_entries:
        data = blob(repo, CANDIDATE, f"{PACKET}/{path}")
        check(f"packet-sha256:{path}", hashlib.sha256(data).hexdigest() == expected, expected)

    for path in (
        "specs/inventory-config-semantics.v1.json",
        "specs/repo-path-semantics.v1.json",
        "specs/resolver-config-semantics.v1.json",
    ):
        strict_json(blob(repo, CANDIDATE, path), path)
    check("strict-machine-artifacts", True, "3/3")

    selection = strict_json(blob(repo, CANDIDATE, f"{PACKET}/evidence/target-selection.json"), "target-selection.json")
    check("selection-schema", selection["schemaVersion"] == "phase0-numeric-target-selection.v1", selection["schemaVersion"])
    check("selection-status", selection["status"] == "candidate-awaiting-independent-review", selection["status"])
    check("selection-targets", selection["targets"] == TARGETS, selection["targets"])
    check("selection-not-achieved", selection["claimBoundary"]["achievedProductBudget"] is False, selection["claimBoundary"])
    check("selection-legacy-not-truth", selection["claimBoundary"]["legacyOutputIsProductTruth"] is False, selection["claimBoundary"])

    truth = strict_json(blob(repo, CANDIDATE, f"{PACKET}/evidence/scale-corpus-expected-truth.json"), "scale-corpus-expected-truth.json")
    scale = selection["scaleCorpus"]
    check("scale-files", scale["fileCount"] == 780, scale["fileCount"])
    check("scale-bytes", scale["totalBytes"] == 7461511, scale["totalBytes"])
    check("scale-content", scale["contentManifestSha256"] == "9e51b070a934c027e6d2d9a4610fac764592ecb8f05bf41a2ab6f5eb46158d3e", scale["contentManifestSha256"])
    check("truth-filter", truth["filters"] == {}, truth["filters"])
    expected_findings = truth["expectedFindings"]
    check("truth-counts", truth["scopeTotal"] == truth["matchedTotal"] == 256 and len(expected_findings) == 256, len(expected_findings))
    check("truth-limitations", truth["limitations"] == [], truth["limitations"])
    tuples = [json.dumps(item, sort_keys=True, separators=(",", ":")) for item in expected_findings]
    check("truth-tuples-unique", len(tuples) == len(set(tuples)), len(set(tuples)))

    with tempfile.TemporaryDirectory(prefix="lumin-numeric-request-") as temp_value:
        temp = Path(temp_value)
        generator = temp / "generate-scale-corpus.py"
        generator.write_bytes(blob(repo, CANDIDATE, f"{PACKET}/source/generate-scale-corpus.py"))
        output = temp / "corpus"
        manifest_out = temp / "manifest.json"
        truth_out = temp / "truth.json"
        result = subprocess.run(
            [sys.executable, str(generator), "--output", str(output), "--manifest", str(manifest_out), "--truth", str(truth_out)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        check("generator-exit", result.returncode == 0, result.stderr.decode("utf-8", errors="replace"))
        check("generator-manifest", manifest_out.read_bytes() == blob(repo, CANDIDATE, f"{PACKET}/evidence/scale-corpus-manifest.json"), manifest_out)
        check("generator-truth", truth_out.read_bytes() == blob(repo, CANDIDATE, f"{PACKET}/evidence/scale-corpus-expected-truth.json"), truth_out)

    arch1 = blob(repo, CANDIDATE, "architecture/001-execution-and-ownership.md").decode("utf-8")
    slice_text = blob(repo, CANDIDATE, "specs/001-foundation-slice.md").decode("utf-8")
    product = blob(repo, CANDIDATE, "specs/000-product-contract.md").decode("utf-8")
    contract = blob(repo, CANDIDATE, f"{PACKET}/TARGET-CONTRACT.md").decode("utf-8")
    anchors = (
        "exact `4,194,304`-byte worker stacks",
        "max(1, min(8, available_parallelism))",
        "scopeTotal == total == 256",
        "No target may be met by muting, implicit filtering, source-scope reduction",
        "any analysis child process invalidates the sample",
        "CI cannot relax a number after seeing the result",
        "It does not claim an OS-cold page cache",
    )
    joined = "\n".join((arch1, slice_text, contract))
    for anchor in anchors:
        check(f"anchor:{anchor[:28]}", anchor in joined, anchor)

    slice_ac = re.findall(r"(?m)^\d+\. ", section(slice_text, "## 14.", "## 15."))
    trace = re.findall(r"(?m)^\| \d+ \|", section(slice_text, "## 15.", "## 16."))
    product_ac = re.findall(r"(?m)^\d+\. ", section(product, "## 4.", "## 5."))
    coverage = re.findall(r"(?m)^\| \d+ [^|]+ \|", section(slice_text, "## 16.", "## 17."))
    check("slice-ac-trace", len(slice_ac) == len(trace) == 38, f"{len(slice_ac)}/{len(trace)}")
    check("product-ac-coverage", len(product_ac) == len(coverage) == 22, f"{len(product_ac)}/{len(coverage)}")

    ledger_meta = git_text(repo, "show", "-s", "--format=%P%n%s", LEDGER).splitlines()
    check("ledger-parent", ledger_meta[0] == CANDIDATE, ledger_meta[0])
    ledger_rows = git_text(repo, "diff-tree", "--no-commit-id", "--name-only", "-r", LEDGER).splitlines()
    check("ledger-isolation", ledger_rows == ["reviews/architecture-v1-independent-verification-2026-07-15.md"], ledger_rows)

    request = strict_json((HERE / "review-request.json").read_bytes(), "review-request.json")
    check("request-candidate", request["candidate"] == CANDIDATE, request["candidate"])
    author_bytes = (HERE / "author-checks.json").read_bytes()
    check("author-checks-sha256", hashlib.sha256(author_bytes).hexdigest() == AUTHOR_CHECKS_SHA256, AUTHOR_CHECKS_SHA256)
    author = strict_json(author_bytes, "author-checks.json")
    check("author-checks-result", author["status"] == "PASS" and author["passed"] == 345 and author["failed"] == 0, author["status"])

    print(json.dumps({"status": "pass", "checkCount": len(checks), "checks": checks}, indent=2))


if __name__ == "__main__":
    main()
