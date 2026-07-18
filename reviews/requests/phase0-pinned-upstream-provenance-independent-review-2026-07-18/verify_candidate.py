#!/usr/bin/env python3
from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path, PurePosixPath


CANDIDATE = "e6147b1b2dfea45d223c87f3ba7ffec543e9f82d"
PARENT = "e2a451e31ab648e17d8ae478b2578c1ccdef0fee"
TREE = "448005cbb75869a680849332a1d6efe23cf088ba"
SUBJECT = "Bind clean upstream provenance evidence"
GATE_BASE = "d65f8a49250bf94a6a05903ee4d8d2a07e64f197"
RUNNER = "25bf5c5dd11da351c68c90da54e40b44e62120ce"
PACKET = "reviews/probes/phase0-pinned-upstream-provenance-2026-07-18"
CANDIDATE_MANIFEST_SHA256 = (
    "ca46f77997c696f8eeefc2feabdb9c1031a6e58e36fcb6f2a7ed4ad1bca84fcd"
)
PACKET_MANIFEST_SHA256 = "77f9790453b7ebad9ba4ba5856f8d6de40bf971f43ec40c899d19fa272762482"
SOURCE_MANIFEST_SHA256 = "0f39d6782d79e980e128a7a70ed316ac0cae314d9e2812bec8e6825422406b92"
EVIDENCE_MANIFEST_SHA256 = (
    "2228b0ac40afe62d4d72c12919e7dae1a8d1c8a6921f507c6ea6790e56dfc28f"
)
ARTIFACT_SHA256 = "d5f25626b8c37808da2115483c41bc3facb14338a21cc68e310da332dde9009d"
HEX_64 = re.compile(r"^[0-9a-f]{64}$")


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def require(condition: bool, detail: str) -> None:
    if not condition:
        raise RuntimeError(detail)


