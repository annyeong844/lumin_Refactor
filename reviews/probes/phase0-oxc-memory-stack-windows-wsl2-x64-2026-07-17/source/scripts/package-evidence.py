#!/usr/bin/env python3
"""Strictly validate and package Windows and WSL2 OXC probe matrices."""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
from pathlib import Path, PurePosixPath
from typing import Any


ARCHITECTURE_COMMIT = "65e60216891bb3d826a4778f84cb8aaa377abe92"
ARCHITECTURE_MANIFEST = "66925583362be22257dd7357072d176cd3f1ae3d4f99874a53f6689b7cf535e0"
CORPUS_ID = "lumin-lab-35290cb-plus-stack-stress-v1"
SOURCE_COMMIT = "35290cb683a37e83bc8c915d1d0f9ca0f3f96fd0"
STACK_CANDIDATES = [262_144, 524_288, 1_048_576, 2_097_152, 4_194_304, 8_388_608]
SOURCE_FILES = [
    ".gitignore",
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "rustfmt.toml",
    "README.md",
    "PROBE-CONTRACT.md",
    "scripts/prepare-corpus.py",
    "scripts/run-matrix.py",
    "scripts/package-evidence.py",
    "src/main.rs",
    "src/model.rs",
    "src/corpus.rs",
    "src/probe.rs",
    "src/memory.rs",
    "src/util.rs",
]


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def file_identity(path: Path) -> dict[str, Any]:
    data = path.read_bytes()
    return {"bytes": len(data), "sha256": sha256(data)}


def load_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def write_json(path: Path, value: Any) -> None:
    path.write_bytes((json.dumps(value, indent=2, sort_keys=True) + "\n").encode("utf-8"))


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--corpus-manifest", required=True, type=Path)
    parser.add_argument("--windows", required=True, type=Path)
    parser.add_argument("--wsl", required=True, type=Path)
    parser.add_argument("--evidence", required=True, type=Path)
    return parser.parse_args()


def canonical_relative(raw: str) -> Path:
    portable = PurePosixPath(raw)
    if not raw or raw.startswith("/") or "\\" in raw or portable.as_posix() != raw:
        raise RuntimeError(f"noncanonical evidence path: {raw!r}")
    if any(part in {"", ".", ".."} for part in portable.parts):
        raise RuntimeError(f"unsafe evidence path: {raw!r}")
    return Path(*portable.parts)


def source_identity(source: Path) -> tuple[list[dict[str, str]], str]:
    files = []
    for raw in SOURCE_FILES:
        path = source / canonical_relative(raw)
        if not path.is_file():
            raise RuntimeError(f"missing source identity file: {raw}")
        files.append({"path": raw, "sha256": sha256(path.read_bytes())})
    lines = sorted(f"{entry['sha256']}  {entry['path']}\n" for entry in files)
    return files, sha256("".join(lines).encode("utf-8"))


def expected_jobs(available: int) -> list[int]:
    values: list[int] = []
    value = 1
    while value <= available:
        values.append(value)
        value *= 2
    if values[-1] != available:
        values.append(available)
    return values


def validate_raw_files(directory: Path, summary: dict[str, Any]) -> None:
    declared = summary.get("raw_files")
    if not isinstance(declared, list):
        raise RuntimeError("matrix raw_files is missing")
    expected_paths = set()
    for entry in declared:
        relative = canonical_relative(entry["path"])
        path = directory / relative
        if not path.is_file() or path.is_symlink():
            raise RuntimeError(f"missing or nonregular matrix evidence: {entry['path']}")
        if file_identity(path) != {"bytes": entry["bytes"], "sha256": entry["sha256"]}:
            raise RuntimeError(f"matrix evidence identity mismatch: {entry['path']}")
        expected_paths.add(entry["path"])
    actual_paths = {
        path.relative_to(directory).as_posix()
        for path in directory.rglob("*")
        if path.is_file() and path.name != "matrix-summary.json"
    }
    if actual_paths != expected_paths:
        raise RuntimeError("matrix raw file set differs from summary")


