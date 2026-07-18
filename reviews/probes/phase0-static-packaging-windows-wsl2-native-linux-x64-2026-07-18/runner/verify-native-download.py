#!/usr/bin/env python3
"""Independently verify the downloaded native Linux workflow artifact."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import struct
from typing import Any


EXPECTED_RUN_ID = "29629760482"
EXPECTED_RUNNER_SHA = "721984d52e75d2385948767ce8ade6f190babaf2"
EXPECTED_ARCHIVE_SHA256 = "073ef5907944f8b79df8eab07d135826365f143c4d590ee3d59d7f57d5926454"
EXPECTED_SOURCE_SHA256 = "dd30eeda67caf9e354838a9ec7974cdd3dc118a9136c2556fcfe56c9f441db45"
EXPECTED_DEPENDENCIES = {
    "anyhow": "1.0.103",
    "oxc_allocator": "0.126.0",
    "oxc_parser": "0.126.0",
    "oxc_span": "0.126.0",
    "rayon": "1.11.0",
    "redb": "4.1.0",
    "serde": "1.0.228",
    "serde_json": "1.0.150",
}


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def strict_json(path: Path) -> Any:
    def reject_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, value in pairs:
            if key in result:
                raise RuntimeError(f"duplicate JSON key in {path}: {key}")
            result[key] = value
        return result

    return json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=reject_duplicates)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--artifact-root", required=True, type=Path)
    parser.add_argument("--archive", required=True, type=Path)
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()

    artifact_root = args.artifact_root.resolve(strict=True)
    archive = args.archive.resolve(strict=True)
    source = args.source.resolve(strict=True)
    evidence = artifact_root / "evidence" / "native-linux-ext4"
    checks: list[str] = []

    def check(name: str, condition: bool) -> None:
        if not condition:
            raise RuntimeError(f"independent native check failed: {name}")
        checks.append(name)

    check("artifact-archive-sha256", digest(archive) == EXPECTED_ARCHIVE_SHA256)
    check("source-manifest-sha256", digest(source / "SHA256SUMS") == EXPECTED_SOURCE_SHA256)

    manifest = (evidence / "SHA256SUMS").read_bytes()
    check("manifest-lf-final", b"\r" not in manifest and manifest.endswith(b"\n"))
    entries: list[str] = []
    for line in manifest.decode("utf-8").splitlines():
        expected, relative = line.split("  ", 1)
        pure = PurePosixPath(relative)
        check(
            f"safe-path:{relative}",
            len(expected) == 64
            and not pure.is_absolute()
            and ".." not in pure.parts
            and "\\" not in relative,
        )
        target = evidence / relative
        check(f"member-hash:{relative}", target.is_file() and digest(target) == expected)
        entries.append(relative)
    check("manifest-cardinality", len(entries) == 18 and len(set(entries)) == 18)
    actual_files = {
        path.relative_to(evidence).as_posix()
        for path in evidence.rglob("*")
        if path.is_file() and path.name != "SHA256SUMS"
    }
    check("manifest-inventory", set(entries) == actual_files)

    summary = strict_json(evidence / "summary.json")
    check(
        "summary-status-scope",
        summary["status"] == "PASS" and summary["scope"] == "native-linux-ext4",
    )
    check(
        "architecture-boundary",
        summary["architectureCandidate"] == "9a0dbe5c89463892c001e864c4f18eeab9e0eaed"
        and summary["architectureManifestSha256"]
        == "e2ca379a8a659f2febbc4e277c89db67bb02035a6b10467cf78a5663f21cd99a",
    )
    check(
        "claim-boundary",
        summary["claimBoundary"]
        == {
            "achievedProductBudgets": False,
            "nativePathRootDto": False,
            "packagedSkills": False,
            "productApiOrScaffold": False,
            "publicProcessBehavior": False,
            "staticPackagingFeasibilityOnly": True,
        },
    )
    host = summary["host"]
    check(
        "runner-identity",
        host["ciRunId"] == EXPECTED_RUN_ID and host["ciSha"] == EXPECTED_RUNNER_SHA,
    )
    check(
        "native-ext4-host",
        host["hostKind"] == "native-linux"
        and host["filesystemType"] == "ext4"
        and "microsoft" not in host["uname"].lower()
        and "wsl" not in host["uname"].lower(),
    )
    check("summary-source-identity", summary["sourceManifestSha256"] == EXPECTED_SOURCE_SHA256)

    metadata = strict_json(evidence / "cargo-metadata.json")
    package_by_id = {package["id"]: package for package in metadata["packages"]}
    root_node = next(node for node in metadata["resolve"]["nodes"] if node["id"] == metadata["resolve"]["root"])
    direct = {
        package_by_id[dependency["pkg"]]["name"]: package_by_id[dependency["pkg"]]["version"]
        for dependency in root_node["deps"]
    }
    check("direct-dependency-versions", direct == EXPECTED_DEPENDENCIES)
    cargo_links = sorted(
        f"{package['name']}@{package['version']}:{package['links']}"
        for package in metadata["packages"]
        if package.get("links")
    )
    check("cargo-links-surface", cargo_links == ["rayon-core@1.13.0:rayon-core"])

    by_label = {item["label"]: item for item in summary["artifacts"]}
    check("artifact-labels", set(by_label) == {"linux-gnu", "linux-musl"})
    for mode in ("gnu", "musl"):
        label = f"linux-{mode}"
        binary = (
            artifact_root
            / "source"
            / "target"
            / f"x86_64-unknown-linux-{mode}"
            / "release"
            / "lumin-phase0-static-packaging-probe"
        )
        data = binary.read_bytes()
        check(f"artifact-hash:{label}", digest(binary) == by_label[label]["sha256"])
        check(f"artifact-size:{label}", len(data) == by_label[label]["sizeBytes"])
        check(
            f"elf64-x86_64:{label}",
            data[:6] == b"\x7fELF\x02\x01" and struct.unpack_from("<H", data, 18)[0] == 62,
        )
        run = strict_json(evidence / f"run-{label}.json")
        check(
            f"run-oracle:{label}",
            run["status"] == "PASS"
            and run["oxcStatementCount"] == 2
            and run["rayonSum"] == 4950
            and run["redbValue"] == 42,
        )

    linkage = (evidence / "linkage-linux-musl.txt").read_text(encoding="utf-8")
    check(
        "musl-no-interpreter",
        "INTERP" not in linkage and "Requesting program interpreter" not in linkage,
    )
    check("musl-no-needed", "(NEEDED)" not in linkage)
    check(
        "musl-static-tool-output",
        "statically linked" in linkage or "not a dynamic executable" in linkage,
    )

    result = {
        "artifactArchiveBytes": archive.stat().st_size,
        "artifactArchiveSha256": EXPECTED_ARCHIVE_SHA256,
        "artifactId": "8425075747",
        "artifactName": f"phase0-static-packaging-native-{EXPECTED_RUNNER_SHA}",
        "checkCount": len(checks),
        "checks": checks,
        "runnerCommit": EXPECTED_RUNNER_SHA,
        "schema": "lumin-phase0-static-packaging-native-independent-checks-v1",
        "sourceManifestSha256": EXPECTED_SOURCE_SHA256,
        "status": "PASS",
        "workflowRunId": EXPECTED_RUN_ID,
    }
    args.output.write_text(
        json.dumps(result, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    print(json.dumps(result, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
