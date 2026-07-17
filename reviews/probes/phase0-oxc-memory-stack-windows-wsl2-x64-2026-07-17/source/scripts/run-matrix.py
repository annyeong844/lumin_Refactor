#!/usr/bin/env python3
"""Run isolated stack, worker-count, and allocator-lifetime OXC probe children."""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import time
from pathlib import Path
from typing import Any


ARCHITECTURE_COMMIT = "65e60216891bb3d826a4778f84cb8aaa377abe92"
ARCHITECTURE_MANIFEST = "66925583362be22257dd7357072d176cd3f1ae3d4f99874a53f6689b7cf535e0"
CORPUS_ID = "lumin-lab-35290cb-plus-stack-stress-v1"
STACK_CANDIDATES = [262_144, 524_288, 1_048_576, 2_097_152, 4_194_304, 8_388_608]
JOBS_STACK_BYTES = 4_194_304


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def file_identity(path: Path) -> dict[str, Any]:
    data = path.read_bytes()
    return {"bytes": len(data), "sha256": sha256(data)}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--corpus-root", required=True, type=Path)
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--platform", required=True)
    parser.add_argument("--filesystem-class", required=True)
    return parser.parse_args()


def ensure_new_directory(path: Path) -> None:
    if path.exists():
        if not path.is_dir() or any(path.iterdir()):
            raise RuntimeError(f"output must be absent or empty: {path}")
    else:
        path.mkdir(parents=True)
    (path / "runs").mkdir()
    (path / "logs").mkdir()


def write_json(path: Path, value: Any) -> None:
    path.write_bytes((json.dumps(value, indent=2, sort_keys=True) + "\n").encode("utf-8"))


def load_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def relative(output: Path, path: Path) -> str:
    return path.relative_to(output).as_posix()


