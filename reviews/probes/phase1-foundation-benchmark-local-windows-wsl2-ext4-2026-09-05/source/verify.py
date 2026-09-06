#!/usr/bin/env python3
"""Verify the retained local Windows and WSL2 ext4 benchmark reports."""

from __future__ import annotations

import argparse
import hashlib
import json
from collections import Counter
from pathlib import Path


REPORTS = {
    "windows-ntfs.json": ("windows-ntfs", "windows-x64"),
    "wsl2-ext4.json": ("wsl2-ext4", "linux-x64"),
}
MODES = (
    "cold-audit-default",
    "cold-audit-jobs-1",
    "warm-audit-default",
    "cold-pre-write-default",
    "warm-pre-write-default",
    "post-write-one-file-default",
    "post-write-32-files-default",
)
FIXTURE = {
    "schemaVersion": "phase1-scale-findings.v1",
    "fileCount": 780,
    "totalBytes": 7_461_511,
    "truthSha256": "1230a9c577fefd0b8df4844c832f26b9e2a33945acd27d867d03132bc36512f0",
}


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_json(path: Path) -> dict:
    value = json.loads(path.read_bytes())
    assert isinstance(value, dict)
    return value


def verify_report(report: dict, environment: str, target: str) -> None:
    assert report["schemaVersion"] == "lumin.phase1-foundation-benchmark.v1"
    assert report["environment"] == environment
    assert report["blocking"] is True
    assert report["status"] == "PASS"
    assert report["package"]["target"] == target
    assert report["package"]["buildId"].startswith("build_")
    assert len(report["package"]["buildId"]) == 70
    assert report["package"]["binaryBytes"] == report["summary"]["binaryBytes"]
    assert report["summary"]["binaryMaximumBytes"] == 12 * 1024 * 1024
    assert report["summary"]["binarySizeMet"] is True
    assert report["summary"]["peakRssMaximumBytes"] == 512 * 1024 * 1024
    assert report["summary"]["peakRssMet"] is True
    assert report["summary"]["scaling"]["maximumRatio"] == 0.75
    assert report["summary"]["scaling"]["met"] is True
    assert report["summary"]["targetMisses"] == []

    for key, expected in FIXTURE.items():
        assert report["fixture"][key] == expected
    assert report["semanticTruth"]["schemaVersion"] == "phase1-scale-finding-id-map.v1"
    assert report["semanticTruth"]["mappingCount"] == 256
    assert len(report["semanticTruth"]["mappings"]) == 256
    assert report["defaultJobs"] == 8
    assert report["workerStackBytes"] == 4 * 1024 * 1024
    assert report["repetitionsPerMode"] == 3
    assert len(report["samples"]) == 21
    assert Counter(sample["mode"] for sample in report["samples"]) == Counter(
        {mode: 3 for mode in MODES}
    )

    semantic_sha256 = report["semanticTruth"]["sha256"]
    for mode in MODES:
        repetitions = sorted(
            sample["repetition"]
            for sample in report["samples"]
            if sample["mode"] == mode
        )
        assert repetitions == [1, 2, 3]
    for sample in report["samples"]:
        process = sample["productProcess"]
        assert process["exitCode"] == 0
        assert process["analysisChildPids"] == []
        assert sample["semanticDumpSha256"] == semantic_sha256
        expected_jobs = 1 if sample["mode"] == "cold-audit-jobs-1" else 8
        assert sample["actualJobs"] == expected_jobs
        assert sample["workerStackBytes"] == 4 * 1024 * 1024
    assert all(entry["met"] is True for entry in report["summary"]["medians"].values())


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("packet", type=Path)
    args = parser.parse_args()
    packet = args.packet.resolve(strict=True)
    evidence = packet / "evidence"

    sums = {}
    for line in (evidence / "SHA256SUMS").read_text(encoding="utf-8").splitlines():
        expected, name = line.split("  ", 1)
        assert name not in sums
        sums[name] = expected
    assert set(sums) == set(REPORTS)
    for name, expected in sums.items():
        assert digest(evidence / name) == expected

    reports = {}
    for name, (environment, target) in REPORTS.items():
        report = load_json(evidence / name)
        verify_report(report, environment, target)
        reports[environment] = report

    windows = reports["windows-ntfs"]
    wsl = reports["wsl2-ext4"]
    assert windows["package"]["buildId"] == wsl["package"]["buildId"]
    assert windows["fixture"] == wsl["fixture"]
    assert windows["scanInvocation"] == wsl["scanInvocation"]
    assert windows["semanticTruth"] == wsl["semanticTruth"]

    print(
        "PASS: retained Windows NTFS and WSL2 ext4 matrices share build, "
        "fixture, invocation, and semantic truth; P1-60 remains open"
    )


if __name__ == "__main__":
    main()
