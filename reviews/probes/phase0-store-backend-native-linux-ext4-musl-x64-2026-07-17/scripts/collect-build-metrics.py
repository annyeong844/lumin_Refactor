#!/usr/bin/env python3
"""Collect reproducible feature-specific Linux build surfaces for the Phase 0 probe."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import time


ARCHITECTURE_COMMIT = "65e60216891bb3d826a4778f84cb8aaa377abe92"
ARCHITECTURE_MANIFEST = (
    "66925583362be22257dd7357072d176cd3f1ae3d4f99874a53f6689b7cf535e0"
)
BACKENDS = (("redb", "redb-backend"), ("sqlite", "sqlite-backend"))
NATIVE_SUFFIXES = {".c", ".h", ".cc", ".cpp", ".S", ".asm"}


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def run(
    command: list[str], cwd: Path, *, log_path: Path | None = None
) -> subprocess.CompletedProcess[str]:
    started = time.perf_counter_ns()
    completed = subprocess.run(
        command,
        cwd=cwd,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    completed.elapsed_millis = (time.perf_counter_ns() - started) // 1_000_000
    if log_path is not None:
        log_path.write_text(completed.stdout, encoding="utf-8", newline="\n")
    if completed.returncode != 0:
        raise RuntimeError(
            f"command failed with {completed.returncode}: {' '.join(command)}\n"
            f"{completed.stdout}"
        )
    return completed


def relative_to_source(path: Path, source: Path) -> str:
    return path.resolve().relative_to(source.resolve()).as_posix()


def remove_measurement_target(path: Path, source: Path, label: str) -> None:
    resolved = path.resolve()
    resolved.relative_to(source.resolve())
    if resolved.name != f"target-measure-{label}":
        raise RuntimeError(f"refusing to remove unexpected target {resolved}")
    if resolved.exists():
        shutil.rmtree(resolved)


def build_command(
    mode: str, target: str, feature: str, target_directory: Path
) -> list[str]:
    common = [
        "--release",
        "--locked",
        "--no-default-features",
        "--features",
        feature,
        "--target-dir",
        str(target_directory),
    ]
    if mode == "gnu":
        return ["cargo", "build", *common]
    return ["cargo", "zigbuild", "--target", target, *common]


def dependency_surface(
    source: Path, evidence: Path, mode: str, backend: str, feature: str, target: str
) -> dict[str, object]:
    tree_path = evidence / f"dependency-tree-{mode}-{backend}.txt"
    tree_command = [
        "cargo",
        "tree",
        "--locked",
        "--no-default-features",
        "--features",
        feature,
        "--target",
        target,
        "--prefix",
        "none",
    ]
    tree = run(tree_command, source)
    tree_path.write_text(tree.stdout, encoding="utf-8", newline="\n")

    metadata_command = [
        "cargo",
        "metadata",
        "--locked",
        "--format-version",
        "1",
        "--no-default-features",
        "--features",
        feature,
        "--filter-platform",
        target,
    ]
    metadata = json.loads(run(metadata_command, source).stdout)
    selected_ids = {node["id"] for node in metadata["resolve"]["nodes"]}
    selected = [package for package in metadata["packages"] if package["id"] in selected_ids]
    dependencies = [
        package for package in selected if package["name"] != "lumin-phase0-store-probe"
    ]

    unsafe_lines = 0
    packages_with_unsafe = 0
    native_files = 0
    native_bytes = 0
    unsafe_pattern = re.compile(r"\bunsafe\b")
    for package in dependencies:
        package_root = Path(package["manifest_path"]).parent
        package_unsafe = 0
        for rust_file in package_root.rglob("*.rs"):
            try:
                with rust_file.open("r", encoding="utf-8", errors="replace") as handle:
                    package_unsafe += sum(
                        1 for line in handle if unsafe_pattern.search(line) is not None
                    )
            except OSError as error:
                raise RuntimeError(f"read dependency source {rust_file}: {error}") from error
        if package_unsafe:
            packages_with_unsafe += 1
            unsafe_lines += package_unsafe
        for candidate in package_root.rglob("*"):
            if candidate.is_file() and candidate.suffix in NATIVE_SUFFIXES:
                native_files += 1
                native_bytes += candidate.stat().st_size

    return {
        "selected_package_count_including_probe": len(selected),
        "transitive_package_count": len(dependencies),
        "rust_unsafe_keyword_line_count": unsafe_lines,
        "packages_with_rust_unsafe_keyword": packages_with_unsafe,
        "native_source_file_count": native_files,
        "native_source_bytes": native_bytes,
        "dependency_tree": relative_to_source(tree_path, source),
        "dependency_tree_sha256": sha256(tree_path),
        "method": (
            "Selected Cargo resolve nodes; dependency .rs lines matching word unsafe; "
            "native .c/.h/.cc/.cpp/.S/.asm files and bytes. This is a reproducible "
            "surface metric, not a safety audit."
        ),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--evidence", required=True, type=Path)
    parser.add_argument("--mode", required=True, choices=("gnu", "musl"))
    parser.add_argument("--target", required=True)
    parser.add_argument("--harness", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--reuse-report", type=Path)
    arguments = parser.parse_args()

    source = arguments.source.resolve(strict=True)
    evidence = arguments.evidence.resolve()
    evidence.mkdir(parents=True, exist_ok=True)
    harness = arguments.harness.resolve(strict=True)
    collector = Path(__file__).resolve(strict=True)
    results: list[dict[str, object]] = []
    reused_results: dict[str, dict[str, object]] = {}
    if arguments.reuse_report is not None:
        previous = json.loads(arguments.reuse_report.resolve(strict=True).read_text())
        if (
            previous.get("architecture_commit") != ARCHITECTURE_COMMIT
            or previous.get("architecture_manifest_sha256") != ARCHITECTURE_MANIFEST
            or previous.get("mode") != arguments.mode
            or previous.get("target") != arguments.target
        ):
            raise RuntimeError("reused build report identity does not match this collection")
        reused_results = {item["backend"]: item for item in previous["results"]}

    for backend, feature in BACKENDS:
        label = f"{arguments.mode}-{backend}"
        if reused_results:
            result = reused_results.get(backend)
            if result is None or result.get("feature") != feature:
                raise RuntimeError(f"reused report is missing {backend}/{feature}")
            target_directory = (source / result["target_directory"]).resolve(strict=True)
            binary = (source / result["binary"]).resolve(strict=True)
            if (
                binary.stat().st_size != result["binary_bytes"]
                or sha256(binary) != result["binary_sha256"]
            ):
                raise RuntimeError(f"reused {backend} binary identity changed")
            for phase in ("clean_build", "incremental_build"):
                log_path = (source / result[phase]["log"]).resolve(strict=True)
                if sha256(log_path) != result[phase]["log_sha256"]:
                    raise RuntimeError(f"reused {backend} {phase} log changed")
            result = dict(result)
        else:
            target_directory = source / f"target-measure-{label}"
            remove_measurement_target(target_directory, source, label)
            clean_log = evidence / f"build-{label}-clean.log"
            incremental_log = evidence / f"build-{label}-incremental.log"
            command = build_command(arguments.mode, arguments.target, feature, target_directory)
            clean = run(command, source, log_path=clean_log)
            incremental = run(command, source, log_path=incremental_log)
            binary = target_directory
            if arguments.mode == "musl":
                binary /= arguments.target
            binary /= "release/lumin-phase0-store-probe"
            binary = binary.resolve(strict=True)
            result = {
                "backend": backend,
                "feature": feature,
                "target_directory": relative_to_source(target_directory, source),
                "binary": relative_to_source(binary, source),
                "clean_build": {
                    "command": " ".join(command),
                    "elapsed_millis": clean.elapsed_millis,
                    "log": relative_to_source(clean_log, source),
                    "log_sha256": sha256(clean_log),
                },
                "incremental_build": {
                    "command": " ".join(command),
                    "elapsed_millis": incremental.elapsed_millis,
                    "log": relative_to_source(incremental_log, source),
                    "log_sha256": sha256(incremental_log),
                },
                "binary_bytes": binary.stat().st_size,
                "binary_sha256": sha256(binary),
            }
        result["dependency_surface"] = dependency_surface(
            source, evidence, arguments.mode, backend, feature, arguments.target
        )
        results.append(result)

    report = {
        "probe_id": "lumin-store-linux-build-surface-v1",
        "architecture_commit": ARCHITECTURE_COMMIT,
        "architecture_manifest_sha256": ARCHITECTURE_MANIFEST,
        "mode": arguments.mode,
        "target": arguments.target,
        "host_uname": run(["uname", "-a"], source).stdout.strip(),
        "filesystem": run(
            ["findmnt", "-T", str(source), "-n", "-o", "TARGET,FSTYPE,SOURCE"],
            source,
        ).stdout.strip(),
        "rustc": run(["rustc", "-Vv"], source).stdout.strip(),
        "cargo": run(["cargo", "-V"], source).stdout.strip(),
        "zig": run(["zig", "version"], source).stdout.strip()
        if arguments.mode == "musl"
        else None,
        "cargo_zigbuild": "0.23.0" if arguments.mode == "musl" else None,
        "collector_sha256": sha256(collector),
        "harness_executable": {
            "path": str(harness),
            "bytes": harness.stat().st_size,
            "sha256": sha256(harness),
        },
        "results": results,
    }
    output = arguments.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8", newline="\n")
    print(output)


if __name__ == "__main__":
    main()
