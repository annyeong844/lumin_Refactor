#!/usr/bin/env python3
from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def rewrite_manifest(evidence: Path) -> None:
    paths = sorted(
        (
            path.relative_to(evidence).as_posix()
            for path in evidence.rglob("*")
            if path.is_file() and path.name != "SHA256SUMS"
        ),
        key=lambda value: value.encode("utf-8"),
    )
    lines = [f"{sha256((evidence / path).read_bytes())}  {path}\n" for path in paths]
    (evidence / "SHA256SUMS").write_bytes("".join(lines).encode("utf-8"))


def mutate_byte(path: Path) -> None:
    data = bytearray(path.read_bytes())
    if not data:
        raise RuntimeError(f"cannot mutate empty file: {path}")
    data[len(data) // 2] ^= 1
    path.write_bytes(data)


def invoke(verifier: Path, repository: Path, evidence: Path, command: str = "verify"):
    completed = subprocess.run(
        [
            sys.executable,
            str(verifier),
            command,
            "--repository-root",
            str(repository),
            "--evidence",
            str(evidence),
        ],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    payload = None
    payload_text = completed.stdout if completed.returncode == 0 else completed.stderr
    if payload_text.strip():
        payload = json.loads(payload_text.strip().splitlines()[-1])
    return completed.returncode, payload


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repository-root", required=True, type=Path)
    parser.add_argument("--evidence", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()

    repository = arguments.repository_root.resolve()
    evidence = arguments.evidence.resolve()
    verifier = (
        repository
        / "reviews/probes/phase0-pinned-upstream-provenance-2026-07-18/source/verify_provenance.py"
    )

    baseline_code, baseline_payload = invoke(verifier, repository, evidence)
    if baseline_code != 0:
        raise RuntimeError(f"baseline verification failed: {baseline_payload}")

    scenarios = [
        (
            "source-byte-substitution-with-resealed-manifest",
            "byte-sha256-mismatch",
            lambda root: mutate_byte(root / "objects/typescript-moduleNameResolver.ts"),
        ),
        (
            "typescript-js-substitution-with-resealed-manifest",
            "derived-byte-mismatch",
            lambda root: mutate_byte(root / "objects/typescript.js"),
        ),
        (
            "oracle-substitution-with-resealed-manifest",
            "evidence-oracle-mismatch",
            lambda root: mutate_byte(root / "oracle.json"),
        ),
        (
            "negative-control-status-forgery",
            "negative-controls-invalid",
            lambda root: (
                root / "negative-controls.json"
            ).write_text(
                json.dumps(
                    {
                        "schemaVersion": "lumin-phase0-provenance-negative-controls.v1",
                        "controls": [
                            {"id": "forged", "status": "fail", "reasonCode": "none"}
                            for _ in range(6)
                        ],
                    },
                    indent=2,
                    sort_keys=True,
                )
                + "\n",
                encoding="utf-8",
            ),
        ),
        (
            "extra-resealed-evidence-member",
            "evidence-inventory-mismatch",
            lambda root: (root / "extra.bin").write_bytes(b"not-authorized"),
        ),
    ]

    results = []
    with tempfile.TemporaryDirectory(prefix="lumin-provenance-adversarial-") as temporary:
        temporary_root = Path(temporary)
        for scenario_id, expected_reason, mutation in scenarios:
            candidate = temporary_root / scenario_id
            shutil.copytree(evidence, candidate)
            mutation(candidate)
            rewrite_manifest(candidate)
            code, payload = invoke(verifier, repository, candidate)
            actual_reason = payload.get("reasonCode") if isinstance(payload, dict) else None
            passed = code != 0 and actual_reason == expected_reason
            results.append(
                {
                    "id": scenario_id,
                    "status": "pass" if passed else "fail",
                    "expectedReasonCode": expected_reason,
                    "actualReasonCode": actual_reason,
                }
            )
            if not passed:
                raise RuntimeError(f"scenario failed: {results[-1]}")

    stale_code, stale_payload = invoke(verifier, repository, evidence, command="capture")
    stale_reason = stale_payload.get("reasonCode") if isinstance(stale_payload, dict) else None
    stale_passed = stale_code != 0 and stale_reason == "stale-evidence"
    results.append(
        {
            "id": "preexisting-evidence-capture",
            "status": "pass" if stale_passed else "fail",
            "expectedReasonCode": "stale-evidence",
            "actualReasonCode": stale_reason,
        }
    )
    if not stale_passed:
        raise RuntimeError(f"stale evidence scenario failed: {results[-1]}")

    output = {
        "schemaVersion": "lumin-phase0-provenance-adversarial-checks.v1",
        "status": "pass",
        "baseline": baseline_payload,
        "scenarioCount": len(results),
        "scenarios": results,
    }
    arguments.output.parent.mkdir(parents=True, exist_ok=True)
    arguments.output.write_text(
        json.dumps(output, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(json.dumps({"status": "pass", "scenarioCount": len(results)}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