def validate_matrix(
    directory: Path,
    *,
    expected_os: str,
    source_files: list[dict[str, str]],
    source_manifest: str,
    corpus_manifest_sha256: str,
) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    summary = load_json(directory / "matrix-summary.json")
    required = {
        "schema": "lumin-phase0-oxc-matrix-v1",
        "status": "PASS",
        "architecture_commit": ARCHITECTURE_COMMIT,
        "architecture_manifest_sha256": ARCHITECTURE_MANIFEST,
        "corpus_manifest_sha256": corpus_manifest_sha256,
        "source_manifest_sha256": source_manifest,
        "source_files": source_files,
        "stack_candidates": STACK_CANDIDATES,
    }
    for key, value in required.items():
        if summary.get(key) != value:
            raise RuntimeError(f"matrix {directory} field mismatch: {key}")
    validate_raw_files(directory, summary)
    available = summary.get("available_parallelism")
    if not isinstance(available, int) or available < 1:
        raise RuntimeError("invalid matrix parallelism")
    if summary.get("jobs_candidates") != expected_jobs(available):
        raise RuntimeError("jobs candidate set is incomplete")

    runs = summary.get("runs", [])
    stack_runs = [run for run in runs if run.get("kind") == "stack"]
    jobs_runs = [run for run in runs if run.get("kind") == "jobs"]
    lifetime_runs = [run for run in runs if run.get("kind") == "allocator-lifetime"]
    if [run.get("stack_bytes") for run in stack_runs] != STACK_CANDIDATES:
        raise RuntimeError("stack run set/order mismatch")
    if [run.get("workers") for run in jobs_runs] != expected_jobs(available):
        raise RuntimeError("jobs run set/order mismatch")
    if len(lifetime_runs) != 1 or lifetime_runs[0].get("waves") != 8:
        raise RuntimeError("allocator lifetime run is missing")
    by_stack = {run["stack_bytes"]: run for run in stack_runs}
    if by_stack[4_194_304].get("status") != "PASS" or by_stack[8_388_608].get("status") != "PASS":
        raise RuntimeError("required probe-validity stack did not pass")
    if any(run.get("status") != "PASS" for run in [*jobs_runs, *lifetime_runs]):
        raise RuntimeError("jobs or allocator-lifetime run failed")

    reports = []
    for run in runs:
        if run.get("status") != "PASS":
            continue
        report_path = directory / canonical_relative(run["report_path"])
        report = load_json(report_path)
        if report.get("host_os") != expected_os:
            raise RuntimeError(f"unexpected report host OS: {report.get('host_os')}")
        checks = {
            "status": "PASS",
            "architecture_commit": ARCHITECTURE_COMMIT,
            "architecture_manifest_sha256": ARCHITECTURE_MANIFEST,
            "corpus_manifest_sha256": corpus_manifest_sha256,
            "source_manifest_sha256": source_manifest,
            "source_files": source_files,
            "semantic_digest": summary["semantic_digest"],
            "requested_workers": run["workers"],
            "actual_workers": run["workers"],
            "worker_stack_bytes": run["stack_bytes"],
            "waves": run["waves"],
        }
        for key, value in checks.items():
            if report.get(key) != value:
                raise RuntimeError(f"run report field mismatch: {run['run_id']} {key}")
        if report.get("executable", {}).get("sha256") != summary["binary"]["sha256"]:
            raise RuntimeError("run binary identity mismatch")
        if any(wave.get("parser_panicked_files") != 0 for wave in report.get("wave_results", [])):
            raise RuntimeError("successful report contains parser-panicked files")
        reports.append({"run": run, "report": report})
    return summary, reports


def platform_metrics(summary: dict[str, Any], reports: list[dict[str, Any]]) -> dict[str, Any]:
    stack_passes = [
        item["run"]["stack_bytes"]
        for item in reports
        if item["run"]["kind"] == "stack"
    ]
    jobs = []
    lifetime_rss = []
    for item in reports:
        run = item["run"]
        report = item["report"]
        peak_values = [
            wave["memory_after_drop"]["peak_rss_bytes"]
            for wave in report["wave_results"]
            if wave["memory_after_drop"]["peak_rss_bytes"] is not None
        ]
        if run["kind"] == "jobs":
            jobs.append(
                {
                    "workers": run["workers"],
                    "child_elapsed_micros": run["elapsed_micros"],
                    "wave_elapsed_micros": [wave["elapsed_micros"] for wave in report["wave_results"]],
                    "peak_rss_bytes": max(peak_values) if peak_values else None,
                    "allocator_capacity_bytes_total": max(
                        wave["allocator_capacity_bytes_total"] for wave in report["wave_results"]
                    ),
                }
            )
        elif run["kind"] == "allocator-lifetime":
            lifetime_rss = [wave["memory_after_drop"]["current_rss_bytes"] for wave in report["wave_results"]]
    return {
        "platform": summary["platform"],
        "filesystem_class": summary["filesystem_class"],
        "available_parallelism": summary["available_parallelism"],
        "binary": summary["binary"],
        "minimum_observed_passing_stack_bytes": min(stack_passes),
        "stack_results": [
            {"stack_bytes": run["stack_bytes"], "status": run["status"], "return_code": run["return_code"]}
            for run in summary["runs"]
            if run["kind"] == "stack"
        ],
        "jobs": jobs,
        "allocator_lifetime_current_rss_bytes": lifetime_rss,
    }