def run_child(
    *,
    binary: Path,
    output: Path,
    run_id: str,
    arguments: list[str],
    report_path: Path,
) -> dict[str, Any]:
    stdout_path = output / "logs" / f"{run_id}.stdout.txt"
    stderr_path = output / "logs" / f"{run_id}.stderr.txt"
    started = time.perf_counter_ns()
    completed = subprocess.run(
        [str(binary), *arguments],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    elapsed_micros = (time.perf_counter_ns() - started) // 1_000
    stdout_path.write_bytes(completed.stdout)
    stderr_path.write_bytes(completed.stderr)
    report_present = report_path.is_file()
    status = "PASS" if completed.returncode == 0 and report_present else "FAIL"
    if completed.returncode == 0 and not report_present:
        raise RuntimeError(f"{run_id} exited successfully without a report")
    return {
        "run_id": run_id,
        "status": status,
        "return_code": completed.returncode,
        "elapsed_micros": elapsed_micros,
        "report_path": relative(output, report_path) if report_present else None,
        "stdout_path": relative(output, stdout_path),
        "stderr_path": relative(output, stderr_path),
        "stdout": file_identity(stdout_path),
        "stderr": file_identity(stderr_path),
    }


def job_counts(available: int) -> list[int]:
    counts: list[int] = []
    value = 1
    while value <= available:
        counts.append(value)
        value *= 2
    if counts[-1] != available:
        counts.append(available)
    return counts


def validate_identity(identity: dict[str, Any], binary_identity: dict[str, Any]) -> None:
    if identity.get("probe_id") != "lumin-phase0-oxc-identity-v1":
        raise RuntimeError("invalid probe identity report")
    if identity.get("architecture_commit") != ARCHITECTURE_COMMIT:
        raise RuntimeError("architecture commit mismatch")
    if identity.get("architecture_manifest_sha256") != ARCHITECTURE_MANIFEST:
        raise RuntimeError("architecture manifest mismatch")
    if identity.get("executable", {}).get("sha256") != binary_identity["sha256"]:
        raise RuntimeError("binary identity mismatch")
    if identity.get("executable", {}).get("bytes") != binary_identity["bytes"]:
        raise RuntimeError("binary byte count mismatch")


def validate_report(
    report: dict[str, Any],
    *,
    identity: dict[str, Any],
    binary_identity: dict[str, Any],
    corpus_manifest_sha256: str,
    workers: int,
    stack_bytes: int,
    waves: int,
    platform: str,
    filesystem_class: str,
) -> None:
    expected = {
        "probe_id": "lumin-phase0-oxc-run-v1",
        "status": "PASS",
        "architecture_commit": ARCHITECTURE_COMMIT,
        "architecture_manifest_sha256": ARCHITECTURE_MANIFEST,
        "platform_label": platform,
        "filesystem_class": filesystem_class,
        "requested_workers": workers,
        "actual_workers": workers,
        "worker_stack_bytes": stack_bytes,
        "waves": waves,
        "corpus_id": CORPUS_ID,
        "corpus_manifest_sha256": corpus_manifest_sha256,
        "source_manifest_sha256": identity["source_manifest_sha256"],
    }
    for key, value in expected.items():
        if report.get(key) != value:
            raise RuntimeError(f"report field mismatch for {key}: {report.get(key)!r} != {value!r}")
    if report.get("source_files") != identity.get("source_files"):
        raise RuntimeError("report source identity differs from identity command")
    executable = report.get("executable", {})
    if executable.get("sha256") != binary_identity["sha256"] or executable.get("bytes") != binary_identity["bytes"]:
        raise RuntimeError("run executable identity mismatch")
    wave_results = report.get("wave_results", [])
    if len(wave_results) != waves:
        raise RuntimeError("wave result count mismatch")
    if any(wave.get("semantic_digest") != report.get("semantic_digest") for wave in wave_results):
        raise RuntimeError("cross-wave semantic digest mismatch")
    if any(wave.get("parser_panicked_files") != 0 for wave in wave_results):
        raise RuntimeError("successful run contains parser-panicked files")


def main() -> None:
    args = parse_args()
    binary = args.binary.resolve()
    corpus_root = args.corpus_root.resolve()
    manifest = args.manifest.resolve()
    output = args.output.resolve()
    if not binary.is_file() or not corpus_root.is_dir() or not manifest.is_file():
        raise RuntimeError("binary, corpus root, and manifest must exist")
    ensure_new_directory(output)
    binary_identity = file_identity(binary)
    manifest_bytes = manifest.read_bytes()
    corpus_manifest_sha256 = sha256(manifest_bytes)
    corpus_manifest = json.loads(manifest_bytes)
    if corpus_manifest.get("corpus_id") != CORPUS_ID:
        raise RuntimeError("unexpected corpus manifest")

    identity_path = output / "identity.json"
    identity_result = run_child(
        binary=binary,
        output=output,
        run_id="identity",
        arguments=["identity", "--output", str(identity_path)],
        report_path=identity_path,
    )
    if identity_result["status"] != "PASS":
        raise RuntimeError("identity command failed")
    identity = load_json(identity_path)
    validate_identity(identity, binary_identity)
    available = identity["available_parallelism"]
    if not isinstance(available, int) or available < 1:
        raise RuntimeError("invalid available_parallelism")

    records: list[dict[str, Any]] = []
    successful_digests: set[str] = set()

    def execute_run(kind: str, run_id: str, workers: int, stack_bytes: int, waves: int) -> dict[str, Any]:
        report_path = output / "runs" / f"{run_id}.json"
        record = run_child(
            binary=binary,
            output=output,
            run_id=run_id,
            arguments=[
                "run",
                "--corpus-root",
                str(corpus_root),
                "--manifest",
                str(manifest),
                "--workers",
                str(workers),
                "--stack-bytes",
                str(stack_bytes),
                "--waves",
                str(waves),
                "--platform",
                args.platform,
                "--filesystem-class",
                args.filesystem_class,
                "--output",
                str(report_path),
            ],
            report_path=report_path,
        )
        record.update(
            {"kind": kind, "workers": workers, "stack_bytes": stack_bytes, "waves": waves}
        )
        if record["status"] == "PASS":
            report = load_json(report_path)
            validate_report(
                report,
                identity=identity,
                binary_identity=binary_identity,
                corpus_manifest_sha256=corpus_manifest_sha256,
                workers=workers,
                stack_bytes=stack_bytes,
                waves=waves,
                platform=args.platform,
                filesystem_class=args.filesystem_class,
            )
            record["semantic_digest"] = report["semantic_digest"]
            record["report"] = file_identity(report_path)
            successful_digests.add(report["semantic_digest"])
        records.append(record)
        return record

    stack_records = [
        execute_run("stack", f"stack-{stack_bytes:08d}", 1, stack_bytes, 1)
        for stack_bytes in STACK_CANDIDATES
    ]
    required_stack = {record["stack_bytes"]: record for record in stack_records}
    for required in (4_194_304, 8_388_608):
        if required_stack[required]["status"] != "PASS":
            raise RuntimeError(f"required probe-validity stack failed: {required}")

    expected_jobs = job_counts(available)
    for workers in expected_jobs:
        record = execute_run("jobs", f"jobs-{workers:04d}", workers, JOBS_STACK_BYTES, 3)
        if record["status"] != "PASS":
            raise RuntimeError(f"jobs run failed: {workers}")
    lifetime = execute_run("allocator-lifetime", "allocator-lifetime", 1, JOBS_STACK_BYTES, 8)
    if lifetime["status"] != "PASS":
        raise RuntimeError("allocator lifetime run failed")
    if len(successful_digests) != 1:
        raise RuntimeError(f"successful runs disagree on semantic digest: {sorted(successful_digests)}")

    raw_files = []
    for path in sorted(output.rglob("*"), key=lambda item: item.relative_to(output).as_posix()):
        if path.is_file():
            raw_files.append({"path": relative(output, path), **file_identity(path)})
    summary = {
        "schema": "lumin-phase0-oxc-matrix-v1",
        "status": "PASS",
        "architecture_commit": ARCHITECTURE_COMMIT,
        "architecture_manifest_sha256": ARCHITECTURE_MANIFEST,
        "platform": args.platform,
        "filesystem_class": args.filesystem_class,
        "binary": {"path": str(binary), **binary_identity},
        "corpus_manifest_sha256": corpus_manifest_sha256,
        "corpus_file_count": len(corpus_manifest["entries"]),
        "corpus_bytes": sum(entry["bytes"] for entry in corpus_manifest["entries"]),
        "available_parallelism": available,
        "stack_candidates": STACK_CANDIDATES,
        "jobs_candidates": expected_jobs,
        "jobs_stack_bytes": JOBS_STACK_BYTES,
        "semantic_digest": next(iter(successful_digests)),
        "source_manifest_sha256": identity["source_manifest_sha256"],
        "source_files": identity["source_files"],
        "identity_result": identity_result,
        "runs": records,
        "raw_files": raw_files,
    }
    write_json(output / "matrix-summary.json", summary)
    print(json.dumps({"status": "PASS", "runs": len(records), "semantic_digest": summary["semantic_digest"]}))


if __name__ == "__main__":
    main()