def git(repository: Path, *arguments: str, check: bool = True) -> bytes:
    completed = subprocess.run(
        ["git", *arguments],
        cwd=repository,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if check:
        require(completed.returncode == 0, completed.stderr.decode("utf-8", "replace"))
    return completed.stdout


def git_blob(repository: Path, revision: str, path: str) -> bytes:
    return git(repository, "show", f"{revision}:{path}")


def safe_path(value: str) -> None:
    path = PurePosixPath(value)
    require(value != "" and "\\" not in value, f"unsafe path: {value}")
    require(not path.is_absolute(), f"absolute path: {value}")
    require(all(part not in ("", ".", "..") for part in path.parts), f"unsafe path: {value}")


def parse_manifest(data: bytes, label: str) -> dict[str, str]:
    text = data.decode("utf-8")
    require(text.endswith("\n") and "\r" not in text, f"bad framing: {label}")
    result = {}
    for line in text.splitlines():
        require(len(line) > 66 and line[64:66] == "  ", f"bad line: {label}")
        digest, path = line[:64], line[66:]
        require(HEX_64.fullmatch(digest) is not None, f"bad digest: {label}")
        safe_path(path)
        require(path not in result, f"duplicate path: {path}")
        result[path] = digest
    require(
        list(result) == sorted(result, key=lambda value: value.encode("utf-8")),
        f"non-ordinal paths: {label}",
    )
    return result


def strict_json(data: bytes, label: str):
    def pairs_hook(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, f"duplicate JSON key {key}: {label}")
            result[key] = value
        return result

    return json.loads(data.decode("utf-8"), object_pairs_hook=pairs_hook)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repository-root", required=True, type=Path)
    parser.add_argument("--request-root", type=Path, default=Path(__file__).resolve().parent)
    arguments = parser.parse_args()
    repository = arguments.repository_root.resolve()
    request_root = arguments.request_root.resolve()
    checks = []

    def passed(name: str, condition: bool = True) -> None:
        require(condition, name)
        checks.append({"name": name, "status": "pass"})

    passed("candidate object", git(repository, "cat-file", "-t", CANDIDATE).strip() == b"commit")
    metadata = git(repository, "show", "-s", "--format=%H%n%P%n%T%n%s", CANDIDATE).decode().splitlines()
    passed("candidate SHA", metadata[0] == CANDIDATE)
    passed("candidate parent", metadata[1] == PARENT)
    passed("candidate tree", metadata[2] == TREE)
    passed("candidate subject", metadata[3] == SUBJECT)

    request = strict_json((request_root / "provenance-request.json").read_bytes(), "request")
    passed("request candidate", request["candidate"] == CANDIDATE)
    passed("request tree", request["candidateTree"] == TREE)
    passed("request artifact", request["cleanRunner"]["artifactArchiveSha256"] == ARTIFACT_SHA256)

    candidate_manifest_bytes = (request_root / "candidate-manifest.txt").read_bytes()
    candidate_manifest = parse_manifest(candidate_manifest_bytes, "candidate manifest")
    passed("candidate manifest digest", sha256(candidate_manifest_bytes) == CANDIDATE_MANIFEST_SHA256)
    passed("candidate manifest cardinality", len(candidate_manifest) == 16)
    for path, expected in candidate_manifest.items():
        passed(f"candidate blob: {path}", sha256(git_blob(repository, CANDIDATE, path)) == expected)

    packet_manifest_bytes = git_blob(repository, CANDIDATE, f"{PACKET}/SHA256SUMS")
    packet_manifest = parse_manifest(packet_manifest_bytes, "packet manifest")
    passed("packet manifest digest", sha256(packet_manifest_bytes) == PACKET_MANIFEST_SHA256)
    passed("packet manifest cardinality", len(packet_manifest) == 34)
    passed("request packet copy", (request_root / "packet-manifest.txt").read_bytes() == packet_manifest_bytes)
    packet_tree = {
        line
        for line in git(
            repository, "ls-tree", "-r", "--name-only", CANDIDATE, PACKET
        ).decode("utf-8").splitlines()
    }
    expected_tree = {f"{PACKET}/{path}" for path in packet_manifest} | {f"{PACKET}/SHA256SUMS"}
    passed("packet exact inventory", packet_tree == expected_tree)
    for path, expected in packet_manifest.items():
        passed(f"packet blob: {path}", sha256(git_blob(repository, CANDIDATE, f"{PACKET}/{path}")) == expected)

    source_manifest = git_blob(repository, CANDIDATE, f"{PACKET}/source/SHA256SUMS")
    evidence_manifest = git_blob(
        repository, CANDIDATE, f"{PACKET}/evidence/native-linux-clean/SHA256SUMS"
    )
    passed("source manifest digest", sha256(source_manifest) == SOURCE_MANIFEST_SHA256)
    passed("source manifest cardinality", len(parse_manifest(source_manifest, "source")) == 4)
    passed("evidence manifest digest", sha256(evidence_manifest) == EVIDENCE_MANIFEST_SHA256)
    passed("evidence manifest cardinality", len(parse_manifest(evidence_manifest, "evidence")) == 18)

    json_paths = [path for path in packet_manifest if path.endswith(".json")]
    for path in json_paths:
        strict_json(git_blob(repository, CANDIDATE, f"{PACKET}/{path}"), path)
        passed(f"strict JSON: {path}")

    result = strict_json(
        git_blob(repository, CANDIDATE, f"{PACKET}/evidence/native-linux-clean/result.json"),
        "result",
    )
    passed("seven upstream checks", len(result["upstreamByteChecks"]) == 7)
    passed("compiler option count", result["compilerOptions"]["count"] == 122)
    passed(
        "compiler option digest",
        result["compilerOptions"]["keyShapeSha256"]
        == "f2fb5da0cf33ea694a8bf4ccae909a1526e7978693c15ac1a6b10b3cdfbc9d9a",
    )
    passed("runner binding", result["runnerCommit"] == RUNNER)

    artifacts = strict_json(
        git_blob(repository, CANDIDATE, f"{PACKET}/runner/workflow-artifacts.json"),
        "workflow artifacts",
    )
    passed("one artifact", artifacts["total_count"] == 1)
    passed("artifact ID", artifacts["artifacts"][0]["id"] == 8427910952)
    passed("artifact digest", artifacts["artifacts"][0]["digest"] == "sha256:" + ARTIFACT_SHA256)

    range_paths = git(
        repository, "diff", "--name-only", f"{GATE_BASE}..{CANDIDATE}"
    ).decode("utf-8").splitlines()
    passed("gate range is probe-only", bool(range_paths) and all(path.startswith(PACKET + "/") for path in range_paths))
    passed(
        "temporary workflow absent",
        git(
            repository,
            "ls-tree",
            "--name-only",
            CANDIDATE,
            ".github/workflows/phase0-pinned-provenance.yml",
        )
        == b"",
    )
    runner_ancestry = subprocess.run(
        ["git", "merge-base", "--is-ancestor", RUNNER, CANDIDATE], cwd=repository
    )
    passed("runner commit ancestry", runner_ancestry.returncode == 0)
    passed(
        "runner workflow exact copy",
        git_blob(
            repository,
            RUNNER,
            ".github/workflows/phase0-pinned-provenance.yml",
        )
        == git_blob(repository, CANDIDATE, f"{PACKET}/runner/workflow.yml"),
    )

    packet_verifier = repository / PACKET / "runner/verify_packet.py"
    completed = subprocess.run(
        [sys.executable, str(packet_verifier), "--repository-root", str(repository)],
        cwd=repository,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    passed("packet verifier consistency", completed.returncode == 0)

    output = {
        "schemaVersion": "lumin-phase0-provenance-author-preflight.v1",
        "status": "pass",
        "candidate": CANDIDATE,
        "checks": checks,
        "checkCount": len(checks),
        "packetVerifier": json.loads(completed.stdout),
        "independenceBoundary": "Author preflight is not independent PASS evidence.",
    }
    print(json.dumps(output, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
