#!/usr/bin/env python3
from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path, PurePosixPath


RUN_ID = 29638671368
JOB_ID = 88065321460
RUNNER_COMMIT = "25bf5c5dd11da351c68c90da54e40b44e62120ce"
ARTIFACT_ID = 8427910952
ARTIFACT_SHA256 = "d5f25626b8c37808da2115483c41bc3facb14338a21cc68e310da332dde9009d"
EVIDENCE_MANIFEST_SHA256 = (
    "2228b0ac40afe62d4d72c12919e7dae1a8d1c8a6921f507c6ea6790e56dfc28f"
)
HEX_64 = re.compile(r"^[0-9a-f]{64}$")

PACKET_ROOT = Path(__file__).resolve().parent.parent
EVIDENCE = PACKET_ROOT / "evidence/native-linux-clean"


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def require(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def strict_json(path: Path):
    def pairs_hook(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise RuntimeError(f"duplicate JSON key {key}: {path}")
            result[key] = value
        return result

    return json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=pairs_hook)


def safe_path(value: str) -> None:
    path = PurePosixPath(value)
    require(value != "" and "\\" not in value, f"unsafe path: {value}")
    require(not path.is_absolute(), f"absolute path: {value}")
    require(all(part not in ("", ".", "..") for part in path.parts), f"unsafe path: {value}")


def parse_manifest(path: Path) -> dict[str, str]:
    data = path.read_bytes()
    text = data.decode("utf-8")
    require(text.endswith("\n") and "\r" not in text, f"bad manifest framing: {path}")
    result = {}
    for line in text.splitlines():
        require(len(line) > 66 and line[64:66] == "  ", f"bad manifest line: {path}")
        digest, relative = line[:64], line[66:]
        require(HEX_64.fullmatch(digest) is not None, f"bad digest: {path}")
        safe_path(relative)
        require(relative not in result, f"duplicate manifest path: {relative}")
        result[relative] = digest
    require(
        list(result) == sorted(result, key=lambda value: value.encode("utf-8")),
        f"non-ordinal manifest: {path}",
    )
    return result


def run_git(repository: Path, *arguments: str, allow_failure: bool = False) -> bytes:
    completed = subprocess.run(
        ["git", *arguments],
        cwd=repository,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if not allow_failure:
        require(completed.returncode == 0, completed.stderr.decode("utf-8", "replace"))
    return completed.stdout


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repository-root", required=True, type=Path)
    arguments = parser.parse_args()
    repository = arguments.repository_root.resolve()
    checks = []

    def passed(name: str, condition: bool) -> None:
        require(condition, name)
        checks.append(name)

    packet_manifest_path = PACKET_ROOT / "SHA256SUMS"
    packet_manifest = parse_manifest(packet_manifest_path)
    packet_inventory = {
        path.relative_to(PACKET_ROOT).as_posix(): path
        for path in PACKET_ROOT.rglob("*")
        if path.is_file() and path != packet_manifest_path
    }
    passed("packet inventory", set(packet_manifest) == set(packet_inventory))
    for relative, expected in packet_manifest.items():
        passed(f"packet hash: {relative}", sha256(packet_inventory[relative].read_bytes()) == expected)

    evidence_manifest = parse_manifest(EVIDENCE / "SHA256SUMS")
    passed("evidence manifest digest", sha256((EVIDENCE / "SHA256SUMS").read_bytes()) == EVIDENCE_MANIFEST_SHA256)
    passed("evidence cardinality", len(evidence_manifest) == 18)

    verifier = PACKET_ROOT / "source/verify_provenance.py"
    completed = subprocess.run(
        [
            sys.executable,
            str(verifier),
            "verify",
            "--repository-root",
            str(repository),
            "--evidence",
            str(EVIDENCE),
        ],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    passed("offline evidence verifier", completed.returncode == 0)
    verifier_result = json.loads(completed.stdout.strip())
    passed("seven upstream byte checks", verifier_result["upstreamByteChecks"] == 7)
    passed("122 compiler options", verifier_result["compilerOptionCount"] == 122)
    passed("six capture controls", verifier_result["negativeControls"] == 6)

    run = strict_json(PACKET_ROOT / "runner/workflow-run.json")
    passed("workflow run ID", run["databaseId"] == RUN_ID)
    passed("workflow runner commit", run["headSha"] == RUNNER_COMMIT)
    passed("workflow conclusion", run["status"] == "completed" and run["conclusion"] == "success")
    passed("workflow job", len(run["jobs"]) == 1 and run["jobs"][0]["databaseId"] == JOB_ID)
    passed("workflow job conclusion", run["jobs"][0]["conclusion"] == "success")

    artifacts = strict_json(PACKET_ROOT / "runner/workflow-artifacts.json")
    passed("one workflow artifact", artifacts["total_count"] == 1 and len(artifacts["artifacts"]) == 1)
    artifact = artifacts["artifacts"][0]
    passed("artifact ID", artifact["id"] == ARTIFACT_ID)
    passed("artifact digest", artifact["digest"] == "sha256:" + ARTIFACT_SHA256)
    passed("artifact runner commit", artifact["workflow_run"]["head_sha"] == RUNNER_COMMIT)

    download = strict_json(PACKET_ROOT / "runner/artifact-download-verification.json")
    passed("direct artifact digest", download["artifactArchiveSha256"] == ARTIFACT_SHA256)
    passed("portable replay", download["capturedNodeVersion"] != download["independentReplayNodeVersion"])
    passed("download verification", download["status"] == "pass" and download["offlineVerification"] == "pass")

    adversarial = strict_json(PACKET_ROOT / "runner/adversarial-checks.json")
    passed("adversarial status", adversarial["status"] == "pass")
    passed("six resealed attacks", adversarial["scenarioCount"] == 6)
    passed("all resealed attacks rejected", all(row["status"] == "pass" for row in adversarial["scenarios"]))

    runner_workflow = run_git(
        repository,
        "show",
        f"{RUNNER_COMMIT}:.github/workflows/phase0-pinned-provenance.yml",
    )
    passed("runner workflow exact copy", runner_workflow == (PACKET_ROOT / "runner/workflow.yml").read_bytes())
    current_workflow = run_git(
        repository,
        "ls-tree",
        "--name-only",
        "HEAD",
        ".github/workflows/phase0-pinned-provenance.yml",
    )
    passed("temporary workflow absent from candidate", current_workflow == b"")
    ancestry = subprocess.run(
        ["git", "merge-base", "--is-ancestor", RUNNER_COMMIT, "HEAD"], cwd=repository
    )
    passed("runner commit retained in ancestry", ancestry.returncode == 0)

    log = (PACKET_ROOT / "runner/workflow-log.txt").read_text(encoding="utf-8")
    passed("workflow log checkout", RUNNER_COMMIT in log)
    passed("workflow log artifact", str(ARTIFACT_ID) in log)
    passed("workflow log oracle", '"upstreamByteChecks": 7' in log and '"compilerOptionCount": 122' in log)

    print(
        json.dumps(
            {
                "status": "pass",
                "checks": len(checks),
                "packetEntries": len(packet_manifest),
                "packetManifestSha256": sha256(packet_manifest_path.read_bytes()),
                "evidenceEntries": len(evidence_manifest),
                "evidenceManifestSha256": EVIDENCE_MANIFEST_SHA256,
                "runnerCommit": RUNNER_COMMIT,
                "artifactArchiveSha256": ARTIFACT_SHA256,
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
