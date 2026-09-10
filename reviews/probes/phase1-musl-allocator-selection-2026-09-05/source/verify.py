#!/usr/bin/env python3
"""Verify the retained Linux-musl allocator comparison and dependency surface."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import tomllib
from collections import Counter
from pathlib import Path


REPORTS = (
    "system-report.json",
    "mimalloc-report-r1.json",
    "mimalloc-report-r2.json",
)
MODES = (
    "cold-audit-default",
    "cold-audit-jobs-1",
    "warm-audit-default",
    "cold-pre-write-default",
    "warm-pre-write-default",
    "post-write-one-file-default",
    "post-write-32-files-default",
)
TARGET = 'cfg(all(target_os = "linux", target_env = "musl"))'
CONTROL_PATCH = """--- candidate/crates/application/cli/src/main.rs
+++ system-control/crates/application/cli/src/main.rs
@@ -5,4 +4,0 @@
-#[cfg(all(target_os = \"linux\", target_env = \"musl\"))]
-#[global_allocator]
-static GLOBAL_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;
-
"""


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_json(path: Path) -> dict:
    value = json.loads(path.read_bytes())
    assert isinstance(value, dict)
    return value


def median(report: dict, mode: str) -> int:
    values = sorted(
        sample["productProcess"]["elapsedNanoseconds"]
        for sample in report["samples"]
        if sample["mode"] == mode
    )
    assert len(values) == 3
    return values[1]


def verify_report(report: dict) -> None:
    assert report["schemaVersion"] == "lumin.phase1-foundation-benchmark.v1"
    assert report["blocking"] is True
    assert report["environment"] == "wsl2-ext4"
    assert report["fixture"]["schemaVersion"] == "phase1-scale-findings.v1"
    assert report["fixture"]["fileCount"] == 780
    assert report["fixture"]["totalBytes"] == 7461511
    assert report["defaultJobs"] == 8
    assert report["observedAvailableParallelism"] == 12
    assert report["workerStackBytes"] == 4194304
    assert report["repetitionsPerMode"] == 3
    assert len(report["samples"]) == 21
    assert Counter(sample["mode"] for sample in report["samples"]) == Counter(
        {mode: 3 for mode in MODES}
    )
    semantic = report["semanticTruth"]["sha256"]
    for sample in report["samples"]:
        process = sample["productProcess"]
        assert process["exitCode"] == 0
        assert process["analysisChildPids"] == []
        assert sample["semanticDumpSha256"] == semantic
        expected_jobs = 1 if sample["mode"] == "cold-audit-jobs-1" else 8
        assert sample["actualJobs"] == expected_jobs
        assert sample["workerStackBytes"] == 4194304
    for summary in report["summary"]["medians"].values():
        assert summary["met"] is True
    assert report["summary"]["binarySizeMet"] is True
    assert report["summary"]["peakRssMet"] is True


def package_map(lock: dict) -> dict[tuple[str, str], dict]:
    result = {}
    for package in lock["package"]:
        key = (package["name"], package["version"])
        assert key not in result
        result[key] = package
    return result


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("packet", type=Path)
    args = parser.parse_args()
    packet = args.packet.resolve(strict=True)
    repo = packet.parents[2]
    evidence = packet / "evidence"

    sums = {}
    for line in (evidence / "SHA256SUMS").read_text(encoding="utf-8").splitlines():
        expected, name = line.split("  ", 1)
        assert name not in sums
        sums[name] = expected
    expected_names = {"dependency-cost.json", *REPORTS}
    assert set(sums) == expected_names
    for name, expected in sums.items():
        assert digest(evidence / name) == expected

    reports = {name: load_json(evidence / name) for name in REPORTS}
    for report in reports.values():
        verify_report(report)
    system = reports["system-report.json"]
    first = reports["mimalloc-report-r1.json"]
    accepted = reports["mimalloc-report-r2.json"]
    reference = system["semanticTruth"]
    fixture = system["fixture"]
    build_id = system["package"]["buildId"]
    for report in (first, accepted):
        assert report["semanticTruth"] == reference
        assert report["fixture"] == fixture
        assert report["package"]["buildId"] == build_id
    assert system["status"] == "FAIL"
    assert first["status"] == "FAIL"
    assert accepted["status"] == "PASS"
    assert len(system["summary"]["targetMisses"]) == 1
    assert len(first["summary"]["targetMisses"]) == 1
    assert accepted["summary"]["targetMisses"] == []
    assert system["summary"]["scaling"]["met"] is False
    assert first["summary"]["scaling"]["met"] is False
    assert accepted["summary"]["scaling"]["met"] is True
    for mode in MODES:
        assert median(accepted, mode) < median(system, mode)

    cost = load_json(evidence / "dependency-cost.json")
    assert cost["schemaVersion"] == "lumin.phase1-musl-allocator-cost.v1"
    assert cost["targetPredicate"] == TARGET
    measurement = cost["measurement"]
    assert measurement["systemBinaryBytes"] == system["package"]["binaryBytes"]
    assert measurement["mimallocBinaryBytes"] == accepted["package"]["binaryBytes"]
    assert measurement["binaryDeltaBytes"] == (
        accepted["package"]["binaryBytes"] - system["package"]["binaryBytes"]
    )
    assert math.isclose(
        measurement["systemScalingRatio"], system["summary"]["scaling"]["ratio"]
    )
    assert measurement["mimallocScalingRatios"] == [
        first["summary"]["scaling"]["ratio"],
        accepted["summary"]["scaling"]["ratio"],
    ]
    assert measurement["systemPeakRssBytes"] == system["summary"]["peakRssBytes"]
    assert measurement["acceptedMimallocPeakRssBytes"] == accepted["summary"]["peakRssBytes"]

    for relative, expected in cost["sourceHashes"].items():
        assert digest(repo / relative) == expected
    root_manifest = tomllib.loads((repo / "Cargo.toml").read_text(encoding="utf-8"))
    assert root_manifest["workspace"]["dependencies"]["mimalloc"] == {
        "version": "=0.1.52",
        "features": ["v2"],
    }
    cli_manifest = tomllib.loads(
        (repo / "crates/application/cli/Cargo.toml").read_text(encoding="utf-8")
    )
    assert cli_manifest["target"][TARGET]["dependencies"]["mimalloc"] == {
        "workspace": True
    }
    main_source = (repo / "crates/application/cli/src/main.rs").read_text(encoding="utf-8")
    assert main_source.count("static GLOBAL_ALLOCATOR: mimalloc::MiMalloc") == 1
    assert (packet / "source/system-allocator-control.patch").read_text(
        encoding="utf-8"
    ) == CONTROL_PATCH

    lock = tomllib.loads((repo / "Cargo.lock").read_text(encoding="utf-8"))
    packages = package_map(lock)
    for expected in cost["lockedPackages"]:
        package = packages[(expected["name"], expected["version"])]
        assert package["checksum"] == expected["checksum"]
    assert packages[("mimalloc", "0.1.52")]["dependencies"] == ["libmimalloc-sys"]
    assert packages[("libmimalloc-sys", "0.1.49")]["dependencies"] == ["cc"]
    assert set(packages[("cc", "1.4.5")]["dependencies"]) == {
        "find-msvc-tools",
        "shlex",
    }

    policy = load_json(repo / "tools/xtask/dependency-surface-policy.v2.json")
    workspace_edges = [
        edge for edge in policy["workspaceDependencies"] if edge["package"] == "mimalloc"
    ]
    assert workspace_edges == [
        {
            "alias": "mimalloc",
            "package": "mimalloc",
            "requirement": "=0.1.52",
            "defaultFeatures": True,
            "features": ["v2"],
            "sourceKind": "crates-io",
        }
    ]
    direct_edges = []
    for member in policy["members"]:
        for edge in member["dependencies"]:
            if edge["package"] == "mimalloc":
                direct_edges.append((member["name"], edge))
    assert len(direct_edges) == 1
    member, edge = direct_edges[0]
    assert member == "lumin-cli"
    assert edge["target"] == TARGET
    assert edge["kind"] == "normal"
    assert edge["resolution"] == {
        "kind": "crates-io",
        "name": "mimalloc",
        "version": "0.1.52",
        "source": "registry+https://github.com/rust-lang/crates.io-index",
    }

    improvement = 1 - median(accepted, "cold-audit-default") / median(
        system, "cold-audit-default"
    )
    print(
        "PASS: allocator packet is self-consistent; "
        f"cold default improved {improvement:.2%}, accepted scaling "
        f"{accepted['summary']['scaling']['ratio']:.6f}"
    )


if __name__ == "__main__":
    main()