def copy_matrix(source: Path, target: Path) -> None:
    for path in source.rglob("*"):
        if path.is_symlink():
            raise RuntimeError(f"matrix contains symlink: {path}")
    shutil.copytree(source, target)


def main() -> None:
    args = parse_args()
    source = args.source.resolve()
    corpus_manifest_path = args.corpus_manifest.resolve()
    windows = args.windows.resolve()
    wsl = args.wsl.resolve()
    evidence = args.evidence.resolve()
    if evidence.exists():
        if not evidence.is_dir() or any(evidence.iterdir()):
            raise RuntimeError(f"evidence target must be absent or empty: {evidence}")
    else:
        evidence.mkdir(parents=True)

    files, source_manifest = source_identity(source)
    corpus_bytes = corpus_manifest_path.read_bytes()
    corpus_manifest_sha256 = sha256(corpus_bytes)
    corpus = json.loads(corpus_bytes)
    if corpus.get("schema") != "lumin-phase0-oxc-corpus-v1":
        raise RuntimeError("invalid corpus schema")
    if corpus.get("corpus_id") != CORPUS_ID or corpus.get("source_commit") != SOURCE_COMMIT:
        raise RuntimeError("invalid named corpus identity")
    if corpus.get("generator_sha256") != sha256((source / "scripts" / "prepare-corpus.py").read_bytes()):
        raise RuntimeError("corpus generator identity differs from current source")
    if corpus.get("legacy_file_count") != 705 or corpus.get("legacy_bytes") != 7_302_528:
        raise RuntimeError("legacy corpus count/byte identity mismatch")

    windows_summary, windows_reports = validate_matrix(
        windows,
        expected_os="windows",
        source_files=files,
        source_manifest=source_manifest,
        corpus_manifest_sha256=corpus_manifest_sha256,
    )
    wsl_summary, wsl_reports = validate_matrix(
        wsl,
        expected_os="linux",
        source_files=files,
        source_manifest=source_manifest,
        corpus_manifest_sha256=corpus_manifest_sha256,
    )
    if windows_summary["semantic_digest"] != wsl_summary["semantic_digest"]:
        raise RuntimeError("Windows and WSL2 semantic digests differ")

    copy_matrix(windows, evidence / "windows")
    copy_matrix(wsl, evidence / "wsl2")
    (evidence / "corpus-manifest.json").write_bytes(corpus_bytes)
    source_lines = sorted(f"{entry['sha256']}  {entry['path']}\n" for entry in files)
    (evidence / "SOURCE-SHA256SUMS").write_text("".join(source_lines), encoding="utf-8", newline="\n")
    summary = {
        "schema": "lumin-phase0-oxc-evidence-v1",
        "status": "PASS",
        "architecture_commit": ARCHITECTURE_COMMIT,
        "architecture_manifest_sha256": ARCHITECTURE_MANIFEST,
        "corpus_id": CORPUS_ID,
        "corpus_manifest_sha256": corpus_manifest_sha256,
        "corpus_file_count": len(corpus["entries"]),
        "corpus_bytes": sum(entry["bytes"] for entry in corpus["entries"]),
        "source_manifest_sha256": source_manifest,
        "semantic_digest": windows_summary["semantic_digest"],
        "platforms": [
            platform_metrics(windows_summary, windows_reports),
            platform_metrics(wsl_summary, wsl_reports),
        ],
        "conclusion": {
            "document_contract": "observed feasible on the named Windows NTFS and WSL2 ext4 matrices",
            "product_stack_policy": "NOT_SELECTED",
            "product_worker_default": "NOT_SELECTED",
            "numeric_budget": "NOT_APPROVED",
            "native_linux_evidence": "PENDING",
        },
    }
    write_json(evidence / "summary.json", summary)

    manifest_lines = []
    for path in sorted(evidence.rglob("*"), key=lambda item: item.relative_to(evidence).as_posix()):
        if path.is_file() and path.name != "SHA256SUMS":
            manifest_lines.append(f"{sha256(path.read_bytes())}  {path.relative_to(evidence).as_posix()}\n")
    (evidence / "SHA256SUMS").write_text("".join(manifest_lines), encoding="utf-8", newline="\n")
    print(
        json.dumps(
            {
                "status": "PASS",
                "files": len(manifest_lines),
                "source_manifest_sha256": source_manifest,
                "semantic_digest": summary["semantic_digest"],
            },
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    main()
